#![allow(clippy::collapsible_if)]

use std::net::SocketAddr;

use eyre::Result;
use regex::Regex;

use crate::config::models::{
    HealthCheckConfig, LoadBalanceStrategy, RateLimitConfig, RouteConfig, RouteConfigEntry,
    ServerConfig, TlsConfig,
};

/// Validation result type alias
pub type ValidationResult<T> = Result<T, ValidationError>;

/// Validation error types
#[derive(Debug, thiserror::Error, Clone)]
pub enum ValidationError {
    #[error("Missing required field: {field}")]
    MissingField { field: String },

    #[error("Invalid field '{field}': {message}")]
    InvalidField { field: String, message: String },

    #[error("Invalid listen address '{address}': {reason}")]
    InvalidListenAddress { address: String, reason: String },

    #[error("Invalid TLS configuration: {message}")]
    InvalidTls { message: String },

    #[error("Route conflict detected: {message}")]
    RouteConflict { message: String },

    #[error("Validation failed: {message}")]
    ValidationFailed { message: String },
}

/// Server configuration validator
pub struct ServerConfigValidator;

impl ServerConfigValidator {
    /// Validate the entire server configuration
    pub fn validate(config: &ServerConfig) -> ValidationResult<()> {
        let mut errors = Vec::new();

        // Validate listen address
        if let Err(e) = Self::validate_listen_address(&config.listen_addr) {
            errors.push(e);
        }

        // Validate routes
        if config.routes.is_empty() {
            errors.push(ValidationError::MissingField {
                field: "routes".to_string(),
            });
        } else {
            for (path, entry) in &config.routes {
                for route_config in entry.iter() {
                    if let Err(mut route_errors) = Self::validate_single_route(path, route_config) {
                        errors.append(&mut route_errors);
                    }
                }
            }
        }

        if let Err(mut health_check_errors) =
            Self::validate_health_check_config(&config.health_check)
        {
            errors.append(&mut health_check_errors);
        }

        if let Some(tls_config) = &config.tls {
            if let Err(e) = Self::validate_tls_config(tls_config) {
                errors.push(e);
            }
        }

        if let Err(conflict_error_list) = Self::check_route_conflicts(&config.routes) {
            errors.extend(conflict_error_list);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::ValidationFailed {
                message: Self::format_multiple_errors(errors),
            })
        }
    }

    /// Validate listen address format
    fn validate_listen_address(address: &str) -> ValidationResult<()> {
        if address.parse::<SocketAddr>().is_err() {
            return Err(ValidationError::InvalidListenAddress {
                address: address.to_string(),
                reason: "Must be in format 'IP:PORT' (e.g., '127.0.0.1:3000' or '0.0.0.0:8080')"
                    .to_string(),
            });
        }
        Ok(())
    }

    /// Validate a single route configuration
    fn validate_single_route(path: &str, config: &RouteConfig) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        if !path.starts_with('/') {
            errors.push(ValidationError::InvalidField {
                field: format!("route path: {path}"),
                message: "Route paths must start with '/'".to_string(),
            });
        }

        // Validate route-specific configurations
        match config {
            RouteConfig::Proxy { target, host, .. } => {
                if let Err(e) = Self::validate_url(target, &format!("route '{path}' target")) {
                    errors.push(e);
                }
                if let Some(h) = host {
                    if let Err(e) = Self::validate_host(h, path) {
                        errors.push(e);
                    }
                }
            }
            RouteConfig::LoadBalance {
                targets,
                strategy,
                host,
                ..
            } => {
                if targets.is_empty() {
                    errors.push(ValidationError::InvalidField {
                        field: format!("route '{path}' targets"),
                        message: "Load balance routes must have at least one target".to_string(),
                    });
                } else {
                    for (i, target) in targets.iter().enumerate() {
                        if let Err(e) =
                            Self::validate_url(target, &format!("route '{path}' target {}", i + 1))
                        {
                            errors.push(e);
                        }
                    }
                }

                // Validate load balance strategy
                if let Err(e) = Self::validate_load_balance_strategy(path, strategy) {
                    errors.push(e);
                }

                if let Some(h) = host {
                    if let Err(e) = Self::validate_host(h, path) {
                        errors.push(e);
                    }
                }
            }
            RouteConfig::Static { root, host, .. } => {
                if !std::path::Path::new(root).exists() {
                    errors.push(ValidationError::InvalidField {
                        field: format!("route '{path}' root"),
                        message: format!("Static root directory '{root}' does not exist"),
                    });
                }
                if let Some(h) = host {
                    if let Err(e) = Self::validate_host(h, path) {
                        errors.push(e);
                    }
                }
            }
            RouteConfig::Redirect {
                target,
                status_code,
                host,
                ..
            } => {
                if target.starts_with("http://") || target.starts_with("https://") {
                    if let Err(e) =
                        Self::validate_url(target, &format!("route '{path}' redirect target"))
                    {
                        errors.push(e);
                    }
                }

                if let Some(code) = status_code {
                    if !Self::is_valid_redirect_status_code(*code) {
                        errors.push(ValidationError::InvalidField {
                            field: format!("route '{path}' redirect status_code"),
                            message: format!("Status code {code} is not a valid redirect code. Use 301, 302, 307, or 308"),
                        });
                    }
                }

                if let Some(h) = host {
                    if let Err(e) = Self::validate_host(h, path) {
                        errors.push(e);
                    }
                }
            }
            RouteConfig::Websocket {
                target,
                max_frame_size,
                max_message_size,
                host,
                ..
            } => {
                if let Err(e) = Self::validate_websocket_url(
                    target,
                    &format!("route '{path}' websocket target"),
                ) {
                    errors.push(e);
                }

                if let Some(frame_size) = max_frame_size {
                    if *frame_size == 0 {
                        errors.push(ValidationError::InvalidField {
                            field: format!("route '{path}' max_frame_size"),
                            message: "WebSocket max frame size must be greater than 0".to_string(),
                        });
                    }
                }

                if let Some(message_size) = max_message_size {
                    if *message_size == 0 {
                        errors.push(ValidationError::InvalidField {
                            field: format!("route '{path}' max_message_size"),
                            message: "WebSocket max message size must be greater than 0"
                                .to_string(),
                        });
                    }
                }

                if let Some(h) = host {
                    if let Err(e) = Self::validate_host(h, path) {
                        errors.push(e);
                    }
                }
            }
        }

        let rate_limit = match config {
            RouteConfig::Proxy { rate_limit, .. } => rate_limit,
            RouteConfig::LoadBalance { rate_limit, .. } => rate_limit,
            RouteConfig::Static { rate_limit, .. } => rate_limit,
            RouteConfig::Redirect { rate_limit, .. } => rate_limit,
            RouteConfig::Websocket { rate_limit, .. } => rate_limit,
        };

        if let Some(rate_limit) = rate_limit {
            if let Err(e) = Self::validate_rate_limit(path, rate_limit) {
                errors.push(e);
            }
        }

        let path_rewrite = match config {
            RouteConfig::Proxy { path_rewrite, .. } => path_rewrite,
            RouteConfig::LoadBalance { path_rewrite, .. } => path_rewrite,
            RouteConfig::Static { .. } => &None,
            RouteConfig::Redirect { .. } => &None,
            RouteConfig::Websocket { path_rewrite, .. } => path_rewrite,
        };

        if let Some(path_rewrite) = path_rewrite {
            if let Err(e) = Self::validate_path_rewrite(path, path_rewrite) {
                errors.push(e);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate URL format
    fn validate_url(url_str: &str, context: &str) -> ValidationResult<()> {
        match url::Url::parse(url_str) {
            Ok(url) => {
                if url.scheme() != "http" && url.scheme() != "https" {
                    return Err(ValidationError::InvalidField {
                        field: context.to_string(),
                        message: format!(
                            "URL scheme must be 'http' or 'https', got '{}'",
                            url.scheme()
                        ),
                    });
                }

                if url.host().is_none() {
                    return Err(ValidationError::InvalidField {
                        field: context.to_string(),
                        message: "URL must have a valid host".to_string(),
                    });
                }

                Ok(())
            }
            Err(e) => Err(ValidationError::InvalidField {
                field: context.to_string(),
                message: format!("Invalid URL format: {e}"),
            }),
        }
    }

    /// Validate WebSocket URL format
    fn validate_websocket_url(url_str: &str, context: &str) -> ValidationResult<()> {
        match url::Url::parse(url_str) {
            Ok(url) => {
                if url.scheme() != "ws" && url.scheme() != "wss" {
                    return Err(ValidationError::InvalidField {
                        field: context.to_string(),
                        message: format!(
                            "WebSocket URL scheme must be 'ws' or 'wss', got '{}'",
                            url.scheme()
                        ),
                    });
                }

                if url.host().is_none() {
                    return Err(ValidationError::InvalidField {
                        field: context.to_string(),
                        message: "WebSocket URL must have a valid host".to_string(),
                    });
                }

                Ok(())
            }
            Err(e) => Err(ValidationError::InvalidField {
                field: context.to_string(),
                message: format!("Invalid WebSocket URL format: {e}"),
            }),
        }
    }

    /// Validate load balance strategy
    fn validate_load_balance_strategy(
        _path: &str,
        _strategy: &LoadBalanceStrategy,
    ) -> ValidationResult<()> {
        // All available strategies are valid
        Ok(())
    }

    /// Validate rate limit configuration
    fn validate_rate_limit(path: &str, config: &RateLimitConfig) -> ValidationResult<()> {
        if config.requests == 0 {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' rate_limit.requests"),
                message: "Rate limit requests must be greater than 0".to_string(),
            });
        }

        // Validate period format (e.g., "1s", "1m", "1h")
        let period_regex = Regex::new(r"^\d+[smh]$").expect("Invalid regex pattern");
        if !period_regex.is_match(&config.period) {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' rate_limit.period"),
                message: "Period must be in format like '1s', '1m', or '1h'".to_string(),
            });
        }

        Ok(())
    }

    /// Validate path rewrite configuration
    fn validate_path_rewrite(path: &str, rewrite: &str) -> ValidationResult<()> {
        if rewrite.is_empty() {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' path_rewrite"),
                message: "Path rewrite cannot be empty".to_string(),
            });
        }

        if !rewrite.starts_with('/') {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' path_rewrite"),
                message: "Path rewrite must start with '/'".to_string(),
            });
        }

        Ok(())
    }

    fn validate_health_check_config(
        config: &HealthCheckConfig,
    ) -> Result<(), Vec<ValidationError>> {
        if !config.enabled {
            return Ok(());
        }

        let mut errors = Vec::new();

        if config.interval_secs == 0 {
            errors.push(ValidationError::InvalidField {
                field: "health_check.interval_secs".to_string(),
                message: "Must be greater than 0 when health checks are enabled".to_string(),
            });
        }

        if config.timeout_secs == 0 {
            errors.push(ValidationError::InvalidField {
                field: "health_check.timeout_secs".to_string(),
                message: "Must be greater than 0 when health checks are enabled".to_string(),
            });
        }

        if config.unhealthy_threshold == 0 {
            errors.push(ValidationError::InvalidField {
                field: "health_check.unhealthy_threshold".to_string(),
                message: "Must be greater than 0 when health checks are enabled".to_string(),
            });
        }

        if config.healthy_threshold == 0 {
            errors.push(ValidationError::InvalidField {
                field: "health_check.healthy_threshold".to_string(),
                message: "Must be greater than 0 when health checks are enabled".to_string(),
            });
        }

        if config.path.trim().is_empty() {
            errors.push(ValidationError::InvalidField {
                field: "health_check.path".to_string(),
                message: "Cannot be empty when health checks are enabled".to_string(),
            });
        } else if !config.path.starts_with('/') {
            errors.push(ValidationError::InvalidField {
                field: "health_check.path".to_string(),
                message: "Must start with '/' when health checks are enabled".to_string(),
            });
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate TLS configuration
    fn validate_tls_config(config: &TlsConfig) -> ValidationResult<()> {
        match (&config.cert_path, &config.key_path) {
            (Some(cert), Some(key)) => {
                // Manual certificate configuration
                if !std::path::Path::new(cert).exists() {
                    return Err(ValidationError::InvalidTls {
                        message: format!("Certificate file does not exist: {cert}"),
                    });
                }

                if !std::path::Path::new(key).exists() {
                    return Err(ValidationError::InvalidTls {
                        message: format!("Private key file does not exist: {key}"),
                    });
                }

                Ok(())
            }
            _ => {
                if let Some(acme) = &config.acme {
                    if acme.domains.is_empty() {
                        return Err(ValidationError::InvalidTls {
                            message: "ACME configuration must include at least one domain"
                                .to_string(),
                        });
                    }

                    if acme.email.trim().is_empty() {
                        return Err(ValidationError::InvalidTls {
                            message: "ACME configuration must include a contact email".to_string(),
                        });
                    }

                    Ok(())
                } else {
                    Err(ValidationError::InvalidTls {
                        message: "TLS configuration must specify either certificate and private key paths, or ACME configuration".to_string(),
                    })
                }
            }
        }
    }

    /// Extract host from a RouteConfig
    fn get_route_host(route: &RouteConfig) -> Option<&String> {
        match route {
            RouteConfig::Static { host, .. } => host.as_ref(),
            RouteConfig::Redirect { host, .. } => host.as_ref(),
            RouteConfig::Proxy { host, .. } => host.as_ref(),
            RouteConfig::LoadBalance { host, .. } => host.as_ref(),
            RouteConfig::Websocket { host, .. } => host.as_ref(),
        }
    }

    /// Check for conflicting route paths
    /// Routes with the same path are allowed if they have different hosts.
    /// Conflicts occur when:
    /// 1. Same path with same host (or both without host)
    /// 2. Path prefix conflicts between routes without hosts
    fn check_route_conflicts(
        routes: &std::collections::HashMap<String, RouteConfigEntry>,
    ) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        // Collect all (path, host) pairs
        let mut route_entries: Vec<(&String, Option<&String>)> = Vec::new();
        for (path, entry) in routes {
            for route in entry.iter() {
                route_entries.push((path, Self::get_route_host(route)));
            }
        }

        // Check for duplicate path+host combinations
        for (i, (path1, host1)) in route_entries.iter().enumerate() {
            for (path2, host2) in route_entries.iter().skip(i + 1) {
                // Same path + same host = conflict
                if path1 == path2 {
                    let host1_lower = host1.map(|h| h.to_lowercase());
                    let host2_lower = host2.map(|h| h.to_lowercase());
                    if host1_lower == host2_lower {
                        let host_desc = host1
                            .map(|h| format!(" (host: {h})"))
                            .unwrap_or_else(|| " (no host)".to_string());
                        errors.push(ValidationError::RouteConflict {
                            message: format!(
                                "Duplicate route configuration for path '{path1}'{host_desc}"
                            ),
                        });
                    }
                }
                // Check path prefix conflicts only for routes without hosts
                else if host1.is_none() && host2.is_none() && Self::paths_conflict(path1, path2) {
                    errors.push(ValidationError::RouteConflict {
                        message: format!("Routes '{path1}' and '{path2}' have conflicting paths"),
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Check if two paths conflict (one is a prefix of another)
    fn paths_conflict(path1: &str, path2: &str) -> bool {
        if path1 == path2 {
            return true;
        }

        let p1_segments: Vec<&str> = path1.trim_matches('/').split('/').collect();
        let p2_segments: Vec<&str> = path2.trim_matches('/').split('/').collect();

        let min_len = p1_segments.len().min(p2_segments.len());

        // Check if one path is a prefix of another
        p1_segments[..min_len] == p2_segments[..min_len]
    }

    /// Check if status code is valid for redirects
    fn is_valid_redirect_status_code(code: u16) -> bool {
        matches!(code, 301 | 302 | 307 | 308)
    }

    /// Validate host field format
    fn validate_host(host: &str, path: &str) -> ValidationResult<()> {
        if host.is_empty() {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' host"),
                message: "Host cannot be empty".to_string(),
            });
        }

        // Check for invalid characters
        if host.contains("://") {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' host"),
                message: "Host should not contain protocol (e.g., use 'example.com' not 'http://example.com')".to_string(),
            });
        }

        // Simple hostname validation
        let hostname_regex = Regex::new(r"^[a-zA-Z0-9]([a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])?)*$").expect("invalid hostname regex");

        if !hostname_regex.is_match(host) {
            return Err(ValidationError::InvalidField {
                field: format!("route '{path}' host"),
                message: format!("Invalid hostname format: '{host}'"),
            });
        }

        Ok(())
    }

    /// Format multiple validation errors into a single message
    fn format_multiple_errors(errors: Vec<ValidationError>) -> String {
        if errors.is_empty() {
            return "No errors".to_string();
        }

        if errors.len() == 1 {
            return errors[0].to_string();
        }

        let mut message = format!("Found {} validation errors:\n", errors.len());
        for (i, error) in errors.iter().enumerate() {
            message.push_str(&format!("  {}. {}\n", i + 1, error));
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::{AcmeConfig, HealthCheckConfig};

    fn minimal_valid_config() -> ServerConfig {
        ServerConfig {
            listen_addr: "127.0.0.1:8080".to_string(),
            routes: [(
                "/".to_string(),
                RouteConfig::Proxy {
                    target: "http://localhost:3000".to_string(),
                    host: None,
                    path_rewrite: None,
                    rate_limit: None,
                    request_headers: None,
                    response_headers: None,
                    request_body: None,
                    response_body: None,
                    middlewares: vec![],
                }
                .into(),
            )]
            .into_iter()
            .collect(),
            ..ServerConfig::default()
        }
    }

    fn base_config_with_tls(tls: TlsConfig) -> ServerConfig {
        let mut config = minimal_valid_config();
        config.tls = Some(tls);
        config
    }

    fn make_valid_enabled_health_check() -> HealthCheckConfig {
        HealthCheckConfig {
            enabled: true,
            interval_secs: 10,
            timeout_secs: 5,
            path: "/health".to_string(),
            unhealthy_threshold: 3,
            healthy_threshold: 2,
        }
    }

    #[test]
    fn validate_accepts_acme_tls_config() {
        let config = base_config_with_tls(TlsConfig {
            cert_path: None,
            key_path: None,
            acme: Some(AcmeConfig {
                domains: vec!["example.com".to_string()],
                email: "admin@example.com".to_string(),
                cache_dir: ".axon/acme_cache".to_string(),
                production: false,
            }),
        });

        assert!(ServerConfigValidator::validate(&config).is_ok());
    }

    #[test]
    fn validate_rejects_empty_acme_domains() {
        let config = base_config_with_tls(TlsConfig {
            cert_path: None,
            key_path: None,
            acme: Some(AcmeConfig {
                domains: vec![],
                email: "admin@example.com".to_string(),
                cache_dir: ".axon/acme_cache".to_string(),
                production: false,
            }),
        });

        assert!(ServerConfigValidator::validate(&config).is_err());
    }

    #[test]
    fn validate_rejects_zero_health_check_interval_when_enabled() {
        let mut config = minimal_valid_config();
        config.health_check = make_valid_enabled_health_check();
        config.health_check.interval_secs = 0;

        assert!(ServerConfigValidator::validate(&config).is_err());
    }

    #[test]
    fn validate_rejects_invalid_health_check_path_when_enabled() {
        let mut config = minimal_valid_config();
        config.health_check = make_valid_enabled_health_check();
        config.health_check.path = "health".to_string();

        assert!(ServerConfigValidator::validate(&config).is_err());
    }
}
