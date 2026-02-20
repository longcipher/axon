//! SQL injection detection module
//!
//! Detects common SQL injection patterns in URIs, headers, and request bodies.

use std::collections::HashSet;

use axum::http::{HeaderMap, Uri};
use once_cell::sync::Lazy;
use regex::Regex;

use super::{SecurityRule, SecurityViolation, ThreatLevel};

/// SQL injection attack patterns
static SQL_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Union-based injection
        Regex::new(r"(?i)\bunion\b.*\bselect\b").expect("valid regex"),
        Regex::new(r"(?i)\bselect\b.*\bfrom\b").expect("valid regex"),
        // Classic boolean injection patterns
        Regex::new(r"(?i)\b(or|and)\b\s+\d+\s*=\s*\d+").expect("valid regex"),
        Regex::new(r#"(?i)'\s*(or|and)\s*'"#).expect("valid regex"),
        Regex::new(r#"(?i)\bor\b\s+["']?\w+["']?\s*=\s*["']?\w+["']?"#).expect("valid regex"),
        // Data manipulation
        Regex::new(r"(?i)\binsert\b.*\binto\b").expect("valid regex"),
        Regex::new(r"(?i)\bdelete\b.*\bfrom\b").expect("valid regex"),
        Regex::new(r"(?i)\bdrop\b.*\b(table|database)\b").expect("valid regex"),
        Regex::new(r"(?i)\bupdate\b.*\bset\b").expect("valid regex"),
        // Statement termination and comments
        Regex::new(r"(?i);\s*\b(drop|delete|update|insert)\b").expect("valid regex"),
        Regex::new(r";\s*--").expect("valid regex"),
        Regex::new(r"'--").expect("valid regex"),
        Regex::new(r"--[^\r\n]*$").expect("valid regex"),
        // SQL execution
        Regex::new(r"(?i)\b(exec|execute)\s*\(").expect("valid regex"),
        Regex::new(r"(?i)\b(xp_|sp_)\w+").expect("valid regex"),
        // Time-based blind injection
        Regex::new(r"(?i)\b(benchmark|sleep|waitfor\s+delay)\s*\(").expect("valid regex"),
        // Hex encoding bypass attempts
        Regex::new(r"(?i)0x[0-9a-f]{2,}").expect("valid regex"),
    ]
});

/// Headers that are generally safe and don't need SQL injection checks
static SAFE_HEADERS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut set = HashSet::new();
    set.insert("accept");
    set.insert("accept-encoding");
    set.insert("accept-language");
    set.insert("content-type");
    set.insert("user-agent");
    set.insert("cache-control");
    set.insert("connection");
    set.insert("upgrade-insecure-requests");
    set.insert("sec-fetch-mode");
    set.insert("sec-fetch-site");
    set.insert("sec-fetch-dest");
    set.insert("sec-ch-ua");
    set.insert("sec-ch-ua-mobile");
    set.insert("sec-ch-ua-platform");
    set.insert("host");
    set
});

/// SQL injection detector
pub struct SqlInjectionDetector {
    /// Whether SQL injection detection is enabled
    pub enabled: bool,
    /// Whether to block requests with SQL injection (true) or just log (false)
    pub block_mode: bool,
}

impl SqlInjectionDetector {
    /// Create a new SQL injection detector
    pub fn new(enabled: bool, block_mode: bool) -> Self {
        Self {
            enabled,
            block_mode,
        }
    }

    /// Check a string for SQL injection patterns
    fn check_string(&self, input: &str) -> bool {
        if input.len() < 3 {
            return false;
        }

        // URL decode first
        let decoded = urlencoding::decode(input).unwrap_or(std::borrow::Cow::Borrowed(input));

        SQL_PATTERNS
            .iter()
            .any(|pattern| pattern.is_match(&decoded))
    }

    /// Check if a header is considered safe
    fn is_safe_header(&self, header_name: &str) -> bool {
        SAFE_HEADERS.contains(header_name.to_lowercase().as_str())
    }
}

impl SecurityRule for SqlInjectionDetector {
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
                "SQL_INJECTION",
                ThreatLevel::Critical,
                format!("SQL injection detected in URI: {uri_str}"),
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
                && self.check_string(val)
            {
                return Err(SecurityViolation::new(
                    "SQL_INJECTION",
                    ThreatLevel::Critical,
                    format!("SQL injection detected in header {name}: {val}"),
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
                "SQL_INJECTION",
                ThreatLevel::Critical,
                "SQL injection detected in request body",
                self.block_mode,
            ));
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "SQL Injection Detector"
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
    fn test_sql_injection_union() {
        let detector = SqlInjectionDetector::new(true, true);
        let uri: Uri = "/?id=1%20UNION%20SELECT%20*%20FROM%20users"
            .parse()
            .expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_sql_injection_or_equals() {
        let detector = SqlInjectionDetector::new(true, true);
        let uri: Uri = "/?username=admin%27%20OR%20%271%27=%271"
            .parse()
            .expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_safe_uri() {
        let detector = SqlInjectionDetector::new(true, true);
        let uri: Uri = "/?search=hello&page=1".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_ok());
    }

    #[test]
    fn test_sql_injection_statement_termination_comment() {
        let detector = SqlInjectionDetector::new(true, true);
        let uri: Uri = "/?q=1;%20--%20comment".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }
}
