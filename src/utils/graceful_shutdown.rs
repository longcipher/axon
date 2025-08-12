//! Graceful shutdown coordination utilities.
//!
//! Provides a broadcast based mechanism for signaling shutdown / restart
//! events across async tasks plus convenience tokens for cooperative
//! cancellation. Supports:
//! * OS signal handling (SIGINT / SIGTERM for graceful shutdown, SIGUSR1 for restart on Unix).
//! * Manual programmatic triggering (e.g. admin API) via `trigger_shutdown`.
//! * Timeout‑guarded waiting (`wait_for_shutdown`) vs infinite wait.
//! * Lightweight cloneable `ShutdownToken` to poll or await signals.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eyre::Result;
use tokio::{signal, sync::broadcast, time::timeout};

/// Enumerates reasons a shutdown signal was emitted.
#[derive(Debug, Clone)]
pub enum ShutdownReason {
    /// Graceful shutdown requested (SIGTERM, SIGINT)
    Graceful,
    /// Restart requested (SIGUSR1)
    Restart,
    /// Force shutdown (timeout exceeded)
    Force,
}

/// Coordinates graceful shutdown / restart broadcast and signal handling.
pub struct GracefulShutdown {
    /// Broadcast sender for shutdown signals
    shutdown_tx: broadcast::Sender<ShutdownReason>,
    /// Flag indicating if shutdown has been initiated
    shutdown_initiated: Arc<AtomicBool>,
    /// Maximum time to wait for graceful shutdown
    shutdown_timeout: Duration,
}

impl GracefulShutdown {
    /// Create with default 30‑second timeout.
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create with caller supplied timeout window.
    pub fn with_timeout(shutdown_timeout: Duration) -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);
        Self {
            shutdown_tx,
            shutdown_initiated: Arc::new(AtomicBool::new(false)),
            shutdown_timeout,
        }
    }

    /// Subscribe to future shutdown events.
    pub fn subscribe(&self) -> broadcast::Receiver<ShutdownReason> {
        self.shutdown_tx.subscribe()
    }

    /// Whether we've already initiated shutdown.
    pub fn is_shutdown_initiated(&self) -> bool {
        self.shutdown_initiated.load(Ordering::Relaxed)
    }

    /// Manually trigger a shutdown / restart (idempotent).
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

    /// Start OS signal listener loop (returns after first signal handled).
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

    /// Await a shutdown signal or force after timeout, returning the reason.
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

    /// Await a shutdown signal indefinitely.
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

    /// Produce a cloneable token for cooperative cancellation in tasks.
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

/// Cloneable handle for checking / awaiting shutdown events.
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
    /// Fast check for shutdown state (no await).
    pub fn is_shutdown_initiated(&self) -> bool {
        self.shutdown_initiated.load(Ordering::Relaxed)
    }

    /// Non‑blocking attempt to consume a shutdown signal if present.
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

    /// Await a shutdown signal (falls back to Force if channel closed).
    pub async fn wait_for_shutdown(&mut self) -> ShutdownReason {
        match self.receiver.recv().await {
            Ok(reason) => reason,
            Err(_) => ShutdownReason::Force,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_graceful_shutdown_creation() {
        let shutdown = GracefulShutdown::new();
        assert!(!shutdown.is_shutdown_initiated());
    }

    #[tokio::test]
    async fn test_manual_trigger_shutdown() {
        let shutdown = GracefulShutdown::new();

        // Subscribe before triggering to ensure we can receive the message
        let mut receiver = shutdown.subscribe();

        // Trigger shutdown manually
        shutdown.trigger_shutdown(ShutdownReason::Graceful).unwrap();
        assert!(shutdown.is_shutdown_initiated());

        // Check if we can receive the signal
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
