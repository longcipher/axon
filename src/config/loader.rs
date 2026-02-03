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
    use crate::config::models::{RouteConfig, RouteConfigEntry};

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
    async fn test_load_toml_array_routes() {
        let toml_content = r#"
listen_addr = "127.0.0.1:3000"

[health_check]
enabled = false

[[routes."/"]]
type = "proxy"
host = "api.example.com"
target = "http://api-backend:3001"

[[routes."/"]]
type = "proxy"
host = "admin.example.com"
target = "http://admin-backend:3002"

[[routes."/"]]
type = "proxy"
target = "http://fallback:5000"
"#;

        let mut temp_file = NamedTempFile::with_suffix(".toml").unwrap();
        write!(temp_file, "{}", toml_content).unwrap();

        let config = load_config(temp_file.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1:3000");
        assert_eq!(config.routes.len(), 1); // One path "/"

        // Check that we have 3 routes for "/"
        let entry = config.routes.get("/").expect("Should have / route");
        match entry {
            RouteConfigEntry::Multiple(routes) => {
                assert_eq!(routes.len(), 3);
                // Check first route has api.example.com host
                if let RouteConfig::Proxy { host, target, .. } = &routes[0] {
                    assert_eq!(host.as_deref(), Some("api.example.com"));
                    assert_eq!(target, "http://api-backend:3001");
                } else {
                    panic!("Expected Proxy route");
                }
                // Check second route has admin.example.com host
                if let RouteConfig::Proxy { host, target, .. } = &routes[1] {
                    assert_eq!(host.as_deref(), Some("admin.example.com"));
                    assert_eq!(target, "http://admin-backend:3002");
                } else {
                    panic!("Expected Proxy route");
                }
                // Check third route has no host (fallback)
                if let RouteConfig::Proxy { host, target, .. } = &routes[2] {
                    assert!(host.is_none());
                    assert_eq!(target, "http://fallback:5000");
                } else {
                    panic!("Expected Proxy route");
                }
            }
            RouteConfigEntry::Single(_) => panic!("Expected Multiple routes"),
        }
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
