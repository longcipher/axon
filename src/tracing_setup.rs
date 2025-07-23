use eyre::{Result, WrapErr};
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize structured logging with JSON output for Axon
pub fn init_tracing() -> Result<()> {
    tracing::info!("Initializing Axon structured logging with JSON output");

    Registry::default()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(true)
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        )
        .init();

    tracing::info!("Axon structured logging initialized successfully");
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

/// Create a request-scoped tracing span
pub fn create_request_span(method: &str, path: &str, request_id: &str) -> tracing::Span {
    tracing::info_span!(
        "request",
        http.method = method,
        http.path = path,
        request.id = request_id,
        http.status_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
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
        let span = create_request_span("GET", "/api/test", "req-123");
        assert_eq!(span.metadata().name(), "request");
    }

    #[test]
    fn test_create_backend_span() {
        let span = create_backend_span("http://backend", "POST", "/data");
        assert_eq!(span.metadata().name(), "backend_request");
    }

    #[test]
    fn test_configure_component_tracing() {
        let span = configure_component_tracing("health_checker");
        assert_eq!(span.metadata().name(), "component");
    }
}
