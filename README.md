# Axon API Gateway

A high-performance API gateway and reverse proxy built in Rust, implementing hexagonal architecture for maintainability, testability, and flexibility.

## Features

- **Modern Protocol Support** with complete HTTP/1.1, HTTP/2, and HTTP/3 (QUIC) implementation
  - ✅ **HTTP/3 (QUIC)** with automatic Alt-Svc advertisement and protocol negotiation
  - ✅ **WebSocket Support** with first-class proxying and configurable frame/message sizes
  - ✅ **TLS Integration** with seamless certificate sharing between HTTP/2 and HTTP/3
  - ✅ **Unified Server Architecture** supporting both TCP (HTTP/1.1, HTTP/2) and UDP (HTTP/3)
- **Advanced Gateway Features**
  - Static file serving with configurable directories
  - HTTP redirects with custom status codes
  - Load balancing (round-robin and random strategies)
  - Path rewriting for proxy and load-balanced routes
  - Health checking for backend services with configurable intervals
- **Enterprise-Grade Features**
  - Rate limiting (by IP, header, or route-wide) with multiple algorithms
  - **Configuration Validation** with detailed error reporting and CLI validation command
  - Real-time configuration reloading without downtime
  - Request and Response Manipulation (Headers & Body) with conditional logic
  - **Production-grade monitoring** with Prometheus metrics
  - **Automatic TLS Certificate Management** with ACME/Let's Encrypt integration
  - **Graceful Shutdown** with connection tracking and zero-downtime restarts

## Architecture

Axon follows a hexagonal architecture pattern, which separates the application into three main areas:

1. **Core Domain** - Contains the business logic of the application
2. **Ports** - Interfaces that define how the core interacts with the outside world
3. **Adapters** - Implementations of the ports that connect to external systems

## Configuration

Configuration is handled through the `config` crate which supports multiple formats (YAML, JSON, TOML, etc.).

### Example configuration.yaml:

```yaml
listen_addr: "127.0.0.1:3000"

protocols:
  http2_enabled: true
  websocket_enabled: true
  http3_enabled: false

health_check:
  enabled: true
  interval_secs: 30
  timeout_secs: 5
  path: "/health"
  unhealthy_threshold: 3
  healthy_threshold: 2

routes:
  "/api/v1":
    type: "proxy"
    target: "http://backend-service:8080"
    path_rewrite: "/"
    rate_limit:
      by: "ip"
      requests: 100
      period: "1m"

  "/static":
    type: "static"
    root: "./public"
    
  "/health":
    type: "proxy" 
    target: "http://backend-service:8080/health"
```

## Usage

### Building

```bash
cargo build --release
```

### Validate Configuration

```bash
./target/release/axon validate --config config.yaml
```

### Run Server

```bash
./target/release/axon serve --config config.yaml
```

## Systemd Deployment

Create a systemd service file at `/etc/systemd/system/axon.service`:

```ini
[Unit]
Description=Axon API Gateway
After=network.target

[Service]
Type=simple
User=axon
Group=axon
WorkingDirectory=/opt/axon
ExecStart=/opt/axon/axon serve --config /opt/axon/config.yaml
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

Then enable and start the service:

```bash
sudo systemctl enable axon
sudo systemctl start axon
sudo systemctl status axon
```
