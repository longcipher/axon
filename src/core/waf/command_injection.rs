//! Command injection detection module
//!
//! Detects attempts to execute shell commands through request parameters.

use std::collections::HashSet;

use axum::http::{HeaderMap, Uri};
use once_cell::sync::Lazy;
use regex::Regex;

use super::{SecurityRule, SecurityViolation, ThreatLevel};

/// Command injection attack patterns
static COMMAND_INJECTION_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Command chaining operators
        Regex::new(r";\s*\w").expect("valid regex"),
        Regex::new(r"\|\s*\w").expect("valid regex"),
        Regex::new(r"\|\|\s*\w").expect("valid regex"),
        Regex::new(r"&&\s*\w").expect("valid regex"),
        Regex::new(r"\n\s*\w").expect("valid regex"),
        // Command substitution
        Regex::new(r"\$\(\s*\w").expect("valid regex"),
        Regex::new(r"`[^`]+`").expect("valid regex"),
        Regex::new(r"\$\{\s*\w").expect("valid regex"),
        // Shell redirections
        Regex::new(r">\s*/").expect("valid regex"),
        Regex::new(r">>\s*/").expect("valid regex"),
        Regex::new(r"<\s*/").expect("valid regex"),
        Regex::new(r"2>&1").expect("valid regex"),
        // Common dangerous commands
        Regex::new(r"(?i)\b(cat|head|tail|less|more)\s+/").expect("valid regex"),
        Regex::new(r"(?i)\b(ls|dir)\s+(-\w+\s+)?/").expect("valid regex"),
        Regex::new(r"(?i)\b(rm|del|rmdir)\s+(-\w+\s+)?").expect("valid regex"),
        Regex::new(r"(?i)\b(wget|curl)\s+").expect("valid regex"),
        Regex::new(r"(?i)\b(nc|netcat|ncat)\s+").expect("valid regex"),
        Regex::new(r"(?i)\b(bash|sh|zsh|ksh|csh)\s+-").expect("valid regex"),
        Regex::new(r"(?i)\b(python|perl|ruby|php)\s+-").expect("valid regex"),
        Regex::new(r"(?i)\b(chmod|chown|chgrp)\s+").expect("valid regex"),
        Regex::new(r"(?i)\bsudo\s+").expect("valid regex"),
        Regex::new(r"(?i)\b(kill|killall|pkill)\s+").expect("valid regex"),
        Regex::new(r"(?i)\b(whoami|id|uname)\b").expect("valid regex"),
        Regex::new(r"(?i)\b(passwd|useradd|userdel)\b").expect("valid regex"),
        Regex::new(r"(?i)\b(ifconfig|ipconfig|netstat)\b").expect("valid regex"),
        Regex::new(r"(?i)\bping\s+-").expect("valid regex"),
        // Shell paths
        Regex::new(r"(?i)/bin/(sh|bash|zsh|ksh|csh|dash)").expect("valid regex"),
        Regex::new(r"(?i)/usr/bin/(sh|bash|python|perl|ruby|php)").expect("valid regex"),
        Regex::new(r"(?i)cmd\.exe").expect("valid regex"),
        Regex::new(r"(?i)powershell").expect("valid regex"),
        // Environment variable access
        Regex::new(r"\$PATH\b").expect("valid regex"),
        Regex::new(r"\$HOME\b").expect("valid regex"),
        Regex::new(r"\$USER\b").expect("valid regex"),
        Regex::new(r"\$SHELL\b").expect("valid regex"),
        Regex::new(r"(?i)%systemroot%").expect("valid regex"),
        Regex::new(r"(?i)%comspec%").expect("valid regex"),
        // Encoded variants
        Regex::new(r"(?i)%3b").expect("valid regex"),
        Regex::new(r"(?i)%7c").expect("valid regex"),
        Regex::new(r"(?i)%26").expect("valid regex"),
        Regex::new(r"(?i)%60").expect("valid regex"),
        Regex::new(r"(?i)%24%28").expect("valid regex"),
    ]
});

/// Headers that are generally safe and don't need command injection checks
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
    set.insert("cookie");
    set
});

/// Command injection detector
pub struct CommandInjectionDetector {
    /// Whether command injection detection is enabled
    pub enabled: bool,
    /// Whether to block requests with command injection (true) or just log (false)
    pub block_mode: bool,
}

impl CommandInjectionDetector {
    /// Create a new command injection detector
    pub fn new(enabled: bool, block_mode: bool) -> Self {
        Self {
            enabled,
            block_mode,
        }
    }

    /// Check a string for command injection patterns
    fn check_string(&self, input: &str) -> Option<String> {
        if input.len() < 2 {
            return None;
        }

        let decoded = urlencoding::decode(input).unwrap_or(std::borrow::Cow::Borrowed(input));

        for pattern in COMMAND_INJECTION_PATTERNS.iter() {
            if pattern.is_match(&decoded) || pattern.is_match(input) {
                return Some(format!("Command injection pattern: {}", pattern.as_str()));
            }
        }

        None
    }

    /// Check if a header is considered safe
    fn is_safe_header(&self, header_name: &str) -> bool {
        SAFE_HEADERS.contains(header_name.to_lowercase().as_str())
    }
}

impl SecurityRule for CommandInjectionDetector {
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
                "COMMAND_INJECTION",
                ThreatLevel::Critical,
                format!("Command injection detected in URI: {uri_str} - {reason}"),
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
                    "COMMAND_INJECTION",
                    ThreatLevel::Critical,
                    format!("Command injection detected in header {name}: {reason}"),
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
                "COMMAND_INJECTION",
                ThreatLevel::Critical,
                format!("Command injection detected in body: {reason}"),
                self.block_mode,
            ));
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "Command Injection Detector"
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
    fn test_command_injection_pipe() {
        let detector = CommandInjectionDetector::new(true, true);
        let uri: Uri = "/?cmd=ls%20%7C%20cat%20/etc/passwd"
            .parse()
            .expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_command_injection_backticks() {
        let detector = CommandInjectionDetector::new(true, true);
        let uri: Uri = "/?cmd=`whoami`".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_err());
    }

    #[test]
    fn test_safe_uri() {
        let detector = CommandInjectionDetector::new(true, true);
        let uri: Uri = "/?search=hello&page=1".parse().expect("valid uri");
        assert!(detector.check(&uri, &HeaderMap::new(), None).is_ok());
    }
}
