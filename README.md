# Axon API Gateway

High-performance, extensible API gateway & reverse proxy in Rust. Hexagonal architecture, structured tracing, metrics, rate limiting & dynamic config reload.

Badges: crates.io | docs.rs | Apache-2.0

## Quick start

1. Run the gateway
  cargo run -- serve --config config.toml

## Examples and smoke tests

Ready-to-run scenarios live in `examples/`:

- Static files: `examples/configs/static_files.toml` (test: `examples/scripts/static_files.sh`)
- Single proxy: `examples/configs/proxy_single.toml` (test: `examples/scripts/proxy_single.sh`)
- Load balancing (round-robin): `examples/configs/load_balance_rr.toml` (test: `examples/scripts/load_balance_rr.sh`)
- Rate limit by IP: `examples/configs/rate_limit_ip.toml` (test: `examples/scripts/rate_limit_ip.sh`)
- Health checks + LB: `examples/configs/health_checks.toml` (test: `examples/scripts/health_checks.sh`)
- Path rewrite (proxy): `examples/configs/path_rewrite.toml` (test: `examples/scripts/path_rewrite.sh`)

Validate a config:

  cargo run -- validate --config examples/configs/static_files.toml

Run a scenario (foreground):

  cargo run -- serve --config examples/configs/static_files.toml

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
- Structured tracing with request & backend spans
- Simple Prometheus-compatible metrics endpoint (`/metrics`)

## Architecture

Axon follows a hexagonal architecture pattern, which separates the application into three main areas:

1. **Core Domain** - Contains the business logic of the application
2. **Ports** - Interfaces that define how the core interacts with the outside world
3. **Adapters** - Implementations of the ports that connect to external systems

## Configuration

Configuration is handled through the `config` crate which supports multiple formats (YAML, JSON, TOML, etc.).

### Example configuration.toml

```toml
listen_addr = "127.0.0.1:3000"

[protocols]
http2_enabled = true
websocket_enabled = true # planned
http3_enabled = false    # not implemented yet

[health_check]
enabled = true
interval_secs = 30
timeout_secs = 5
path = "/health"
unhealthy_threshold = 3
healthy_threshold = 2

[routes."/api/v1"]
type = "proxy"
target = "http://backend-service:8080"
path_rewrite = "/"

  [routes."/api/v1".rate_limit]
  by = "ip"
  requests = 100
  period = "1m"

[routes."/static"]
type = "static"
root = "./public"

[routes."/health"]
type = "proxy"
target = "http://backend-service:8080/health"
```

## Metrics

Axon exposes a minimal text exposition at `/metrics` (Prometheus scrapeable). Key metric families:

| Name | Type | Labels | Description |
|------|------|--------|-------------|
| axon_requests_total | counter | path, method, status | Total HTTP requests processed |
| axon_request_duration_seconds | histogram | path, method | Inbound request latency |
| axon_backend_requests_total | counter | backend, path, method, status | Requests forwarded to backends |
| axon_backend_request_duration_seconds | histogram | backend, path, method | Backend latency |
| axon_backend_health_status | gauge | backend | 1 healthy / 0 unhealthy |
| axon_active_connections | gauge | - | Open connections |
| axon_active_requests | gauge | - | In‑flight requests |

## Rate Limiting

Per-route rate limiting supports algorithms: `token_bucket`, `sliding_window`, `fixed_window`.
Limiters can be keyed by:

- route (global quota per route)
- ip (client IP via connection info)
- header (specific header value)

Example snippet:

```toml
[routes."/api".rate_limit]
by = "ip"
requests = 100
period = "1m"
algorithm = "token_bucket"
status_code = 429
message = "Too Many Requests"
```

## Health Checking

Configurable active health checks use success/failure thresholds to avoid flapping:

```toml
[health_check]
enabled = true
interval_secs = 30
timeout_secs = 5
path = "/health"
unhealthy_threshold = 3
healthy_threshold = 2
```

Per-backend override paths:

```toml
[backend_health_paths]
"http://service-a:8080" = "/ready"
"http://service-b:8080" = "/live"
```

## Tracing

Structured logging via `tracing` with automatic span creation for each request and backend call.
TTY output uses compact human format; non‑TTY emits JSON (ideal for log aggregation). Customize with env:

```bash
RUST_LOG=info ./target/release/axon serve --config config.toml
```

## Roadmap

- WebSocket proxying
- HTTP/3 (QUIC) support
- Pluggable authentication / authz middlewares
- Advanced load balancing (least‑requests, EWMA)
- Distributed rate limiting backend

## Contributing

Issues & PRs welcome. Please run tests and keep rustdocs up to date.

## License

Licensed under Apache-2.0. See [LICENSE](LICENSE).

## Usage

### Building

```bash
cargo build --release
```

### Validate Configuration

```bash
./target/release/axon validate --config config.toml
```

### Run Server

```bash
./target/release/axon serve --config config.toml
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
ExecStart=/opt/axon/axon serve --config /opt/axon/config.toml
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
