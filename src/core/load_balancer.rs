use std::sync::atomic::{AtomicUsize, Ordering};

use rand::Rng;

/// Trait defining the interface for load balancing strategies.
///
/// A strategy is stateless or internally synchronized and can be shared across
/// threads. Implementors should avoid heavy contention in `select_target` as it
/// runs in the request hot path.
pub trait LoadBalancingStrategy: Send + Sync + 'static {
    /// Select a target from a list of targets
    fn select_target(&self, targets: &[String]) -> Option<String>;
    /// Create a new instance of this strategy as a boxed trait object
    fn boxed(self) -> Box<dyn LoadBalancingStrategy>
    where
        Self: Sized,
    {
        Box::new(self)
    }
}

/// Round-robin load balancing strategy.
///
/// Uses an atomic counter cycling through the slice index space.
pub struct RoundRobinStrategy {
    counter: AtomicUsize,
}

impl Default for RoundRobinStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl RoundRobinStrategy {
    /// Create a new round-robin strategy
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
        }
    }
}

impl LoadBalancingStrategy for RoundRobinStrategy {
    fn select_target(&self, targets: &[String]) -> Option<String> {
        if targets.is_empty() {
            return None;
        }
        let count = self.counter.fetch_add(1, Ordering::SeqCst);
        Some(targets[count % targets.len()].clone())
    }
}

/// Random selection load balancing strategy.
///
/// Utilizes the thread‑local `rand::rng()` to pick an index uniformly.
pub struct RandomStrategy;

impl Default for RandomStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomStrategy {
    /// Create a new random selection strategy
    pub fn new() -> Self {
        Self
    }
}

impl LoadBalancingStrategy for RandomStrategy {
    fn select_target(&self, targets: &[String]) -> Option<String> {
        if targets.is_empty() {
            return None;
        }
        let index = rand::rng().random_range(0..targets.len());
        Some(targets[index].clone())
    }
}

/// Factory for creating load balancing strategies from configuration values.
pub struct LoadBalancerFactory;

impl LoadBalancerFactory {
    /// Create a new load balancing strategy based on configuration
    pub fn create_strategy(
        strategy: &crate::config::LoadBalanceStrategy,
    ) -> Box<dyn LoadBalancingStrategy> {
        match strategy {
            crate::config::LoadBalanceStrategy::RoundRobin => RoundRobinStrategy::new().boxed(),
            crate::config::LoadBalanceStrategy::Random => RandomStrategy::new().boxed(),
            crate::config::LoadBalanceStrategy::LeastConnections => {
                tracing::warn!(
                    "LeastConnections strategy not yet implemented, falling back to RoundRobin"
                );
                RoundRobinStrategy::new().boxed()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_robin_strategy() {
        let strategy = RoundRobinStrategy::new();
        let targets = vec![
            "server1".to_string(),
            "server2".to_string(),
            "server3".to_string(),
        ];

        // Test multiple selections to ensure round-robin behavior
        assert_eq!(
            strategy.select_target(&targets),
            Some("server1".to_string())
        );
        assert_eq!(
            strategy.select_target(&targets),
            Some("server2".to_string())
        );
        assert_eq!(
            strategy.select_target(&targets),
            Some("server3".to_string())
        );
        assert_eq!(
            strategy.select_target(&targets),
            Some("server1".to_string())
        ); // Wraps around
    }

    #[test]
    fn test_round_robin_empty_targets() {
        let strategy = RoundRobinStrategy::new();
        let targets: Vec<String> = vec![];
        assert_eq!(strategy.select_target(&targets), None);
    }

    #[test]
    fn test_random_strategy() {
        let strategy = RandomStrategy::new();
        let targets = vec![
            "server1".to_string(),
            "server2".to_string(),
            "server3".to_string(),
        ];

        // Test that random strategy returns something from the targets
        let selected = strategy.select_target(&targets);
        assert!(selected.is_some());
        assert!(targets.contains(&selected.unwrap()));
    }

    #[test]
    fn test_random_strategy_empty_targets() {
        let strategy = RandomStrategy::new();
        let targets: Vec<String> = vec![];
        assert_eq!(strategy.select_target(&targets), None);
    }

    #[test]
    fn test_load_balancer_factory() {
        use crate::config::LoadBalanceStrategy;

        // Test round robin factory
        let rr_strategy = LoadBalancerFactory::create_strategy(&LoadBalanceStrategy::RoundRobin);
        let targets = vec!["server1".to_string(), "server2".to_string()];
        assert!(rr_strategy.select_target(&targets).is_some());

        // Test random factory
        let random_strategy = LoadBalancerFactory::create_strategy(&LoadBalanceStrategy::Random);
        assert!(random_strategy.select_target(&targets).is_some());
    }
}
