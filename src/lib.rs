/// Axon - A high-performance API gateway and reverse proxy
///
/// This crate provides an API gateway and reverse proxy server with features like:
/// - Static file serving
/// - HTTP/HTTPS support  
/// - Load balancing
/// - Health checking
/// - Path-based routing
/// - Rate limiting
/// - Request/Response manipulation
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
