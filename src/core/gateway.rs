//! Core gateway orchestration service.
//!
//! The `GatewayService` aggregates immutable configuration (`ServerConfig`)
//! with runtime state (backend health, per‑route rate limiters). It provides:
//! * Longest‑prefix route lookup
//! * Backend collection / enumeration utilities
//! * Health status queries & filtering
//! * Load‑balancing backend selection (simple round‑robin over healthy set)
//! * Access to constructed rate limiters
//!
//! This layer deliberately avoids I/O and only manipulates in‑memory data so
//! it remains fast and easily testable in isolation.
use std::{collections::HashMap as StdHashMap, sync::Arc};

use axum::http::{HeaderMap, Uri};
use scc::HashMap;

use crate::{
    config::{HealthCheckConfig, HealthStatus, RouteConfig, ServerConfig},
    core::{
        backend::{BackendHealth, BackendUrl},
        rate_limiter::RouteRateLimiter,
        waf::{SecurityViolation, WafEngine},
    },
};

/// Central orchestrator for routing, backend selection, health status lookup
/// and per‑route rate limiting. An instance is cheap to clone (Arc inside).
///
/// Construct with [`GatewayService::new`] by passing an `Arc<ServerConfig>`.
/// The service pre-builds backend health tracking structures and any
/// configured rate limiters so lookups stay fast in the hot path.
pub struct GatewayService {
    config: Arc<ServerConfig>,
    backend_health: Arc<HashMap<String, BackendHealth>>,
    rate_limiters: Arc<HashMap<String, RouteRateLimiter>>, // keyed by route prefix
    waf_engine: Option<Arc<WafEngine>>,
}

impl GatewayService {
    /// Create a new gateway service from a server configuration.
    ///
    /// This eagerly instantiates backend health entries and route-level
    /// rate limiters so that subsequent request handling avoids allocation.
    pub fn new(config: Arc<ServerConfig>) -> Self {
        let backend_health = Arc::new(HashMap::new());
        let rate_limiters = Arc::new(HashMap::new());

        let backends = Self::collect_backends(&config.routes);

        // In scc 3.x, need to use async insert with tokio runtime
        // This is safe to use during initialization since we're in a sync context
        for backend in &backends {
            if let Ok(backend_url) = BackendUrl::new(backend) {
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(
                        backend_health
                            .insert_async(backend.clone(), BackendHealth::new(backend_url)),
                    )
                });
            } else {
                tracing::error!("Invalid backend URL: {}", backend);
            }
        }

        // Build route-level rate limiters
        for (prefix, route) in &config.routes {
            let rate_limit_cfg_opt = match route {
                RouteConfig::Proxy { rate_limit, .. } => rate_limit,
                RouteConfig::LoadBalance { rate_limit, .. } => rate_limit,
                RouteConfig::Static { rate_limit, .. } => rate_limit,
                RouteConfig::Redirect { rate_limit, .. } => rate_limit,
                RouteConfig::Websocket { rate_limit, .. } => rate_limit,
            };
            if let Some(rate_cfg) = rate_limit_cfg_opt {
                match RouteRateLimiter::new(rate_cfg) {
                    Ok(limiter) => {
                        let _ = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(rate_limiters.insert_async(prefix.clone(), limiter))
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to create rate limiter for route '{}': {}",
                            prefix,
                            e
                        );
                    }
                }
            }
        }

        let waf_engine = if let Some(waf_config) = &config.waf {
            match WafEngine::from_config(waf_config) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(e) => {
                    tracing::error!("Failed to initialize WAF engine: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            config,
            backend_health,
            rate_limiters,
            waf_engine,
        }
    }

    /// Check if WAF is enabled
    pub fn is_waf_enabled(&self) -> bool {
        self.waf_engine
            .as_ref()
            .map(|e| e.is_enabled())
            .unwrap_or(false)
    }

    /// Check request against WAF rules
    pub fn check_waf(
        &self,
        uri: &Uri,
        headers: &HeaderMap,
        body: Option<&[u8]>,
        client_ip: Option<&str>,
    ) -> Result<(), SecurityViolation> {
        if let Some(engine) = &self.waf_engine {
            engine.check_request(uri, headers, body, client_ip)
        } else {
            Ok(())
        }
    }

    /// Access the concurrent backend health map (mainly for adapters / diagnostics).
    pub fn backend_health(&self) -> &HashMap<String, BackendHealth> {
        &self.backend_health
    }

    /// Get a cloned route-level rate limiter for a given prefix, if configured
    /// Fetch a cloned per‑route rate limiter if present.
    pub async fn get_rate_limiter(&self, route_prefix: &str) -> Option<RouteRateLimiter> {
        self.rate_limiters
            .get_async(&route_prefix.to_string())
            .await
            .map(|entry| entry.get().clone())
    }

    /// Collect all unique backend target URLs defined in the set of routes.
    pub fn collect_backends(routes: &StdHashMap<String, RouteConfig>) -> Vec<String> {
        let mut backends = routes
            .values()
            .flat_map(|route_config| match route_config {
                RouteConfig::LoadBalance { targets, .. } => targets.clone(),
                RouteConfig::Proxy { target, .. } => vec![target.clone()],
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();

        backends.sort();
        backends.dedup();
        backends
    }

    /// Longest‑prefix match to find a route configuration for an incoming path.
    /// If host is provided, will also filter by matching host header.
    /// Routes with host specified take priority over routes without host.
    pub fn find_matching_route(
        &self,
        path: &str,
        host: Option<&str>,
    ) -> Option<(String, RouteConfig)> {
        // First, try to find routes with matching host
        let with_host = self
            .config
            .routes
            .iter()
            .filter(|(prefix, route_config)| {
                if !path.starts_with(*prefix) {
                    return false;
                }

                // Extract host from route config
                let route_host = match route_config {
                    RouteConfig::Static { host, .. } => host,
                    RouteConfig::Redirect { host, .. } => host,
                    RouteConfig::Proxy { host, .. } => host,
                    RouteConfig::LoadBalance { host, .. } => host,
                    RouteConfig::Websocket { host, .. } => host,
                };

                // If route has a host specified, it must match the request host
                if let Some(route_host) = route_host {
                    if let Some(req_host) = host {
                        route_host.eq_ignore_ascii_case(req_host)
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .max_by_key(|(prefix, _)| prefix.len());

        // If found a route with matching host, return it
        if let Some((prefix, config)) = with_host {
            return Some((prefix.to_string(), config.clone()));
        }

        // Otherwise, fallback to routes without host requirement
        self.config
            .routes
            .iter()
            .filter(|(prefix, route_config)| {
                if !path.starts_with(*prefix) {
                    return false;
                }

                // Only match routes without host specified
                let route_host = match route_config {
                    RouteConfig::Static { host, .. } => host,
                    RouteConfig::Redirect { host, .. } => host,
                    RouteConfig::Proxy { host, .. } => host,
                    RouteConfig::LoadBalance { host, .. } => host,
                    RouteConfig::Websocket { host, .. } => host,
                };

                route_host.is_none()
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(prefix, config)| (prefix.to_string(), config.clone()))
    }

    /// Return the global health check configuration.
    pub fn health_config(&self) -> &HealthCheckConfig {
        &self.config.health_check
    }

    /// Resolve the health probe path for a specific backend target (per‑backend override or global default).
    pub fn get_backend_health_path(&self, target: &str) -> String {
        self.config
            .backend_health_paths
            .get(target)
            .cloned()
            .unwrap_or_else(|| self.config.health_check.path.clone())
    }

    /// Return the last observed health status for a backend (Healthy if unknown / untracked).
    pub async fn get_backend_health_status(&self, target: &str) -> HealthStatus {
        self.backend_health
            .get_async(target)
            .await
            .map(|entry| entry.get().status())
            .unwrap_or(HealthStatus::Healthy)
    }

    /// Filter the provided targets list to only healthy backends (or all if health checking disabled).
    pub async fn get_healthy_backends(&self, targets: &[String]) -> Vec<String> {
        if !self.config.health_check.enabled {
            return targets.to_vec();
        }

        let mut healthy = Vec::new();
        for target in targets {
            if self.get_backend_health_status(target).await == HealthStatus::Healthy {
                healthy.push(target.clone());
            }
        }
        healthy
    }

    /// Total number of tracked backends.
    pub fn backend_count(&self) -> usize {
        self.backend_health.len()
    }

    /// Count of currently healthy backends (or total if health checks disabled).
    pub async fn healthy_backend_count(&self) -> usize {
        if !self.config.health_check.enabled {
            return self.backend_health.len();
        }

        let mut count = 0;
        let count_ref = &mut count;
        self.backend_health
            .retain_async(|_, backend| {
                if backend.status() == HealthStatus::Healthy {
                    *count_ref += 1;
                }
                true
            })
            .await;
        count
    }

    /// Select a backend from a set of (already matched) targets. Applies health filtering
    /// then a simple static round‑robin counter.
    pub async fn select_backend(
        &self,
        targets: &[String],
        strategy: Option<crate::config::LoadBalanceStrategy>,
    ) -> Option<String> {
        let healthy_backends = self.get_healthy_backends(targets).await;
        if healthy_backends.is_empty() {
            return None;
        }

        match strategy.unwrap_or(crate::config::LoadBalanceStrategy::RoundRobin) {
            crate::config::LoadBalanceStrategy::RoundRobin => {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static COUNTER: AtomicUsize = AtomicUsize::new(0);
                let index = COUNTER.fetch_add(1, Ordering::Relaxed) % healthy_backends.len();
                healthy_backends.get(index).cloned()
            }
            crate::config::LoadBalanceStrategy::Random => {
                use rand::Rng;
                let index = rand::rng().random_range(0..healthy_backends.len());
                healthy_backends.get(index).cloned()
            }
            crate::config::LoadBalanceStrategy::LeastConnections => {
                let mut best_backend = None;
                let mut min_conns = usize::MAX;

                for backend_url in &healthy_backends {
                    if let Some(entry) = self.backend_health.get_async(backend_url).await {
                        let conns = entry.get().active_connections();
                        if conns < min_conns {
                            min_conns = conns;
                            best_backend = Some(backend_url.clone());
                        }
                    }
                }
                best_backend.or_else(|| healthy_backends.first().cloned())
            }
        }
    }
}
