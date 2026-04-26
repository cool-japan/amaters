//! Log rotation support for the AmateRS server.
//!
//! Provides configurable file-based log rotation supporting both time-based
//! (hourly/daily via `tracing-appender`) and size-based rotation (custom
//! writer that rotates when a file exceeds a byte threshold).

use std::io::Write;
use std::path::{Path, PathBuf};

use tracing_subscriber::prelude::*;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Rotation policy for the log writer.
#[derive(Debug, Clone, PartialEq)]
pub enum LogRotation {
    /// Rotate every hour (delegates to `tracing_appender`).
    Hourly,
    /// Rotate every day (delegates to `tracing_appender`).
    Daily,
    /// Rotate when the log file exceeds `u64` bytes.
    Size(u64),
    /// Never rotate (useful for testing / short-lived processes).
    Never,
}

/// Full configuration for log rotation.
#[derive(Debug, Clone)]
pub struct LogRotationConfig {
    /// Directory where log files are written.
    pub log_dir: PathBuf,
    /// Base filename prefix (e.g. `"amaters-server"`).
    pub file_prefix: String,
    /// Rotation policy.
    pub rotation: LogRotation,
    /// Maximum number of log files to retain (`0` = unlimited).
    pub max_files: usize,
    /// Whether to also write to stdout.
    pub also_stdout: bool,
}

/// RAII guard that keeps the background writer thread alive.
///
/// Drop this value only when you are ready to flush and stop logging.
pub struct LogGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Errors produced by this module.
#[derive(Debug, thiserror::Error)]
pub enum LogRotationError {
    #[error("Failed to create log directory: {0}")]
    DirectoryCreation(#[from] std::io::Error),
    #[error("Failed to initialize logger: {0}")]
    LoggerInit(String),
}

// ---------------------------------------------------------------------------
// Size-based rotating writer
// ---------------------------------------------------------------------------

/// A writer that rotates the underlying log file when it exceeds a byte
/// threshold.  After each rotation the oldest files are pruned so that at
/// most `max_files` remain (0 = unlimited).
struct SizeRotatingWriter {
    log_dir: PathBuf,
    file_prefix: String,
    threshold_bytes: u64,
    max_files: usize,
    current_file: std::fs::File,
    current_path: PathBuf,
    bytes_written: u64,
}

impl SizeRotatingWriter {
    fn new(
        log_dir: &Path,
        file_prefix: &str,
        threshold_bytes: u64,
        max_files: usize,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(log_dir)?;
        let current_path = log_dir.join(file_prefix);
        let current_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current_path)?;
        let bytes_written = current_file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            log_dir: log_dir.to_owned(),
            file_prefix: file_prefix.to_owned(),
            threshold_bytes,
            max_files,
            current_file,
            current_path,
            bytes_written,
        })
    }

    /// Rotate: rename the current file to a timestamped backup, open a fresh
    /// file at `current_path`, and clean up old backups if needed.
    fn rotate(&mut self) -> std::io::Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Flush before renaming.
        self.current_file.flush()?;

        // Build a backup name using wall-clock nanos to guarantee uniqueness.
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let backup_name = format!("{}.{}", self.file_prefix, ts);
        let backup_path = self.log_dir.join(&backup_name);
        std::fs::rename(&self.current_path, &backup_path)?;

        // Open a fresh file.
        self.current_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.current_path)?;
        self.bytes_written = 0;

        // Prune old files if requested.
        if self.max_files > 0 {
            let _ = cleanup_old_logs(&self.log_dir, &self.file_prefix, self.max_files);
        }

        Ok(())
    }
}

impl Write for SizeRotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.current_file.write(buf)?;
        self.bytes_written += n as u64;
        if self.bytes_written >= self.threshold_bytes {
            // Best-effort rotation — log a warning on failure but don't
            // surface the error to the caller (tracing internals expect
            // writes to succeed).
            if let Err(e) = self.rotate() {
                eprintln!("[amaters-server] log rotation failed: {e}");
            }
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.current_file.flush()
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set up a global [`tracing`] subscriber that writes to a rotating log file.
///
/// For time-based rotation ([`LogRotation::Hourly`] / [`LogRotation::Daily`] /
/// [`LogRotation::Never`]) this delegates to `tracing_appender`'s rolling
/// appenders.  For [`LogRotation::Size`] a custom byte-counting writer is used
/// instead.
///
/// # Errors
/// Returns [`LogRotationError::DirectoryCreation`] if the log directory cannot
/// be created, or [`LogRotationError::LoggerInit`] if a global subscriber is
/// already installed.
pub fn setup_rotating_logger(config: &LogRotationConfig) -> Result<LogGuard, LogRotationError> {
    std::fs::create_dir_all(&config.log_dir)?;

    match &config.rotation {
        LogRotation::Size(threshold) => {
            let writer = SizeRotatingWriter::new(
                &config.log_dir,
                &config.file_prefix,
                *threshold,
                config.max_files,
            )?;
            let (non_blocking, guard) = tracing_appender::non_blocking(writer);
            let file_layer = tracing_subscriber::fmt::layer().with_writer(non_blocking);

            let result = if config.also_stdout {
                let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);
                tracing_subscriber::registry()
                    .with(file_layer)
                    .with(stdout_layer)
                    .try_init()
            } else {
                tracing_subscriber::registry().with(file_layer).try_init()
            };

            result.map_err(|e| LogRotationError::LoggerInit(e.to_string()))?;
            Ok(LogGuard { _guard: guard })
        }
        _ => {
            // Time-based or Never — delegate to tracing_appender.
            let appender = build_time_appender(config);
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            let file_layer = tracing_subscriber::fmt::layer().with_writer(non_blocking);

            let result = if config.also_stdout {
                let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);
                tracing_subscriber::registry()
                    .with(file_layer)
                    .with(stdout_layer)
                    .try_init()
            } else {
                tracing_subscriber::registry().with(file_layer).try_init()
            };

            result.map_err(|e| LogRotationError::LoggerInit(e.to_string()))?;
            Ok(LogGuard { _guard: guard })
        }
    }
}

/// Scan `dir` for files whose name starts with `prefix`, sort by modification
/// time (oldest first), and delete the excess so that at most `max_files`
/// remain.
///
/// Returns the number of files deleted.  If `max_files == 0`, returns `0`
/// without deleting anything.
pub fn cleanup_old_logs(dir: &Path, prefix: &str, max_files: usize) -> std::io::Result<usize> {
    if max_files == 0 {
        return Ok(0);
    }

    // Collect matching entries with their modification times.
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with(prefix) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();

    if entries.len() <= max_files {
        return Ok(0);
    }

    // Oldest first.
    entries.sort_by_key(|(t, _)| *t);

    let to_delete = entries.len() - max_files;
    let mut deleted = 0usize;
    for (_, path) in entries.into_iter().take(to_delete) {
        std::fs::remove_file(&path)?;
        deleted += 1;
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn build_time_appender(
    config: &LogRotationConfig,
) -> tracing_appender::rolling::RollingFileAppender {
    match config.rotation {
        LogRotation::Hourly => {
            tracing_appender::rolling::hourly(&config.log_dir, &config.file_prefix)
        }
        LogRotation::Daily => {
            tracing_appender::rolling::daily(&config.log_dir, &config.file_prefix)
        }
        // Never or Size — Size is handled by the caller; Never uses `never()`.
        _ => tracing_appender::rolling::never(&config.log_dir, &config.file_prefix),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_log_rotation_config_default() {
        let config = LogRotationConfig {
            log_dir: PathBuf::from("/var/log/amaters"),
            file_prefix: "amaters-server".to_string(),
            rotation: LogRotation::Daily,
            max_files: 7,
            also_stdout: false,
        };
        assert_eq!(config.file_prefix, "amaters-server");
        assert_eq!(config.max_files, 7);
        assert_eq!(config.rotation, LogRotation::Daily);
        assert!(!config.also_stdout);
    }

    #[test]
    fn test_log_rotation_enum() {
        assert_eq!(LogRotation::Hourly, LogRotation::Hourly);
        assert_eq!(LogRotation::Daily, LogRotation::Daily);
        assert_eq!(LogRotation::Never, LogRotation::Never);
        assert_ne!(LogRotation::Hourly, LogRotation::Daily);
        assert_ne!(LogRotation::Daily, LogRotation::Never);
        assert_eq!(LogRotation::Size(1024), LogRotation::Size(1024));
        assert_ne!(LogRotation::Size(512), LogRotation::Size(1024));
    }

    fn make_temp_log_files(dir: &Path, prefix: &str, count: usize) -> std::io::Result<()> {
        for i in 0..count {
            let path = dir.join(format!("{}.{:04}", prefix, i));
            File::create(&path)?;
        }
        Ok(())
    }

    #[test]
    fn test_cleanup_old_logs_under_limit() {
        let base = std::env::temp_dir().join("amaters_cleanup_under");
        std::fs::create_dir_all(&base).expect("create temp dir");
        make_temp_log_files(&base, "test-server", 3).expect("create files");

        let deleted = cleanup_old_logs(&base, "test-server", 5).expect("cleanup");
        assert_eq!(deleted, 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_cleanup_old_logs_over_limit() {
        let base = std::env::temp_dir().join("amaters_cleanup_over");
        std::fs::create_dir_all(&base).expect("create temp dir");
        make_temp_log_files(&base, "test-server", 5).expect("create files");

        let deleted = cleanup_old_logs(&base, "test-server", 3).expect("cleanup");
        assert_eq!(deleted, 2);

        // Verify exactly 3 remain.
        let remaining = std::fs::read_dir(&base)
            .expect("read dir")
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name();
                if name.to_string_lossy().starts_with("test-server") {
                    Some(())
                } else {
                    None
                }
            })
            .count();
        assert_eq!(remaining, 3);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Verify that `SizeRotatingWriter` triggers a rotation once the threshold
    /// is exceeded.
    #[test]
    fn test_log_rotation_size_triggers() {
        let base = std::env::temp_dir().join(format!(
            "amaters_size_rotate_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("create temp dir");

        let prefix = "logtest";
        // Threshold: 100 bytes.
        let threshold: u64 = 100;
        let mut writer =
            SizeRotatingWriter::new(&base, prefix, threshold, 10).expect("create writer");

        // Write 90 bytes — should NOT yet rotate.
        let payload_a = vec![b'A'; 90];
        writer.write_all(&payload_a).expect("write A");

        let files_before_rotation: Vec<_> = std::fs::read_dir(&base)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        // Exactly 1 file at this point (the active log).
        assert_eq!(
            files_before_rotation.len(),
            1,
            "expected exactly 1 file before rotation"
        );

        // Write 20 more bytes — pushes total to 110, exceeding threshold.
        let payload_b = vec![b'B'; 20];
        writer.write_all(&payload_b).expect("write B");

        // After the write that triggered rotation there should be 2 files:
        // the backup (renamed) and the new active file.
        let files_after_rotation: Vec<_> = std::fs::read_dir(&base)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
            .collect();
        assert!(
            files_after_rotation.len() >= 2,
            "expected at least 2 files after rotation, got {}",
            files_after_rotation.len()
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
