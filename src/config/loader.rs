use std::path::Path;

use config::{Config, File, FileFormat};
use eyre::{Context, Result};

use crate::config::models::ServerConfig;

/// Load configuration from a file using the config crate
/// Supports multiple formats: YAML, JSON, TOML, etc.
pub async fn load_config(config_path: &str) -> Result<ServerConfig> {
    load_config_sync(config_path)
}

/// Load configuration synchronously
pub fn load_config_sync(config_path: &str) -> Result<ServerConfig> {
    let config_path = Path::new(config_path);

    // Determine file format based on extension
    let format = match config_path.extension().and_then(|ext| ext.to_str()) {
        Some("yaml") | Some("yml") => FileFormat::Yaml,
        Some("json") => FileFormat::Json,
        Some("toml") => FileFormat::Toml,
        Some("ini") => FileFormat::Ini,
        _ => FileFormat::Yaml, // Default to YAML
    };

    let settings = Config::builder()
        .add_source(File::new(
            config_path
                .to_str()
                .ok_or_else(|| eyre::eyre!("Invalid UTF-8 path: {}", config_path.display()))?,
            format,
        ))
        .build()
        .with_context(|| format!("Failed to build config from {}", config_path.display()))?;

    let server_config: ServerConfig = settings.try_deserialize().with_context(|| {
        format!(
            "Failed to deserialize config from {}",
            config_path.display()
        )
    })?;

    Ok(server_config)
}

/// Load configuration without validation (used for validation command)
pub async fn load_config_unchecked(config_path: &str) -> Result<ServerConfig> {
    load_config_sync(config_path)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[tokio::test]
    async fn test_load_yaml_config() {
        let yaml_content = r#"
listen_addr: "127.0.0.1:3000"
protocols:
  http2_enabled: true
  websocket_enabled: true
health_check:
  enabled: true
  interval_secs: 30
routes:
  "/api":
    type: "proxy"
    target: "http://backend:8080"
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "{}", yaml_content).unwrap();

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
