# Axon API Gateway - AI Coding Agent Instructions

Welcome to the Axon API Gateway project! This document provides comprehensive guidance for AI coding agents working on this codebase. Axon is a high-performance API gateway and reverse proxy built in Rust using hexagonal architecture.

## 🏗️ Architecture Overview

Axon follows **hexagonal architecture** (ports and adapters) with clean separation of concerns:

```
src/
├── core/           # Domain logic (business rules)
├── ports/          # Interfaces (traits defining contracts) 
├── adapters/       # Implementations (concrete adapters for ports)
├── config/         # Configuration handling with validation
└── utils/          # Shared utilities and helpers
```

### Core Principles
- **Dependency Inversion**: Core domain depends only on ports (traits), not concrete implementations
- **Testability**: Core logic can be tested without external dependencies
- **Flexibility**: Adapters can be swapped without changing core logic
- **Clear Boundaries**: Each layer has well-defined responsibilities

## 📦 Key Dependencies & Their Usage

### Error Handling: `eyre` (NOT `anyhow`)
- **Always use `eyre::Result<T>` for fallible functions**
- **Use `.wrap_err()` and `.wrap_err_with()` for context**
- **Never use `unwrap()` or `expect()` in production code**

```rust
use eyre::{Result, WrapErr};

fn load_config() -> Result<Config> {
    std::fs::read_to_string("config.yaml")
        .wrap_err("Failed to read config file")?;
    // ... more processing
}
```

### Concurrent Collections: `scc::HashMap` (NOT `dashmap`)
- **Use `scc::HashMap` for thread-safe key-value storage**
- **Iteration requires careful handling of async contexts**
- **Always handle potential lock contention gracefully**

```rust
use scc::HashMap as ConcurrentMap;

// ✅ Correct usage
let map: ConcurrentMap<String, Backend> = ConcurrentMap::new();

// Insertion
map.insert("key".to_string(), backend);

// Async-safe iteration
map.scan_async(|key, value| {
    // Process entries
    key.clone()
}).await;
```

### Configuration: `config` crate
- **Support YAML, JSON, and TOML formats**
- **Use builder pattern for complex configurations**
- **Always validate configurations before use**

```rust
use config::{Config, ConfigError, Environment, File};

let settings = Config::builder()
    .add_source(File::with_name("config"))
    .add_source(Environment::with_prefix("AXON"))
    .build()?;
```

### HTTP Framework: `axum` + `hyper`
- **Use `axum::body::Body` as the standard body type**
- **Implement `IntoResponse` for custom response types**
- **Handle errors with proper status codes**

## 🔧 Code Patterns & Best Practices

### 1. Port Implementation Pattern
When implementing a port (trait), follow this pattern:

```rust
// Port definition (in ports/)
pub trait HttpClient: Send + Sync + 'static {
    async fn send_request(&self, request: Request<Body>) -> Result<Response<Body>, HttpClientError>;
}

// Adapter implementation (in adapters/)
pub struct HttpClientAdapter {
    client: hyper::Client<HttpsConnector<HttpConnector>>,
}

impl HttpClient for HttpClientAdapter {
    async fn send_request(&self, request: Request<Body>) -> Result<Response<Body>, HttpClientError> {
        self.client.request(request)
            .await
            .map_err(HttpClientError::ConnectionError)
    }
}
```

### 2. Configuration Validation Pattern
All configuration should be validated before use:

```rust
impl ServerConfig {
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        
        // Validate listen address
        if let Err(e) = self.listen_addr.parse::<SocketAddr>() {
            errors.push(ValidationError::new("Invalid listen address"));
        }
        
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}
```

### 3. Error Handling Pattern
Consistent error handling across the codebase:

```rust
use eyre::{Result, WrapErr};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Network error: {0}")]
    Network(String),
}

async fn proxy_request(req: Request<Body>) -> Result<Response<Body>> {
    let backend = select_backend(&req)
        .wrap_err("Failed to select backend for request")?;
        
    send_to_backend(backend, req)
        .await
        .wrap_err_with(|| format!("Failed to proxy to backend {}", backend.url))
}
```

### 4. Resource Management Pattern
Always use RAII and proper cleanup:

```rust
pub struct ConnectionTracker {
    active_connections: Arc<AtomicUsize>,
}

impl ConnectionTracker {
    pub fn track_connection(&self) -> ConnectionGuard {
        self.active_connections.fetch_add(1, Ordering::SeqCst);
        ConnectionGuard { tracker: self.active_connections.clone() }
    }
}

pub struct ConnectionGuard {
    tracker: Arc<AtomicUsize>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.tracker.fetch_sub(1, Ordering::SeqCst);
    }
}
```

## 🧪 Testing Guidelines

### Unit Testing Core Logic
Test domain logic without external dependencies:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_load_balancer_round_robin() {
        let backends = vec!["http://api1", "http://api2"];
        let lb = RoundRobinStrategy::new();
        
        assert_eq!(lb.select(&backends), Some("http://api1"));
        assert_eq!(lb.select(&backends), Some("http://api2"));
        assert_eq!(lb.select(&backends), Some("http://api1"));
    }
}
```

### Integration Testing with Adapters
Use real implementations for integration tests:

```rust
#[tokio::test]
async fn test_gateway_proxy() {
    let config = ServerConfig::default();
    let gateway = GatewayService::new(config);
    
    let request = Request::get("http://test.local/api/v1")
        .body(Body::empty()).unwrap();
        
    let response = gateway.handle_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
```

## 🚀 Performance Considerations

### 1. Connection Pooling
Reuse HTTP connections for better performance:

```rust
let https_connector = HttpsConnectorBuilder::new()
    .with_native_roots()
    .https_or_http()
    .enable_http1()
    .enable_http2()
    .build();

let client = hyper::Client::builder()
    .pool_max_idle_per_host(20)
    .pool_idle_timeout(Duration::from_secs(30))
    .build(https_connector);
```

### 2. Async Considerations
- Prefer `Arc<Mutex<_>>` over `Rc<RefCell<_>>` for shared state
- Use `tokio::spawn` for CPU-bound tasks
- Avoid blocking operations in async contexts

### 3. Memory Management
- Use streaming for large request/response bodies
- Implement proper backpressure mechanisms
- Monitor memory usage in long-running processes

## 🔍 Debugging & Observability

### Structured Logging
Use `tracing` for structured logging:

```rust
use tracing::{info, warn, error, instrument};

#[instrument(skip(request))]
async fn handle_request(request: Request<Body>) -> Result<Response<Body>> {
    info!("Processing request to {}", request.uri());
    
    match proxy_request(request).await {
        Ok(response) => {
            info!("Request completed successfully");
            Ok(response)
        }
        Err(e) => {
            error!("Request failed: {:?}", e);
            Err(e)
        }
    }
}
```

### Metrics Collection
Implement metrics for monitoring:

```rust
use metrics::{counter, histogram, gauge};

fn record_request_metrics(path: &str, method: &str, status: u16, duration: Duration) {
    counter!("axon_requests_total", "path" => path, "method" => method, "status" => status.to_string()).increment(1);
    histogram!("axon_request_duration_seconds", "path" => path).record(duration.as_secs_f64());
}
```

## 🛠️ Development Workflow

### 1. Configuration Changes
- Update models in `config/models.rs`
- Add validation in `config/validation.rs`
- Update example configuration files
- Test with `axon validate --config config.yaml`

### 2. Adding New Features
1. Define the port (trait) in `ports/`
2. Implement the adapter in `adapters/`
3. Integrate in core logic
4. Add comprehensive tests
5. Update documentation

### 3. Error Handling Updates
1. Define new error types in relevant modules
2. Use `thiserror` for error definitions
3. Add context with `eyre::WrapErr`
4. Ensure proper error propagation

## ⚡ Common Patterns to Follow

### Configuration Loading
```rust
pub fn load_config(path: &str) -> Result<ServerConfig> {
    let settings = Config::builder()
        .add_source(File::with_name(path))
        .add_source(Environment::with_prefix("AXON"))
        .build()
        .wrap_err("Failed to build configuration")?;
        
    let config: ServerConfig = settings
        .try_deserialize()
        .wrap_err("Failed to deserialize configuration")?;
        
    config.validate()
        .map_err(|errors| eyre::eyre!("Configuration validation failed: {:?}", errors))?;
        
    Ok(config)
}
```

### Graceful Shutdown
```rust
pub struct GracefulShutdown {
    shutdown_tx: broadcast::Sender<()>,
}

impl GracefulShutdown {
    pub async fn shutdown(&self) {
        info!("Initiating graceful shutdown");
        let _ = self.shutdown_tx.send(());
        
        // Wait for active connections to finish
        while self.active_connections.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
        info!("Graceful shutdown completed");
    }
}
```

## 📚 Key Files to Understand

- **`src/core/gateway.rs`**: Main service orchestration logic
- **`src/config/models.rs`**: Complete configuration structure
- **`src/adapters/http_handler.rs`**: HTTP request processing
- **`src/ports/`**: All trait definitions and contracts
- **`src/main.rs`**: Application bootstrapping and CLI

## 🚨 Critical Don'ts

- ❌ Don't use `anyhow` (use `eyre` instead)
- ❌ Don't use `dashmap` (use `scc::HashMap` instead)
- ❌ Don't use `unwrap()` or `expect()` without very good reason
- ❌ Don't break hexagonal architecture boundaries
- ❌ Don't add dependencies to `core/` that aren't ports
- ❌ Don't ignore configuration validation
- ❌ Don't write blocking code in async contexts
- ❌ Don't forget error context with `.wrap_err()`

## ✅ Critical Do's

- ✅ Always use `eyre::Result<T>` for fallible functions
- ✅ Add `.wrap_err()` context to all error chains
- ✅ Validate all configuration before use
- ✅ Follow hexagonal architecture principles
- ✅ Write comprehensive tests for core logic
- ✅ Use structured logging with `tracing`
- ✅ Handle graceful shutdown properly
- ✅ Implement proper metrics collection

---

Remember: Axon prioritizes reliability, performance, and maintainability. Always consider the impact of changes on production systems and follow the established patterns for consistency.
