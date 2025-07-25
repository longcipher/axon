use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::{
    adapters::health_checker::HealthChecker, config::models::ServerConfig, core::GatewayService,
    ports::http_client::HttpClient,
};

/// Helper function to spawn a new health checker task
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

/// Helper function to create a health checker task without immediately spawning it
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

/// Validate health check configuration
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

/// Get health check status for all backends
pub async fn get_backend_health_status(gateway_service: &GatewayService) -> Vec<(String, bool)> {
    let mut status = Vec::new();

    // Get all backend URLs by scanning the backend health map
    let backend_health = gateway_service.backend_health();
    backend_health.scan(|target, _| {
        let health_status = gateway_service.get_backend_health_status(target);
        let is_healthy = health_status == crate::config::HealthStatus::Healthy;
        status.push((target.clone(), is_healthy));
    });

    status
}

/// Check if any backends are healthy
pub async fn has_healthy_backends(gateway_service: &GatewayService) -> bool {
    let status = get_backend_health_status(gateway_service).await;
    status.iter().any(|(_, is_healthy)| *is_healthy)
}

/// Get count of healthy vs unhealthy backends
pub async fn get_backend_health_summary(gateway_service: &GatewayService) -> (usize, usize) {
    let status = get_backend_health_status(gateway_service).await;
    let healthy_count = status.iter().filter(|(_, is_healthy)| *is_healthy).count();
    let unhealthy_count = status.len() - healthy_count;
    (healthy_count, unhealthy_count)
}

/// Log health check summary
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
    use crate::config::models::{BackendConfig, HealthCheckConfig};

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
