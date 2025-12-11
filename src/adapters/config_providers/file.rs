use std::path::{Path, PathBuf};

use async_trait::async_trait;
use eyre::{Context, Result};
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::{
    config::{loader::load_config, models::ServerConfig},
    ports::config_provider::ConfigProvider,
};

/// Configuration provider that loads from a local file and watches for changes.
pub struct FileConfigProvider {
    path: PathBuf,
    // We keep the watcher alive by storing it, even though we don't access it directly after init
    _watcher: Option<notify::RecommendedWatcher>,
    // The channel receiver is moved out in `watch()`, so we store the sender to clone for the watcher
    update_tx: mpsc::Sender<()>,
    // We store the receiver in an Option so we can take it once
    update_rx: std::sync::Mutex<Option<mpsc::Receiver<()>>>,
}

impl FileConfigProvider {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let (tx, rx) = mpsc::channel(1);

        let mut provider = Self {
            path: path.clone(),
            _watcher: None,
            update_tx: tx,
            update_rx: std::sync::Mutex::new(Some(rx)),
        };

        provider.init_watcher()?;
        Ok(provider)
    }

    fn init_watcher(&mut self) -> Result<()> {
        let tx = self.update_tx.clone();
        let config_path = self.path.clone();
        let config_filename = config_path
            .file_name()
            .ok_or_else(|| eyre::eyre!("Invalid config path"))?
            .to_owned();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        if (event.kind.is_modify()
                            || event.kind.is_create()
                            || event.kind.is_remove())
                            && event
                                .paths
                                .iter()
                                .any(|p| p.file_name() == Some(&config_filename))
                        {
                            tracing::debug!("Config file changed: {:?}", event.kind);
                            // Try to send update signal, ignore if channel full or closed
                            let _ = tx.try_send(());
                        }
                    }
                    Err(e) => tracing::error!("File watch error: {:?}", e),
                }
            })?;

        let watch_dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        watcher
            .watch(watch_dir, RecursiveMode::NonRecursive)
            .wrap_err("Failed to watch config directory")?;

        self._watcher = Some(watcher);
        Ok(())
    }
}

#[async_trait]
impl ConfigProvider for FileConfigProvider {
    async fn load_config(&self) -> Result<ServerConfig> {
        let path_str = self
            .path
            .to_str()
            .ok_or_else(|| eyre::eyre!("Invalid path"))?;
        load_config(path_str).await
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
    use std::{fs::File, io::Write};

    use tempfile::tempdir;
    use tokio::time::{Duration, sleep};

    use super::*;

    #[tokio::test]
    async fn test_file_config_provider() -> Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("config.toml");

        // Initial config
        let initial_config = r#"
            listen_addr = "127.0.0.1:8080"
            [routes."/"]
            type = "static"
            root = "./static"
        "#;
        {
            let mut file = File::create(&file_path)?;
            file.write_all(initial_config.as_bytes())?;
        }

        let provider = FileConfigProvider::new(&file_path)?;
        let config = provider.load_config().await?;
        assert_eq!(config.listen_addr, "127.0.0.1:8080");

        // Watch for changes
        let mut rx = provider.watch();

        // Update config
        let new_config = r#"
            listen_addr = "127.0.0.1:9090"
            [routes."/"]
            type = "static"
            root = "./static"
        "#;

        // Sleep briefly to ensure file system timestamp difference if needed
        sleep(Duration::from_millis(100)).await;

        {
            let mut file = File::create(&file_path)?;
            file.write_all(new_config.as_bytes())?;
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
