//! WAF (Web Application Firewall) module for Axon.
//!
//! Provides security rules to detect and block common web attacks:
//! - SQL injection
//! - XSS (Cross-Site Scripting)
//! - Command injection
//! - Path traversal
//! - Bot detection
//! - IP filtering

pub mod body_inspector;
pub mod bot_detector;
pub mod command_injection;
pub mod engine;
pub mod ip_filter;
pub mod path_traversal;
pub mod sql_injection;
pub mod xss_detector;

use axum::http::{HeaderMap, Uri};
pub use body_inspector::BodyInspector;
pub use bot_detector::{BotDetector, BotType};
pub use command_injection::CommandInjectionDetector;
pub use engine::WafEngine;
pub use ip_filter::IpFilter;
pub use path_traversal::PathTraversalDetector;
pub use sql_injection::SqlInjectionDetector;
pub use xss_detector::XssDetector;

/// Threat level classification for security violations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatLevel {
    /// Low severity threat
    Low,
    /// Medium severity threat
    Medium,
    /// High severity threat
    High,
    /// Critical severity threat
    Critical,
}

impl ThreatLevel {
    /// Get the numeric score for the threat level (1-4)
    pub fn score(&self) -> u8 {
        match self {
            ThreatLevel::Low => 1,
            ThreatLevel::Medium => 2,
            ThreatLevel::High => 3,
            ThreatLevel::Critical => 4,
        }
    }

    /// Get string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            ThreatLevel::Low => "low",
            ThreatLevel::Medium => "medium",
            ThreatLevel::High => "high",
            ThreatLevel::Critical => "critical",
        }
    }
}

/// Security violation detected by a WAF rule
#[derive(Debug, Clone)]
pub struct SecurityViolation {
    /// Type of threat (e.g., "SQL_INJECTION", "XSS")
    pub threat_type: String,
    /// Severity level of the threat
    pub threat_level: ThreatLevel,
    /// Human-readable description of the violation
    pub description: String,
    /// Whether the request should be blocked
    pub blocked: bool,
}

impl SecurityViolation {
    /// Create a new security violation
    pub fn new(
        threat_type: impl Into<String>,
        threat_level: ThreatLevel,
        description: impl Into<String>,
        blocked: bool,
    ) -> Self {
        Self {
            threat_type: threat_type.into(),
            threat_level,
            description: description.into(),
            blocked,
        }
    }
}

/// Security rule trait for detecting threats
pub trait SecurityRule: Send + Sync {
    /// Check a request for security violations
    ///
    /// # Arguments
    /// * `uri` - The request URI
    /// * `headers` - The request headers
    /// * `body` - Optional request body
    ///
    /// # Returns
    /// * `Ok(())` if no violation detected
    /// * `Err(SecurityViolation)` if a violation is detected
    fn check(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        body: Option<&[u8]>,
    ) -> Result<(), SecurityViolation>;

    /// Get the name of this security rule
    fn name(&self) -> &str;

    /// Check if the rule is enabled
    fn is_enabled(&self) -> bool;
}
