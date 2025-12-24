//! OpenTelemetry metrics helpers for Axon.
//!
//! This module provides metrics using OpenTelemetry OTLP for high-performance export.
//!
//! Provided metrics (labels vary by family):
//! * `axon_requests_total` (counter)
//! * `axon_request_duration_seconds` (histogram)
//! * `axon_backend_requests_total` (counter)
//! * `axon_backend_request_duration_seconds` (histogram)
//! * `axon_backend_health_status` (gauge per backend)
//! * `axon_active_connections` (gauge)
//! * `axon_active_requests` (gauge)
//!
//! The `*_timer` structs leverage `Drop` to record durations safely even when
//! early returns or errors occur.

use std::{collections::HashMap, sync::Mutex, time::Instant};

use once_cell::sync::Lazy;
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram},
};

// Axon-specific metric names
pub const AXON_BACKEND_HEALTH_STATUS: &str = "axon_backend_health_status";
pub const AXON_REQUESTS_TOTAL: &str = "axon_requests_total"; // labels: path, method, status, protocol
pub const AXON_REQUEST_DURATION_SECONDS: &str = "axon_request_duration_seconds"; // labels: path, method, protocol
pub const AXON_HTTP3_REQUESTS_TOTAL: &str = "axon_http3_requests_total"; // dedicated http3 counter (experimental)
pub const AXON_HTTP3_REQUEST_DURATION_SECONDS: &str = "axon_http3_request_duration_seconds"; // experimental
pub const AXON_BACKEND_REQUESTS_TOTAL: &str = "axon_backend_requests_total";
pub const AXON_BACKEND_REQUEST_DURATION_SECONDS: &str = "axon_backend_request_duration_seconds";
pub const AXON_ACTIVE_CONNECTIONS: &str = "axon_active_connections";
pub const AXON_ACTIVE_REQUESTS: &str = "axon_active_requests";
pub const AXON_WEBSOCKET_CONNECTIONS_TOTAL: &str = "axon_websocket_connections_total";
pub const AXON_WEBSOCKET_MESSAGES_TOTAL: &str = "axon_websocket_messages_total"; // labels: direction, opcode
pub const AXON_WEBSOCKET_BYTES_TOTAL: &str = "axon_websocket_bytes_total"; // labels: direction
pub const AXON_WEBSOCKET_CLOSE_CODES_TOTAL: &str = "axon_websocket_close_codes_total"; // labels: code
pub const AXON_WAF_VIOLATIONS_TOTAL: &str = "axon_waf_violations_total"; // labels: threat_type, threat_level, blocked
pub const AXON_WAF_CHECKS_TOTAL: &str = "axon_waf_checks_total"; // labels: result

/// Global meter
static METER: Lazy<opentelemetry::metrics::Meter> = Lazy::new(|| global::meter("axon"));

/// Counters
static REQUESTS_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_REQUESTS_TOTAL).build());
static HTTP3_REQUESTS_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_HTTP3_REQUESTS_TOTAL).build());
static BACKEND_REQUESTS_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_BACKEND_REQUESTS_TOTAL).build());
static WEBSOCKET_CONNECTIONS_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_WEBSOCKET_CONNECTIONS_TOTAL).build());
static WEBSOCKET_MESSAGES_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_WEBSOCKET_MESSAGES_TOTAL).build());
static WEBSOCKET_BYTES_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_WEBSOCKET_BYTES_TOTAL).build());
static WEBSOCKET_CLOSE_CODES_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_WEBSOCKET_CLOSE_CODES_TOTAL).build());
static WAF_VIOLATIONS_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_WAF_VIOLATIONS_TOTAL).build());
static WAF_CHECKS_TOTAL: Lazy<Counter<u64>> =
    Lazy::new(|| METER.u64_counter(AXON_WAF_CHECKS_TOTAL).build());

/// Histograms
static REQUEST_DURATION_SECONDS: Lazy<Histogram<f64>> =
    Lazy::new(|| METER.f64_histogram(AXON_REQUEST_DURATION_SECONDS).build());
static HTTP3_REQUEST_DURATION_SECONDS: Lazy<Histogram<f64>> = Lazy::new(|| {
    METER
        .f64_histogram(AXON_HTTP3_REQUEST_DURATION_SECONDS)
        .build()
});
static BACKEND_REQUEST_DURATION_SECONDS: Lazy<Histogram<f64>> = Lazy::new(|| {
    METER
        .f64_histogram(AXON_BACKEND_REQUEST_DURATION_SECONDS)
        .build()
});

/// Gauges
static ACTIVE_CONNECTIONS: Lazy<Gauge<f64>> =
    Lazy::new(|| METER.f64_gauge(AXON_ACTIVE_CONNECTIONS).build());
static ACTIVE_REQUESTS: Lazy<Gauge<f64>> =
    Lazy::new(|| METER.f64_gauge(AXON_ACTIVE_REQUESTS).build());

/// Storage for backend health status gauges
pub static BACKEND_HEALTH_GAUGES: Lazy<Mutex<HashMap<String, f64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Initialize OpenTelemetry metrics with OTLP exporter
pub async fn init_metrics() -> eyre::Result<()> {
    use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .build()?;

    let reader = PeriodicReader::builder(exporter).build();

    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    opentelemetry::global::set_meter_provider(provider);

    Ok(())
}

/// Set (and record) the health status gauge for a backend.
pub fn set_backend_health_status(backend_id: &str, is_healthy: bool) {
    let health_value = if is_healthy { 1.0 } else { 0.0 };

    if let Ok(mut gauges) = BACKEND_HEALTH_GAUGES.lock() {
        gauges.insert(backend_id.to_string(), health_value);
    } else {
        tracing::error!("Failed to acquire lock for backend health gauges");
    }

    // Note: OpenTelemetry gauge requires observer pattern, simplified here
    // In production, use ObservableGauge
}

/// Increment the total request counter for an inbound gateway request.
pub fn increment_request_total(path: &str, method: &str, status: u16, protocol: &str) {
    REQUESTS_TOTAL.add(
        1,
        &[
            KeyValue::new("path", path.to_string()),
            KeyValue::new("method", method.to_string()),
            KeyValue::new("status", status.to_string()),
            KeyValue::new("protocol", protocol.to_string()),
        ],
    );
    if protocol == "http3" {
        HTTP3_REQUESTS_TOTAL.add(1, &[]);
    }
}

/// Record a completed inbound request's duration.
pub fn record_request_duration(
    path: &str,
    method: &str,
    protocol: &str,
    duration: std::time::Duration,
) {
    REQUEST_DURATION_SECONDS.record(
        duration.as_secs_f64(),
        &[
            KeyValue::new("path", path.to_string()),
            KeyValue::new("method", method.to_string()),
            KeyValue::new("protocol", protocol.to_string()),
        ],
    );
    if protocol == "http3" {
        HTTP3_REQUEST_DURATION_SECONDS.record(duration.as_secs_f64(), &[]);
    }
}

/// Increment total count of proxied backend requests.
pub fn increment_backend_request_total(backend: &str, path: &str, method: &str, status: u16) {
    BACKEND_REQUESTS_TOTAL.add(
        1,
        &[
            KeyValue::new("backend", backend.to_string()),
            KeyValue::new("path", path.to_string()),
            KeyValue::new("method", method.to_string()),
            KeyValue::new("status", status.to_string()),
        ],
    );
}

/// Record a completed backend request duration.
pub fn record_backend_request_duration(
    backend: &str,
    path: &str,
    method: &str,
    duration: std::time::Duration,
) {
    BACKEND_REQUEST_DURATION_SECONDS.record(
        duration.as_secs_f64(),
        &[
            KeyValue::new("backend", backend.to_string()),
            KeyValue::new("path", path.to_string()),
            KeyValue::new("method", method.to_string()),
        ],
    );
}

/// Set current active connection count.
pub fn set_active_connections(count: usize) {
    // Simplified, in production use ObservableGauge
    ACTIVE_CONNECTIONS.record(count as f64, &[]);
}

/// Set current active in‑flight request count.
pub fn set_active_requests(count: u64) {
    // Simplified, in production use ObservableGauge
    ACTIVE_REQUESTS.record(count as f64, &[]);
}

/// RAII helper measuring inbound request duration.
pub struct RequestTimer {
    start: Instant,
    path: String,
    method: String,
    protocol: String,
}

impl RequestTimer {
    pub fn new(path: &str, method: &str, protocol: &str) -> Self {
        Self {
            start: Instant::now(),
            path: path.to_string(),
            method: method.to_string(),
            protocol: protocol.to_string(),
        }
    }
}

impl Drop for RequestTimer {
    fn drop(&mut self) {
        record_request_duration(
            &self.path,
            &self.method,
            &self.protocol,
            self.start.elapsed(),
        );
    }
}

/// RAII helper measuring backend request duration.
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

/// Increment WebSocket connection counter.
pub fn increment_ws_connections() {
    WEBSOCKET_CONNECTIONS_TOTAL.add(1, &[]);
}

/// Record a WebSocket message (direction ingress/egress, opcode string).
pub fn increment_ws_message(direction: &str, opcode: &str) {
    WEBSOCKET_MESSAGES_TOTAL.add(
        1,
        &[
            KeyValue::new("direction", direction.to_string()),
            KeyValue::new("opcode", opcode.to_string()),
        ],
    );
}

/// Add bytes transferred for WebSocket payload.
pub fn add_ws_bytes(direction: &str, bytes: usize) {
    WEBSOCKET_BYTES_TOTAL.add(
        bytes as u64,
        &[KeyValue::new("direction", direction.to_string())],
    );
}

/// Increment close code occurrence.
pub fn increment_ws_close_code(code: u16) {
    WEBSOCKET_CLOSE_CODES_TOTAL.add(1, &[KeyValue::new("code", code.to_string())]);
}

/// Collect a snapshot of gauge values used for ad‑hoc exports.
pub fn get_current_metrics() -> HashMap<String, f64> {
    let mut metrics = HashMap::new();

    if let Ok(gauges) = BACKEND_HEALTH_GAUGES.lock() {
        for (backend, health) in gauges.iter() {
            metrics.insert(format!("backend_health_{backend}"), *health);
        }
    }

    metrics
}

/// Record a WAF violation
pub fn record_waf_violation(threat_type: &str, threat_level: &str, blocked: bool) {
    WAF_VIOLATIONS_TOTAL.add(
        1,
        &[
            KeyValue::new("threat_type", threat_type.to_string()),
            KeyValue::new("threat_level", threat_level.to_string()),
            KeyValue::new(
                "blocked",
                if blocked { "true" } else { "false" }.to_string(),
            ),
        ],
    );
}

/// Record a WAF check (pass or fail)
pub fn record_waf_check(passed: bool) {
    WAF_CHECKS_TOTAL.add(
        1,
        &[KeyValue::new(
            "result",
            if passed { "pass" } else { "fail" }.to_string(),
        )],
    );
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
        let timer = RequestTimer::new("/test", "GET", "http");
        // Timer will record duration when dropped
        drop(timer);
    }

    #[test]
    fn test_backend_request_timer() {
        let timer = BackendRequestTimer::new("http://backend", "/test", "POST");
        // Timer will record duration when dropped
        drop(timer);
    }

    #[tokio::test]
    async fn test_init_metrics() {
        let result = init_metrics().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_current_metrics() {
        set_backend_health_status("http://test", true);
        let metrics = get_current_metrics();
        assert!(metrics.contains_key("backend_health_http://test"));
    }
}
