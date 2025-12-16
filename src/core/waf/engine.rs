//! WAF engine that coordinates all security rules

use std::sync::Arc;

use axum::http::{HeaderMap, Uri};
use tracing::{debug, warn};

use super::{
    BotDetector, CommandInjectionDetector, IpFilter, PathTraversalDetector, SecurityRule,
    SecurityViolation, SqlInjectionDetector, XssDetector,
};
use crate::config::WafConfig;

/// WAF engine that runs all enabled security rules
pub struct WafEngine {
    /// SQL injection detector
    sql_injection: Option<SqlInjectionDetector>,
    /// XSS detector
    xss: Option<XssDetector>,
    /// Command injection detector
    command_injection: Option<CommandInjectionDetector>,
    /// Path traversal detector
    path_traversal: Option<PathTraversalDetector>,
    /// Bot detector
    bot_detector: Option<BotDetector>,
    /// IP filter
    ip_filter: Option<Arc<IpFilter>>,
    /// Whether WAF is enabled globally
    enabled: bool,
}

impl WafEngine {
    /// Create a new WAF engine from configuration
    pub fn from_config(config: &WafConfig) -> eyre::Result<Self> {
        let sql_injection = if config.sql_injection.enabled {
            Some(SqlInjectionDetector::new(
                config.sql_injection.enabled,
                config.sql_injection.block_mode,
            ))
        } else {
            None
        };

        let xss = if config.xss.enabled {
            Some(XssDetector::new(config.xss.enabled, config.xss.block_mode))
        } else {
            None
        };

        let command_injection = if config.command_injection.enabled {
            Some(CommandInjectionDetector::new(
                config.command_injection.enabled,
                config.command_injection.block_mode,
            ))
        } else {
            None
        };

        let path_traversal = if config.path_traversal.enabled {
            Some(PathTraversalDetector::new(
                config.path_traversal.enabled,
                config.path_traversal.block_mode,
            ))
        } else {
            None
        };

        let bot_detector = if config.bot_detection.enabled {
            let mut detector = BotDetector::new(
                config.bot_detection.enabled,
                config.bot_detection.block_mode,
                config.bot_detection.allow_known_bots,
            );

            // Add custom patterns and identifiers
            for pattern in &config.bot_detection.custom_bad_patterns {
                detector
                    .add_bad_bot_pattern(pattern)
                    .map_err(|e| eyre::eyre!("Invalid bot pattern '{}': {}", pattern, e))?;
            }

            for identifier in &config.bot_detection.custom_good_identifiers {
                detector.add_good_bot_identifier(identifier);
            }

            Some(detector)
        } else {
            None
        };

        let ip_filter = if config.ip_filter.enabled {
            let mut filter = IpFilter::new(config.ip_filter.enabled);

            for ip_or_cidr in &config.ip_filter.whitelist {
                filter
                    .add_to_whitelist(ip_or_cidr)
                    .map_err(|e| eyre::eyre!("Invalid whitelist entry '{}': {}", ip_or_cidr, e))?;
            }

            for ip_or_cidr in &config.ip_filter.blacklist {
                filter
                    .add_to_blacklist(ip_or_cidr)
                    .map_err(|e| eyre::eyre!("Invalid blacklist entry '{}': {}", ip_or_cidr, e))?;
            }

            Some(Arc::new(filter))
        } else {
            None
        };

        Ok(Self {
            sql_injection,
            xss,
            command_injection,
            path_traversal,
            bot_detector,
            ip_filter,
            enabled: config.enabled,
        })
    }

    /// Check a request against all enabled rules
    ///
    /// Returns Ok(()) if no violations are detected, or Err(SecurityViolation) if a violation is found.
    /// The violation's `blocked` field indicates whether the request should be blocked.
    pub fn check_request(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        body: Option<&[u8]>,
        client_ip: Option<&str>,
    ) -> Result<(), SecurityViolation> {
        if !self.enabled {
            return Ok(());
        }

        // Check IP filter first (most efficient to fail fast)
        if let Some(ref filter) = self.ip_filter
            && let Some(ip) = client_ip
            && let Err(violation) = filter.check_ip(ip)
        {
            warn!(
                ip = ip,
                threat_type = violation.threat_type,
                "IP filter violation"
            );
            return Err(violation);
        }

        // Check bot detector
        if let Some(ref detector) = self.bot_detector
            && let Err(violation) = detector.check(uri, headers, body)
        {
            if violation.blocked {
                warn!(
                    uri = %uri,
                    threat_type = violation.threat_type,
                    "Bot detection violation"
                );
                return Err(violation);
            } else {
                debug!(
                    uri = %uri,
                    threat_type = violation.threat_type,
                    "Bot detection violation (log only)"
                );
            }
        }

        // Check SQL injection
        if let Some(ref detector) = self.sql_injection
            && let Err(violation) = detector.check(uri, headers, body)
        {
            warn!(
                uri = %uri,
                threat_type = violation.threat_type,
                "SQL injection detected"
            );
            return Err(violation);
        }

        // Check XSS
        if let Some(ref detector) = self.xss
            && let Err(violation) = detector.check(uri, headers, body)
        {
            warn!(
                uri = %uri,
                threat_type = violation.threat_type,
                "XSS attack detected"
            );
            return Err(violation);
        }

        // Check command injection
        if let Some(ref detector) = self.command_injection
            && let Err(violation) = detector.check(uri, headers, body)
        {
            warn!(
                uri = %uri,
                threat_type = violation.threat_type,
                "Command injection detected"
            );
            return Err(violation);
        }

        // Check path traversal
        if let Some(ref detector) = self.path_traversal
            && let Err(violation) = detector.check(uri, headers, body)
        {
            warn!(
                uri = %uri,
                threat_type = violation.threat_type,
                "Path traversal detected"
            );
            return Err(violation);
        }

        Ok(())
    }

    /// Check if WAF is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}
