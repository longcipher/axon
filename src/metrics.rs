use std::{collections::HashMap, sync::Mutex, time::Instant};

use metrics::{
    Unit, counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
};
use once_cell::sync::Lazy;

// Axon-specific metric names
pub const AXON_BACKEND_HEALTH_STATUS: &str = "axon_backend_health_status";
pub const AXON_REQUESTS_TOTAL: &str = "axon_requests_total";
pub const AXON_REQUEST_DURATION_SECONDS: &str = "axon_request_duration_seconds";
pub const AXON_BACKEND_REQUESTS_TOTAL: &str = "axon_backend_requests_total";
pub const AXON_BACKEND_REQUEST_DURATION_SECONDS: &str = "axon_backend_request_duration_seconds";
pub const AXON_ACTIVE_CONNECTIONS: &str = "axon_active_connections";
pub const AXON_ACTIVE_REQUESTS: &str = "axon_active_requests";

/// Storage for backend health status gauges
pub static BACKEND_HEALTH_GAUGES: Lazy<Mutex<HashMap<String, f64>>> = Lazy::new(|| {
    // Register metric descriptions
    describe_gauge!(
        AXON_BACKEND_HEALTH_STATUS,
        "Health status of individual backends (1 for healthy, 0 for unhealthy)"
    );
    describe_counter!(
        AXON_REQUESTS_TOTAL,
        Unit::Count,
        "Total number of HTTP requests processed by the gateway."
    );
    describe_histogram!(
        AXON_REQUEST_DURATION_SECONDS,
        Unit::Seconds,
        "Latency of HTTP requests processed by the gateway."
    );
    describe_counter!(
        AXON_BACKEND_REQUESTS_TOTAL,
        Unit::Count,
        "Total number of HTTP requests forwarded to backend services."
    );
    describe_histogram!(
        AXON_BACKEND_REQUEST_DURATION_SECONDS,
        Unit::Seconds,
        "Latency of HTTP requests forwarded to backend services."
    );
    describe_gauge!(
        AXON_ACTIVE_CONNECTIONS,
        "Number of currently active connections to the gateway."
    );
    describe_gauge!(
        AXON_ACTIVE_REQUESTS,
        "Number of currently active requests being processed."
    );

    Mutex::new(HashMap::new())
});

/// Set the health status of a backend
pub fn set_backend_health_status(backend_id: &str, is_healthy: bool) {
    let health_value = if is_healthy { 1.0 } else { 0.0 };

    if let Ok(mut gauges) = BACKEND_HEALTH_GAUGES.lock() {
        gauges.insert(backend_id.to_string(), health_value);
    } else {
        tracing::error!("Failed to acquire lock for backend health gauges");
        return;
    }

    let backend_label = backend_id.to_string();
    gauge!(AXON_BACKEND_HEALTH_STATUS, "backend" => backend_label).set(health_value);
}

/// Increment the total request counter
pub fn increment_request_total(path: &str, method: &str, status: u16) {
    counter!(
        AXON_REQUESTS_TOTAL,
        "path" => path.to_string(),
        "method" => method.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

/// Record request duration
pub fn record_request_duration(path: &str, method: &str, duration: std::time::Duration) {
    histogram!(
        AXON_REQUEST_DURATION_SECONDS,
        "path" => path.to_string(),
        "method" => method.to_string()
    )
    .record(duration.as_secs_f64());
}

/// Increment backend request counter
pub fn increment_backend_request_total(backend: &str, path: &str, method: &str, status: u16) {
    counter!(
        AXON_BACKEND_REQUESTS_TOTAL,
        "backend" => backend.to_string(),
        "path" => path.to_string(),
        "method" => method.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

/// Record backend request duration
pub fn record_backend_request_duration(
    backend: &str,
    path: &str,
    method: &str,
    duration: std::time::Duration,
) {
    histogram!(
        AXON_BACKEND_REQUEST_DURATION_SECONDS,
        "backend" => backend.to_string(),
        "path" => path.to_string(),
        "method" => method.to_string()
    )
    .record(duration.as_secs_f64());
}

/// Set the number of active connections
pub fn set_active_connections(count: usize) {
    gauge!(AXON_ACTIVE_CONNECTIONS).set(count as f64);
}

/// Set the number of active requests
pub fn set_active_requests(count: u64) {
    gauge!(AXON_ACTIVE_REQUESTS).set(count as f64);
}

/// Helper struct for measuring request duration using RAII
pub struct RequestTimer {
    start: Instant,
    path: String,
    method: String,
}

impl RequestTimer {
    pub fn new(path: &str, method: &str) -> Self {
        Self {
            start: Instant::now(),
            path: path.to_string(),
            method: method.to_string(),
        }
    }
}

impl Drop for RequestTimer {
    fn drop(&mut self) {
        record_request_duration(&self.path, &self.method, self.start.elapsed());
    }
}

/// Helper struct for measuring backend request duration using RAII
pub struct BackendRequestTimer {
    start: Instant,
    backend: String,
    path: String,
    method: String,
}

impl BackendRequestTimer {
    pub fn new(backend: &str, path: &str, method: &str) -> Self {
        Self {
            start: Instant::now(),
            backend: backend.to_string(),
            path: path.to_string(),
            method: method.to_string(),
        }
    }
}

impl Drop for BackendRequestTimer {
    fn drop(&mut self) {
        record_backend_request_duration(
            &self.backend,
            &self.path,
            &self.method,
            self.start.elapsed(),
        );
    }
}

/// Initialize metrics system
pub fn init_metrics() -> eyre::Result<()> {
    tracing::info!("Initializing Axon metrics system");

    // Force lazy initialization of metrics descriptions
    Lazy::force(&BACKEND_HEALTH_GAUGES);

    tracing::info!("Axon metrics system initialized successfully");
    Ok(())
}

/// Collect and return current metrics in a simple format
pub fn get_current_metrics() -> HashMap<String, f64> {
    let mut metrics = HashMap::new();

    if let Ok(gauges) = BACKEND_HEALTH_GAUGES.lock() {
        for (backend, health) in gauges.iter() {
            metrics.insert(format!("backend_health_{backend}"), *health);
        }
    }

    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_backend_health_status() {
        set_backend_health_status("http://test-backend", true);

        if let Ok(gauges) = BACKEND_HEALTH_GAUGES.lock() {
            assert_eq!(gauges.get("http://test-backend"), Some(&1.0));
        }

        set_backend_health_status("http://test-backend", false);

        if let Ok(gauges) = BACKEND_HEALTH_GAUGES.lock() {
            assert_eq!(gauges.get("http://test-backend"), Some(&0.0));
        }
    }

    #[test]
    fn test_request_timer() {
        let timer = RequestTimer::new("/test", "GET");
        // Timer will record duration when dropped
        drop(timer);
    }

    #[test]
    fn test_backend_request_timer() {
        let timer = BackendRequestTimer::new("http://backend", "/test", "POST");
        // Timer will record duration when dropped
        drop(timer);
    }

    #[test]
    fn test_init_metrics() {
        let result = init_metrics();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_current_metrics() {
        set_backend_health_status("http://test", true);
        let metrics = get_current_metrics();
        assert!(metrics.contains_key("backend_health_http://test"));
    }
}
