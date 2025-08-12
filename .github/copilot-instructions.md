## Axon AI Coding Agent Quick Guide

Purpose: High‑performance API gateway & reverse proxy (Rust) using strict hexagonal architecture (core ↔ ports ↔ adapters). Keep boundaries clean and add value without leaking infrastructure concerns into `core/`.

### 1. Layout & Responsibilities
```
core/   domain logic (routing, load balancing, rate limiting)
ports/  traits (HttpClient, FileSystem, etc.) consumed by core
adapters/ concrete impls (HTTP handler, client, fs, health checker, http3, ws)
config/ models + loading + validation
utils/  infra helpers (graceful shutdown, trackers)
metrics.rs central metric names + helper fns
```
Rules: core depends ONLY on ports; adapters depend on core+ports; no adapter → adapter tight coupling unless via ports.

### 2. Key Conventions
- Errors: always `eyre::Result<T>` + `.wrap_err()` for context. Never `unwrap()` in non-test code.
- Concurrency: prefer `scc::HashMap` for shared mutable maps; avoid `dashmap` / manual locks unless justified.
- Bodies: use `axum::body::Body` (alias `AxumBody`). HTTP/3 adapter streams request & response bodies; don’t re-buffer downstream.
- Metrics: update via helpers in `metrics.rs` (request + backend counters/histograms). All request metrics now include a `protocol` label (`http`, `http3`). Keep label cardinality low (path, method, status, backend, protocol).
- Configuration: add fields in `config/models.rs`, validate in `config/validation.rs`, and extend examples under `examples/configs/`.

### 3. Development Workflow
- Format: `just format` (taplo + rustfmt nightly)
- Lint: `just lint` (clippy denies `unwrap_used` & warns become errors)
- Test: `just test` (unit + integration). Feature HTTP/3: `cargo test --features http3 --test http3_basic`.
- Run: `cargo run -- serve --config config.toml`
- Validate config: `cargo run -- validate --config <file>`
- Smoke scenarios: `just example-run name=static_files` (scripts in `examples/scripts/`).

### 4. Adding a Feature
1. Define trait in `ports/` if new external interaction.
2. Implement adapter in `adapters/`; keep constructor minimal & inject dependencies (Arc where shared).
3. Integrate in `core/` (call only the trait, not concrete type).
4. Extend config (models + validation + example file).
5. Add metrics (reuse existing families or add constant + helper in `metrics.rs`).
6. Write unit tests for core logic; add integration test if IO involved.

### 5. HTTP/3 Status (feature `http3`)
- Listener: `adapters/http3.rs` via Quinn + h3, uses existing `HttpHandler`.
- Request bodies: streamed into Axum handler via mpsc + `from_stream` (do NOT revert to full buffering).
- Graceful shutdown: select on broadcast token; ensure any new loops also observe shutdown.
- When expanding: keep protocol-specific logic inside adapter, surface only normalized request to handler.

### 6. Metrics Patterns
- Use helper: `increment_request_total(path, method, status, protocol)` & `record_request_duration(path, method, protocol, duration)`.
- Backend metrics: add protocol label ONLY if it adds diagnostic value; avoid high-cardinality labels (no raw query strings, client IPs).
- If new histogram: choose sane buckets (exported centrally) instead of ad‑hoc inline definitions.

### 7. Error & Resource Patterns
- Provide context at each boundary crossing (IO, parse, config) with `.wrap_err("action object")`.
- Use RAII guards (`connection_tracker`) for counts; never manually decrement counters on multiple return paths.

### 8. Rate Limiting & LB
- Rate limiter logic in `core/rate_limiter.rs`; algorithms keyed by route config. Add new algorithm behind an enum variant + validation.
- Load balancer strategies live in `core/load_balancer.rs` – keep selection pure & side‑effect free.

### 9. Hot Config Reload
- File watcher triggers reload; validation must succeed before swap. If you add new config fields, ensure default/backward compatibility so reloads don’t fail unexpectedly.

### 10. DO / DON’T (Project Specific)
DO: keep adapter logic thin; add metrics via helpers; stream large bodies; prefer structured logs with spans.  
DON’T: add external deps to `core/`; introduce blocking IO in async paths; create new metrics with unbounded label values; bypass validation; leak adapter types across ports.

### 11. Quick File Landmarks
`src/main.rs` (boot + CLI), `core/gateway.rs` (request orchestration), `adapters/http_handler.rs` (HTTP/1/2 path), `adapters/http3.rs` (QUIC listener), `metrics.rs` (all metric façade), `config/loader.rs` (multi-format load + env merge), `utils/graceful_shutdown.rs` (broadcast token lifecycle).

### 12. Checklist Before PR
- cargo fmt / clippy clean (`just lint`)
- No `unwrap()` added
- Config & examples updated (if feature)
- Tests cover new logic (happy + 1 edge case)
- Metrics labels reviewed for cardinality
- Graceful shutdown path unaffected (or extended) 

If uncertain about an addition, prefer a trait in `ports` + adapter; ask (or leave a comment) when crossing a boundary.

Happy hacking — keep it lean, observable, and safe.
