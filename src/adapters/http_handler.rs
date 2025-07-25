use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Instant,
};

use axum::{
    body::Body as AxumBody,
    http::{HeaderMap, StatusCode, header},
};
use eyre::{Result, WrapErr};
use hyper::{Request, Response};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    adapters::FileSystemAdapter,
    config::models::{RouteConfig, ServerConfig},
    core::GatewayService,
    ports::{file_system::FileSystem, http_client::HttpClient},
    tracing_setup,
    utils::ConnectionTracker,
};

/// HTTP handler for the Axon API Gateway
pub struct HttpHandler {
    gateway_service: Arc<GatewayService>,
    http_client: Arc<dyn HttpClient>,
    file_system: Arc<FileSystemAdapter>,
    connection_tracker: Arc<ConnectionTracker>,
    config: Arc<RwLock<Arc<ServerConfig>>>,
}

impl HttpHandler {
    pub fn new(
        gateway_service: Arc<GatewayService>,
        http_client: Arc<dyn HttpClient>,
        file_system: Arc<FileSystemAdapter>,
        connection_tracker: Arc<ConnectionTracker>,
        config: Arc<RwLock<Arc<ServerConfig>>>,
    ) -> Self {
        Self {
            gateway_service,
            http_client,
            file_system,
            connection_tracker,
            config,
        }
    }

    /// Main request handler that routes requests appropriately
    pub async fn handle_request(
        &self,
        req: Request<AxumBody>,
        client_addr: Option<SocketAddr>,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        let start_time = Instant::now();
        let method = req.method().clone();
        let uri = req.uri().clone();
        let path = uri.path();
        let request_id = Uuid::new_v4().to_string();

        // Extract client info for logging
        let client_ip = client_addr.map(|addr| addr.ip().to_string());
        let user_agent = req
            .headers()
            .get(header::USER_AGENT)
            .and_then(|h| h.to_str().ok())
            .map(String::from);

        // Create request span with comprehensive info
        let span = tracing_setup::create_request_span(
            method.as_str(),
            path,
            &request_id,
            client_ip.as_deref(),
            user_agent.as_deref(),
        );

        let result: Result<Response<AxumBody>, eyre::Error> =
            async { self.route_request(req, client_addr).await }
                .instrument(span)
                .await;

        // Log request completion with timing and outcome
        let duration = start_time.elapsed();
        match &result {
            Ok(response) => {
                tracing::Span::current().record("http.status_code", response.status().as_u16());
                tracing::Span::current().record("duration_ms", duration.as_millis() as u64);

                tracing::info!(
                    status = response.status().as_u16(),
                    duration_ms = duration.as_millis(),
                    "request completed"
                );
            }
            Err(e) => {
                tracing::Span::current().record("http.status_code", 500u16);
                tracing::Span::current().record("duration_ms", duration.as_millis() as u64);

                tracing::error!(
                    error = %e,
                    duration_ms = duration.as_millis(),
                    "request failed"
                );
            }
        }

        result
    }

    /// Route request to appropriate handler
    async fn route_request(
        &self,
        req: Request<AxumBody>,
        client_addr: Option<SocketAddr>,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        let path = req.uri().path();

        // Handle special paths first
        match path {
            "/health" => return self.handle_health_check().await,
            "/metrics" => return self.handle_metrics().await,
            "/status" => return self.handle_status().await,
            _ => {}
        }

        // Check if there's a matching route in configuration
        if let Some((prefix, route_config)) = self.gateway_service.find_matching_route(path) {
            tracing::Span::current().record("route.prefix", &prefix);

            match route_config {
                RouteConfig::Static { .. } => {
                    return self.handle_static_file(req, &prefix).await;
                }
                RouteConfig::Proxy { target, .. } => {
                    tracing::Span::current().record("backend.url", target);
                    return self.handle_proxy_request(req, client_addr).await;
                }
                RouteConfig::LoadBalance { targets, .. } => {
                    let target_list = targets.join(",");
                    tracing::Span::current().record("backend.targets", &target_list);
                    return self.handle_proxy_request(req, client_addr).await;
                }
                RouteConfig::Websocket { .. } => {
                    return self.handle_proxy_request(req, client_addr).await;
                }
                RouteConfig::Redirect {
                    target,
                    status_code,
                    ..
                } => {
                    return self.handle_redirect(&target, &status_code).await;
                }
            }
        } else {
            tracing::warn!("no route match");
        }

        // If no route matches, return 404
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(AxumBody::from("Route not found"))
            .wrap_err("Failed to build 404 response")
    }

    /// Handle health check endpoint
    async fn handle_health_check(&self) -> Result<Response<AxumBody>, eyre::Error> {
        let (healthy_backends, total_backends) = {
            let backend_count = self.gateway_service.backend_count();
            let healthy_count = self.gateway_service.healthy_backend_count();
            (healthy_count, backend_count)
        };

        let status = if healthy_backends > 0 {
            StatusCode::OK
        } else if total_backends > 0 {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::NOT_FOUND
        };

        let health_data = serde_json::json!({
            "status": if status == StatusCode::OK { "healthy" } else { "unhealthy" },
            "backends": {
                "healthy": healthy_backends,
                "total": total_backends
            },
            "timestamp": chrono::Utc::now().to_rfc3339()
        });

        let response = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(health_data.to_string()))
            .wrap_err("Failed to build health check response")?;

        Ok(response)
    }

    /// Handle metrics endpoint
    async fn handle_metrics(&self) -> Result<Response<AxumBody>, eyre::Error> {
        // TODO: Implement proper metrics collection
        let metrics_data = serde_json::json!({
            "connections": {
                "active": self.connection_tracker.active_connection_count(),
                "total_requests": self.connection_tracker.total_active_requests()
            },
            "backends": {
                "total": self.gateway_service.backend_count(),
                "healthy": self.gateway_service.healthy_backend_count()
            },
            "timestamp": chrono::Utc::now().to_rfc3339()
        });

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(metrics_data.to_string()))
            .wrap_err("Failed to build metrics response")?;

        Ok(response)
    }

    /// Handle status endpoint
    async fn handle_status(&self) -> Result<Response<AxumBody>, eyre::Error> {
        let stats = self.connection_tracker.get_stats();
        let config = self
            .config
            .read()
            .map_err(|_| eyre::eyre!("Failed to acquire config lock"))?
            .clone();

        let status_data = serde_json::json!({
            "service": "Axon API Gateway",
            "version": env!("CARGO_PKG_VERSION"),
            "uptime": "N/A", // TODO: Track actual uptime
            "connections": {
                "total": stats.total_connections,
                "active": stats.active_connections,
                "idle": stats.idle_connections,
                "active_requests": stats.total_active_requests,
                "oldest_connection_age_secs": stats.oldest_connection_age.as_secs()
            },
            "backends": {
                "total": self.gateway_service.backend_count(),
                "healthy": self.gateway_service.healthy_backend_count()
            },
            "configuration": {
                "listen_addr": &config.listen_addr,
                "health_check_enabled": config.health_check.enabled,
                "tls_enabled": config.tls.is_some(),
                "protocols": {
                    "http2_enabled": config.protocols.http2_enabled,
                    "http3_enabled": config.protocols.http3_enabled,
                    "websocket_enabled": config.protocols.websocket_enabled
                }
            },
            "timestamp": chrono::Utc::now().to_rfc3339()
        });

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(status_data.to_string()))
            .wrap_err("Failed to build status response")?;

        Ok(response)
    }

    /// Handle static file requests
    async fn handle_static_file(
        &self,
        req: Request<AxumBody>,
        route_prefix: &str,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        let path = req.uri().path().to_string();

        // Find the matching static route
        if let Some((_, RouteConfig::Static { root, .. })) =
            self.gateway_service.find_matching_route(&path)
        {
            // Extract the file path by removing the route prefix
            let file_path = path.strip_prefix(route_prefix).unwrap_or(&path);

            // Security check: prevent path traversal
            if file_path.contains("..") {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(AxumBody::from("Invalid file path"))
                    .wrap_err("Failed to build error response");
            }

            match self.file_system.serve_file(&root, file_path, req).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    tracing::warn!(error = %e, path = file_path, "static file not found");
                    return Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(AxumBody::from("File or directory not found"))
                        .wrap_err("Failed to build 404 response");
                }
            }
        }

        // If no static route found, return 404
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(AxumBody::from("Static route not found"))
            .wrap_err("Failed to build 404 response")
    }

    /// Handle redirect requests
    async fn handle_redirect(
        &self,
        target: &str,
        status_code: &Option<u16>,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        let status = match status_code {
            Some(code) => StatusCode::from_u16(*code)
                .map_err(|_| eyre::eyre!("Invalid status code: {}", code))?,
            None => StatusCode::FOUND, // 302 by default
        };

        let response = Response::builder()
            .status(status)
            .header(header::LOCATION, target)
            .body(AxumBody::empty())
            .wrap_err("Failed to build redirect response")?;

        Ok(response)
    }

    /// Handle proxy requests to backend services
    async fn handle_proxy_request(
        &self,
        req: Request<AxumBody>,
        client_addr: Option<SocketAddr>,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        // Register connection if we have client address
        let connection_info =
            client_addr.map(|addr| self.connection_tracker.register_connection(addr));

        // Increment request count
        if let Some(ref conn_info) = connection_info {
            conn_info.increment_requests();
        }

        let result = self.proxy_request_to_backend(req).await;

        // Decrement request count
        if let Some(ref conn_info) = connection_info {
            conn_info.decrement_requests();
        }

        result
    }

    /// Proxy the request to a backend service
    async fn proxy_request_to_backend(
        &self,
        mut req: Request<AxumBody>,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        let path = req.uri().path();

        // Find the matching route configuration
        let (route_prefix, route_config) = self
            .gateway_service
            .find_matching_route(path)
            .ok_or_else(|| eyre::eyre!("No matching route found for path: {}", path))?;

        // Get targets and path rewrite from the route configuration
        let (targets, path_rewrite) = match &route_config {
            RouteConfig::Proxy {
                target,
                path_rewrite,
                ..
            } => (vec![target.clone()], path_rewrite.as_ref()),
            RouteConfig::LoadBalance {
                targets,
                path_rewrite,
                ..
            } => (targets.clone(), path_rewrite.as_ref()),
            _ => return Err(eyre::eyre!("Route is not a proxy or load balance route")),
        };

        // Select a backend using the load balancer
        let backend = self
            .gateway_service
            .select_backend(&targets)
            .ok_or_else(|| eyre::eyre!("No healthy backends available"))?;

        // Record selected backend in span
        tracing::Span::current().record("backend.url", &backend);

        // Handle path rewriting
        let original_uri = req.uri().clone();
        let rewritten_path = if let Some(rewrite) = path_rewrite {
            // Strip the route prefix and prepend the rewrite path
            let remaining_path = path.strip_prefix(&route_prefix).unwrap_or(path);
            if remaining_path.is_empty() {
                rewrite.to_string()
            } else {
                format!("{}/{}", rewrite.trim_end_matches('/'), remaining_path)
            }
        } else {
            // No rewrite, use original path
            original_uri.path().to_string()
        };

        // Construct the backend URI with the rewritten path
        let backend_uri = if let Some(query) = original_uri.query() {
            format!(
                "{}{}?{}",
                backend.trim_end_matches('/'),
                rewritten_path,
                query
            )
        } else {
            format!("{}{}", backend.trim_end_matches('/'), rewritten_path)
        };

        *req.uri_mut() = backend_uri
            .parse()
            .wrap_err("Failed to parse backend URI")?;

        // Add forwarded headers
        let headers = req.headers_mut();
        if let Some(client_ip) = self.extract_client_ip(headers) {
            headers.insert(
                "X-Forwarded-For",
                client_ip
                    .parse()
                    .map_err(|e| eyre::eyre!("Failed to parse client IP: {}", e))?,
            );
        }
        headers.insert(
            "X-Forwarded-Proto",
            "http"
                .parse()
                .map_err(|e| eyre::eyre!("Failed to parse protocol: {}", e))?,
        ); // TODO: Use actual protocol
        headers.insert(
            "X-Forwarded-Host",
            original_uri
                .host()
                .unwrap_or("unknown")
                .parse()
                .map_err(|e| eyre::eyre!("Failed to parse host: {}", e))?,
        );

        // Send request to backend
        let backend_start = Instant::now();
        match self.http_client.send_request(req).await {
            Ok(response) => {
                let backend_duration = backend_start.elapsed();
                tracing::info!(
                    backend_status = response.status().as_u16(),
                    backend_duration_ms = backend_duration.as_millis(),
                    "backend response"
                );
                Ok(response)
            }
            Err(e) => {
                let backend_duration = backend_start.elapsed();
                tracing::error!(
                    error = %e,
                    backend_duration_ms = backend_duration.as_millis(),
                    "backend failed"
                );
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(AxumBody::from("Backend request failed"))
                    .wrap_err("Failed to build bad gateway response")?)
            }
        }
    }

    /// Extract client IP from headers or connection info
    fn extract_client_ip(&self, headers: &HeaderMap) -> Option<String> {
        // Check various forwarded headers
        #[allow(clippy::collapsible_if)]
        if let Some(forwarded_for) = headers.get("X-Forwarded-For") {
            if let Ok(value) = forwarded_for.to_str() {
                // Get the first IP in the chain
                return value.split(',').next().map(|ip| ip.trim().to_string());
            }
        }

        #[allow(clippy::collapsible_if)]
        if let Some(real_ip) = headers.get("X-Real-IP") {
            if let Ok(value) = real_ip.to_str() {
                return Some(value.to_string());
            }
        }

        None
    }

    /// Handle WebSocket upgrade requests
    pub async fn handle_websocket_upgrade(
        &self,
        _req: Request<AxumBody>,
    ) -> Result<Response<AxumBody>, eyre::Error> {
        // TODO: Implement WebSocket proxying
        tracing::warn!("websocket not implemented");

        Response::builder()
            .status(StatusCode::NOT_IMPLEMENTED)
            .body(AxumBody::from("WebSocket proxying not yet implemented"))
            .wrap_err("Failed to build not implemented response")
    }
}

impl Clone for HttpHandler {
    fn clone(&self) -> Self {
        Self {
            gateway_service: self.gateway_service.clone(),
            http_client: self.http_client.clone(),
            file_system: self.file_system.clone(),
            connection_tracker: self.connection_tracker.clone(),
            config: self.config.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::ServerConfig;

    fn create_test_handler() -> HttpHandler {
        let config = Arc::new(ServerConfig::default());
        let gateway_service = Arc::new(GatewayService::new(config.clone()));
        let http_client =
            Arc::new(crate::adapters::HttpClientAdapter::new().unwrap()) as Arc<dyn HttpClient>;
        let file_system = Arc::new(FileSystemAdapter::new());
        let connection_tracker = Arc::new(ConnectionTracker::new());
        let config_holder = Arc::new(RwLock::new(config));

        HttpHandler::new(
            gateway_service,
            http_client,
            file_system,
            connection_tracker,
            config_holder,
        )
    }

    #[tokio::test]
    async fn test_health_check_handler() {
        let handler = create_test_handler();
        let result = handler.handle_health_check().await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND); // No backends configured
    }

    #[tokio::test]
    async fn test_metrics_handler() {
        let handler = create_test_handler();
        let result = handler.handle_metrics().await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[tokio::test]
    async fn test_status_handler() {
        let handler = create_test_handler();
        let result = handler.handle_status().await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }
}
