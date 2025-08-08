# Axon API Gateway

## Quick start

1. Run the gateway
  cargo run -- serve --config config.yaml

## Examples and smoke tests

Ready-to-run scenarios live in `examples/`:

- Static files: `examples/configs/static_files.yaml` (test: `examples/scripts/static_files.sh`)
- Single proxy: `examples/configs/proxy_single.yaml` (test: `examples/scripts/proxy_single.sh`)
- Load balancing (round-robin): `examples/configs/load_balance_rr.yaml` (test: `examples/scripts/load_balance_rr.sh`)
- Rate limit by IP: `examples/configs/rate_limit_ip.yaml` (test: `examples/scripts/rate_limit_ip.sh`)
- Health checks + LB: `examples/configs/health_checks.yaml` (test: `examples/scripts/health_checks.sh`)
- Path rewrite (proxy): `examples/configs/path_rewrite.yaml` (test: `examples/scripts/path_rewrite.sh`)

Validate a config:

  cargo run -- validate --config examples/configs/static_files.yaml

Run a scenario (foreground):

  cargo run -- serve --config examples/configs/static_files.yaml

Or run a smoke test script (needs Python 3 for tiny backends where applicable):

  examples/scripts/static_files.sh

## Overview

A high-performance API gateway and reverse proxy built in Rust, implementing hexagonal architecture for maintainability, testability, and flexibility.

## Features

- Protocols: HTTP/1.1 and HTTP/2 (via Hyper/Rustls); WebSocket proxying is planned
- Static file serving with configurable directories
- HTTP redirects with custom status codes
- Load balancing (round-robin and random strategies)
- Path rewriting for proxy and load-balanced routes
- Health checking for backend services with configurable intervals
- Rate limiting (by IP, header, or route-wide)
- Configuration validation with detailed error reporting and CLI validation command
- Live configuration reloading (file watcher)
- Basic Prometheus-compatible metrics at `/metrics`
- Graceful shutdown with connection tracking

## Architecture

Axon follows a hexagonal architecture pattern, which separates the application into three main areas:

1. **Core Domain** - Contains the business logic of the application
2. **Ports** - Interfaces that define how the core interacts with the outside world
3. **Adapters** - Implementations of the ports that connect to external systems

## Configuration

Configuration is handled through the `config` crate which supports multiple formats (YAML, JSON, TOML, etc.).

### Example configuration.yaml

```yaml
listen_addr: "127.0.0.1:3000"

protocols:
  http2_enabled: true
  websocket_enabled: true # planned
  http3_enabled: false    # not implemented yet

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
