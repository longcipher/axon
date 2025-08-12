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
- WebSocket echo: `examples/configs/ws_echo.toml` (tests: `examples/scripts/ws_echo.sh`, `ws_binary.sh`, `ws_ping_pong.sh`, `ws_close.sh`, `ws_large_payload.sh`)
- HTTP/3 (QUIC) proxy (feature-flagged): `examples/configs/http3_proxy.toml` (smoke script: `examples/scripts/http3_proxy.sh`) – requires building with `--features http3`

Validate a config:

  cargo run -- validate --config examples/configs/static_files.toml

Run a scenario (foreground):

  cargo run -- serve --config examples/configs/static_files.toml

Or run a smoke test script (needs Python 3 for tiny backends where applicable):

  examples/scripts/static_files.sh

## Overview

A high-performance API gateway and reverse proxy built in Rust, implementing hexagonal architecture for maintainability, testability, and flexibility.

## Features

- Protocols: HTTP/1.1 and HTTP/2 (via Hyper/Rustls); WebSocket proxying; optional experimental HTTP/3 (QUIC) via `--features http3`
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
websocket_enabled = true # enable websocket proxy
http3_enabled = false    # set true + provide TLS cert/key when built with --features http3

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
| axon_websocket_connections_total | counter | - | Total WebSocket connections established |
| axon_websocket_messages_total | counter | direction, opcode | WebSocket messages proxied |
| axon_websocket_bytes_total | counter | direction | WebSocket payload bytes proxied |
| axon_websocket_close_codes_total | counter | code | WebSocket close frames observed |

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

## HTTP/3 (QUIC) (Experimental)

Axon ships an experimental HTTP/3 listener behind the `http3` cargo feature. It reuses the existing `HttpHandler` so routing, proxying, and middleware behavior match HTTP/1/2. Current state:

### Capabilities

- Accepts HTTP/3 (QUIC) connections using Rustls + Quinn
- Proxies GET/POST etc. to configured backends (same route map)
- Streams response bodies without full buffering (chunked send over QUIC)

### Limitations (work in progress)

- Request bodies still fully buffered before proxy dispatch
- No per‑request timeout / cancellation wiring yet
- Metrics do not yet include protocol=HTTP/3 label differentiation
- Graceful shutdown integration not implemented for QUIC endpoint
- Limited error classification / backoff strategies

Enabling:

```bash
cargo run --features http3 -- serve --config examples/configs/http3_proxy.toml
```
Example smoke test (generates self‑signed cert, requires curl with HTTP/3):


```bash
examples/scripts/http3_proxy.sh
```

Integration test (requires feature):

```bash
cargo test --features http3 --test http3_basic
```

Provide a TLS cert/key path in the config. Self‑signed is acceptable for local testing.

## Roadmap

- Stabilize WebSocket advanced features (compression, fragmentation, richer metrics)
- Enhance HTTP/3: request streaming, richer metrics, graceful shutdown, backpressure
- Pluggable authentication / authz middlewares
- Advanced load balancing (least‑requests, EWMA)
- Distributed rate limiting backend

## Current WebSocket Limitations

The WebSocket proxy is functional (upgrade, subprotocol negotiation, bidirectional text/binary frame forwarding, close propagation) but not yet feature‑complete. Remaining gaps before declaring full production stability:

Protocol features:

- No permessage-deflate (compression) negotiation yet
- Fragmented (continuation) frames not explicitly tested (forwarded transparently by tungstenite but unverified)
- Ping/Pong pass through but no active keepalive or timeout logic
- Large payload paths (>125 bytes using extended lengths 126/127) not covered by tests

Observability & control:

- Missing dedicated WebSocket metrics (connections, messages, bytes, close codes)
- No configurable idle timeout / max connection count / per-session duration limit

Backpressure & resource safety:

- No explicit slow consumer handling or queue size limits

Security / policy:

- Origin validation & auth/ACL hooks not yet integrated into upgrade path

Testing:

- Additional scripted tests (binary frames, ping/pong timing, large frames, close code mapping) are being added incrementally

Until these are addressed, external integrations should treat the WebSocket layer as stable for basic echo / streaming scenarios but subject to change for advanced features.

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
