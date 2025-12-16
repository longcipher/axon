use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body as AxumBody;
use eyre::Result;
use http_body_util::BodyExt;
use hyper::{Request, Response, Version, header, header::HeaderValue};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use rustls_native_certs::load_native_certs;
use tokio::time::timeout;

use crate::ports::http_client::{HttpClient, HttpClientError, HttpClientResult};

/// HTTP client adapter using Hyper with Rustls (HTTP/1.1 + HTTP/2).
///
/// Responsibilities:
/// * Adds a small set of sensible default headers
/// * Forces request version to HTTP/1.1 while allowing ALPN to negotiate h2
/// * Performs HEAD based health checks with timeout
/// * Converts between Hyper body and Axum body types
///
/// This adapter is intentionally minimal; higher level retries / circuit breaking
/// can be layered on a different abstraction if required.
pub struct HttpClientAdapter {
    client: Client<HttpsConnector<HttpConnector>, AxumBody>,
}

impl HttpClientAdapter {
    /// Create a new HTTP client adapter.
    pub fn new() -> Result<Self> {
        // Install default crypto provider for rustls if not already set
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let mut http_connector = HttpConnector::new();
        http_connector.enforce_http(false); // Allow HTTPS URLs

        // Build rustls client config with modern protocols
        let mut root_cert_store = rustls::RootCertStore::empty();
        let native_certs = load_native_certs();

        if !native_certs.certs.is_empty() {
            for cert in native_certs.certs {
                if root_cert_store.add(cert).is_err() {
                    tracing::warn!("Failed to add native certificate to rustls RootCertStore");
                }
            }
            tracing::info!("Loaded {} native root certificates.", root_cert_store.len());
        }

        if !native_certs.errors.is_empty() {
            tracing::warn!(
                "Some native certificates failed to load: {:?}",
                native_certs.errors
            );
        }

        // Configure TLS with ALPN for HTTP/2 negotiation
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();

        // Build HTTPS connector with HTTP/2 support
        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls_config)
            .https_or_http()
            .enable_http1() // Support HTTP/1.1
            .wrap_connector(http_connector);

        // Create client with TokioExecutor for async runtime
        let client = Client::builder(TokioExecutor::new()).build::<_, AxumBody>(https_connector);

        tracing::info!("Created new HTTP client with HTTP/2 and HTTP/1.1 support");
        Ok(Self { client })
    }

    /// Add common HTTP headers to requests
    /// Inject a consistent set of headers if absent (User-Agent, Accept ...).
    fn add_common_headers(req: &mut Request<AxumBody>) {
        let headers = req.headers_mut();
        if !headers.contains_key(header::USER_AGENT) {
            headers.insert(
                header::USER_AGENT,
                header::HeaderValue::from_static("Axon-Gateway/1.0"),
            );
        }
        if !headers.contains_key(header::ACCEPT) {
            headers.insert(
                header::ACCEPT,
                header::HeaderValue::from_static(
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
                ),
            );
        }
        if !headers.contains_key(header::ACCEPT_LANGUAGE) {
            headers.insert(
                header::ACCEPT_LANGUAGE,
                header::HeaderValue::from_static("en-US,en;q=0.5"),
            );
        }
        if !headers.contains_key(header::CACHE_CONTROL) {
            headers.insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("max-age=0"),
            );
        }
    }
}

impl Default for HttpClientAdapter {
    fn default() -> Self {
        Self::new().expect("Failed to create HTTP client")
    }
}

#[async_trait]
impl HttpClient for HttpClientAdapter {
    async fn send_request(
        &self,
        mut req: Request<AxumBody>,
    ) -> HttpClientResult<Response<AxumBody>> {
        Self::add_common_headers(&mut req);

        let client = self.client.clone();

        // Extract backend information for logging and metrics
        let backend_identifier = format!(
            "{}://{}",
            req.uri().scheme_str().unwrap_or("http"),
            req.uri()
                .authority()
                .map_or_else(|| "unknown".to_string(), |a| a.to_string())
        );
        let request_path = req.uri().path().to_string();
        let request_method = req.method().to_string();

        // Create a tracing span for the backend request
        let span = tracing::info_span!(
            "backend_request",
            backend.url = %backend_identifier,
            http.method = %request_method,
            http.path = %request_path,
            http.status_code = tracing::field::Empty,
        );
        let _enter = span.enter();

        // Set Host header if not present
        if let Some(host_str) = req.uri().host() {
            let host_header_val = if let Some(port) = req.uri().port() {
                HeaderValue::from_str(&format!("{host_str}:{}", port.as_u16()))
                    .unwrap_or_else(|_| HeaderValue::from_static(""))
            } else {
                HeaderValue::from_str(host_str).unwrap_or_else(|_| HeaderValue::from_static(""))
            };
            if !host_header_val.is_empty() {
                req.headers_mut()
                    .insert(hyper::header::HOST, host_header_val);
            }
        } else {
            tracing::error!("Outgoing URI has no host: {}", req.uri());
            return Err(HttpClientError::InvalidRequest(
                "Outgoing URI has no host".to_string(),
            ));
        }

        let (mut parts, axum_body) = req.into_parts();
        parts.version = Version::HTTP_11;

        tracing::info!(
            "Sending request: {} {} (Version set to HTTP/1.1, ALPN negotiates actual version)",
            parts.method,
            parts.uri
        );
        tracing::debug!("Outgoing request headers: {:?}", parts.headers);

        // Use the streaming body directly
        let outgoing_request = Request::from_parts(parts, axum_body);

        let method_for_error_log = outgoing_request.method().clone();
        let uri_for_error_log = outgoing_request.uri().clone();

        // Send request
        match client.request(outgoing_request).await {
            Ok(response) => {
                let status_code = response.status().as_u16();

                // Record status code in the tracing span
                tracing::Span::current().record("http.status_code", status_code);

                // Convert Hyper response body back to AxumBody
                let (mut parts, hyper_body) = response.into_parts();

                // Remove Transfer-Encoding header since the body is being decoded/streamed
                // and the downstream server (Axum) will handle framing.
                parts.headers.remove(header::TRANSFER_ENCODING);

                let axum_body = AxumBody::new(hyper_body);

                Ok(Response::from_parts(parts, axum_body))
            }
            Err(e) => {
                // Record error status code in the tracing span
                tracing::Span::current().record("http.status_code", 599u16);

                tracing::error!(
                    "Error making request to backend {} ({} {}): {}",
                    backend_identifier,
                    method_for_error_log,
                    uri_for_error_log,
                    e
                );

                Err(HttpClientError::ConnectionError(format!(
                    "Request to {method_for_error_log} {uri_for_error_log} failed: {e}"
                )))
            }
        }
    }

    async fn health_check(&self, url: &str, timeout_secs: u64) -> HttpClientResult<bool> {
        let client = self.client.clone();

        let request = Request::builder()
            .method("HEAD")
            .uri(url)
            .version(Version::HTTP_11)
            .body(AxumBody::empty())
            .map_err(|e| HttpClientError::InvalidRequest(e.to_string()))?;

        tracing::debug!("Health checking URL: {} (Version set to HTTP/1.1)", url);
        let timeout_duration = Duration::from_secs(timeout_secs);

        match timeout(timeout_duration, client.request(request)).await {
            Ok(result) => match result {
                Ok(response) => {
                    let is_healthy = response.status().is_success();
                    // Consume the body to prevent resource leaks
                    let _ = response.into_body().collect().await;
                    tracing::debug!("Health check for {} result: {}", url, is_healthy);
                    Ok(is_healthy)
                }
                Err(err) => {
                    tracing::debug!("Health check error for {}: {}", url, err);
                    // Return Ok(false) for connection errors during health check
                    Ok(false)
                }
            },
            Err(_) => {
                tracing::debug!("Health check timeout for {}", url);
                Err(HttpClientError::Timeout(timeout_secs))
            }
        }
    }
}

impl HttpClientAdapter {
    /// Convenience method for GET requests
    pub async fn get(&self, url: &str) -> HttpClientResult<Response<AxumBody>> {
        let request = Request::builder()
            .method("GET")
            .uri(url)
            .body(AxumBody::empty())
            .map_err(|e| HttpClientError::InvalidRequest(e.to_string()))?;

        self.send_request(request).await
    }

    /// Convenience method for POST requests
    pub async fn post(&self, url: &str, body: AxumBody) -> HttpClientResult<Response<AxumBody>> {
        let request = Request::builder()
            .method("POST")
            .uri(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .map_err(|e| HttpClientError::InvalidRequest(e.to_string()))?;

        self.send_request(request).await
    }

    /// Convenience method for PUT requests
    pub async fn put(&self, url: &str, body: AxumBody) -> HttpClientResult<Response<AxumBody>> {
        let request = Request::builder()
            .method("PUT")
            .uri(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .map_err(|e| HttpClientError::InvalidRequest(e.to_string()))?;

        self.send_request(request).await
    }

    /// Convenience method for DELETE requests
    pub async fn delete(&self, url: &str) -> HttpClientResult<Response<AxumBody>> {
        let request = Request::builder()
            .method("DELETE")
            .uri(url)
            .body(AxumBody::empty())
            .map_err(|e| HttpClientError::InvalidRequest(e.to_string()))?;

        self.send_request(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_http_client_creation() {
        let client = HttpClientAdapter::new();
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_add_common_headers() {
        let mut req = Request::builder()
            .uri("https://example.com")
            .body(AxumBody::empty())
            .unwrap();

        HttpClientAdapter::add_common_headers(&mut req);

        let headers = req.headers();
        assert!(headers.contains_key(header::USER_AGENT));
        assert!(headers.contains_key(header::ACCEPT));
        assert!(headers.contains_key(header::ACCEPT_LANGUAGE));
        assert!(headers.contains_key(header::CACHE_CONTROL));

        // Verify User-Agent is set to Axon-Gateway
        assert_eq!(
            headers.get(header::USER_AGENT).unwrap(),
            HeaderValue::from_static("Axon-Gateway/1.0")
        );
    }

    #[tokio::test]
    async fn test_health_check_invalid_url() {
        let client = HttpClientAdapter::new().unwrap();
        let result = client.health_check("invalid-url", 5).await;

        // Should return Ok(false) for connection errors
        match result {
            Ok(false) => {} // Expected
            _ => panic!("Expected Ok(false) for invalid URL"),
        }
    }
}
