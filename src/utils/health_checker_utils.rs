//! Helper utilities for managing health checker tasks and summarizing backend health.
//!
//! This module offers small orchestration helpers that sit above the raw
//! [`HealthChecker`] adapter: spawning tasks, building futures for embedding in
//! supervisor logic, validating configuration fields early, and aggregating
//! health status snapshots for logging / dashboards.
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::{
    adapters::health_checker::HealthChecker, config::models::ServerConfig, core::GatewayService,
    ports::http_client::HttpClient,
};

/// Spawn a detached tokio task running the health checker loop if enabled.
pub fn spawn_health_checker_task(
    gateway_service: Arc<GatewayService>,
    http_client: Arc<dyn HttpClient>,
    config: Arc<ServerConfig>,
    source_log_prefix: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if config.health_check.enabled {
            tracing::info!(
                "({}) Health checker task started. Interval: {}s, Path: {}, Unhealthy Threshold: {}, Healthy Threshold: {}",
                source_log_prefix,
                config.health_check.interval_secs,
                config.health_check.path,
                config.health_check.unhealthy_threshold,
                config.health_check.healthy_threshold
            );
            let health_checker = HealthChecker::new(gateway_service, http_client);
            if let Err(e) = health_checker.run().await {
                tracing::error!("({}) Health checker run error: {}", source_log_prefix, e);
            }
        } else {
            tracing::info!(
                "({}) Health checking is disabled by current configuration snapshot. Health checker task not running.",
                source_log_prefix
            );
        }
    })
}

/// Prepare a future for the health checker without spawning it (caller decides runtime context).
pub async fn create_health_checker_task(
    gateway_service: Arc<GatewayService>,
    http_client: Arc<dyn HttpClient>,
    config: Arc<ServerConfig>,
    source_log_prefix: String,
) -> Option<impl std::future::Future<Output = eyre::Result<()>>> {
    if config.health_check.enabled {
        tracing::info!(
            "({}) Health checker configured. Interval: {}s, Path: {}, Unhealthy Threshold: {}, Healthy Threshold: {}",
            source_log_prefix,
            config.health_check.interval_secs,
            config.health_check.path,
            config.health_check.unhealthy_threshold,
            config.health_check.healthy_threshold
        );
        let health_checker = HealthChecker::new(gateway_service, http_client);
        Some(async move { health_checker.run().await })
    } else {
        tracing::info!(
            "({}) Health checking is disabled by current configuration.",
            source_log_prefix
        );
        None
    }
}

/// Validate health check related configuration parameters (sanity checks only).
pub fn validate_health_check_config(config: &ServerConfig) -> eyre::Result<()> {
    if !config.health_check.enabled {
        return Ok(());
    }

    if config.health_check.interval_secs == 0 {
        return Err(eyre::eyre!("Health check interval must be greater than 0"));
    }

    if config.health_check.unhealthy_threshold == 0 {
        return Err(eyre::eyre!("Unhealthy threshold must be greater than 0"));
    }

    if config.health_check.healthy_threshold == 0 {
        return Err(eyre::eyre!("Healthy threshold must be greater than 0"));
    }

    if config.health_check.path.is_empty() {
        return Err(eyre::eyre!("Health check path cannot be empty"));
    }

    if !config.health_check.path.starts_with('/') {
        return Err(eyre::eyre!("Health check path must start with '/'"));
    }

    Ok(())
}

/// Collect (backend_url, healthy) pairs.
pub async fn get_backend_health_status(gateway_service: &GatewayService) -> Vec<(String, bool)> {
    let mut status = Vec::new();
    let mut backend_urls = Vec::new();
    
    // First collect all backend URLs
    let backend_health = gateway_service.backend_health();
    backend_health.retain_async(|target, _| {
        backend_urls.push(target.to_string());
        true
    }).await;

    // Then check health status for each backend (can use async operations)
    for url in backend_urls {
        let health_status = gateway_service.get_backend_health_status(&url).await;
        let is_healthy = health_status == crate::config::HealthStatus::Healthy;
        status.push((url, is_healthy));
    }

    status
}

/// Return true if at least one backend is healthy.
pub async fn has_healthy_backends(gateway_service: &GatewayService) -> bool {
    let status = get_backend_health_status(gateway_service).await;
    status.iter().any(|(_, is_healthy)| *is_healthy)
}

/// Count healthy vs unhealthy backends.
pub async fn get_backend_health_summary(gateway_service: &GatewayService) -> (usize, usize) {
    let status = get_backend_health_status(gateway_service).await;
    let healthy_count = status.iter().filter(|(_, is_healthy)| *is_healthy).count();
    let unhealthy_count = status.len() - healthy_count;
    (healthy_count, unhealthy_count)
}

/// Emit a concise health summary log line (warns if any unhealthy or none configured).
pub async fn log_health_summary(gateway_service: &GatewayService, log_prefix: &str) {
    let (healthy, unhealthy) = get_backend_health_summary(gateway_service).await;
    let total = healthy + unhealthy;

    if total == 0 {
        tracing::warn!("({}) No backends configured", log_prefix);
        return;
    }

    if unhealthy > 0 {
        tracing::warn!(
            "({}) Backend health summary: {}/{} healthy, {} unhealthy",
            log_prefix,
            healthy,
            total,
            unhealthy
        );
    } else {
        tracing::info!(
            "({}) Backend health summary: all {} backends healthy",
            log_prefix,
            total
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::HealthCheckConfig;

    fn create_test_config(enabled: bool) -> ServerConfig {
        ServerConfig {
            health_check: HealthCheckConfig {
                enabled,
                interval_secs: 30,
                path: "/health".to_string(),
                unhealthy_threshold: 3,
                healthy_threshold: 2,
                timeout_secs: 5,
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_health_check_config_disabled() {
        let config = create_test_config(false);
        assert!(validate_health_check_config(&config).is_ok());
    }

    #[test]
    fn test_validate_health_check_config_enabled_valid() {
        let config = create_test_config(true);
        assert!(validate_health_check_config(&config).is_ok());
    }

    #[test]
    fn test_validate_health_check_config_invalid_interval() {
        let mut config = create_test_config(true);
        config.health_check.interval_secs = 0;
        assert!(validate_health_check_config(&config).is_err());
    }

    #[test]
    fn test_validate_health_check_config_invalid_path() {
        let mut config = create_test_config(true);
        config.health_check.path = "health".to_string(); // missing leading slash
        assert!(validate_health_check_config(&config).is_err());
    }

    #[test]
    fn test_validate_health_check_config_empty_path() {
        let mut config = create_test_config(true);
        config.health_check.path = "".to_string();
        assert!(validate_health_check_config(&config).is_err());
    }

    #[test]
    fn test_validate_health_check_config_zero_thresholds() {
        let mut config = create_test_config(true);
        config.health_check.unhealthy_threshold = 0;
        assert!(validate_health_check_config(&config).is_err());

        let mut config = create_test_config(true);
        config.health_check.healthy_threshold = 0;
        assert!(validate_health_check_config(&config).is_err());
    }
}
