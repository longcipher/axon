// Test for verifying multiple routes on same path with different hosts
#[cfg(test)]
mod test {
    use std::sync::Arc;

    use axon::{
        config::models::{RouteConfig, ServerConfig},
        core::GatewayService,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn test_same_path_different_hosts() {
        let mut config = ServerConfig::default();
        config.listen_addr = "127.0.0.1:8080".to_string();

        // Route 1: "/" with host "api.example.com"
        config.routes.insert(
            "/api-host".to_string(), // Unique key but path will be checked
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

        // Route 2: "/" without host (fallback)
        config.routes.insert(
            "/".to_string(),
            RouteConfig::Proxy {
                target: "http://fallback-backend:5555".to_string(),
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

        // Test 1: Request to /api-host with matching host
        let route = gateway.find_matching_route("/api-host/users", Some("api.example.com"));
        assert!(route.is_some());
        let (_, route_config) = route.unwrap();
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://api-backend:3001");
        }

        // Test 2: Request to / with no matching host - should use fallback
        let route = gateway.find_matching_route("/", Some("unknown.example.com"));
        assert!(route.is_some());
        let (prefix, route_config) = route.unwrap();
        assert_eq!(prefix, "/");
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://fallback-backend:5555");
        }

        // Test 3: Request to /anything with no host - should use fallback
        let route = gateway.find_matching_route("/anything", None);
        assert!(route.is_some());
        let (prefix, route_config) = route.unwrap();
        assert_eq!(prefix, "/");
        if let RouteConfig::Proxy { target, .. } = route_config {
            assert_eq!(target, "http://fallback-backend:5555");
        }
    }
}
