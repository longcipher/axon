use std::path::Path;

use config::{Config, File};
use eyre::{Context, Result};

use crate::config::models::ServerConfig;

/// Load configuration from a file using the config crate.
/// Supports multiple formats automatically based on file extension handled by `config` crate.
pub async fn load_config(config_path: &str) -> Result<ServerConfig> {
    let path = Path::new(config_path);
    let path_str = path
        .to_str()
        .ok_or_else(|| eyre::eyre!("Invalid UTF-8 path: {}", path.display()))?;

    let settings = Config::builder()
        .add_source(File::with_name(path_str))
        .build()
        .with_context(|| format!("Failed to build config from {}", path.display()))?;

    let server_config: ServerConfig = settings
        .try_deserialize()
        .with_context(|| format!("Failed to deserialize config from {}", path.display()))?;

    Ok(server_config)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[tokio::test]
    async fn test_load_toml_config() {
        let toml_content = r#"
listen_addr = "127.0.0.1:3000"

[protocols]
http2_enabled = true
websocket_enabled = true

[health_check]
enabled = true
interval_secs = 30

[routes."/api"]
type = "proxy"
target = "http://backend:8080"
"#;

        let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
        write!(temp_file, "{}", toml_content).unwrap();

        let config = load_config(temp_file.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1:3000");
        assert_eq!(config.routes.len(), 1);
    }

    #[tokio::test]
    async fn test_load_json_config() {
        let json_content = r#"
{
  "listen_addr": "127.0.0.1:3000",
  "protocols": {
    "http2_enabled": true,
    "websocket_enabled": true
  },
  "health_check": {
    "enabled": true,
    "interval_secs": 30
  },
  "routes": {
    "/api": {
      "type": "proxy",
      "target": "http://backend:8080"
    }
  }
}
"#;

        let mut temp_file = NamedTempFile::with_suffix(".json").unwrap();
        write!(temp_file, "{}", json_content).unwrap();

        let config = load_config(temp_file.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1:3000");
        assert_eq!(config.routes.len(), 1);
    }
}
