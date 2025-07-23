#![allow(clippy::collapsible_if)]

use std::net::SocketAddr;

use eyre::Result;
use regex::Regex;

use crate::config::models::{
    AcmeConfig, LoadBalanceStrategy, RateLimitConfig, RouteConfig, ServerConfig, TlsConfig,
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

    #[error("Invalid ACME configuration: {message}")]
    InvalidAcme { message: String },

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
            for (path, route_config) in &config.routes {
                if let Err(mut route_errors) = Self::validate_single_route(path, route_config) {
                    errors.append(&mut route_errors);
                }
            }
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
            RouteConfig::Proxy { target, .. } => {
                if let Err(e) = Self::validate_url(target, &format!("route '{path}' target")) {
                    errors.push(e);
                }
            }
            RouteConfig::LoadBalance {
                targets, strategy, ..
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
            }
            RouteConfig::Static { root, .. } => {
                if !std::path::Path::new(root).exists() {
                    errors.push(ValidationError::InvalidField {
                        field: format!("route '{path}' root"),
                        message: format!("Static root directory '{root}' does not exist"),
                    });
                }
            }
            RouteConfig::Redirect {
                target,
                status_code,
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
            }
            RouteConfig::Websocket {
                target,
                max_frame_size,
                max_message_size,
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

    /// Validate TLS configuration
    fn validate_tls_config(config: &TlsConfig) -> ValidationResult<()> {
        match (&config.cert_path, &config.key_path, &config.acme) {
            (Some(cert), Some(key), None) => {
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
            (None, None, Some(acme_config)) => {
                // ACME configuration
                Self::validate_acme_config(acme_config)
            }
            (None, None, None) => Err(ValidationError::InvalidTls {
                message:
                    "TLS configuration must specify either certificate paths or ACME configuration"
                        .to_string(),
            }),
            _ => Err(ValidationError::InvalidTls {
                message:
                    "TLS configuration cannot specify both certificate paths and ACME configuration"
                        .to_string(),
            }),
        }
    }

    /// Validate ACME configuration
    fn validate_acme_config(config: &AcmeConfig) -> ValidationResult<()> {
        if !config.enabled {
            return Ok(());
        }

        if config.domains.is_empty() {
            return Err(ValidationError::InvalidAcme {
                message: "ACME configuration must specify at least one domain".to_string(),
            });
        }

        // Validate email format
        let email_regex = Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").expect("Invalid email regex");
        if !email_regex.is_match(&config.email) {
            return Err(ValidationError::InvalidAcme {
                message: format!("Invalid email address: {}", config.email),
            });
        }

        // Validate domains
        for domain in &config.domains {
            if domain.is_empty() || domain.contains(' ') {
                return Err(ValidationError::InvalidAcme {
                    message: format!("Invalid domain: {domain}"),
                });
            }
        }

        // Validate renewal period
        if let Some(days) = config.renewal_days_before_expiry {
            if days == 0 || days > 89 {
                return Err(ValidationError::InvalidAcme {
                    message: format!(
                        "Renewal days before expiry must be between 1 and 89, got {days}"
                    ),
                });
            }
        }

        Ok(())
    }

    /// Check for conflicting route paths
    fn check_route_conflicts(
        routes: &std::collections::HashMap<String, RouteConfig>,
    ) -> Result<(), Vec<ValidationError>> {
        let route_paths: Vec<&String> = routes.keys().collect();
        let mut errors = Vec::new();

        for (i, path1) in route_paths.iter().enumerate() {
            for path2 in route_paths.iter().skip(i + 1) {
                if Self::paths_conflict(path1, path2) {
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
