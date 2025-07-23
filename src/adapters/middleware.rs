use std::{
    sync::{Arc, RwLock},
    time::Instant,
};

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};

use crate::config::models::ServerConfig;

/// Middleware that adds Alt-Svc header when HTTP/3 is enabled
pub async fn add_alt_svc_header(
    req: Request,
    next: Next,
    config_holder: Arc<RwLock<Arc<ServerConfig>>>,
) -> Response {
    let mut response = next.run(req).await;

    // Check if HTTP/3 is enabled in the configuration
    let should_add_alt_svc = {
        match config_holder.read() {
            Ok(config) => config.protocols.http3_enabled && config.tls.is_some(),
            Err(e) => {
                tracing::warn!(
                    "Failed to acquire config read lock for Alt-Svc header: {}",
                    e
                );
                false
            }
        }
    };

    if should_add_alt_svc {
        // Add Alt-Svc header to advertise HTTP/3 support
        let header_value = HeaderValue::from_static("h3=\":443\"; ma=3600");
        response.headers_mut().insert("alt-svc", header_value);
    }

    response
}

/// Creates a closure for the Alt-Svc middleware
pub fn create_alt_svc_middleware(
    config_holder: Arc<RwLock<Arc<ServerConfig>>>,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
+ Clone {
    move |req, next| {
        let config_holder = config_holder.clone();
        Box::pin(async move { add_alt_svc_header(req, next, config_holder).await })
    }
}

/// Middleware for request timing and logging
pub async fn request_timing_middleware(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let uri = req.uri().clone();
    let version = req.version();

    tracing::info!("Started processing {} {} {:?}", method, uri, version);

    let response = next.run(req).await;
    let duration = start.elapsed();

    tracing::info!(
        "Completed {} {} {:?} - {} in {:?}",
        method,
        uri,
        version,
        response.status(),
        duration
    );

    response
}

/// Middleware for adding security headers
pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Add security headers
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    headers.insert(
        "X-XSS-Protection",
        HeaderValue::from_static("1; mode=block"),
    );
    headers.insert(
        "Referrer-Policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    response
}

/// Middleware for CORS handling
pub async fn cors_middleware(req: Request, next: Next) -> Response {
    let origin = req.headers().get("origin").cloned();
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Add CORS headers
    if let Some(origin) = origin {
        headers.insert("Access-Control-Allow-Origin", origin);
    }
    headers.insert(
        "Access-Control-Allow-Methods",
        HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        HeaderValue::from_static("Content-Type, Authorization, X-Requested-With"),
    );
    headers.insert("Access-Control-Max-Age", HeaderValue::from_static("86400"));

    response
}

/// Middleware for adding custom headers from configuration
pub async fn custom_headers_middleware(
    req: Request,
    next: Next,
    custom_headers: Arc<std::collections::HashMap<String, String>>,
) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Add custom headers from configuration
    for (name, value) in custom_headers.iter() {
        if let (Ok(header_name), Ok(header_value)) = (
            name.parse::<hyper::header::HeaderName>(),
            HeaderValue::from_str(value),
        ) {
            headers.insert(header_name, header_value);
        } else {
            tracing::warn!("Invalid custom header: {} = {}", name, value);
        }
    }

    response
}

/// Creates a closure for custom headers middleware
pub fn create_custom_headers_middleware(
    custom_headers: Arc<std::collections::HashMap<String, String>>,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
+ Clone {
    move |req, next| {
        let custom_headers = custom_headers.clone();
        Box::pin(async move { custom_headers_middleware(req, next, custom_headers).await })
    }
}

/// Middleware for request ID generation and tracking
pub async fn request_id_middleware(req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();

    // Add request ID to tracing span
    let span = tracing::info_span!("request", request_id = %request_id);
    let _enter = span.enter();

    let mut response = next.run(req).await;

    // Add request ID to response headers
    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("X-Request-ID", header_value);
    }

    response
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::{body::Body, http::StatusCode};

    use super::*;

    #[tokio::test]
    async fn test_security_headers_middleware() {
        let req = Request::builder().body(Body::empty()).unwrap();
        let next = Next::new(|_: Request| async {
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .body(Body::empty())
                .unwrap()
        });

        let response = security_headers_middleware(req, next).await;
        let headers = response.headers();

        assert!(headers.contains_key("X-Content-Type-Options"));
        assert!(headers.contains_key("X-Frame-Options"));
        assert!(headers.contains_key("X-XSS-Protection"));
        assert!(headers.contains_key("Referrer-Policy"));
    }

    #[tokio::test]
    async fn test_custom_headers_middleware() {
        let mut custom_headers = HashMap::new();
        custom_headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());
        let custom_headers = Arc::new(custom_headers);

        let req = Request::builder().body(Body::empty()).unwrap();
        let next = Next::new(|_: Request| async {
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .body(Body::empty())
                .unwrap()
        });

        let response = custom_headers_middleware(req, next, custom_headers).await;
        let headers = response.headers();

        assert_eq!(
            headers.get("X-Custom-Header").unwrap(),
            HeaderValue::from_static("custom-value")
        );
    }

    #[tokio::test]
    async fn test_request_id_middleware() {
        let req = Request::builder().body(Body::empty()).unwrap();
        let next = Next::new(|_: Request| async {
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .body(Body::empty())
                .unwrap()
        });

        let response = request_id_middleware(req, next).await;
        let headers = response.headers();

        assert!(headers.contains_key("X-Request-ID"));

        // Verify it's a valid UUID
        let request_id = headers.get("X-Request-ID").unwrap().to_str().unwrap();
        assert!(uuid::Uuid::parse_str(request_id).is_ok());
    }
}
