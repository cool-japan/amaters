//! io_uring-backed Write-Ahead Log writer for AmateRS
//!
//! This module provides a high-performance WAL writer using the Linux io_uring
//! interface via the `tokio-uring` crate. By bypassing the standard kernel I/O
//! path and submitting write operations directly through io_uring submission
//! queues, this writer achieves lower syscall overhead and better throughput
//! for write-heavy workloads.
//!
//! # Architecture
//!
//! `tokio-uring` requires its own runtime (`tokio_uring::start`) that is
//! incompatible with standard tokio. To bridge the two worlds:
//!
//! 1. A **dedicated OS thread** is spawned at construction time.
//! 2. That thread calls `tokio_uring::start(async { … })` to enter the
//!    io_uring event loop.
//! 3. Async callers communicate via a `tokio::sync::mpsc::UnboundedSender`
//!    (`Send + Sync`), embedding a `tokio::sync::oneshot::Sender` for the
//!    per-request reply.
//! 4. The io_uring thread drains the channel, **batches** writes up to
//!    `UringWalConfig::batch_size` bytes, submits them via
//!    `tokio_uring::fs::File::write_at`, then signals each caller via its
//!    oneshot.
//!
//! # O_DIRECT limitation
//!
//! `O_DIRECT` requires 512-byte (sector-size) aligned buffers and file
//! positions. The current implementation supports `direct_io = false` only;
//! direct-I/O alignment is left as a documented follow-up because it requires
//! either per-allocation alignment (e.g. `posix_memalign`) or a slab
//! allocator—neither fits cleanly into the generic `Vec<u8>` payload model.
//! Setting `direct_io = true` in config is accepted but has no additional
//! effect beyond a tracing warning at writer startup.
//!
//! # Wire format
//!
//! Each record is identical to the standard WAL:
//! ```text
//! [len: u32 le][magic: u32 le][seq: u64 le][type: u8]
//! [key_len: u32 le][key: bytes][val_len: u32 le][val: bytes]
//! [crc32: u32 le]
//! ```
//! The `len` prefix includes every byte that follows it (identical to the
//! blocking WAL so that a standard `WalReader` can replay the file).
//!
//! # Shutdown
//!
//! Call [`UringWalWriter::close`] before dropping to join the io_uring thread
//! cleanly. Dropping without closing causes the thread to receive a channel
//! error on its next `recv()` and exit gracefully, but the join handle is
//! leaked (the OS will reclaim the thread at process exit).

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::wal::{WalEntry, WalEntryType};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the io_uring WAL writer.
#[derive(Debug, Clone)]
pub struct UringWalConfig {
    /// Size of the io_uring submission and completion rings. Must be a power of 2.
    /// Larger values allow more in-flight operations but consume more kernel memory.
    pub ring_size: u32,
    /// Maximum number of serialized WAL-entry bytes to batch per ring submission.
    /// Writes are grouped until this threshold is reached or the channel is empty,
    /// whichever comes first. Default: 64 KiB.
    pub batch_size: usize,
    /// Whether to open the WAL file with `O_DIRECT` (bypass page cache).
    /// **Currently unimplemented**: setting this to `true` emits a tracing warning
    /// and falls back to buffered I/O. See module-level docs for the rationale.
    pub direct_io: bool,
    /// Soft capacity hint for the internal MPSC channel. An unbounded channel is
    /// used in practice; this field is reserved for future bounded-channel work.
    pub channel_capacity: usize,
}

impl Default for UringWalConfig {
    fn default() -> Self {
        Self {
            ring_size: 128,
            batch_size: 64 * 1024, // 64 KiB per batch
            direct_io: false,
            channel_capacity: 1024,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal channel protocol
// ---------------------------------------------------------------------------

/// A single write request travelling to the io_uring thread.
struct UringWriteRequest {
    /// Fully serialized WAL record (length-prefix + encoded entry).
    payload: Vec<u8>,
    /// Oneshot sender; the io_uring thread sends `Ok(())` or an `io::Error`.
    done_tx: tokio::sync::oneshot::Sender<std::io::Result<()>>,
}

/// Control messages understood by the io_uring thread.
enum UringControl {
    /// Submit a serialized WAL record.
    Write(UringWriteRequest),
    /// Force an `fsync` / `fdatasync` and signal when complete.
    Flush {
        done_tx: tokio::sync::oneshot::Sender<std::io::Result<()>>,
    },
    /// Shut down the io_uring thread cleanly (breaks the receive loop).
    Shutdown,
}

// ---------------------------------------------------------------------------
// Public writer type
// ---------------------------------------------------------------------------

/// An async WAL writer backed by a dedicated io_uring thread.
///
/// # Cloning
///
/// `UringWalWriter` is cheaply `Clone`-able. All clones share the same
/// underlying io_uring thread and file descriptor; writes from different
/// clones are serialized through the MPSC channel.
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(all(target_os = "linux", feature = "io-uring"))]
/// # async fn example() -> amaters_core::error::Result<()> {
/// use amaters_core::storage::wal_uring::{UringWalConfig, UringWalWriter};
/// use amaters_core::storage::wal::WalEntry;
/// use amaters_core::types::{Key, CipherBlob};
///
/// let config = UringWalConfig::default();
/// let writer = UringWalWriter::open("/tmp/my.wal", config)?;
///
/// let entry = WalEntry::put(1, Key::from_str("hello"), CipherBlob::new(b"world".to_vec()));
/// writer.append(&entry).await?;
/// writer.flush().await?;
/// writer.close()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct UringWalWriter {
    /// Sender half shared across all clones.
    tx: Arc<tokio::sync::mpsc::UnboundedSender<UringControl>>,
    /// Join handle is stored behind a `Mutex<Option<…>>` so that only the
    /// first call to `close` actually joins.
    thread_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    /// Configuration snapshot (kept for diagnostics / re-open).
    config: UringWalConfig,
    /// Absolute path to the WAL file.
    path: PathBuf,
}

impl UringWalWriter {
    /// Open (or create) a WAL file and start the io_uring background thread.
    ///
    /// This call returns immediately after spawning the thread; the thread
    /// opens the file asynchronously inside `tokio_uring::start`.
    ///
    /// # Errors
    ///
    /// Returns an error if the thread cannot be spawned.
    pub fn open(path: impl AsRef<Path>, config: UringWalConfig) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if config.direct_io {
            tracing::warn!(
                path = %path.display(),
                "UringWalWriter: direct_io = true is not yet implemented; \
                 falling back to buffered I/O (O_DIRECT omitted). \
                 This will be fixed in a future release once aligned-buffer \
                 allocation is integrated."
            );
        }

        // Validate ring_size is a power of two
        if config.ring_size == 0 || !config.ring_size.is_power_of_two() {
            return Err(AmateRSError::ConfigError(ErrorContext::new(format!(
                "UringWalConfig.ring_size must be a non-zero power of two, got {}",
                config.ring_size
            ))));
        }

        // Create the channel before spawning so the sender can be returned to
        // the caller while the receiver lives on the thread.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<UringControl>();

        let thread_path = path.clone();
        let thread_config = config.clone();

        let join_handle = thread::Builder::new()
            .name("amaters-uring-wal".to_owned())
            .spawn(move || {
                run_uring_thread(thread_path, thread_config, rx);
            })
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "Failed to spawn io_uring WAL thread: {}",
                    e
                )))
            })?;

        Ok(Self {
            tx: Arc::new(tx),
            thread_handle: Arc::new(Mutex::new(Some(join_handle))),
            config,
            path,
        })
    }

    /// Serialize and append a [`WalEntry`] to the WAL.
    ///
    /// The call returns once the kernel has accepted the write into the
    /// io_uring submission queue and signalled completion. For full
    /// durability, call [`flush`](Self::flush) afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error if the io_uring thread has shut down or if the
    /// underlying `write_at` syscall fails.
    pub async fn append(&self, entry: &WalEntry) -> Result<()> {
        let payload = serialize_entry(entry);

        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(UringControl::Write(UringWriteRequest { payload, done_tx }))
            .map_err(|_| {
                AmateRSError::IoError(ErrorContext::new(
                    "io_uring WAL thread has shut down; cannot append entry",
                ))
            })?;

        done_rx
            .await
            .map_err(|_| {
                AmateRSError::IoError(ErrorContext::new(
                    "io_uring WAL thread dropped the oneshot sender unexpectedly",
                ))
            })?
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "io_uring write_at failed: {}",
                    e
                )))
            })
    }

    /// Issue an `fsync` on the WAL file and wait for completion.
    ///
    /// All writes submitted before this call are guaranteed to be durable
    /// on return (barring hardware failure).
    ///
    /// # Errors
    ///
    /// Returns an error if the io_uring thread has shut down or if `sync_all`
    /// fails.
    pub async fn flush(&self) -> Result<()> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        self.tx.send(UringControl::Flush { done_tx }).map_err(|_| {
            AmateRSError::IoError(ErrorContext::new(
                "io_uring WAL thread has shut down; cannot flush",
            ))
        })?;

        done_rx
            .await
            .map_err(|_| {
                AmateRSError::IoError(ErrorContext::new(
                    "io_uring WAL thread dropped the flush oneshot sender",
                ))
            })?
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!(
                    "io_uring sync_all failed: {}",
                    e
                )))
            })
    }

    /// Send a [`UringControl::Shutdown`] message and join the io_uring thread.
    ///
    /// Only the first call to `close` actually performs the join; subsequent
    /// calls are no-ops. This is important because `UringWalWriter` is
    /// `Clone`-able and multiple handles may exist.
    ///
    /// # Errors
    ///
    /// Returns an error if sending the shutdown message fails (the thread
    /// already exited) or if `thread::JoinHandle::join` panics.
    pub fn close(self) -> Result<()> {
        // Best-effort send; ignore error if thread already exited.
        let _ = self.tx.send(UringControl::Shutdown);

        let mut guard = self.thread_handle.lock().expect(
            "UringWalWriter: thread_handle mutex was poisoned; cannot join io_uring thread",
        );

        if let Some(handle) = guard.take() {
            handle.join().map_err(|_| {
                AmateRSError::IoError(ErrorContext::new(
                    "io_uring WAL thread panicked during join",
                ))
            })?;
        }

        Ok(())
    }

    /// Return the absolute path of the WAL file this writer targets.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return a reference to the configuration used to open this writer.
    pub fn config(&self) -> &UringWalConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Serialization helper
// ---------------------------------------------------------------------------

/// Serialize a [`WalEntry`] into the on-disk wire format.
///
/// The wire layout is:
/// ```text
/// [record_len: u32 le]      — total bytes that follow (excl. this field)
/// [magic: u32 le]           — 0x57414C ("WAL")
/// [seq: u64 le]
/// [entry_type: u8]          — 1 = Put, 2 = Delete
/// [key_len: u32 le]
/// [key: bytes]
/// [val_len: u32 le]
/// [val: bytes]              — 0 bytes for Delete entries
/// [crc32: u32 le]
/// ```
///
/// This is identical to the format produced by [`WalEntry::encode`] with
/// a 4-byte length prefix prepended, matching what the standard [`Wal`]
/// writer emits so that [`WalReader`] can replay files written by either
/// backend.
///
/// [`Wal`]: crate::storage::wal::Wal
/// [`WalReader`]: crate::storage::wal::WalReader
fn serialize_entry(entry: &WalEntry) -> Vec<u8> {
    // Inner encoding (matches WalEntry::encode exactly)
    let mut inner = Vec::with_capacity(64);

    // Magic
    inner.extend_from_slice(&0x57414Cu32.to_le_bytes());

    // Sequence
    inner.extend_from_slice(&entry.sequence.to_le_bytes());

    // Entry type
    let type_byte: u8 = match entry.entry_type {
        WalEntryType::Put => 1,
        WalEntryType::Delete => 2,
    };
    inner.push(type_byte);

    // Key
    inner.extend_from_slice(&(entry.key.len() as u32).to_le_bytes());
    inner.extend_from_slice(entry.key.as_bytes());

    // Value
    if let Some(ref value) = entry.value {
        inner.extend_from_slice(&(value.len() as u32).to_le_bytes());
        inner.extend_from_slice(value.as_bytes());
    } else {
        inner.extend_from_slice(&0u32.to_le_bytes());
    }

    // Checksum
    inner.extend_from_slice(&entry.checksum.to_le_bytes());

    // Prepend the length prefix (matches write_entry in wal.rs)
    let mut record = Vec::with_capacity(4 + inner.len());
    record.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    record.extend_from_slice(&inner);
    record
}

// ---------------------------------------------------------------------------
// io_uring thread body
// ---------------------------------------------------------------------------

/// Entry-point for the dedicated io_uring thread.
///
/// This function is `!async` — it blocks the OS thread while running the
/// `tokio_uring` runtime. All io_uring operations happen inside the async
/// closure passed to `tokio_uring::start`.
fn run_uring_thread(
    path: PathBuf,
    config: UringWalConfig,
    rx: tokio::sync::mpsc::UnboundedReceiver<UringControl>,
) {
    tokio_uring::builder()
        .entries(config.ring_size)
        .start(async move {
            if let Err(e) = uring_event_loop(path, config, rx).await {
                tracing::error!("io_uring WAL thread exited with error: {}", e);
            }
        });
}

/// The async event loop that runs inside `tokio_uring::start`.
///
/// # Design
///
/// 1. Open the WAL file in append mode using `tokio_uring::fs::OpenOptions`.
/// 2. Wait for the first `UringControl` message.
/// 3. Attempt to drain additional messages without blocking (batching) up to
///    `config.batch_size` bytes of write payload.
/// 4. Submit the batch sequentially via `file.write_at(buf, offset)`.
///    io_uring has no built-in "append" operation; we maintain a `u64` file
///    cursor that starts at the file's current end and advances after each
///    write.
/// 5. On `Flush`: call `file.sync_all()` (triggers `fdatasync` via io_uring).
/// 6. On `Shutdown`: break cleanly.
///
/// Note on `BufResult`: `tokio_uring` returns `(std::io::Result<usize>, Vec<u8>)`
/// so we must always destructure to reclaim the buffer even on error.
async fn uring_event_loop(
    path: PathBuf,
    config: UringWalConfig,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<UringControl>,
) -> std::io::Result<()> {
    // Open (or create) the WAL file in append mode.
    let file = tokio_uring::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;

    // Determine the starting write position from the current file length.
    // We use std::fs::metadata here because tokio_uring doesn't expose a
    // non-blocking stat; this is a one-time call at startup.
    let mut write_offset: u64 = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    tracing::debug!(
        path = %path.display(),
        write_offset,
        "io_uring WAL writer started"
    );

    loop {
        // Block until we have at least one message.
        let first = match rx.recv().await {
            Some(ctrl) => ctrl,
            None => {
                // Channel closed (all senders dropped) — exit cleanly.
                tracing::debug!("io_uring WAL channel closed; thread exiting");
                break;
            }
        };

        match first {
            UringControl::Shutdown => {
                tracing::debug!("io_uring WAL received Shutdown; thread exiting");
                break;
            }

            UringControl::Flush { done_tx } => {
                let result = file.sync_all().await;
                // Signal the caller; ignore send error (caller may have timed out).
                let _ = done_tx.send(result);
            }

            UringControl::Write(first_req) => {
                // -------------------------------------------------------
                // Batching phase
                // -------------------------------------------------------
                // Collect the first request, then drain as many additional
                // Write messages as are immediately available (non-blocking
                // try_recv), until either `batch_size` bytes are accumulated
                // or no more messages are ready.
                let mut batch: Vec<UringWriteRequest> = Vec::new();
                let mut batch_bytes: usize = first_req.payload.len();
                batch.push(first_req);

                while batch_bytes < config.batch_size {
                    match rx.try_recv() {
                        Ok(UringControl::Write(req)) => {
                            batch_bytes += req.payload.len();
                            batch.push(req);
                        }
                        Ok(UringControl::Flush { done_tx }) => {
                            // Flush message arrived mid-batch. Submit the
                            // accumulated batch first, then handle flush.
                            write_offset = submit_batch(&file, batch, write_offset).await;
                            let result = file.sync_all().await;
                            let _ = done_tx.send(result);
                            // Start fresh for the next iteration.
                            batch = Vec::new();
                            batch_bytes = 0;
                            break;
                        }
                        Ok(UringControl::Shutdown) => {
                            // Submit what we have, then exit. The returned
                            // offset is discarded because we return immediately.
                            let _ = submit_batch(&file, batch, write_offset).await;
                            tracing::debug!("io_uring WAL received Shutdown mid-batch; exiting");
                            return Ok(());
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                            // No more messages ready — submit what we have.
                            break;
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            // All senders dropped — submit and exit. The returned
                            // offset is discarded because we return immediately.
                            let _ = submit_batch(&file, batch, write_offset).await;
                            tracing::debug!("io_uring WAL channel disconnected mid-batch; exiting");
                            return Ok(());
                        }
                    }
                }

                if !batch.is_empty() {
                    write_offset = submit_batch(&file, batch, write_offset).await;
                }
            }
        }
    }

    // File is dropped here → kernel closes the fd.
    tracing::debug!(path = %path.display(), "io_uring WAL file closed");
    Ok(())
}

/// Submit a batch of write requests to the io_uring ring and signal each caller.
///
/// Writes are issued sequentially (not in parallel) because WAL records must
/// be appended in order. Each write uses `file.write_at(buf, pos)` — the
/// io_uring equivalent of `pwrite64(2)`.
///
/// Returns the updated file offset after all writes in the batch.
async fn submit_batch(
    file: &tokio_uring::fs::File,
    batch: Vec<UringWriteRequest>,
    mut offset: u64,
) -> u64 {
    for req in batch {
        let payload_len = req.payload.len() as u64;
        let buf = req.payload;

        // `write_at` may write fewer bytes than requested (short write).
        // We loop until all bytes are submitted.
        let result = write_all_at(file, buf, offset).await;

        match result {
            Ok(bytes_written) => {
                offset += bytes_written as u64;
                let _ = req.done_tx.send(Ok(()));
            }
            Err(e) => {
                // Signal the caller, then continue with the next request
                // (best-effort: do not abandon remaining batch entries).
                let _ = req.done_tx.send(Err(e));
                // Advance offset by the claimed payload size to maintain
                // monotonicity even if the write failed partially.
                // The WAL reader will detect corruption via the CRC32 field.
                offset += payload_len;
            }
        }
    }

    offset
}

/// Write all bytes of `buf` at `offset` using `tokio_uring::fs::File::write_at`,
/// looping on short writes.
///
/// Returns the total number of bytes written (equal to `buf.len()` on success).
async fn write_all_at(
    file: &tokio_uring::fs::File,
    mut buf: Vec<u8>,
    mut offset: u64,
) -> std::io::Result<usize> {
    let total = buf.len();
    let mut written = 0usize;

    while written < total {
        // `write_at` returns `(Result<usize>, Vec<u8>)` — must destructure to
        // reclaim the buffer ownership even on error.
        let slice = buf[written..].to_vec();
        // `write_at` returns an `UnsubmittedWrite` (a built-but-unsubmitted op);
        // call `.submit()` to push it onto the ring and obtain the awaitable
        // `InFlightOneshot`, which yields `(Result<usize>, Vec<u8>)`.
        let (result, _returned_buf) = file.write_at(slice, offset + written as u64).submit().await;

        match result {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "io_uring write_at returned 0 bytes written",
                ));
            }
            Ok(n) => {
                written += n;
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    Ok(written)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, target_os = "linux", feature = "io-uring"))]
mod tests {
    use super::*;
    use crate::storage::wal::{Wal, WalReader};
    use crate::types::{CipherBlob, Key};
    use tempfile::TempDir;

    // Helper: build a deterministic WalEntry for tests.
    fn make_put_entry(seq: u64, key_suffix: &str) -> WalEntry {
        let key = Key::from_str(&format!("test_key_{}", key_suffix));
        let value = CipherBlob::new(format!("test_value_{}", key_suffix).into_bytes());
        WalEntry::put(seq, key, value)
    }

    fn make_delete_entry(seq: u64, key_suffix: &str) -> WalEntry {
        let key = Key::from_str(&format!("del_key_{}", key_suffix));
        WalEntry::delete(seq, key)
    }

    // Helper: read all entries from a WAL file using the standard WalReader.
    fn read_all_entries(path: &Path) -> Vec<WalEntry> {
        let mut reader = WalReader::open(path).expect("WalReader::open failed");
        let mut entries = Vec::new();
        loop {
            match reader.read_entry() {
                Ok(Some(e)) => entries.push(e),
                Ok(None) => break,
                Err(e) => panic!("WalReader::read_entry error: {}", e),
            }
        }
        entries
    }

    // -----------------------------------------------------------------------
    // Test 1: open and close — file must exist after close
    // -----------------------------------------------------------------------
    #[test]
    fn test_uring_wal_open_and_close() {
        let tmp = TempDir::new().expect("TempDir::new failed");
        let wal_path = tmp.path().join("test_open_close.wal");

        let writer =
            UringWalWriter::open(&wal_path, UringWalConfig::default()).expect("open failed");

        assert!(
            writer.path() == wal_path,
            "path() should return the path passed to open()"
        );

        writer.close().expect("close failed");

        // After close the file must exist (created by tokio_uring inside the thread).
        // Give the thread a brief moment to create it if the OS is slow.
        assert!(
            wal_path.exists(),
            "WAL file should exist after open+close: {}",
            wal_path.display()
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: append 3 entries, read them back with WalReader
    // -----------------------------------------------------------------------
    #[test]
    fn test_uring_wal_append_and_read_back() {
        let tmp = TempDir::new().expect("TempDir::new failed");
        let wal_path = tmp.path().join("test_append.wal");

        // We need a tokio runtime for the async append/flush calls.
        let rt = tokio::runtime::Runtime::new().expect("tokio Runtime::new failed");

        let writer =
            UringWalWriter::open(&wal_path, UringWalConfig::default()).expect("open failed");

        let entries = vec![
            make_put_entry(1, "alpha"),
            make_put_entry(2, "beta"),
            make_delete_entry(3, "gamma"),
        ];

        rt.block_on(async {
            for e in &entries {
                writer.append(e).await.expect("append failed");
            }
            writer.flush().await.expect("flush failed");
        });

        writer.close().expect("close failed");

        let recovered = read_all_entries(&wal_path);

        assert_eq!(
            recovered.len(),
            entries.len(),
            "recovered entry count mismatch"
        );

        for (original, recovered_entry) in entries.iter().zip(recovered.iter()) {
            assert_eq!(
                original.sequence, recovered_entry.sequence,
                "sequence mismatch"
            );
            assert_eq!(
                original.entry_type, recovered_entry.entry_type,
                "entry_type mismatch"
            );
            assert_eq!(
                original.key.as_bytes(),
                recovered_entry.key.as_bytes(),
                "key mismatch"
            );
            assert_eq!(
                original.checksum, recovered_entry.checksum,
                "checksum mismatch"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: append then flush — verify on-disk via WalReader
    // -----------------------------------------------------------------------
    #[test]
    fn test_uring_wal_flush() {
        let tmp = TempDir::new().expect("TempDir::new failed");
        let wal_path = tmp.path().join("test_flush.wal");

        let rt = tokio::runtime::Runtime::new().expect("tokio Runtime::new failed");

        let writer = UringWalWriter::open(&wal_path, UringWalConfig::default())
            .expect("UringWalWriter::open failed");

        let entry = make_put_entry(42, "flush_test");

        rt.block_on(async {
            writer.append(&entry).await.expect("append failed");
            // flush forces fdatasync — after this the data is on stable storage
            writer.flush().await.expect("flush failed");
        });

        writer.close().expect("close failed");

        let recovered = read_all_entries(&wal_path);
        assert_eq!(recovered.len(), 1, "expected exactly 1 recovered entry");
        assert_eq!(recovered[0].sequence, 42);
        assert_eq!(
            recovered[0].key.as_bytes(),
            b"test_key_flush_test",
            "key bytes mismatch"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: 100 concurrent appends via tokio::join_all, verify all present
    // -----------------------------------------------------------------------
    #[test]
    fn test_uring_wal_batch_writes() {
        let tmp = TempDir::new().expect("TempDir::new failed");
        let wal_path = tmp.path().join("test_batch.wal");

        let rt = tokio::runtime::Runtime::new().expect("tokio Runtime::new failed");

        let writer = Arc::new(
            UringWalWriter::open(&wal_path, UringWalConfig::default())
                .expect("UringWalWriter::open failed"),
        );

        const NUM_ENTRIES: u64 = 100;

        // Build all entries ahead of time so we know what to expect.
        let entries: Vec<WalEntry> = (0..NUM_ENTRIES)
            .map(|i| make_put_entry(i, &i.to_string()))
            .collect();

        rt.block_on(async {
            // Fan out all appends concurrently.
            let futs: Vec<_> = entries
                .iter()
                .map(|e| {
                    let w = Arc::clone(&writer);
                    async move { w.append(e).await }
                })
                .collect();

            let results = futures::future::join_all(futs).await;
            for (i, res) in results.into_iter().enumerate() {
                res.unwrap_or_else(|err| panic!("append[{}] failed: {}", i, err));
            }

            writer.flush().await.expect("flush failed");
        });

        // Clone the writer handle, then close the original Arc reference.
        // We need to close the writer to ensure the thread exits before reading.
        // Unwrap the Arc (should be the last reference after the block above).
        let writer =
            Arc::try_unwrap(writer).expect("Arc should have only one reference at this point");
        writer.close().expect("close failed");

        let recovered = read_all_entries(&wal_path);

        assert_eq!(
            recovered.len(),
            NUM_ENTRIES as usize,
            "expected {} recovered entries, got {}",
            NUM_ENTRIES,
            recovered.len()
        );

        // All sequence numbers 0..NUM_ENTRIES must be present (order may differ
        // due to concurrent submission).
        let mut seen_seqs: Vec<u64> = recovered.iter().map(|e| e.sequence).collect();
        seen_seqs.sort_unstable();
        let expected: Vec<u64> = (0..NUM_ENTRIES).collect();
        assert_eq!(seen_seqs, expected, "sequence numbers mismatch");
    }
}
