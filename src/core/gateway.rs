use std::{collections::HashMap as StdHashMap, sync::Arc};

use scc::HashMap;

use crate::{
    config::{HealthCheckConfig, HealthStatus, RouteConfig, ServerConfig},
    core::backend::{BackendHealth, BackendUrl},
};

pub struct GatewayService {
    config: Arc<ServerConfig>,
    backend_health: Arc<HashMap<String, BackendHealth>>,
}

impl GatewayService {
    pub fn new(config: Arc<ServerConfig>) -> Self {
        let backend_health = Arc::new(HashMap::new());

        let backends = Self::collect_backends(&config.routes);

        for backend in &backends {
            if let Ok(backend_url) = BackendUrl::new(backend) {
                let _ = backend_health.insert(backend.clone(), BackendHealth::new(backend_url));
            } else {
                tracing::error!("Invalid backend URL: {}", backend);
            }
        }

        Self {
            config,
            backend_health,
        }
    }

    pub fn backend_health(&self) -> &HashMap<String, BackendHealth> {
        &self.backend_health
    }

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

    pub fn find_matching_route(&self, path: &str) -> Option<(String, RouteConfig)> {
        self.config
            .routes
            .iter()
            .filter(|(prefix, _)| path.starts_with(*prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(prefix, config)| (prefix.to_string(), config.clone()))
    }

    pub fn health_config(&self) -> &HealthCheckConfig {
        &self.config.health_check
    }

    pub fn get_backend_health_path(&self, target: &str) -> String {
        self.config
            .backend_health_paths
            .get(target)
            .cloned()
            .unwrap_or_else(|| self.config.health_check.path.clone())
    }

    pub fn get_backend_health_status(&self, target: &str) -> HealthStatus {
        self.backend_health
            .read(target, |_, backend| backend.status())
            .unwrap_or(HealthStatus::Healthy)
    }

    pub fn get_healthy_backends(&self, targets: &[String]) -> Vec<String> {
        if !self.config.health_check.enabled {
            return targets.to_vec();
        }

        targets
            .iter()
            .filter(|target| self.get_backend_health_status(target) == HealthStatus::Healthy)
            .cloned()
            .collect()
    }

    pub fn backend_count(&self) -> usize {
        self.backend_health.len()
    }

    pub fn healthy_backend_count(&self) -> usize {
        if !self.config.health_check.enabled {
            return self.backend_health.len();
        }

        let mut count = 0;
        self.backend_health.scan(|_, backend| {
            if backend.status() == HealthStatus::Healthy {
                count += 1;
            }
        });
        count
    }

    pub fn select_backend(&self, targets: &[String]) -> Option<String> {
        let healthy_backends = self.get_healthy_backends(targets);
        if healthy_backends.is_empty() {
            return None;
        }

        // Simple round-robin selection - in a real implementation you might want
        // to use a more sophisticated load balancing algorithm
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        let index = COUNTER.fetch_add(1, Ordering::Relaxed) % healthy_backends.len();
        healthy_backends.get(index).cloned()
    }
}
