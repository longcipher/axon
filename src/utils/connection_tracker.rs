//! Connection tracking & graceful draining utilities.
//!
//! Each TCP connection is registered with a lightweight record keeping active
//! request count and age. During shutdown the tracker can wait for in‑flight
//! requests to finish (bounded by a timeout) and optionally close idle ones.
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use scc::HashMap;
use tokio::{sync::broadcast, time::sleep};

/// Unique identifier for a connection
pub type ConnectionId = u64;

/// Information about an active connection.
#[derive(Debug)]
pub struct ConnectionInfo {
    pub id: ConnectionId,
    pub remote_addr: SocketAddr,
    pub established_at: Instant,
    pub active_requests: AtomicU64,
}

impl Clone for ConnectionInfo {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            remote_addr: self.remote_addr,
            established_at: self.established_at,
            active_requests: AtomicU64::new(self.active_requests.load(Ordering::Relaxed)),
        }
    }
}

impl ConnectionInfo {
    pub fn new(id: ConnectionId, remote_addr: SocketAddr) -> Self {
        Self {
            id,
            remote_addr,
            established_at: Instant::now(),
            active_requests: AtomicU64::new(0),
        }
    }

    pub fn increment_requests(&self) {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_requests(&self) {
        self.active_requests.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn active_request_count(&self) -> u64 {
        self.active_requests.load(Ordering::Relaxed)
    }

    pub fn is_idle(&self) -> bool {
        self.active_request_count() == 0
    }

    pub fn age(&self) -> Duration {
        self.established_at.elapsed()
    }
}

/// Manages active connections and provides graceful draining capabilities
#[derive(Clone)]
pub struct ConnectionTracker {
    connections: Arc<HashMap<ConnectionId, Arc<ConnectionInfo>>>,
    next_id: Arc<AtomicU64>,
    shutdown_tx: broadcast::Sender<()>,
}

impl ConnectionTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);
        Self {
            connections: Arc::new(HashMap::new()),
            next_id: Arc::new(AtomicU64::new(1)),
            shutdown_tx,
        }
    }

    /// Register a new connection and return its info
    /// Register a new connection and return an Arc to its info record.
    pub fn register_connection(&self, remote_addr: SocketAddr) -> Arc<ConnectionInfo> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let info = Arc::new(ConnectionInfo::new(id, remote_addr));

        let _ = self.connections.insert(id, info.clone());

        tracing::debug!(
            "Connection registered: id={}, remote_addr={}, total_connections={}",
            id,
            remote_addr,
            self.connections.len()
        );

        info
    }

    /// Unregister a connection
    /// Remove (unregister) a connection by id.
    pub fn unregister_connection(&self, connection_id: ConnectionId) {
        if let Some((_, info)) = self.connections.remove(&connection_id) {
            tracing::debug!(
                "Connection unregistered: id={}, age={:?}, total_connections={}",
                connection_id,
                info.age(),
                self.connections.len()
            );
        }
    }

    /// Get connection information
    /// Lookup a connection info record by id.
    pub fn get_connection_info(&self, connection_id: ConnectionId) -> Option<Arc<ConnectionInfo>> {
        self.connections
            .read(&connection_id, |_, info| info.clone())
    }

    /// Get the total number of active connections
    /// Current number of active (registered) connections.
    pub fn active_connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Get the total number of requests across all connections
    /// Sum of active requests across all connections.
    pub fn total_active_requests(&self) -> u64 {
        let mut total = 0;
        self.connections.scan(|_, info| {
            total += info.active_request_count();
        });
        total
    }

    /// Get all connection information
    /// Return a snapshot list of all connection records.
    pub fn get_all_connections(&self) -> Vec<Arc<ConnectionInfo>> {
        let mut connections = Vec::new();
        self.connections.scan(|_, info| {
            connections.push(info.clone());
        });
        connections
    }

    /// Check if any connections have active requests
    /// Whether any connection currently has >0 active requests.
    pub fn has_active_requests(&self) -> bool {
        let mut has_active = false;
        self.connections.scan(|_, info| {
            if info.active_request_count() > 0 {
                has_active = true;
            }
        });
        has_active
    }

    /// Get connections that are idle (no active requests)
    /// All connections with zero active requests.
    pub fn get_idle_connections(&self) -> Vec<Arc<ConnectionInfo>> {
        let mut idle_connections = Vec::new();
        self.connections.scan(|_, info| {
            if info.is_idle() {
                idle_connections.push(info.clone());
            }
        });
        idle_connections
    }

    /// Get connections with active requests
    /// All connections with at least one active request.
    pub fn get_active_connections(&self) -> Vec<Arc<ConnectionInfo>> {
        let mut active_connections = Vec::new();
        self.connections.scan(|_, info| {
            if !info.is_idle() {
                active_connections.push(info.clone());
            }
        });
        active_connections
    }

    /// Signal shutdown to all connections
    /// Broadcast a shutdown signal to subscribers.
    pub fn signal_shutdown(&self) {
        tracing::info!("Signaling shutdown to all connections");
        let _ = self.shutdown_tx.send(());
    }

    /// Subscribe to shutdown signals
    /// Subscribe to the shutdown broadcast channel.
    pub fn subscribe_shutdown(&self) -> broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    /// Wait for all connections to drain (become idle) with timeout
    /// Wait until all connections are idle or the timeout elapses.
    pub async fn wait_for_drain(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        let mut check_interval = Duration::from_millis(100);

        while start.elapsed() < timeout {
            if !self.has_active_requests() {
                tracing::info!("All connections drained successfully");
                return true;
            }

            let active_count = self.total_active_requests();
            tracing::debug!(
                "Waiting for connections to drain: {} active requests remaining, elapsed: {:?}",
                active_count,
                start.elapsed()
            );

            sleep(check_interval).await;

            // Exponential backoff up to 1 second
            check_interval = std::cmp::min(check_interval * 2, Duration::from_secs(1));
        }

        let remaining_requests = self.total_active_requests();
        tracing::warn!(
            "Drain timeout exceeded: {} active requests still remaining after {:?}",
            remaining_requests,
            timeout
        );

        false
    }

    /// Force close all idle connections
    /// Force removal of all idle connections.
    pub fn close_idle_connections(&self) {
        let idle_connections = self.get_idle_connections();
        tracing::info!("Closing {} idle connections", idle_connections.len());

        for connection in idle_connections {
            self.unregister_connection(connection.id);
        }
    }

    /// Get connection statistics
    /// Aggregate snapshot statistics.
    pub fn get_stats(&self) -> ConnectionStats {
        let all_connections = self.get_all_connections();
        let total_connections = all_connections.len();
        let mut total_requests = 0;
        let mut idle_connections = 0;
        let mut oldest_connection_age = Duration::from_secs(0);

        for connection in &all_connections {
            total_requests += connection.active_request_count();
            if connection.is_idle() {
                idle_connections += 1;
            }

            let age = connection.age();
            if age > oldest_connection_age {
                oldest_connection_age = age;
            }
        }

        ConnectionStats {
            total_connections,
            active_connections: total_connections - idle_connections,
            idle_connections,
            total_active_requests: total_requests,
            oldest_connection_age,
        }
    }
}

impl Default for ConnectionTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about current connections
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub total_connections: usize,
    pub active_connections: usize,
    pub idle_connections: usize,
    pub total_active_requests: u64,
    pub oldest_connection_age: Duration,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn test_connection_registration() {
        let tracker = ConnectionTracker::new();
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let conn = tracker.register_connection(addr);
        assert_eq!(conn.remote_addr, addr);
        assert_eq!(tracker.active_connection_count(), 1);

        tracker.unregister_connection(conn.id);
        assert_eq!(tracker.active_connection_count(), 0);
    }

    #[tokio::test]
    async fn test_request_tracking() {
        let tracker = ConnectionTracker::new();
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let conn = tracker.register_connection(addr);
        assert_eq!(conn.active_request_count(), 0);
        assert!(conn.is_idle());

        conn.increment_requests();
        assert_eq!(conn.active_request_count(), 1);
        assert!(!conn.is_idle());

        conn.decrement_requests();
        assert_eq!(conn.active_request_count(), 0);
        assert!(conn.is_idle());
    }

    #[tokio::test]
    async fn test_drain_functionality() {
        let tracker = ConnectionTracker::new();
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let conn = tracker.register_connection(addr);
        conn.increment_requests();

        // Should not drain immediately with active requests
        assert!(!tracker.wait_for_drain(Duration::from_millis(50)).await);

        // Decrement request and should drain
        conn.decrement_requests();
        assert!(tracker.wait_for_drain(Duration::from_millis(50)).await);
    }

    #[tokio::test]
    async fn test_connection_stats() {
        let tracker = ConnectionTracker::new();
        let addr1: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:8081".parse().unwrap();

        let conn1 = tracker.register_connection(addr1);
        let _conn2 = tracker.register_connection(addr2);

        conn1.increment_requests();
        // conn2 remains idle

        let stats = tracker.get_stats();
        assert_eq!(stats.total_connections, 2);
        assert_eq!(stats.active_connections, 1);
        assert_eq!(stats.idle_connections, 1);
        assert_eq!(stats.total_active_requests, 1);
    }
}
