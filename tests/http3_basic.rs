#![cfg(feature = "http3")]
//! Basic HTTP/3 integration test.
//!
//! Spins up:
//! * A simple HTTP backend (HTTP/1.1) responding on /api/test
//! * Gateway core objects + HttpHandler
//! * HTTP/3 QUIC listener via `spawn_http3`
//! * An h3 client (quinn) performing a GET /api/test over HTTP/3
//!
//! Verifies that the proxy path works end-to-end and returns the expected body.
//!
//! NOTE: This test purposefully avoids spawning the full binary; it assembles the
//! required pieces directly to keep the test fast and deterministic.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use axon::{
    adapters::{FileSystemAdapter, HttpClientAdapter, HttpHandler, http3},
    config::models::{ProtocolConfig, RouteConfig, ServerConfig},
    core::GatewayService,
    ports::http_client::HttpClient,
    utils::ConnectionTracker,
};
use eyre::{Result, WrapErr};
use rcgen::generate_simple_self_signed;
use tokio::time::sleep;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http3_proxy_basic() -> Result<()> {
    // Ensure a rustls CryptoProvider is installed (avoid panic if multiple providers enabled).
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Generate self-signed cert for localhost
    let cert = generate_simple_self_signed(["localhost".to_string()])
        .wrap_err("generate self signed cert")?;
    let cert_der = cert.cert.der().to_vec();
    // rcgen 0.14: private key available via signing_key
    let key_der = cert.signing_key.serialize_der();

    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let cert_chain: Vec<CertificateDer> = vec![CertificateDer::from(cert_der.clone())];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.clone()));

    // Pick random ports
    let backend_port = pick_free_port();
    let h3_port = pick_free_port();

    // Start backend HTTP server (axum minimal)
    let backend_addr: SocketAddr = format!("127.0.0.1:{backend_port}").parse().unwrap();
    let backend_listener = tokio::net::TcpListener::bind(backend_addr).await.unwrap();
    tokio::spawn(async move {
        use axum::{Router, routing::get};
        let app = Router::new().route("/api/test", get(|| async { "ok-h3" }));
        if let Err(e) = axum::serve(backend_listener, app).await {
            eprintln!("backend server error: {e}");
        }
    });

    // Build ServerConfig with proxy route
    let mut routes = HashMap::new();
    routes.insert(
        "/api".to_string(),
        RouteConfig::Proxy {
            target: format!("http://127.0.0.1:{backend_port}"),
            path_rewrite: None,
            rate_limit: None,
            request_headers: None,
            response_headers: None,
            request_body: None,
            response_body: None,
        },
    );
    let mut protocols = ProtocolConfig::default();
    protocols.http3_enabled = true;
    let server_config = ServerConfig {
        listen_addr: format!("127.0.0.1:{h3_port}"),
        routes,
        protocols,
        ..ServerConfig::default()
    };

    let config_arc = Arc::new(server_config);
    let config_holder = Arc::new(RwLock::new(config_arc.clone()));
    let gateway = Arc::new(GatewayService::new(config_arc.clone()));
    let gateway_holder = Arc::new(RwLock::new(gateway));

    let http_client: Arc<dyn HttpClient> = Arc::new(HttpClientAdapter::new()?);
    let file_system = Arc::new(FileSystemAdapter::new());
    let connection_tracker = Arc::new(ConnectionTracker::new());
    let handler = Arc::new(HttpHandler::new(
        gateway_holder.clone(),
        http_client.clone(),
        file_system.clone(),
        connection_tracker.clone(),
        config_holder.clone(),
    ));

    // Build rustls server config with ALPN h3
    let mut rustls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain.clone(), key)
        .wrap_err("build rustls server config")?;
    rustls_config.alpn_protocols = vec![b"h3".to_vec()];

    // Start HTTP/3 listener
    let listen_addr: SocketAddr = format!("127.0.0.1:{h3_port}").parse().unwrap();
    let _h3_task = http3::spawn_http3(listen_addr, handler.clone(), rustls_config, None).await?;

    // Retry loop for client connect (server startup is async)
    // Build client config trusting our self-signed cert
    use quinn::{ClientConfig, Endpoint};
    use rustls::{ClientConfig as RustlsClientConfig, RootCertStore};
    let mut roots = RootCertStore::empty();
    roots.add(cert_der.into()).unwrap();
    let mut client_crypto = RustlsClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    let client_config = ClientConfig::new(Arc::new(client_crypto));

    let mut client_endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);

    // Connect + issue h3 request
    let url = format!("https://localhost:{h3_port}/api/test");
    let quinn_conn = loop {
        match client_endpoint.connect(listen_addr, "localhost") {
            Ok(connecting) => match connecting.await {
                Ok(c) => break c,
                Err(_e) => {
                    sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            Err(_e) => {
                sleep(Duration::from_millis(50)).await;
                continue;
            }
        }
    };

    // Build h3 client (returns (Connection, SendRequest))
    let (_h3_conn, mut send_req) = h3::client::new(h3_quinn::Connection::new(quinn_conn))
        .await
        .expect("h3 client connection");

    // Build request
    let req = http::Request::builder()
        .method("GET")
        .uri(url)
        .header("host", format!("localhost:{h3_port}"))
        .body(())
        .unwrap();
    let mut req_stream = send_req.send_request(req).await.expect("send h3 request");
    // No request body, so finish explicitly.
    req_stream.finish().await.expect("finish request");
    let response = req_stream.recv_response().await.expect("recv response");
    assert_eq!(response.status(), 200, "expected 200 OK over h3");
    // Receive body data frames
    use bytes::Buf;
    let mut body = Vec::new();
    while let Some(mut chunk) = req_stream.recv_data().await.expect("recv data") {
        while chunk.has_remaining() {
            let b = chunk.copy_to_bytes(chunk.remaining());
            body.extend_from_slice(&b);
        }
    }
    assert_eq!(String::from_utf8_lossy(&body), "ok-h3");

    Ok(())
}

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
