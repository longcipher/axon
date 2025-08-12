pub mod file_system;
pub mod health_checker;
#[cfg(feature = "http3")]
pub mod http3;
pub mod http_client;
pub mod http_handler;
pub mod middleware; // HTTP/3 (QUIC) support

/// Re-export commonly used types from adapters
pub use file_system::FileSystemAdapter;
pub use health_checker::HealthChecker;
pub use http_client::HttpClientAdapter;
pub use http_handler::HttpHandler;
pub use middleware::*;
