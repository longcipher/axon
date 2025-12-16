//! Path traversal detection module
//!
//! Detects attempts to access files outside the intended directory.

use std::collections::HashSet;

use axum::http::{HeaderMap, Uri};
use once_cell::sync::Lazy;
use regex::Regex;

use super::{SecurityRule, SecurityViolation, ThreatLevel};

/// Path traversal attack patterns
static PATH_TRAVERSAL_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Basic directory traversal
        Regex::new(r"\.\.\/").expect("valid regex"),
        Regex::new(r"\.\.\\").expect("valid regex"),
        Regex::new(r"\.\.%2f").expect("valid regex"),
        Regex::new(r"\.\.%5c").expect("valid regex"),
        // URL encoded variants
        Regex::new(r"(?i)%2e%2e%2f").expect("valid regex"),
        Regex::new(r"(?i)%2e%2e/").expect("valid regex"),
        Regex::new(r"(?i)%2e%2e%5c").expect("valid regex"),
        Regex::new(r"(?i)%2e%2e\\").expect("valid regex"),
        // Double encoding
        Regex::new(r"(?i)%252e%252e%252f").expect("valid regex"),
        Regex::new(r"(?i)%252e%252e/").expect("valid regex"),
        // Unicode/overlong encoding
        Regex::new(r"(?i)%c0%ae%c0%ae/").expect("valid regex"),
        Regex::new(r"(?i)%c0%ae%c0%ae%c0%af").expect("valid regex"),
        // Null byte injection
        Regex::new(r"%00").expect("valid regex"),
        Regex::new(r"\\x00").expect("valid regex"),
        // Bypass attempts
        Regex::new(r"\.\.\.\./").expect("valid regex"),
        Regex::new(r"\.\.//").expect("valid regex"),
        Regex::new(r"\.\./\./").expect("valid regex"),
    ]
});

/// Sensitive file paths that should never be accessed
static SENSITIVE_PATHS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Unix sensitive files
        Regex::new(r"(?i)/etc/passwd").expect("valid regex"),
        Regex::new(r"(?i)/etc/shadow").expect("valid regex"),
        Regex::new(r"(?i)/etc/hosts").expect("valid regex"),
        Regex::new(r"(?i)/etc/group").expect("valid regex"),
        Regex::new(r"(?i)/proc/").expect("valid regex"),
        Regex::new(r"(?i)/sys/").expect("valid regex"),
        Regex::new(r"(?i)/var/log/").expect("valid regex"),
        Regex::new(r"(?i)/root/").expect("valid regex"),
        Regex::new(r"(?i)\.ssh/").expect("valid regex"),
        Regex::new(r"(?i)\.bash_history").expect("valid regex"),
        Regex::new(r"(?i)\.env").expect("valid regex"),
        Regex::new(r"(?i)id_rsa").expect("valid regex"),
        Regex::new(r"(?i)id_dsa").expect("valid regex"),
        // Windows sensitive files
        Regex::new(r"(?i)c:\\windows").expect("valid regex"),
        Regex::new(r"(?i)c:\\boot\.ini").expect("valid regex"),
        Regex::new(r"(?i)\\windows\\system32").expect("valid regex"),
        Regex::new(r"(?i)win\.ini").expect("valid regex"),
        Regex::new(r"(?i)system\.ini").expect("valid regex"),
        // Web server configs
        Regex::new(r"(?i)\.htaccess").expect("valid regex"),
        Regex::new(r"(?i)\.htpasswd").expect("valid regex"),
        Regex::new(r"(?i)web\.config").expect("valid regex"),
        Regex::new(r"(?i)nginx\.conf").expect("valid regex"),
        Regex::new(r"(?i)httpd\.conf").expect("valid regex"),
        // Application configs
        Regex::new(r"(?i)config\.php").expect("valid regex"),
        Regex::new(r"(?i)database\.yml").expect("valid regex"),
        Regex::new(r"(?i)settings\.py").expect("valid regex"),
        Regex::new(r"(?i)wp-config\.php").expect("valid regex"),
    ]
});

/// Headers that are generally safe and don't need path traversal checks
static SAFE_HEADERS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("accept");
    set.insert("accept-encoding");
    set.insert("accept-language");
    set.insert("content-type");
    set.insert("user-agent");
    set.insert("cache-control");
    set.insert("connection");
    set.insert("host");
    set.insert("origin");
    set.insert("referer");
    set
});

/// Path traversal detector
pub struct PathTraversalDetector {
    /// Whether path traversal detection is enabled
    pub enabled: bool,
    /// Whether to block requests with path traversal (true) or just log (false)
    pub block_mode: bool,
}

impl PathTraversalDetector {
    /// Create a new path traversal detector
    pub fn new(enabled: bool, block_mode: bool) -> Self {
        Self {
            enabled,
            block_mode,
        }
    }

    /// Check a string for path traversal patterns
    fn check_string(&self, input: &str) -> Option<String> {
        if input.len() < 2 {
            return None;
        }

        let decoded = urlencoding::decode(input).unwrap_or(std::borrow::Cow::Borrowed(input));

        // Check for path traversal patterns
        for pattern in PATH_TRAVERSAL_PATTERNS.iter() {
            if pattern.is_match(&decoded) || pattern.is_match(input) {
                return Some(format!("Path traversal pattern: {}", pattern.as_str()));
            }
        }

        // Check for sensitive file paths
        for pattern in SENSITIVE_PATHS.iter() {
            if pattern.is_match(&decoded) || pattern.is_match(input) {
                return Some(format!("Sensitive path access: {}", pattern.as_str()));
            }
        }

        None
    }

    /// Check if a header is considered safe
    fn is_safe_header(&self, header_name: &str) -> bool {
        SAFE_HEADERS.contains(header_name.to_lowercase().as_str())
    }
}

impl SecurityRule for PathTraversalDetector {
    fn check(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        body: Option<&[u8]>,
    ) -> Result<(), SecurityViolation> {
        if !self.enabled {
            return Ok(());
        }

        // Check URI
        let uri_str = uri.to_string();
        if let Some(reason) = self.check_string(&uri_str) {
            return Err(SecurityViolation::new(
                "PATH_TRAVERSAL",
                ThreatLevel::High,
                format!("Path traversal detected in URI: {uri_str} - {reason}"),
                self.block_mode,
            ));
        }

        // Check headers
        for (name, value) in headers.iter() {
            let header_name = name.as_str();

            if self.is_safe_header(header_name) {
                continue;
            }

            if let Ok(val) = value.to_str()
                && let Some(reason) = self.check_string(val)
            {
                return Err(SecurityViolation::new(
                    "PATH_TRAVERSAL",
                    ThreatLevel::High,
                    format!("Path traversal detected in header {name}: {reason}"),
                    self.block_mode,
                ));
            }
        }

        // Check body
        if let Some(body_bytes) = body
            && let Ok(body_str) = std::str::from_utf8(body_bytes)
            && let Some(reason) = self.check_string(body_str)
        {
            return Err(SecurityViolation::new(
                "PATH_TRAVERSAL",
                ThreatLevel::High,
                format!("Path traversal detected in body: {reason}"),
                self.block_mode,
            ));
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "Path Traversal Detector"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::*;

    #[test]
    fn test_path_traversal_basic() {
        let detector = PathTraversalDetector::new(true, true);
        let uri: Uri = "/?file=../../etc/passwd".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_path_traversal_encoded() {
        let detector = PathTraversalDetector::new(true, true);
        let uri: Uri = "/?file=%2e%2e%2f%2e%2e%2fetc%2fpasswd"
            .parse()
            .expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_safe_uri() {
        let detector = PathTraversalDetector::new(true, true);
        let uri: Uri = "/?file=document.pdf".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_ok());
    }
}
