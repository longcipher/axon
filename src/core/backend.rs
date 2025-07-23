use std::{
    fmt,
    str::FromStr,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
};

use thiserror::Error;

use crate::{config::HealthStatus, metrics::set_backend_health_status};

// Constants for health status to replace magic numbers
const HEALTH_STATUS_UNHEALTHY: u8 = 0;
const HEALTH_STATUS_HEALTHY: u8 = 1;

/// Errors related to backend operations
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum BackendError {
    /// Error when URL is invalid
    #[error("Invalid backend URL: {0}")]
    InvalidUrl(String),
}

/// Result type for backend operations
pub type BackendResult<T> = Result<T, BackendError>;

/// A type-safe representation of a backend URL
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendUrl {
    /// The underlying URL as a string
    url: String,
    /// Whether the URL is secure (HTTPS)
    is_secure: bool,
}

impl BackendUrl {
    /// Creates a new BackendUrl if the provided string is a valid URL
    ///
    /// # Arguments
    /// * `url` - The URL string to validate and wrap
    ///
    /// # Returns
    /// A result containing the BackendUrl or an error
    pub fn new(url: &str) -> BackendResult<Self> {
        // Basic validation - ensure URL starts with http:// or https://
        let is_secure = url.starts_with("https://");
        let is_http = url.starts_with("http://");

        if !is_secure && !is_http {
            return Err(BackendError::InvalidUrl(format!(
                "Backend URL must start with http:// or https://, got: {url}"
            )));
        }

        Ok(BackendUrl {
            url: url.to_string(),
            is_secure,
        })
    }

    /// Get the underlying URL as a string reference
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// Check if the URL is using HTTPS
    pub fn is_secure(&self) -> bool {
        self.is_secure
    }

    /// Get the underlying URL as a string
    pub fn into_string(self) -> String {
        self.url
    }
}

impl FromStr for BackendUrl {
    type Err = BackendError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BackendUrl::new(s)
    }
}

impl fmt::Display for BackendUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.url)
    }
}

/// Tracks the health status of a backend
#[derive(Debug)]
pub struct BackendHealth {
    target_url: BackendUrl,
    /// Current health status (uses atomic for thread safety)
    status: AtomicU8, // Uses HEALTH_STATUS_* constants
    /// Counter for consecutive successful health checks
    pub consecutive_successes: AtomicU32,
    /// Counter for consecutive failed health checks
    pub consecutive_failures: AtomicU32,
}

impl BackendHealth {
    /// Creates a new BackendHealth instance
    ///
    /// # Arguments
    /// * `target` - The backend URL this health status is tracking
    ///
    /// # Returns
    /// A new BackendHealth instance initialized as healthy
    pub fn new(target: BackendUrl) -> Self {
        Self {
            target_url: target,
            status: AtomicU8::new(HEALTH_STATUS_HEALTHY), // Start as healthy
            consecutive_successes: AtomicU32::new(0),
            consecutive_failures: AtomicU32::new(0),
        }
    }

    /// Get the current health status
    ///
    /// # Returns
    /// The current health status (Healthy or Unhealthy)
    pub fn status(&self) -> HealthStatus {
        // Use Acquire ordering for better correctness when reading status
        if self.status.load(Ordering::Acquire) == HEALTH_STATUS_HEALTHY {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        }
    }

    /// Mark the backend as healthy and reset failure count
    pub fn mark_healthy(&self) {
        // Use Release ordering for updates to ensure visibility to other threads
        self.status.store(HEALTH_STATUS_HEALTHY, Ordering::Release);
        self.consecutive_failures.store(0, Ordering::Release);

        // Increment consecutive successes
        let current = self.consecutive_successes.load(Ordering::Relaxed);
        self.consecutive_successes
            .store(current + 1, Ordering::Release);
        set_backend_health_status(self.target_url.as_str(), true);
    }

    /// Mark the backend as unhealthy and reset success count
    pub fn mark_unhealthy(&self) {
        // Use Release ordering for updates to ensure visibility to other threads
        self.status
            .store(HEALTH_STATUS_UNHEALTHY, Ordering::Release);
        self.consecutive_successes.store(0, Ordering::Release);

        // Increment consecutive failures
        let current = self.consecutive_failures.load(Ordering::Relaxed);
        self.consecutive_failures
            .store(current + 1, Ordering::Release);
        set_backend_health_status(self.target_url.as_str(), false);
    }

    /// Get the number of consecutive successful health checks
    pub fn consecutive_successes(&self) -> u32 {
        self.consecutive_successes.load(Ordering::Relaxed)
    }

    /// Get the number of consecutive failed health checks
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_url_valid() {
        let url = "http://example.com";
        let backend_url = BackendUrl::new(url).expect("Valid HTTP URL should parse");
        assert_eq!(backend_url.as_str(), url);
        assert!(!backend_url.is_secure());

        let secure_url = "https://secure.example.com";
        let secure_backend_url = BackendUrl::new(secure_url).expect("Valid HTTPS URL should parse");
        assert_eq!(secure_backend_url.as_str(), secure_url);
        assert!(secure_backend_url.is_secure());
    }

    #[test]
    fn test_backend_url_invalid() {
        let result = BackendUrl::new("example.com");
        assert!(result.is_err());

        let result = BackendUrl::new("ftp://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_backend_url_from_str() {
        let url = "http://example.com";
        let backend_url: BackendUrl = url
            .parse()
            .expect("Parsing valid URL string should succeed");
        assert_eq!(backend_url.as_str(), url);
    }

    #[test]
    fn test_backend_health_initial_state() {
        let url = BackendUrl::new("http://example.com").unwrap();
        let health = BackendHealth::new(url);

        assert_eq!(health.status(), HealthStatus::Healthy);
        assert_eq!(health.consecutive_successes(), 0);
        assert_eq!(health.consecutive_failures(), 0);
    }

    #[test]
    fn test_backend_health_mark_unhealthy() {
        let url = BackendUrl::new("http://example.com").unwrap();
        let health = BackendHealth::new(url);

        health.mark_unhealthy();
        assert_eq!(health.status(), HealthStatus::Unhealthy);
        assert_eq!(health.consecutive_failures(), 1);
        assert_eq!(health.consecutive_successes(), 0);
    }

    #[test]
    fn test_backend_health_mark_healthy() {
        let url = BackendUrl::new("http://example.com").unwrap();
        let health = BackendHealth::new(url);

        // First mark as unhealthy
        health.mark_unhealthy();
        assert_eq!(health.consecutive_failures(), 1);

        // Then mark as healthy
        health.mark_healthy();
        assert_eq!(health.status(), HealthStatus::Healthy);
        assert_eq!(health.consecutive_successes(), 1);
        assert_eq!(health.consecutive_failures(), 0);
    }
}
