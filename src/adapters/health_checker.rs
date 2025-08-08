use std::{sync::Arc, time::Duration};

use eyre::{Result, WrapErr};
use tokio::time::sleep;

use crate::{
    config::{HealthCheckConfig, HealthStatus},
    core::{GatewayService, backend::BackendHealth},
    ports::http_client::HttpClient,
};

/// Health checker adapter for monitoring backend health
pub struct HealthChecker {
    gateway_service: Arc<GatewayService>,
    http_client: Arc<dyn HttpClient>,
}

impl HealthChecker {
    pub fn new(gateway_service: Arc<GatewayService>, http_client: Arc<dyn HttpClient>) -> Self {
        Self {
            gateway_service,
            http_client,
        }
    }

    /// Run the health checker loop
    pub async fn run(&self) -> Result<()> {
        let health_config = self.gateway_service.health_config();

        if !health_config.enabled {
            tracing::info!("Health checking is disabled");
            return Ok(());
        }

        let interval = Duration::from_secs(health_config.interval_secs);
        let timeout_secs = health_config.timeout_secs;

        tracing::info!(
            "Starting health checker with interval: {}s, timeout: {}s, default path: {}",
            health_config.interval_secs,
            health_config.timeout_secs,
            health_config.path
        );

        loop {
            // Sleep at the beginning to allow the server to start up
            sleep(interval).await;

            tracing::info!("Running health checks on all backends...");

            // Check each backend using scc::HashMap scan method
            let backend_health = self.gateway_service.backend_health();
            let mut backends_to_check = Vec::new();

            backend_health.scan(|target, _| {
                backends_to_check.push(target.clone());
            });

            for target in backends_to_check {
                // Get backend-specific health check path or use default
                let backend_path = self.gateway_service.get_backend_health_path(&target);

                // Construct health check URL
                let health_check_url = format!("{target}{backend_path}");

                tracing::debug!("Health checking: {}", health_check_url);

                // Perform the health check with timeout
                match self
                    .http_client
                    .health_check(&health_check_url, timeout_secs)
                    .await
                {
                    Ok(is_healthy) => {
                        // Update counters and status using thresholds
                        backend_health.update(&target, |_, h| {
                            if is_healthy {
                                self.handle_health_check_success(&target, h, health_config);
                            } else {
                                self.handle_health_check_failure(
                                    &target,
                                    h,
                                    health_config,
                                    "backend reported unhealthy",
                                );
                            }
                        });
                    }
                    Err(err) => {
                        tracing::warn!("Health check failed for backend {}: {}", target, err);
                        backend_health.update(&target, |_, h| {
                            self.handle_health_check_failure(
                                &target,
                                h,
                                health_config,
                                &format!("request error: {err}"),
                            );
                        });
                    }
                }
            }

            tracing::debug!("Health check cycle completed");
        }
    }

    /// Handle successful health check
    #[allow(dead_code)]
    fn handle_health_check_success(
        &self,
        target: &str,
        backend_health: &BackendHealth,
        health_config: &HealthCheckConfig,
    ) {
        // Increment success counter
        let successes = backend_health
            .consecutive_successes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;

        // Reset failure counter
        backend_health
            .consecutive_failures
            .store(0, std::sync::atomic::Ordering::Relaxed);

        // Log successful health check
        tracing::debug!(
            "Health check for {} succeeded ({} consecutive successes)",
            target,
            successes
        );

        // If we've reached the threshold, mark as healthy
        if successes >= health_config.healthy_threshold
            && backend_health.status() == HealthStatus::Unhealthy
        {
            tracing::info!(
                "Backend {} is now HEALTHY (after {} consecutive successes)",
                target,
                successes
            );
            backend_health.mark_healthy();
        }
    }

    /// Handle failed health check
    #[allow(dead_code)]
    fn handle_health_check_failure(
        &self,
        target: &str,
        backend_health: &BackendHealth,
        health_config: &HealthCheckConfig,
        reason: &str,
    ) {
        // Atomically increment failure counter and get new value
        let failures = backend_health
            .consecutive_failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;

        // Reset success counter atomically
        backend_health
            .consecutive_successes
            .store(0, std::sync::atomic::Ordering::Relaxed);

        // Log all failures at the INFO level for better visibility
        tracing::info!(
            "Health check failed for {}: {} (failures: {}/{})",
            target,
            reason,
            failures,
            health_config.unhealthy_threshold
        );

        // Mark as unhealthy if threshold reached and current status is healthy
        if failures >= health_config.unhealthy_threshold
            && backend_health.status() == HealthStatus::Healthy
        {
            tracing::warn!(
                "Backend {} is now UNHEALTHY (after {} consecutive failures): {}",
                target,
                failures,
                reason
            );
            backend_health.mark_unhealthy();
        }
    }

    /// Perform a single health check for a specific backend
    pub async fn check_backend_health(&self, backend_url: &str) -> Result<bool> {
        let health_config = self.gateway_service.health_config();
        let backend_path = self.gateway_service.get_backend_health_path(backend_url);
        let health_check_url = format!("{backend_url}{backend_path}");

        self.http_client
            .health_check(&health_check_url, health_config.timeout_secs)
            .await
            .wrap_err_with(|| format!("Failed to check health for backend: {backend_url}"))
    }

    /// Get current health status of all backends
    pub fn get_backend_health_status(&self) -> Vec<(String, HealthStatus)> {
        let mut status = Vec::new();

        self.gateway_service.backend_health().scan(|url, health| {
            status.push((url.clone(), health.status()));
        });

        status
    }

    /// Check if there are any healthy backends
    pub fn has_healthy_backends(&self) -> bool {
        let mut has_healthy = false;
        self.gateway_service
            .backend_health()
            .scan(|_, backend_health| {
                if backend_health.status() == HealthStatus::Healthy {
                    has_healthy = true;
                }
            });
        has_healthy
    }

    /// Get count of healthy vs unhealthy backends
    pub fn get_health_summary(&self) -> (usize, usize) {
        let mut healthy = 0;
        let mut unhealthy = 0;

        self.gateway_service
            .backend_health()
            .scan(|_, backend_health| match backend_health.status() {
                HealthStatus::Healthy => healthy += 1,
                HealthStatus::Unhealthy => unhealthy += 1,
            });

        (healthy, unhealthy)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::{
        config::models::{HealthCheckConfig, ServerConfig},
        ports::http_client::HttpClientError,
    };

    // Mock HTTP client for testing
    struct MockHttpClient {
        should_succeed: bool,
    }

    impl MockHttpClient {
        fn new(should_succeed: bool) -> Self {
            Self { should_succeed }
        }
    }

    use axum::body::Body as AxumBody;
    #[async_trait::async_trait]
    impl HttpClient for MockHttpClient {
        async fn send_request(
            &self,
            _req: hyper::Request<AxumBody>,
        ) -> Result<hyper::Response<AxumBody>, HttpClientError> {
            Err(HttpClientError::ConnectionError(
                "not used in tests".to_string(),
            ))
        }

        async fn health_check(
            &self,
            _url: &str,
            _timeout_secs: u64,
        ) -> Result<bool, HttpClientError> {
            Ok(self.should_succeed)
        }
    }

    fn create_test_health_config() -> HealthCheckConfig {
        HealthCheckConfig {
            enabled: true,
            interval_secs: 30,
            path: "/health".to_string(),
            unhealthy_threshold: 3,
            healthy_threshold: 2,
            timeout_secs: 5,
        }
    }

    fn create_test_backend_health() -> BackendHealth {
        BackendHealth::new("http://example.com".parse().unwrap())
    }

    #[test]
    fn test_handle_health_check_success() {
        let gateway_service = Arc::new(GatewayService::new(Arc::new(ServerConfig::default())));
        let http_client = Arc::new(MockHttpClient::new(true)) as Arc<dyn HttpClient>;
        let health_checker = HealthChecker::new(gateway_service, http_client);

        let backend_health = create_test_backend_health();
        let health_config = create_test_health_config();

        // Mark as unhealthy first
        backend_health.mark_unhealthy();

        // Should not mark as healthy until threshold is reached
        health_checker.handle_health_check_success("http://test", &backend_health, &health_config);
        assert_eq!(backend_health.status(), HealthStatus::Unhealthy);

        // Second success should mark as healthy (threshold = 2)
        health_checker.handle_health_check_success("http://test", &backend_health, &health_config);
        assert_eq!(backend_health.status(), HealthStatus::Healthy);

        assert_eq!(
            backend_health.consecutive_successes.load(Ordering::Relaxed),
            2
        );
        assert_eq!(
            backend_health.consecutive_failures.load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn test_handle_health_check_failure() {
        let gateway_service = Arc::new(GatewayService::new(Arc::new(ServerConfig::default())));
        let http_client = Arc::new(MockHttpClient::new(false)) as Arc<dyn HttpClient>;
        let health_checker = HealthChecker::new(gateway_service, http_client);

        let backend_health = create_test_backend_health();
        let health_config = create_test_health_config();

        // Should not mark as unhealthy until threshold is reached
        health_checker.handle_health_check_failure(
            "http://test",
            &backend_health,
            &health_config,
            "test failure",
        );
        assert_eq!(backend_health.status(), HealthStatus::Healthy);

        health_checker.handle_health_check_failure(
            "http://test",
            &backend_health,
            &health_config,
            "test failure",
        );
        assert_eq!(backend_health.status(), HealthStatus::Healthy);

        // Third failure should mark as unhealthy (threshold = 3)
        health_checker.handle_health_check_failure(
            "http://test",
            &backend_health,
            &health_config,
            "test failure",
        );
        assert_eq!(backend_health.status(), HealthStatus::Unhealthy);

        assert_eq!(
            backend_health.consecutive_failures.load(Ordering::Relaxed),
            3
        );
        assert_eq!(
            backend_health.consecutive_successes.load(Ordering::Relaxed),
            0
        );
    }

    #[tokio::test]
    async fn test_check_backend_health() {
        let gateway_service = Arc::new(GatewayService::new(Arc::new(ServerConfig::default())));
        let http_client = Arc::new(MockHttpClient::new(true)) as Arc<dyn HttpClient>;
        let health_checker = HealthChecker::new(gateway_service, http_client);

        let result = health_checker
            .check_backend_health("http://example.com")
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
