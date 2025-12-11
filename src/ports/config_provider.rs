use async_trait::async_trait;
use eyre::Result;
use tokio::sync::mpsc;

use crate::config::models::ServerConfig;

/// Trait for configuration providers that can load and watch for configuration changes.
#[async_trait]
pub trait ConfigProvider: Send + Sync {
    /// Load the current configuration.
    async fn load_config(&self) -> Result<ServerConfig>;

    /// Return a channel that signals when the configuration has changed.
    /// The receiver should trigger a reload by calling `load_config`.
    fn watch(&self) -> mpsc::Receiver<()>;
}
