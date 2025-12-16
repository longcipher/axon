// Integration tests for host-based routing functionality
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axon::{
        config::models::{RouteConfig, ServerConfig},
        core::GatewayService,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn test_host_based_routing_priority() {
        // Create a test config with host-specific and default routes
        let mut config = ServerConfig::default();
        config.listen_addr = "127.0.0.1:8080".to_string();

        // Add a route with host specified
        config.routes.insert(
            "/api-with-host".to_string(),
            RouteConfig::Proxy {
                target: "http://api-backend:3001".to_string(),
                host: Some("api.example.com".to_string()),
                path_rewrite: None,
                rate_limit: None,
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                middlewares: vec![],
            },
        );

        // Add a default route without host for same path pattern
        config.routes.insert(
            "/api".to_string(),
            RouteConfig::Proxy {
                target: "http://default-backend:5000".to_string(),
                host: None,
                path_rewrite: None,
                rate_limit: None,
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                middlewares: vec![],
            },
        );

        let gateway = GatewayService::new(Arc::new(config));

        // Test with matching host - should use host-specific route
        let route = gateway.find_matching_route("/api-with-host/users", Some("api.example.com"));
        assert!(route.is_some());
        let (_, route_config) = route.unwrap();
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://api-backend:3001");
        } else {
            panic!("Expected Proxy route");
        }

        // Test with non-matching host - should use default route
        let route = gateway.find_matching_route("/api/users", Some("other.example.com"));
        assert!(route.is_some());
        let (_, route_config) = route.unwrap();
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://default-backend:5000");
        } else {
            panic!("Expected Proxy route");
        }

        // Test without host - should use default route
        let route = gateway.find_matching_route("/api/users", None);
        assert!(route.is_some());
        let (_, route_config) = route.unwrap();
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://default-backend:5000");
        } else {
            panic!("Expected Proxy route");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_host_matching_case_insensitive() {
        let mut config = ServerConfig::default();
        config.listen_addr = "127.0.0.1:8080".to_string();

        config.routes.insert(
            "/".to_string(),
            RouteConfig::Proxy {
                target: "http://backend:3000".to_string(),
                host: Some("Example.Com".to_string()),
                path_rewrite: None,
                rate_limit: None,
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                middlewares: vec![],
            },
        );

        let gateway = GatewayService::new(Arc::new(config));

        // All these should match (case-insensitive)
        assert!(
            gateway
                .find_matching_route("/", Some("example.com"))
                .is_some()
        );
        assert!(
            gateway
                .find_matching_route("/", Some("EXAMPLE.COM"))
                .is_some()
        );
        assert!(
            gateway
                .find_matching_route("/", Some("Example.Com"))
                .is_some()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_longest_prefix_with_host() {
        let mut config = ServerConfig::default();
        config.listen_addr = "127.0.0.1:8080".to_string();

        // Longer path with host
        config.routes.insert(
            "/api/v2".to_string(),
            RouteConfig::Proxy {
                target: "http://api-v2:3002".to_string(),
                host: Some("api.example.com".to_string()),
                path_rewrite: None,
                rate_limit: None,
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                middlewares: vec![],
            },
        );

        // Shorter path with same host
        config.routes.insert(
            "/api".to_string(),
            RouteConfig::Proxy {
                target: "http://api-v1:3001".to_string(),
                host: Some("api.example.com".to_string()),
                path_rewrite: None,
                rate_limit: None,
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                middlewares: vec![],
            },
        );

        let gateway = GatewayService::new(Arc::new(config));

        // Should match the longer prefix
        let route = gateway.find_matching_route("/api/v2/users", Some("api.example.com"));
        assert!(route.is_some());
        let (prefix, route_config) = route.unwrap();
        assert_eq!(prefix, "/api/v2");
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://api-v2:3002");
        }

        // Should match the shorter prefix
        let route = gateway.find_matching_route("/api/users", Some("api.example.com"));
        assert!(route.is_some());
        let (prefix, route_config) = route.unwrap();
        assert_eq!(prefix, "/api");
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://api-v1:3001");
        }
    }
}
