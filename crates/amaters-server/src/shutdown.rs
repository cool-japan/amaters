//! Graceful shutdown handling
//!
//! This module provides signal handling for graceful server shutdown,
//! coordinating the shutdown of all components in the correct order.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Shutdown coordinator
///
/// Manages graceful shutdown across all server components
#[derive(Clone)]
pub struct ShutdownCoordinator {
    /// Broadcast channel for shutdown signal
    sender: broadcast::Sender<()>,
    /// Atomic flag indicating shutdown initiated
    shutdown_initiated: Arc<AtomicBool>,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self {
            sender,
            shutdown_initiated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Subscribe to shutdown notifications
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.sender.subscribe()
    }

    /// Initiate shutdown
    pub fn shutdown(&self) {
        if self.shutdown_initiated.swap(true, Ordering::SeqCst) {
            // Already shutting down
            return;
        }

        info!("Initiating graceful shutdown");

        // Broadcast shutdown signal to all subscribers
        if let Err(e) = self.sender.send(()) {
            warn!("Failed to broadcast shutdown signal: {}", e);
        }
    }

    /// Check if shutdown has been initiated
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_initiated.load(Ordering::SeqCst)
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Setup signal handlers for graceful shutdown
///
/// Listens for SIGTERM and SIGINT signals and triggers shutdown
pub async fn setup_signal_handlers(coordinator: ShutdownCoordinator) {
    tokio::spawn(async move {
        if let Err(e) = wait_for_signal().await {
            warn!("Error setting up signal handlers: {}", e);
            return;
        }

        info!("Received shutdown signal");
        coordinator.shutdown();
    });
}

/// Wait for shutdown signal (SIGTERM or SIGINT)
async fn wait_for_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT");
            }
        }
    }

    #[cfg(not(unix))]
    {
        use tokio::signal;
        signal::ctrl_c().await?;
        info!("Received Ctrl+C");
    }

    Ok(())
}

/// Shutdown guard for automatic cleanup
///
/// Triggers shutdown when dropped (useful for panic recovery)
pub struct ShutdownGuard {
    coordinator: ShutdownCoordinator,
    disarmed: Arc<AtomicBool>,
}

impl ShutdownGuard {
    /// Create a new shutdown guard
    pub fn new(coordinator: ShutdownCoordinator) -> Self {
        Self {
            coordinator,
            disarmed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Disarm the guard (won't trigger shutdown on drop)
    pub fn disarm(&self) {
        self.disarmed.store(true, Ordering::SeqCst);
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if !self.disarmed.load(Ordering::SeqCst) {
            warn!("ShutdownGuard dropped without disarming - triggering shutdown");
            self.coordinator.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let coordinator = ShutdownCoordinator::new();
        let mut receiver = coordinator.subscribe();

        assert!(!coordinator.is_shutting_down());

        coordinator.shutdown();

        assert!(coordinator.is_shutting_down());

        // Should receive shutdown signal
        let result = timeout(Duration::from_millis(100), receiver.recv()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let coordinator = ShutdownCoordinator::new();
        let mut rx1 = coordinator.subscribe();
        let mut rx2 = coordinator.subscribe();
        let mut rx3 = coordinator.subscribe();

        coordinator.shutdown();

        // All subscribers should receive the signal
        assert!(
            timeout(Duration::from_millis(100), rx1.recv())
                .await
                .is_ok()
        );
        assert!(
            timeout(Duration::from_millis(100), rx2.recv())
                .await
                .is_ok()
        );
        assert!(
            timeout(Duration::from_millis(100), rx3.recv())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_shutdown_idempotent() {
        let coordinator = ShutdownCoordinator::new();

        coordinator.shutdown();
        coordinator.shutdown(); // Second call should be a no-op

        assert!(coordinator.is_shutting_down());
    }

    #[test]
    fn test_shutdown_guard_disarm() {
        let coordinator = ShutdownCoordinator::new();
        let guard = ShutdownGuard::new(coordinator.clone());

        guard.disarm();
        drop(guard);

        // Should not have triggered shutdown
        assert!(!coordinator.is_shutting_down());
    }

    #[test]
    fn test_shutdown_guard_trigger() {
        let coordinator = ShutdownCoordinator::new();
        let guard = ShutdownGuard::new(coordinator.clone());

        drop(guard);

        // Should have triggered shutdown
        assert!(coordinator.is_shutting_down());
    }
}
