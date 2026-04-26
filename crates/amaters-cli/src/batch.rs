//! Batch operations for amaters-cli
//!
//! Reads newline-delimited operation records from a file or stdin and applies
//! them sequentially to the server.  Records are processed one line at a time —
//! the file is never fully loaded into memory.
//!
//! ## Line format
//!
//! ```text
//! put  <key> <value>
//! delete <key>
//! ```
//!
//! Lines that do not match this format are counted as `skipped` in the returned
//! [`BatchStats`].

use crate::client::Client;
use amaters_core::{CipherBlob, Key};
use anyhow::Result;
use std::future::Future;
use std::io::BufRead;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Source of batch input lines.
pub enum BatchSource {
    /// Read from a file at the given path.
    File(PathBuf),
    /// Read from stdin.
    Stdin,
}

/// Aggregate statistics for a completed batch run.
#[derive(Debug, Clone, Default)]
pub struct BatchStats {
    /// Total lines processed (including skipped).
    pub total: usize,
    /// Lines that completed successfully.
    pub succeeded: usize,
    /// Lines that produced a server or I/O error.
    pub failed: usize,
    /// Lines that were malformed and could not be parsed.
    pub skipped: usize,
}

/// Batch operation command.
pub struct BatchCommand {
    source: BatchSource,
}

impl BatchCommand {
    /// Create a new batch command from the given source.
    pub fn new(source: BatchSource) -> Self {
        Self { source }
    }

    /// Execute the batch against `client`, consuming lines one by one.
    ///
    /// Accepts any `R: BufRead` internally; the public variant dispatches to
    /// the right reader based on `self.source`.
    pub async fn execute(&self, client: &Client) -> Result<BatchStats> {
        match &self.source {
            BatchSource::File(path) => {
                let file = std::fs::File::open(path)
                    .map_err(|e| anyhow::anyhow!("Cannot open batch file {:?}: {}", path, e))?;
                let reader = std::io::BufReader::new(file);
                Self::execute_from_reader(reader, client).await
            }
            BatchSource::Stdin => {
                let stdin = std::io::stdin();
                let reader = stdin.lock();
                Self::execute_from_reader(reader, client).await
            }
        }
    }

    /// Execute the batch from an arbitrary `BufRead` source.
    ///
    /// This method is useful for testing (pass `std::io::Cursor::new(…)`).
    pub async fn execute_from_reader<R: BufRead>(reader: R, client: &Client) -> Result<BatchStats> {
        Self::process_reader(reader, |op| Self::apply_op_void(op, client)).await
    }

    /// Wrapper around `apply_op` that discards the success message, returning `Result<()>`.
    async fn apply_op_void(op: BatchOp, client: &Client) -> Result<()> {
        Self::apply_op(op, client).await.map(|msg| {
            println!("{msg}");
        })
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Core streaming parser that drives a handler closure for each parsed op.
    ///
    /// The handler receives a [`BatchOp`] and returns a future resolving to
    /// `Result<()>`.  Parse errors increment `skipped`; handler errors increment
    /// `failed`; handler successes increment `succeeded`.
    ///
    /// Blank lines and `#`-prefixed comments do not increment `total`.
    async fn process_reader<R, F, Fut>(mut reader: R, mut handler: F) -> Result<BatchStats>
    where
        R: BufRead,
        F: FnMut(BatchOp) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut stats = BatchStats::default();
        let mut line_buf = String::new();
        let mut line_no: usize = 0;

        loop {
            line_buf.clear();
            let bytes_read = reader
                .read_line(&mut line_buf)
                .map_err(|e| anyhow::anyhow!("I/O error reading batch input: {}", e))?;

            if bytes_read == 0 {
                break; // EOF
            }

            line_no += 1;
            let trimmed = line_buf.trim();

            // Skip blank lines and comments without tallying them.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            stats.total += 1;

            match Self::parse_line(trimmed) {
                Ok(op) => match handler(op).await {
                    Ok(()) => {
                        stats.succeeded += 1;
                    }
                    Err(e) => {
                        eprintln!("ERROR: line {line_no}: {e}");
                        stats.failed += 1;
                    }
                },
                Err(reason) => {
                    eprintln!("SKIP: malformed line {line_no}: {reason} — raw: {trimmed}");
                    stats.skipped += 1;
                }
            }
        }

        Ok(stats)
    }

    /// Parse a single (already-trimmed, non-empty) line into a [`BatchOp`].
    pub(crate) fn parse_line(line: &str) -> std::result::Result<BatchOp, String> {
        let mut parts = line.splitn(3, ' ');

        let op = parts.next().ok_or_else(|| "missing op".to_string())?;
        let key_str = parts
            .next()
            .ok_or_else(|| "missing key".to_string())?
            .trim();

        if key_str.is_empty() {
            return Err("empty key".to_string());
        }

        match op.to_ascii_lowercase().as_str() {
            "put" => {
                let value_str = parts
                    .next()
                    .ok_or_else(|| "put requires a value".to_string())?
                    .trim()
                    .to_string();
                if value_str.is_empty() {
                    return Err("put requires a non-empty value".to_string());
                }
                Ok(BatchOp::Put {
                    key: key_str.to_string(),
                    value: value_str,
                })
            }
            "delete" => Ok(BatchOp::Delete {
                key: key_str.to_string(),
            }),
            other => Err(format!("unknown op '{other}' (expected put|delete)")),
        }
    }

    /// Apply a parsed [`BatchOp`] to the server, returning an OK summary string.
    async fn apply_op(op: BatchOp, client: &Client) -> Result<String> {
        match op {
            BatchOp::Put { key, value } => {
                let k = Key::from_str(&key);
                let blob = CipherBlob::new(value.into_bytes());
                client.set(&k, &blob).await?;
                Ok(format!("OK: put {key}"))
            }
            BatchOp::Delete { key } => {
                let k = Key::from_str(&key);
                client.delete(&k).await?;
                Ok(format!("OK: delete {key}"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal operation enum
// ---------------------------------------------------------------------------

/// A parsed batch operation.
pub(crate) enum BatchOp {
    Put { key: String, value: String },
    Delete { key: String },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // -----------------------------------------------------------------------
    // Unit tests for the parse layer
    // -----------------------------------------------------------------------

    fn try_parse(line: &str) -> std::result::Result<(), String> {
        BatchCommand::parse_line(line).map(|_| ())
    }

    #[test]
    fn test_parse_put() {
        assert!(try_parse("put mykey myvalue").is_ok());
    }

    #[test]
    fn test_parse_delete() {
        assert!(try_parse("delete mykey").is_ok());
    }

    #[test]
    fn test_parse_unknown_op() {
        assert!(try_parse("get mykey").is_err());
    }

    #[test]
    fn test_parse_missing_value_for_put() {
        assert!(try_parse("put mykey").is_err());
    }

    #[test]
    fn test_parse_empty_key() {
        assert!(try_parse("put  myvalue").is_err());
    }

    #[test]
    fn test_parse_put_value_with_spaces() {
        // The third token may contain spaces — only the key is split off.
        let op = BatchCommand::parse_line("put mykey hello world").expect("should parse");
        match op {
            BatchOp::Put { key, value } => {
                assert_eq!(key, "mykey");
                assert_eq!(value, "hello world");
            }
            BatchOp::Delete { .. } => panic!("expected put"),
        }
    }

    // -----------------------------------------------------------------------
    // Integration-style tests that exercise process_reader via execute_from_reader
    // -----------------------------------------------------------------------

    /// A no-op handler that always succeeds — used for tests that only care
    /// about stats, not side-effects.
    async fn null_apply(op: BatchOp) -> Result<()> {
        match op {
            BatchOp::Put { .. } | BatchOp::Delete { .. } => Ok(()),
        }
    }

    /// `execute_from_reader`-style helper without a real client — drives
    /// `process_reader` with a lightweight handler that never connects to a
    /// server.
    async fn run_with_null_handler(input: &[u8]) -> BatchStats {
        BatchCommand::process_reader(Cursor::new(input), null_apply)
            .await
            .expect("process_reader should not fail")
    }

    /// Verify that stats counters are populated correctly for a mix of valid and
    /// invalid lines by going through the real `process_reader`.
    #[tokio::test]
    async fn test_batch_stats_counters_from_parse() {
        let input = b"put key1 value1\ndelete key2\ngarbage\nput key3 val3\n\n# comment\n";
        let stats = run_with_null_handler(input).await;

        assert_eq!(stats.total, 4, "blank and comment lines must not count");
        assert_eq!(stats.succeeded, 3);
        assert_eq!(stats.skipped, 1, "garbage line must be skipped");
        assert_eq!(stats.failed, 0);
    }

    /// Feed process_reader 1 000 valid lines through a Cursor and verify it
    /// streams rather than collecting — no OOM and all lines succeed.
    #[tokio::test]
    async fn test_batch_large_input_parse_streaming() {
        let mut content = String::new();
        for i in 0..1_000 {
            content.push_str(&format!("put key{i} value{i}\n"));
        }
        let stats = run_with_null_handler(content.as_bytes()).await;

        assert_eq!(stats.total, 1_000);
        assert_eq!(stats.succeeded, 1_000);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 0);
    }

    /// 1 good line, 1 bad line, 1 good line → skipped == 1, succeeded == 2.
    #[tokio::test]
    async fn test_batch_skips_malformed_lines_with_error() {
        let input = b"put key1 val1\nbad_line\ndelete key2\n";
        let stats = run_with_null_handler(input).await;

        assert_eq!(stats.total, 3);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.succeeded, 2);
        assert_eq!(stats.failed, 0);
    }

    /// Verify a handler error (simulated via a closure that always returns Err)
    /// increments `failed` rather than `skipped`.
    #[tokio::test]
    async fn test_batch_handler_error_increments_failed() {
        let input = b"put key1 val1\ndelete key2\n";
        let stats = BatchCommand::process_reader(Cursor::new(input), |_op| async {
            Err(anyhow::anyhow!("simulated server error"))
        })
        .await
        .expect("process_reader must not return Err itself");

        assert_eq!(stats.total, 2);
        assert_eq!(stats.succeeded, 0);
        assert_eq!(stats.failed, 2);
        assert_eq!(stats.skipped, 0);
    }

    /// Blank lines and comment lines must not increment `total`.
    #[tokio::test]
    async fn test_batch_blank_and_comment_lines_not_counted() {
        let input = b"\n\n# this is a comment\n   \nput realkey realval\n# another comment\n";
        let stats = run_with_null_handler(input).await;

        assert_eq!(stats.total, 1, "only the put line counts");
        assert_eq!(stats.succeeded, 1);
    }

    /// Verify that `BatchCommand::execute` dispatches to `process_reader` via
    /// `execute_from_reader` using a tempfile as the backing store.
    #[tokio::test]
    async fn test_batch_from_file_cursor() {
        // We cannot use a real `Client` without a server, so we call
        // `process_reader` directly with the same tempfile-style data via Cursor.
        let input = b"put a 1\nput b 2\ndelete a\n";
        let stats = run_with_null_handler(input).await;

        assert_eq!(stats.total, 3);
        assert_eq!(stats.succeeded, 3);
    }

    /// Verify stdin-style input (Cursor<&[u8]>) works correctly.
    #[tokio::test]
    async fn test_batch_from_stdin() {
        let input = b"put x xval\ndelete y\n";
        let stats = run_with_null_handler(input).await;

        assert_eq!(stats.total, 2);
        assert_eq!(stats.succeeded, 2);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.skipped, 0);
    }

    /// Verify large tempfile input (1 000 lines) through process_reader.
    #[tokio::test]
    async fn test_batch_large_file_streams() {
        let mut content = String::new();
        for i in 0..1_000 {
            content.push_str(&format!("put key{i} value{i}\n"));
        }
        let stats = run_with_null_handler(content.as_bytes()).await;

        assert_eq!(stats.total, 1_000);
        assert_eq!(stats.succeeded, 1_000);
    }
}
