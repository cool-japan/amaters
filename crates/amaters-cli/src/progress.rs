//! Progress bar and spinner utilities for long-running operations

use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Create a progress bar for determinate operations
pub fn create_progress_bar(len: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .expect("failed to create progress bar template")
            .progress_chars("=>-"),
    );
    pb.set_message(message.to_string());
    pb
}

/// Create a spinner for indeterminate operations
pub fn create_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("failed to create spinner template"),
    );
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner
}

/// Create a progress bar for byte-based operations
pub fn create_bytes_progress_bar(len: u64, message: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")
            .expect("failed to create bytes progress bar template")
            .progress_chars("=>-"),
    );
    pb.set_message(message.to_string());
    pb
}

/// Execute a task with a spinner
pub async fn with_spinner<F, T>(message: &str, task: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let spinner = create_spinner(message);
    let result = task.await;
    spinner.finish_and_clear();
    result
}

/// Execute a task with a progress bar
pub async fn with_progress<F, T>(len: u64, message: &str, task: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let pb = create_progress_bar(len, message);
    let result = task.await;
    pb.finish_and_clear();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_progress_bar() {
        let pb = create_progress_bar(100, "Testing");
        assert_eq!(pb.length().unwrap_or(0), 100);
    }

    #[test]
    fn test_create_spinner() {
        let spinner = create_spinner("Loading");
        assert!(spinner.length().is_none());
    }

    #[test]
    fn test_create_bytes_progress_bar() {
        let pb = create_bytes_progress_bar(1024 * 1024, "Downloading");
        assert_eq!(pb.length().unwrap_or(0), 1024 * 1024);
    }

    #[tokio::test]
    async fn test_with_spinner() {
        let result = with_spinner("Testing", async { 42 }).await;
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_with_progress() {
        let result = with_progress(100, "Testing", async { 42 }).await;
        assert_eq!(result, 42);
    }
}
