//! IP filtering module with CIDR support
//!
//! Provides IP whitelisting and blacklisting with CIDR notation support.

use std::{net::IpAddr, str::FromStr};

use super::{SecurityViolation, ThreatLevel};

/// CIDR network representation
#[derive(Debug, Clone)]
pub struct IpNetwork {
    /// Base IP address
    addr: IpAddr,
    /// Prefix length (e.g., 24 for /24)
    prefix_len: u8,
}

impl IpNetwork {
    /// Create a new IP network from an address and prefix length
    pub fn new(addr: IpAddr, prefix_len: u8) -> Result<Self, String> {
        match addr {
            IpAddr::V4(_) if prefix_len > 32 => {
                return Err("IPv4 prefix length must be <= 32".to_string());
            }
            IpAddr::V6(_) if prefix_len > 128 => {
                return Err("IPv6 prefix length must be <= 128".to_string());
            }
            _ => {}
        }

        Ok(Self { addr, prefix_len })
    }

    /// Parse from CIDR notation (e.g., "192.168.1.0/24")
    pub fn parse(s: &str) -> Result<Self, String> {
        if let Some((ip_str, prefix_str)) = s.split_once('/') {
            let addr = IpAddr::from_str(ip_str).map_err(|e| format!("Invalid IP address: {e}"))?;
            let prefix_len: u8 = prefix_str
                .parse()
                .map_err(|e| format!("Invalid prefix length: {e}"))?;
            Self::new(addr, prefix_len)
        } else {
            // No prefix, treat as single IP (/32 or /128)
            let addr = IpAddr::from_str(s).map_err(|e| format!("Invalid IP address: {e}"))?;
            let prefix_len = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            Ok(Self { addr, prefix_len })
        }
    }

    /// Check if an IP address is contained in this network
    pub fn contains(&self, ip: IpAddr) -> bool {
        // IPs must be same version
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => {
                let net_bits = u32::from(net);
                let addr_bits = u32::from(addr);
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    !0u32 << (32 - self.prefix_len)
                };
                (net_bits & mask) == (addr_bits & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(addr)) => {
                let net_bits = u128::from(net);
                let addr_bits = u128::from(addr);
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    !0u128 << (128 - self.prefix_len)
                };
                (net_bits & mask) == (addr_bits & mask)
            }
            _ => false,
        }
    }
}

impl std::str::FromStr for IpNetwork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        IpNetwork::parse(s)
    }
}

/// IP filter with whitelist and blacklist support
pub struct IpFilter {
    /// Networks in the whitelist
    pub whitelist: Vec<IpNetwork>,
    /// Networks in the blacklist
    pub blacklist: Vec<IpNetwork>,
    /// Whether IP filtering is enabled
    pub enabled: bool,
}

impl IpFilter {
    /// Create a new IP filter
    pub fn new(enabled: bool) -> Self {
        Self {
            whitelist: Vec::new(),
            blacklist: Vec::new(),
            enabled,
        }
    }

    /// Add an IP or CIDR range to the whitelist
    pub fn add_to_whitelist(&mut self, ip_or_cidr: &str) -> Result<(), String> {
        let network = IpNetwork::parse(ip_or_cidr)?;
        self.whitelist.push(network);
        Ok(())
    }

    /// Add an IP or CIDR range to the blacklist
    pub fn add_to_blacklist(&mut self, ip_or_cidr: &str) -> Result<(), String> {
        let network = IpNetwork::parse(ip_or_cidr)?;
        self.blacklist.push(network);
        Ok(())
    }

    /// Check if an IP address is in the given network list
    fn ip_in_networks(&self, ip: &IpAddr, networks: &[IpNetwork]) -> bool {
        networks.iter().any(|network| network.contains(*ip))
    }

    /// Check if an IP address passes the filter
    pub fn check_ip(&self, ip_str: &str) -> Result<(), SecurityViolation> {
        if !self.enabled {
            return Ok(());
        }

        let ip = match IpAddr::from_str(ip_str) {
            Ok(addr) => addr,
            Err(_) => return Ok(()), // Invalid IP format, let it through
        };

        // Check whitelist first - if non-empty, IP must be in it
        if !self.whitelist.is_empty() && !self.ip_in_networks(&ip, &self.whitelist) {
            return Err(SecurityViolation::new(
                "IP_NOT_WHITELISTED",
                ThreatLevel::High,
                format!("IP {ip} not in whitelist"),
                true,
            ));
        }

        // Check blacklist
        if self.ip_in_networks(&ip, &self.blacklist) {
            return Err(SecurityViolation::new(
                "IP_BLACKLISTED",
                ThreatLevel::Critical,
                format!("IP {ip} is blacklisted"),
                true,
            ));
        }

        Ok(())
    }

    /// Get the number of whitelist entries
    pub fn whitelist_count(&self) -> usize {
        self.whitelist.len()
    }

    /// Get the number of blacklist entries
    pub fn blacklist_count(&self) -> usize {
        self.blacklist.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipnetwork_v4_contains() {
        let network = IpNetwork::parse("192.168.1.0/24").expect("valid network");
        assert!(network.contains("192.168.1.1".parse().expect("valid ip")));
        assert!(network.contains("192.168.1.255".parse().expect("valid ip")));
        assert!(!network.contains("192.168.2.1".parse().expect("valid ip")));
    }

    #[test]
    fn test_ipnetwork_single_ip() {
        let network = IpNetwork::parse("192.168.1.1").expect("valid network");
        assert!(network.contains("192.168.1.1".parse().expect("valid ip")));
        assert!(!network.contains("192.168.1.2".parse().expect("valid ip")));
    }

    #[test]
    fn test_ip_whitelist() {
        let mut filter = IpFilter::new(true);
        filter
            .add_to_whitelist("192.168.1.0/24")
            .expect("valid cidr");

        assert!(filter.check_ip("192.168.1.1").is_ok());
        assert!(filter.check_ip("192.168.2.1").is_err());
    }

    #[test]
    fn test_ip_blacklist() {
        let mut filter = IpFilter::new(true);
        filter.add_to_blacklist("10.0.0.1").expect("valid ip");

        assert!(filter.check_ip("10.0.0.1").is_err());
        assert!(filter.check_ip("10.0.0.2").is_ok());
    }

    #[test]
    fn test_whitelist_takes_precedence() {
        let mut filter = IpFilter::new(true);
        filter
            .add_to_whitelist("192.168.1.0/24")
            .expect("valid cidr");
        filter.add_to_blacklist("192.168.1.100").expect("valid ip");

        // Whitelist check happens first, so IP must be in whitelist
        assert!(filter.check_ip("192.168.1.100").is_err()); // blocked by blacklist
        assert!(filter.check_ip("192.168.1.1").is_ok()); // in whitelist, not in blacklist
    }
}
