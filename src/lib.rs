//! Axon - A high-performance API gateway and reverse proxy.
//!
//! Axon is an opinionated, extensible API gateway implementing a **hexagonal architecture**.
//! It focuses on correctness, observability, and ergonomic configuration. This library exposes
//! the core building blocks so you can embed the gateway or compose parts of it inside
//! your own application.
//!
//! # Features
//! - Static file serving
//! - HTTP/1.1 + HTTP/2 (with optional TLS) and planned WebSocket/WebTransport support
//! - Pluggable load balancing strategies (round-robin, random)
//! - Active health checking and per-backend status tracking
//! - Path-based routing supporting proxy, load-balance, redirect, static, websocket
//! - Per‑route rate limiting (by route / IP / header) with multiple algorithms
//! - Request / response header mutation hooks (add / remove) and basic body transforms
//! - Live configuration hot‑reload & validation
//! - Metrics (Prometheus style) & structured tracing via `tracing`
//! - Graceful shutdown & connection tracking
//!
//! # Quick Example
//! ```no_run
//! use std::sync::Arc;
//!
//! use axon::{GatewayService, config::ServerConfig};
//!
//! # #[tokio::main] async fn main() -> eyre::Result<()> {
//! // Load a configuration (see examples/configs/*.toml)
//! let cfg: ServerConfig = axon::config::loader::load_config("config.toml").await?;
//! let gateway = Arc::new(GatewayService::new(Arc::new(cfg)));
//! // You would normally wire this into the provided HttpHandler adapter (see binary crate)
//! # Ok(()) }
//! ```
//!
//! # Architecture
//! The crate separates **ports** (traits) from **adapters** (implementations) while keeping
//! business logic inside `core`. End users should prefer the re‑exports documented below
//! instead of reaching into internal modules directly.
//!
//! # Minimum Supported Rust Version (MSRV)
//! The MSRV is **1.78** (aligned with current stable practices). It may be bumped in a minor
//! release with a note in the changelog.
//!
//! # Error Handling
//! All fallible APIs return `eyre::Result<T>` or a domain specific error type. A custom error
//! context is always attached using `WrapErr` for debuggability.
//!
//! # Concurrency & Data Structures
//! For shared mutable maps the project uses `scc::HashMap` instead of `dashmap` to maintain
//! predictable performance characteristics under contention.
//!
//! # Feature Flags (future)
//! Planned optional Cargo features (not yet stabilized): `tls`, `metrics`, `http3`.
//!
//! # Stability
//! This crate is early stage; APIs may evolve. Semantic versioning will be followed after 1.0.
//!
//! # License
//! Dual-licensed under either MIT or Apache-2.0 at your option.
//!
//! See README for more extensive usage patterns.
// Re-export public modules with explicit visibility controls
pub mod config;
pub mod metrics;
pub mod ports;
pub mod tracing_setup;
pub mod utils;

// These modules are implementation details and should not be directly used by users
pub mod adapters;
pub mod core;

// Re-export the specific types needed by the binary crate
pub use crate::{
    adapters::{FileSystemAdapter, HealthChecker, HttpClientAdapter, HttpHandler},
    core::GatewayService,
    ports::http_client::HttpClient,
    utils::{ConnectionTracker, GracefulShutdown},
};
