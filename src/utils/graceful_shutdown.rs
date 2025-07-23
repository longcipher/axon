use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eyre::Result;
use tokio::{signal, sync::broadcast, time::timeout};

/// Represents different shutdown reasons
#[derive(Debug, Clone)]
pub enum ShutdownReason {
    /// Graceful shutdown requested (SIGTERM, SIGINT)
    Graceful,
    /// Restart requested (SIGUSR1)
    Restart,
    /// Force shutdown (timeout exceeded)
    Force,
}

/// Manages graceful shutdown and restart functionality
pub struct GracefulShutdown {
    /// Broadcast sender for shutdown signals
    shutdown_tx: broadcast::Sender<ShutdownReason>,
    /// Flag indicating if shutdown has been initiated
    shutdown_initiated: Arc<AtomicBool>,
    /// Maximum time to wait for graceful shutdown
    shutdown_timeout: Duration,
}

impl GracefulShutdown {
    /// Create a new GracefulShutdown manager with default 30-second timeout
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create a new GracefulShutdown manager with custom timeout
    pub fn with_timeout(shutdown_timeout: Duration) -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);
        Self {
            shutdown_tx,
            shutdown_initiated: Arc::new(AtomicBool::new(false)),
            shutdown_timeout,
        }
    }

    /// Get a receiver for shutdown signals
    pub fn subscribe(&self) -> broadcast::Receiver<ShutdownReason> {
        self.shutdown_tx.subscribe()
    }

    /// Check if shutdown has been initiated
    pub fn is_shutdown_initiated(&self) -> bool {
        self.shutdown_initiated.load(Ordering::Relaxed)
    }

    /// Manually trigger shutdown (useful for API-triggered restarts)
    pub fn trigger_shutdown(&self, reason: ShutdownReason) -> Result<()> {
        if self
            .shutdown_initiated
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            tracing::info!("Shutdown manually triggered: {:?}", reason);
            let _ = self.shutdown_tx.send(reason);
        }
        Ok(())
    }

    /// Start listening for OS signals and manage shutdown process
    pub async fn run_signal_handler(&self) -> Result<()> {
        tracing::info!(
            "Signal handler started. Listening for SIGTERM, SIGINT (graceful shutdown) and SIGUSR1 (restart)"
        );

        // Handle different signals concurrently
        tokio::select! {
            _ = signal::ctrl_c() => {
                tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown...");
                self.initiate_shutdown(ShutdownReason::Graceful);
            }
            _ = self.wait_for_sigterm() => {
                tracing::info!("Received SIGTERM, initiating graceful shutdown...");
                self.initiate_shutdown(ShutdownReason::Graceful);
            }
            _ = self.wait_for_sigusr1() => {
                tracing::info!("Received SIGUSR1, initiating graceful restart...");
                self.initiate_shutdown(ShutdownReason::Restart);
            }
        }

        tracing::info!("Signal handler shutting down");
        Ok(())
    }

    #[cfg(unix)]
    async fn wait_for_sigterm(&self) {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
        sigterm.recv().await;
    }

    #[cfg(not(unix))]
    async fn wait_for_sigterm(&self) {
        // On non-Unix systems, we only have Ctrl+C
        std::future::pending::<()>().await;
    }

    #[cfg(unix)]
    async fn wait_for_sigusr1(&self) {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigusr1 =
            signal(SignalKind::user_defined1()).expect("Failed to register SIGUSR1 handler");
        sigusr1.recv().await;
    }

    #[cfg(not(unix))]
    async fn wait_for_sigusr1(&self) {
        // On non-Unix systems, SIGUSR1 is not available
        std::future::pending::<()>().await;
    }

    fn initiate_shutdown(&self, reason: ShutdownReason) {
        if self
            .shutdown_initiated
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            tracing::info!("Processing shutdown signal: {:?}", reason);
            if let Err(e) = self.shutdown_tx.send(reason) {
                tracing::error!("Failed to send shutdown signal: {}", e);
            }
        } else {
            tracing::warn!("Shutdown already initiated, ignoring signal");
        }
    }

    /// Wait for shutdown with timeout, returns the reason for shutdown
    pub async fn wait_for_shutdown(&self) -> ShutdownReason {
        let mut receiver = self.subscribe();

        match timeout(self.shutdown_timeout, receiver.recv()).await {
            Ok(Ok(reason)) => {
                tracing::info!("Shutdown signal received: {:?}", reason);
                reason
            }
            Ok(Err(_)) => {
                tracing::warn!("Shutdown channel closed unexpectedly");
                ShutdownReason::Force
            }
            Err(_) => {
                tracing::error!(
                    "Shutdown timeout exceeded ({:?}), forcing shutdown",
                    self.shutdown_timeout
                );
                ShutdownReason::Force
            }
        }
    }

    /// Wait indefinitely for shutdown signal (used in main application loop)
    pub async fn wait_for_shutdown_signal(&self) -> ShutdownReason {
        let mut receiver = self.subscribe();

        match receiver.recv().await {
            Ok(reason) => {
                tracing::info!("Shutdown signal received: {:?}", reason);
                reason
            }
            Err(_) => {
                tracing::warn!("Shutdown channel closed unexpectedly");
                ShutdownReason::Force
            }
        }
    }

    /// Create a shutdown token that can be used to cancel operations
    pub fn shutdown_token(&self) -> ShutdownToken {
        ShutdownToken {
            receiver: self.subscribe(),
            shutdown_initiated: self.shutdown_initiated.clone(),
        }
    }
}

impl Default for GracefulShutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// A token that can be used to check for shutdown signals
pub struct ShutdownToken {
    receiver: broadcast::Receiver<ShutdownReason>,
    shutdown_initiated: Arc<AtomicBool>,
}

impl Clone for ShutdownToken {
    fn clone(&self) -> Self {
        Self {
            receiver: self.receiver.resubscribe(),
            shutdown_initiated: self.shutdown_initiated.clone(),
        }
    }
}

impl ShutdownToken {
    /// Check if shutdown has been initiated
    pub fn is_shutdown_initiated(&self) -> bool {
        self.shutdown_initiated.load(Ordering::Relaxed)
    }

    /// Wait for shutdown signal (non-blocking check)
    pub fn try_shutdown(&mut self) -> Option<ShutdownReason> {
        match self.receiver.try_recv() {
            Ok(reason) => Some(reason),
            Err(broadcast::error::TryRecvError::Empty) => None,
            Err(broadcast::error::TryRecvError::Closed) => Some(ShutdownReason::Force),
            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                // If we're lagged, assume shutdown was requested
                Some(ShutdownReason::Force)
            }
        }
    }

    /// Wait for shutdown signal (blocking)
    pub async fn wait_for_shutdown(&mut self) -> ShutdownReason {
        match self.receiver.recv().await {
            Ok(reason) => reason,
            Err(_) => ShutdownReason::Force,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::sleep;

    use super::*;

    #[tokio::test]
    async fn test_graceful_shutdown_creation() {
        let shutdown = GracefulShutdown::new();
        assert!(!shutdown.is_shutdown_initiated());
    }

    #[tokio::test]
    async fn test_manual_trigger_shutdown() {
        let shutdown = GracefulShutdown::new();

        // Trigger shutdown manually
        shutdown.trigger_shutdown(ShutdownReason::Graceful).unwrap();
        assert!(shutdown.is_shutdown_initiated());

        // Get a receiver and check if we can receive the signal
        let mut receiver = shutdown.subscribe();
        let reason = receiver.try_recv().unwrap();
        assert!(matches!(reason, ShutdownReason::Graceful));
    }

    #[tokio::test]
    async fn test_shutdown_token() {
        let shutdown = GracefulShutdown::new();
        let mut token = shutdown.shutdown_token();

        assert!(!token.is_shutdown_initiated());
        assert!(token.try_shutdown().is_none());

        // Trigger shutdown
        shutdown.trigger_shutdown(ShutdownReason::Restart).unwrap();

        assert!(token.is_shutdown_initiated());
        let reason = token.try_shutdown().unwrap();
        assert!(matches!(reason, ShutdownReason::Restart));
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let shutdown = GracefulShutdown::new();
        let mut receiver1 = shutdown.subscribe();
        let mut receiver2 = shutdown.subscribe();

        shutdown.trigger_shutdown(ShutdownReason::Graceful).unwrap();

        assert!(matches!(
            receiver1.try_recv().unwrap(),
            ShutdownReason::Graceful
        ));
        assert!(matches!(
            receiver2.try_recv().unwrap(),
            ShutdownReason::Graceful
        ));
    }
}
