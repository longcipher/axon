use std::time::Duration;

use async_trait::async_trait;
use eyre::{Context, Result};
use reqwest::Client;
use tokio::sync::mpsc;

use crate::{config::models::ServerConfig, ports::config_provider::ConfigProvider};

/// Configuration provider that fetches from an HTTP URL and polls for changes.
pub struct HttpConfigProvider {
    url: String,
    client: Client,
    update_rx: std::sync::Mutex<Option<mpsc::Receiver<()>>>,
}

impl HttpConfigProvider {
    pub fn new(url: String, poll_interval: Duration) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        let provider = Self {
            url: url.clone(),
            client: client.clone(),
            update_rx: std::sync::Mutex::new(Some(rx)),
        };

        // Start polling task
        tokio::spawn(async move {
            let mut last_config_hash: Option<u64> = None;
            let mut interval = tokio::time::interval(poll_interval);

            loop {
                interval.tick().await;

                // Fetch config to check for changes (naive approach: fetch & hash)
                // In a real system, use ETag or Last-Modified
                match Self::fetch_config(&client, &url).await {
                    Ok(config) => {
                        use std::{
                            collections::hash_map::DefaultHasher,
                            hash::{Hash, Hasher},
                        };

                        // We can't easily hash ServerConfig without deriving Hash.
                        // For now, let's just serialize to JSON and hash that.
                        if let Ok(json) = serde_json::to_string(&config) {
                            let mut hasher = DefaultHasher::new();
                            json.hash(&mut hasher);
                            let new_hash = hasher.finish();

                            if let Some(last) = last_config_hash {
                                if last != new_hash {
                                    tracing::info!("Remote configuration changed");
                                    let _ = tx.try_send(());
                                    last_config_hash = Some(new_hash);
                                }
                            } else {
                                last_config_hash = Some(new_hash);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to poll remote config: {}", e);
                    }
                }
            }
        });

        provider
    }

    async fn fetch_config(client: &Client, url: &str) -> Result<ServerConfig> {
        let resp = client
            .get(url)
            .send()
            .await
            .context("Failed to send request")?;
        let config = resp
            .json::<ServerConfig>()
            .await
            .context("Failed to parse JSON config")?;
        Ok(config)
    }
}

#[async_trait]
impl ConfigProvider for HttpConfigProvider {
    async fn load_config(&self) -> Result<ServerConfig> {
        Self::fetch_config(&self.client, &self.url).await
    }

    fn watch(&self) -> mpsc::Receiver<()> {
        self.update_rx
            .lock()
            .expect("failed to lock update_rx mutex")
            .take()
            .expect("Watch can only be called once")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, routing::get};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn test_http_config_provider() -> Result<()> {
        // Shared state to update config dynamically
        let config_state = Arc::new(Mutex::new(ServerConfig::default()));
        config_state.lock().unwrap().listen_addr = "127.0.0.1:8080".to_string();

        let state = config_state.clone();
        let app = Router::new().route(
            "/config",
            get(move || async move {
                let config = state.lock().unwrap().clone();
                Json(config)
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let url = format!("http://{}/config", addr);

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Create provider with short poll interval
        let provider = HttpConfigProvider::new(url, Duration::from_millis(100));

        // Initial load
        let config = provider.load_config().await?;
        assert_eq!(config.listen_addr, "127.0.0.1:8080");

        // Watch for changes
        let mut rx = provider.watch();

        // Wait for the background task to perform its initial fetch
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Update mock server config
        {
            let mut lock = config_state.lock().unwrap();
            lock.listen_addr = "127.0.0.1:9090".to_string();
        }

        // Wait for notification
        let notification = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(notification.is_ok(), "Timed out waiting for config update");
        assert!(
            notification.unwrap().is_some(),
            "Channel closed unexpectedly"
        );

        // Verify new config loads
        let config = provider.load_config().await?;
        assert_eq!(config.listen_addr, "127.0.0.1:9090");

        Ok(())
    }
}
