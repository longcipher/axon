//! Bot detection module
//!
//! Identifies and categorizes bots (good vs bad) based on User-Agent strings.

use std::collections::HashSet;

use axum::http::{HeaderMap, Uri};
use once_cell::sync::Lazy;
use regex::Regex;

use super::{SecurityRule, SecurityViolation, ThreatLevel};

/// Known malicious bot User-Agent patterns
static BAD_BOT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Security scanners and vulnerability tools
        Regex::new(r"(?i)\bsqlmap\b").expect("valid regex"),
        Regex::new(r"(?i)\bnikto\b").expect("valid regex"),
        Regex::new(r"(?i)\bnmap\b").expect("valid regex"),
        Regex::new(r"(?i)\bmasscan\b").expect("valid regex"),
        Regex::new(r"(?i)\bmetasploit\b").expect("valid regex"),
        Regex::new(r"(?i)\bburpsuite\b").expect("valid regex"),
        Regex::new(r"(?i)\bacunetix\b").expect("valid regex"),
        Regex::new(r"(?i)\bnessus\b").expect("valid regex"),
        Regex::new(r"(?i)\bowasp\b.*\bzap\b").expect("valid regex"),
        Regex::new(r"(?i)\bdirbuster\b").expect("valid regex"),
        Regex::new(r"(?i)\bgobuster\b").expect("valid regex"),
        Regex::new(r"(?i)\bwpscan\b").expect("valid regex"),
        Regex::new(r"(?i)\bjoomscan\b").expect("valid regex"),
        Regex::new(r"(?i)\bw3af\b").expect("valid regex"),
        Regex::new(r"(?i)\barachni\b").expect("valid regex"),
        Regex::new(r"(?i)\bskipfish\b").expect("valid regex"),
        // Web scrapers
        Regex::new(r"(?i)\bscrapy\b").expect("valid regex"),
        Regex::new(r"(?i)\bwebharvest\b").expect("valid regex"),
        Regex::new(r"(?i)\bhttrack\b").expect("valid regex"),
        Regex::new(r"(?i)\bwebcopier\b").expect("valid regex"),
        Regex::new(r"(?i)\boffline\s*explorer\b").expect("valid regex"),
        Regex::new(r"(?i)\bteleport\s*pro\b").expect("valid regex"),
        Regex::new(r"(?i)\bwebzip\b").expect("valid regex"),
        // Spam bots
        Regex::new(r"(?i)\bsemrush\b").expect("valid regex"),
        Regex::new(r"(?i)\bahrefs\b").expect("valid regex"),
        Regex::new(r"(?i)\bmj12bot\b").expect("valid regex"),
        Regex::new(r"(?i)\bdotbot\b").expect("valid regex"),
        Regex::new(r"(?i)\bseekport\b").expect("valid regex"),
        Regex::new(r"(?i)\bblexbot\b").expect("valid regex"),
        // Generic bad patterns
        Regex::new(r"(?i)\bcrawler\b.*\bbot\b").expect("valid regex"),
        Regex::new(r"(?i)\bspider\b.*\bbot\b").expect("valid regex"),
        // Library defaults that are often abused
        Regex::new(r"^python-requests").expect("valid regex"),
        Regex::new(r"^python-urllib").expect("valid regex"),
        Regex::new(r"^Java/").expect("valid regex"),
        Regex::new(r"^libwww-perl").expect("valid regex"),
        Regex::new(r"^Go-http-client").expect("valid regex"),
        Regex::new(r"^curl/").expect("valid regex"),
        Regex::new(r"(?i)^wget/").expect("valid regex"),
    ]
});

/// Known good bot identifiers
static GOOD_BOT_IDENTIFIERS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    let mut set = HashSet::new();
    // Search engine crawlers
    set.insert("googlebot");
    set.insert("bingbot");
    set.insert("slurp");
    set.insert("duckduckbot");
    set.insert("baiduspider");
    set.insert("yandexbot");
    set.insert("sogou");
    set.insert("exabot");
    set.insert("facebot");
    set.insert("facebookexternalhit");
    set.insert("ia_archiver");
    // Social media
    set.insert("twitterbot");
    set.insert("linkedinbot");
    set.insert("pinterestbot");
    set.insert("slackbot");
    set.insert("telegrambot");
    set.insert("discordbot");
    set.insert("whatsapp");
    // Monitoring
    set.insert("uptimerobot");
    set.insert("pingdom");
    set.insert("statuscake");
    set.insert("site24x7");
    set.insert("gtmetrix");
    // Feed readers
    set.insert("feedly");
    set.insert("feedburner");
    set
});

/// Bot detection result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotType {
    /// Known good bot
    GoodBot(String),
    /// Known bad bot
    BadBot(String),
    /// Suspicious bot
    SuspiciousBot(String),
    /// Regular user
    NotBot,
}

/// Bot detector
pub struct BotDetector {
    /// Whether bot detection is enabled
    pub enabled: bool,
    /// Whether to block bad bots (true) or just log (false)
    pub block_mode: bool,
    /// Whether to allow known good bots
    pub allow_known_bots: bool,
    /// Custom bad bot patterns
    custom_bad_patterns: Vec<Regex>,
    /// Custom good bot identifiers
    custom_good_identifiers: HashSet<String>,
}

impl BotDetector {
    /// Create a new bot detector
    pub fn new(enabled: bool, block_mode: bool, allow_known_bots: bool) -> Self {
        Self {
            enabled,
            block_mode,
            allow_known_bots,
            custom_bad_patterns: Vec::new(),
            custom_good_identifiers: HashSet::new(),
        }
    }

    /// Add a custom bad bot pattern
    pub fn add_bad_bot_pattern(&mut self, pattern: &str) -> Result<(), String> {
        let regex = Regex::new(pattern).map_err(|e| e.to_string())?;
        self.custom_bad_patterns.push(regex);
        Ok(())
    }

    /// Add a custom good bot identifier
    pub fn add_good_bot_identifier(&mut self, identifier: &str) {
        self.custom_good_identifiers
            .insert(identifier.to_lowercase());
    }

    /// Check if User-Agent matches a known good bot
    fn is_good_bot(&self, user_agent: &str) -> Option<String> {
        let ua_lower = user_agent.to_lowercase();

        for identifier in GOOD_BOT_IDENTIFIERS.iter() {
            if ua_lower.contains(identifier) {
                return Some(identifier.to_string());
            }
        }

        for identifier in &self.custom_good_identifiers {
            if ua_lower.contains(identifier) {
                return Some(identifier.clone());
            }
        }

        None
    }

    /// Check if User-Agent matches a known bad bot
    fn is_bad_bot(&self, user_agent: &str) -> Option<String> {
        for pattern in BAD_BOT_PATTERNS.iter() {
            if pattern.is_match(user_agent) {
                return Some(pattern.as_str().to_string());
            }
        }

        for pattern in &self.custom_bad_patterns {
            if pattern.is_match(user_agent) {
                return Some(pattern.as_str().to_string());
            }
        }

        None
    }

    /// Detect bot type from User-Agent
    pub fn detect_bot(&self, user_agent: &str) -> BotType {
        if user_agent.is_empty() {
            return BotType::SuspiciousBot("Empty User-Agent".to_string());
        }

        if let Some(bot) = self.is_good_bot(user_agent) {
            return BotType::GoodBot(bot);
        }

        if let Some(bot) = self.is_bad_bot(user_agent) {
            return BotType::BadBot(bot);
        }

        BotType::NotBot
    }
}

impl SecurityRule for BotDetector {
    fn check(
        &self,
        _uri: &Uri,
        headers: &HeaderMap,
        _body: Option<&[u8]>,
    ) -> Result<(), SecurityViolation> {
        if !self.enabled {
            return Ok(());
        }

        let user_agent = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        match self.detect_bot(user_agent) {
            BotType::GoodBot(_) => {
                if self.allow_known_bots {
                    Ok(())
                } else {
                    Err(SecurityViolation::new(
                        "BOT_DETECTED",
                        ThreatLevel::Low,
                        format!("Good bot detected: {user_agent}"),
                        self.block_mode,
                    ))
                }
            }
            BotType::BadBot(bot) => Err(SecurityViolation::new(
                "BAD_BOT",
                ThreatLevel::High,
                format!("Malicious bot detected: {bot}"),
                self.block_mode,
            )),
            BotType::SuspiciousBot(reason) => Err(SecurityViolation::new(
                "SUSPICIOUS_BOT",
                ThreatLevel::Medium,
                format!("Suspicious bot: {reason}"),
                self.block_mode,
            )),
            BotType::NotBot => Ok(()),
        }
    }

    fn name(&self) -> &str {
        "Bot Detector"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, Uri};

    use super::*;

    #[test]
    fn test_good_bot_detection() {
        let detector = BotDetector::new(true, false, true);
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_static("Mozilla/5.0 (compatible; Googlebot/2.1)"),
        );
        let uri: Uri = "/".parse().expect("valid uri");
        assert!(detector.check(&uri, &headers, None).is_ok());
    }

    #[test]
    fn test_bad_bot_detection() {
        let detector = BotDetector::new(true, true, true);
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("sqlmap/1.0"));
        let uri: Uri = "/".parse().expect("valid uri");
        assert!(detector.check(&uri, &headers, None).is_err());
    }

    #[test]
    fn test_normal_user_agent() {
        let detector = BotDetector::new(true, true, true);
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            ),
        );
        let uri: Uri = "/".parse().expect("valid uri");
        assert!(detector.check(&uri, &headers, None).is_ok());
    }
}
