//! XSS (Cross-Site Scripting) detection module
//!
//! Detects common XSS attack patterns in URIs, headers, and request bodies.

use std::collections::HashSet;

use axum::http::{HeaderMap, Uri};
use once_cell::sync::Lazy;
use regex::Regex;

use super::{SecurityRule, SecurityViolation, ThreatLevel};

/// XSS attack patterns
static XSS_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Script tags
        Regex::new(r"(?i)<script[^>]*>").expect("valid regex"),
        Regex::new(r"(?i)</script>").expect("valid regex"),
        // Event handlers
        Regex::new(r"(?i)\bon\w+\s*=").expect("valid regex"),
        // JavaScript protocol
        Regex::new(r"(?i)javascript:\s*\w").expect("valid regex"),
        // Dangerous tags with attributes
        Regex::new(r"(?i)<iframe[^>]*>").expect("valid regex"),
        Regex::new(r"(?i)<object[^>]*>").expect("valid regex"),
        Regex::new(r"(?i)<embed[^>]*>").expect("valid regex"),
        Regex::new(r"(?i)<img[^>]*\bon\w+").expect("valid regex"),
        Regex::new(r"(?i)<body[^>]*\bon\w+").expect("valid regex"),
        // JavaScript functions
        Regex::new(r"(?i)\beval\s*\(").expect("valid regex"),
        Regex::new(r"(?i)\balert\s*\(").expect("valid regex"),
        Regex::new(r"(?i)expression\s*\(").expect("valid regex"),
        // Data URLs with scripts
        Regex::new(r"(?i)data:text/html").expect("valid regex"),
    ]
});

/// Headers that are generally safe and don't need XSS checks
static SAFE_HEADERS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("accept");
    set.insert("accept-encoding");
    set.insert("accept-language");
    set.insert("content-type");
    set.insert("user-agent");
    set.insert("cache-control");
    set.insert("connection");
    set.insert("referer");
    set.insert("origin");
    set.insert("host");
    set
});

/// XSS detector
pub struct XssDetector {
    /// Whether XSS detection is enabled
    pub enabled: bool,
    /// Whether to block requests with XSS (true) or just log (false)
    pub block_mode: bool,
}

impl XssDetector {
    /// Create a new XSS detector
    pub fn new(enabled: bool, block_mode: bool) -> Self {
        Self {
            enabled,
            block_mode,
        }
    }

    /// Check a string for XSS patterns
    fn check_string(&self, input: &str) -> bool {
        if input.len() < 3 {
            return false;
        }

        let decoded = urlencoding::decode(input).unwrap_or(std::borrow::Cow::Borrowed(input));
        XSS_PATTERNS
            .iter()
            .any(|pattern| pattern.is_match(&decoded))
    }

    /// Check if a header is considered safe
    fn is_safe_header(&self, header_name: &str) -> bool {
        SAFE_HEADERS.contains(header_name.to_lowercase().as_str())
    }
}

impl SecurityRule for XssDetector {
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
        if self.check_string(&uri_str) {
            return Err(SecurityViolation::new(
                "XSS",
                ThreatLevel::High,
                format!("XSS attack detected in URI: {uri_str}"),
                self.block_mode,
            ));
        }

        // Check headers
        for (name, value) in headers.iter() {
            if self.is_safe_header(name.as_str()) {
                continue;
            }

            if let Ok(val) = value.to_str()
                && self.check_string(val)
            {
                return Err(SecurityViolation::new(
                    "XSS",
                    ThreatLevel::High,
                    format!("XSS attack detected in header {name}"),
                    self.block_mode,
                ));
            }
        }

        // Check body
        if let Some(body_bytes) = body
            && let Ok(body_str) = std::str::from_utf8(body_bytes)
            && self.check_string(body_str)
        {
            return Err(SecurityViolation::new(
                "XSS",
                ThreatLevel::High,
                "XSS attack detected in request body",
                self.block_mode,
            ));
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "XSS Detector"
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
    fn test_xss_script_tag() {
        let detector = XssDetector::new(true, true);
        let uri: Uri = "/?name=%3Cscript%3Ealert%28%27xss%27%29%3C%2Fscript%3E"
            .parse()
            .expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_xss_event_handler() {
        let detector = XssDetector::new(true, true);
        let uri: Uri = "/?html=%3Cimg%20src=x%20onerror=alert%281%29%3E"
            .parse()
            .expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_safe_uri() {
        let detector = XssDetector::new(true, true);
        let uri: Uri = "/?search=hello&page=1".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_ok());
    }
}
