use std::io::IsTerminal;

use eyre::{Result, WrapErr};
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize structured logging with automatic format detection
/// Uses human-friendly format for terminal output, JSON for non-terminal
pub fn init_tracing() -> Result<()> {
    let is_terminal = std::io::stderr().is_terminal();

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if is_terminal {
        // Terminal output: human-friendly, compact format
        Registry::default()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .compact()
                    .with_target(false)
                    .with_thread_ids(false)
                    .with_file(false)
                    .with_line_number(false)
                    .with_ansi(true),
            )
            .init();
        tracing::info!("🚀 Axon logging initialized (terminal mode)");
    } else {
        // Non-terminal: structured JSON format for log aggregation
        Registry::default()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(false)
                    .with_span_list(false)
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_file(false)
                    .with_line_number(false),
            )
            .init();
        tracing::info!("Axon logging initialized (JSON mode)");
    }

    Ok(())
}

/// Initialize console-friendly logging for development
pub fn init_console_tracing() -> Result<()> {
    tracing::info!("Initializing Axon console logging");

    Registry::default()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .pretty()
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        )
        .init();

    tracing::info!("Axon console logging initialized successfully");
    Ok(())
}

/// Initialize tracing with custom configuration
pub fn init_tracing_with_config(level: &str, json_format: bool, include_spans: bool) -> Result<()> {
    tracing::info!(
        "Initializing Axon logging with level: {}, json: {}, spans: {}",
        level,
        json_format,
        include_spans
    );

    let env_filter =
        EnvFilter::try_new(level).wrap_err_with(|| format!("Invalid log level: {level}"))?;

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    if json_format {
        Registry::default()
            .with(env_filter)
            .with(
                fmt_layer
                    .json()
                    .with_current_span(include_spans)
                    .with_span_list(include_spans),
            )
            .init();
    } else {
        Registry::default()
            .with(env_filter)
            .with(fmt_layer.pretty().with_ansi(true))
            .init();
    }

    tracing::info!("Axon logging initialized with custom configuration");
    Ok(())
}

/// Shutdown tracing gracefully
pub fn shutdown_tracing() {
    tracing::info!("Axon tracing shutdown initiated");
    // tracing-subscriber doesn't provide explicit shutdown,
    // but we can flush any pending logs
    tracing::info!("Axon tracing shutdown complete");
}

/// Configure tracing for specific service components
pub fn configure_component_tracing(component: &str) -> tracing::Span {
    tracing::info_span!("component", name = component)
}

/// Create a request-scoped tracing span with comprehensive request info
pub fn create_request_span(
    method: &str,
    path: &str,
    request_id: &str,
    client_ip: Option<&str>,
    user_agent: Option<&str>,
) -> tracing::Span {
    tracing::info_span!(
        "request",
        http.method = method,
        http.path = path,
        request.id = request_id,
        client.ip = client_ip,
        http.user_agent = user_agent,
        http.status_code = tracing::field::Empty,
        backend.url = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
        bytes_sent = tracing::field::Empty,
        bytes_received = tracing::field::Empty,
    )
}

/// Create a backend request span
pub fn create_backend_span(backend_url: &str, method: &str, path: &str) -> tracing::Span {
    tracing::info_span!(
        "backend_request",
        backend.url = backend_url,
        http.method = method,
        http.path = path,
        http.status_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_tracing_with_config() {
        let result = init_tracing_with_config("debug", true, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_request_span() {
        let span = create_request_span(
            "GET",
            "/api/test",
            "req-123",
            Some("192.168.1.1"),
            Some("curl/7.68.0"),
        );
        assert_eq!(
            span.metadata().expect("Should have metadata").name(),
            "request"
        );
    }

    #[test]
    fn test_create_backend_span() {
        let span = create_backend_span("http://backend", "POST", "/data");
        assert_eq!(
            span.metadata().expect("Should have metadata").name(),
            "backend_request"
        );
    }

    #[test]
    fn test_configure_component_tracing() {
        let span = configure_component_tracing("health_checker");
        assert_eq!(
            span.metadata().expect("Should have metadata").name(),
            "component"
        );
    }
}
