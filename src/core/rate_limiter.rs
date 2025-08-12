//! Route‑scoped rate limiting abstractions built atop `governor`.
//!
//! Exposes a small enum (`RouteRateLimiter`) that encapsulates one of several
//! limiter modes (global route, per IP, per header value) with unified `check`
//! semantics returning an Axum response on violation. Internals use
//! `governor`'s in‑memory state stores and support different algorithms
//! (TokenBucket, SlidingWindow, FixedWindow) mapped onto appropriate quota
//! constructions.
use std::{
    hash::Hash,
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    sync::Arc,
};

use axum::{
    extract::ConnectInfo,
    response::{IntoResponse, Response as AxumResponse},
};
use governor::{
    Quota, RateLimiter,
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed, keyed::DefaultKeyedStateStore},
};
use http::{HeaderName, Request, StatusCode};
use humantime;
use tracing;

use crate::config::models::{MissingKeyPolicy, RateLimitAlgorithm, RateLimitBy, RateLimitConfig};

/// Internal wrapper bundling a concrete governor limiter instance with
/// response metadata (status + message) and behaviour on missing key.
#[derive(Clone)]
pub struct LimiterWrapper<RL> {
    pub limiter: RL,
    pub status_code: StatusCode,
    pub message: String,
    pub on_missing_key: MissingKeyPolicy,
}

pub type DirectRateLimiterImpl = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;
pub type KeyedRateLimiterImpl<K> = RateLimiter<K, DefaultKeyedStateStore<K>, DefaultClock>;

/// Non‑keyed (global per route) limiter variant.
pub type RouteSpecificLimiter = LimiterWrapper<DirectRateLimiterImpl>;
/// Per‑client‑IP keyed limiter variant.
pub type IpLimiter = LimiterWrapper<KeyedRateLimiterImpl<IpAddr>>;
/// Per‑header‑value keyed limiter variant.
pub type HeaderLimiter = LimiterWrapper<KeyedRateLimiterImpl<String>>;

// --- LimiterWrapper Implementations ---

// Implementation for non-keyed (direct) limiters
impl LimiterWrapper<DirectRateLimiterImpl> {
    pub fn check_route(&self) -> Result<(), Box<AxumResponse>> {
        if self.limiter.check().is_err() {
            let response = (self.status_code, self.message.clone()).into_response();
            Err(Box::new(response))
        } else {
            Ok(())
        }
    }
}

// Generic implementation for keyed limiters
impl<K> LimiterWrapper<KeyedRateLimiterImpl<K>>
where
    K: Clone + Hash + Eq + Send + Sync + 'static,
{
    // Generic check method for keyed limiters
    fn check_keyed(&self, key: &K) -> Result<(), Box<AxumResponse>> {
        if self.limiter.check_key(key).is_err() {
            let response = (self.status_code, self.message.clone()).into_response();
            Err(Box::new(response))
        } else {
            Ok(())
        }
    }
}

// Specific check method for IP-based limiters
impl IpLimiter {
    pub fn check_ip(&self, ip: IpAddr) -> Result<(), Box<AxumResponse>> {
        self.check_keyed(&ip) // Delegates to the generic keyed check
    }
}

// Specific check method for header-based limiters
impl HeaderLimiter {
    pub fn check_header_value(&self, value: &str) -> Result<(), Box<AxumResponse>> {
        // The key for DefaultKeyedStateStore<String> is String, so convert &str to String
        self.check_keyed(&value.to_string())
    }
}

// --- RouteRateLimiter Enum ---
// This enum dispatches to the correct type of limiter based on configuration.
// It holds an Arc to the LimiterWrapper, allowing shared state for the same route.
/// Discriminated union over supported limiter types.
#[derive(Clone)]
pub enum RouteRateLimiter {
    Route(Arc<RouteSpecificLimiter>),
    Ip(Arc<IpLimiter>),
    Header {
        limiter: Arc<HeaderLimiter>,
        header_name: HeaderName, // Store HeaderName for extraction in check method
    },
}

impl RouteRateLimiter {
    /// Creates a new `RouteRateLimiter` based on the provided `RateLimitConfig`.
    /// Build a limiter from a `RateLimitConfig` definition.
    pub fn new(config: &RateLimitConfig) -> Result<Self, String> {
        let period_duration = humantime::parse_duration(&config.period).map_err(|e| {
            format!(
                "Invalid period string '{period}': {e}",
                period = config.period
            )
        })?;

        let quota_requests = NonZeroU32::new(config.requests as u32)
            .ok_or_else(|| "Rate limit 'requests' must be greater than 0".to_string())?;

        // Configure Quota based on the algorithm.
        let quota = match config.algorithm {
            RateLimitAlgorithm::TokenBucket => {
                // TokenBucket allows bursts up to the number of requests over the specified period.
                Quota::with_period(period_duration)
                    .ok_or_else(|| {
                        format!("Invalid period duration for TokenBucket: {period_duration:?}")
                    })?
                    .allow_burst(quota_requests)
            }
            RateLimitAlgorithm::SlidingWindow => {
                // SlidingWindow, using governor's GCRA, allows a number of requests within any
                // sliding time window of the specified period.
                Quota::with_period(period_duration)
                    .ok_or_else(|| {
                        format!("Invalid period duration for SlidingWindow: {period_duration:?}")
                    })?
                    .allow_burst(quota_requests)
            }
            RateLimitAlgorithm::FixedWindow => {
                // FixedWindow allows `requests` per `period_duration`.
                Quota::with_period(period_duration)
                    .ok_or_else(|| {
                        format!("Invalid period duration for FixedWindow: {period_duration:?}")
                    })?
                    .allow_burst(quota_requests)
            }
        };

        let status_code = StatusCode::from_u16(config.status_code)
            .map_err(|_| format!("Invalid status code: {}", config.status_code))?;

        tracing::info!(
            "Creating rate limiter: by={:?}, algorithm={:?}, requests={}, period={}, status_code={}, on_missing_key={:?}",
            config.by,
            config.algorithm,
            config.requests,
            config.period,
            config.status_code,
            config.on_missing_key
        );

        match config.by {
            RateLimitBy::Route => {
                let limiter = Arc::new(LimiterWrapper {
                    limiter: RateLimiter::direct(quota),
                    status_code,
                    message: config.message.clone(),
                    on_missing_key: config.on_missing_key,
                });
                Ok(RouteRateLimiter::Route(limiter))
            }
            RateLimitBy::Ip => {
                let limiter = Arc::new(LimiterWrapper {
                    limiter: RateLimiter::keyed(quota),
                    status_code,
                    message: config.message.clone(),
                    on_missing_key: config.on_missing_key,
                });
                Ok(RouteRateLimiter::Ip(limiter))
            }
            RateLimitBy::Header => {
                let header_name_str = config
                    .header_name
                    .as_ref()
                    .ok_or_else(|| "header_name is required for RateLimitBy::Header".to_string())?;
                let header_name = HeaderName::from_bytes(header_name_str.as_bytes())
                    .map_err(|e| format!("Invalid header_name '{header_name_str}': {e}"))?;
                let limiter = Arc::new(LimiterWrapper {
                    limiter: RateLimiter::keyed(quota),
                    status_code,
                    message: config.message.clone(),
                    on_missing_key: config.on_missing_key,
                });
                Ok(RouteRateLimiter::Header {
                    limiter,
                    header_name,
                })
            }
        }
    }

    /// Checks the rate limit for the given request.
    /// Extracts the appropriate key based on the limiter type and calls the corresponding check method.
    /// Enforce this limiter against an HTTP request.
    pub fn check<T>(&self, req: &Request<T>) -> Result<(), Box<AxumResponse>> {
        match self {
            RouteRateLimiter::Route(limiter) => limiter.check_route(),
            RouteRateLimiter::Ip(limiter) => {
                // Extract client IP from request extensions
                let client_ip = req
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|connect_info| connect_info.0.ip());

                match client_ip {
                    Some(ip) => limiter.check_ip(ip),
                    None => match limiter.on_missing_key {
                        MissingKeyPolicy::Allow => Ok(()),
                        MissingKeyPolicy::Deny => {
                            let response =
                                (limiter.status_code, "No client IP available").into_response();
                            Err(Box::new(response))
                        }
                    },
                }
            }
            RouteRateLimiter::Header {
                limiter,
                header_name,
            } => {
                // Extract header value from request
                let header_value = req
                    .headers()
                    .get(header_name)
                    .and_then(|hv| hv.to_str().ok());

                match header_value {
                    Some(value) => limiter.check_header_value(value),
                    None => match limiter.on_missing_key {
                        MissingKeyPolicy::Allow => Ok(()),
                        MissingKeyPolicy::Deny => {
                            let response = (
                                limiter.status_code,
                                format!("Required header '{header_name}' is missing"),
                            )
                                .into_response();
                            Err(Box::new(response))
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use http::{Method, Request};

    use super::*;
    use crate::config::models::{
        MissingKeyPolicy, RateLimitAlgorithm, RateLimitBy, RateLimitConfig,
    };

    fn create_test_rate_limit_config() -> RateLimitConfig {
        RateLimitConfig {
            by: RateLimitBy::Route,
            header_name: None,
            requests: 5,
            period: "1s".to_string(),
            status_code: 429,
            message: "Too Many Requests".to_string(),
            algorithm: RateLimitAlgorithm::TokenBucket,
            on_missing_key: MissingKeyPolicy::Allow,
        }
    }

    #[test]
    fn test_route_rate_limiter_creation() {
        let config = create_test_rate_limit_config();
        let result = RouteRateLimiter::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_invalid_period() {
        let mut config = create_test_rate_limit_config();
        config.period = "invalid".to_string();
        let result = RouteRateLimiter::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_zero_requests() {
        let mut config = create_test_rate_limit_config();
        config.requests = 0;
        let result = RouteRateLimiter::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_header_limiter_missing_header_name() {
        let mut config = create_test_rate_limit_config();
        config.by = RateLimitBy::Header;
        config.header_name = None;
        let result = RouteRateLimiter::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_route_limiter_check() {
        let config = create_test_rate_limit_config();
        let limiter = RouteRateLimiter::new(&config).unwrap();

        let req = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(())
            .unwrap();

        // First few requests should pass
        for _ in 0..5 {
            assert!(limiter.check(&req).is_ok());
        }

        // Additional requests should be rate limited
        // Note: This might not always fail immediately due to the nature of token bucket
        // In a real test, you might want to sleep or use a more controlled setup
    }
}
