use std::{path::Path, sync::Arc, time::Duration};

use arc_swap::ArcSwap;
#[cfg(feature = "http3")]
use axon::adapters::http3; // HTTP/3 spawn function
use axon::{
    adapters::{
        FileConfigProvider, FileSystemAdapter, HealthChecker, HttpClientAdapter, HttpConfigProvider,
    },
    config::models::ServerConfig,
    core::GatewayService,
    metrics,
    ports::{config_provider::ConfigProvider, http_client::HttpClient},
    tracing_setup,
    utils::graceful_shutdown::GracefulShutdown,
};
use axum::serve::{Listener, ListenerExt};
use clap::Parser;
use color_eyre::{
    Result,
    eyre::{Context, eyre},
};
use futures_util::StreamExt;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex as TokioMutex,
};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(subcommand)]
    command: Option<Commands>,

    #[clap(short, long, default_value = "config.toml")]
    config: String,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Validate configuration file
    Validate {
        /// Configuration file to validate
        #[clap(short, long, default_value = "config.toml")]
        config: String,
    },
    /// Initialize a new configuration file
    Init {
        /// Output path for the new config file
        #[clap(short, long, default_value = "config.toml")]
        config: String,
    },
    /// Start the gateway server (default)
    Serve {
        /// Configuration file to use
        #[clap(short, long, default_value = "config.toml")]
        config: String,
    },
}

struct AxumListener<S> {
    stream: S,
    local_addr: std::net::SocketAddr,
}

impl<S, I, E> Listener for AxumListener<S>
where
    S: futures_util::Stream<Item = Result<(I, std::net::SocketAddr), E>> + Unpin + Send + 'static,
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    type Io = I;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.stream.next().await {
                Some(Ok((io, addr))) => return (io, addr),
                Some(Err(e)) => tracing::debug!("Accept error: {}", e),
                None => std::future::pending().await,
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

fn create_config_provider(config_path: &str) -> Result<Arc<dyn ConfigProvider>> {
    if config_path.starts_with("http://") || config_path.starts_with("https://") {
        Ok(Arc::new(HttpConfigProvider::new(
            config_path.to_string(),
            Duration::from_secs(10),
        )))
    } else {
        Ok(Arc::new(FileConfigProvider::new(config_path)?))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let args = Args::parse();

    // Determine the command to run
    let (command, config_path) = match args.command {
        Some(Commands::Validate { config }) => ("validate", config),
        Some(Commands::Init { config }) => ("init", config),
        Some(Commands::Serve { config }) => ("serve", config),
        None => ("serve", args.config), // Default to serve with config from args
    };

    match command {
        "validate" => {
            return validate_config_command(&config_path).await;
        }
        "init" => {
            return init_config_command(&config_path).await;
        }
        "serve" => {
            // Continue with normal server startup
        }
        _ => unreachable!(),
    }

    let provider = rustls::crypto::aws_lc_rs::default_provider();
    if let Err(e) = rustls::crypto::CryptoProvider::install_default(provider) {
        tracing::warn!(
            "CryptoProvider::install_default for aws-lc-rs reported an error: {:?}. \
            This can happen if a provider was already installed. \
            The application will proceed; ensure a crypto provider is effectively available.",
            e
        );
    } else {
        tracing::info!("Successfully installed aws-lc-rs as the default crypto provider.");
    }

    // Configure tracing_subscriber for JSON output with OpenTelemetry
    tracing_setup::init_tracing().map_err(|e| eyre!("Failed to initialize tracing: {}", e))?;

    // Initialize OpenTelemetry metrics
    metrics::init_metrics()
        .await
        .map_err(|e| eyre!("Failed to initialize metrics: {}", e))?;

    tracing::info!("Loading initial configuration from {config_path}");

    // Create config provider
    let config_provider =
        create_config_provider(&config_path).context("Failed to create config provider")?;

    let initial_server_config_data: ServerConfig = config_provider
        .load_config()
        .await
        .with_context(|| format!("Failed to load initial config from {config_path}"))?;

    let initial_config_arc = Arc::new(initial_server_config_data);
    let config_holder = Arc::new(ArcSwap::new(initial_config_arc.clone()));

    let http_client: Arc<dyn HttpClient> =
        Arc::new(HttpClientAdapter::new().context("Failed to create HTTP client adapter")?);
    let file_system = Arc::new(FileSystemAdapter::new());

    let initial_gateway_service = Arc::new(GatewayService::new(config_holder.load_full()));
    let gateway_service_holder = Arc::new(ArcSwap::new(initial_gateway_service.clone()));

    let health_checker_handle_arc_mutex =
        Arc::new(TokioMutex::new(None::<tokio::task::JoinHandle<()>>));

    {
        let mut handle_guard = health_checker_handle_arc_mutex.lock().await;
        let current_config = config_holder.load_full();
        if current_config.health_check.enabled {
            tracing::info!("Starting initial health checker...");

            let health_checker =
                HealthChecker::new(gateway_service_holder.load_full(), http_client.clone());

            *handle_guard = Some(tokio::spawn(async move {
                tracing::info!(
                    "Initial health checker task started. Interval: {}s, Path: {}, Unhealthy Threshold: {}, Healthy Threshold: {}",
                    current_config.health_check.interval_secs,
                    current_config.health_check.path,
                    current_config.health_check.unhealthy_threshold,
                    current_config.health_check.healthy_threshold
                );
                if let Err(e) = health_checker.run().await {
                    tracing::error!("Initial health checker run error: {}", e);
                }
            }));
        } else {
            tracing::info!("Initial configuration has health checking disabled.");
        }
    }

    // Config Watcher Task
    let config_holder_clone = config_holder.clone();
    let gateway_service_holder_clone = gateway_service_holder.clone();
    let health_handle_for_watcher = health_checker_handle_arc_mutex.clone();
    let http_client_for_watcher = http_client.clone();
    let debounce_duration = Duration::from_secs(2);

    let mut notify_rx = config_provider.watch();
    let config_provider_for_watcher = config_provider.clone();
    let config_path_for_watcher = config_path.clone();

    tokio::spawn(async move {
        tracing::info!("Config watcher task started.");
        let mut last_reload_attempt_time = tokio::time::Instant::now();
        last_reload_attempt_time = last_reload_attempt_time
            .checked_sub(debounce_duration)
            .unwrap_or(last_reload_attempt_time);

        while notify_rx.recv().await.is_some() {
            // Debounce
            if last_reload_attempt_time.elapsed() < debounce_duration {
                tracing::info!("Debouncing config reload event. Still within cooldown period.");
                while notify_rx.try_recv().is_ok() {}
                continue;
            }
            last_reload_attempt_time = tokio::time::Instant::now();

            tracing::info!(
                "Attempting to reload configuration from {}",
                config_path_for_watcher
            );

            match config_provider_for_watcher.load_config().await {
                Ok(new_config_data) => {
                    let new_config_arc: Arc<ServerConfig> = Arc::new(new_config_data);
                    tracing::info!("Successfully loaded new configuration.");

                    config_holder_clone.store(new_config_arc.clone());
                    tracing::info!("Global ServerConfig Arc updated.");

                    let new_gateway_service = Arc::new(GatewayService::new(new_config_arc.clone()));
                    gateway_service_holder_clone.store(new_gateway_service.clone());
                    tracing::info!("Global GatewayService Arc updated.");

                    let mut handle_guard = health_handle_for_watcher.lock().await;

                    // Stop existing health checker
                    if let Some(health_handle) = handle_guard.take() {
                        health_handle.abort();
                    }

                    if new_config_arc.health_check.enabled {
                        let health_checker = HealthChecker::new(
                            new_gateway_service.clone(),
                            http_client_for_watcher.clone(),
                        );
                        let config_for_logging = new_config_arc.clone();

                        *handle_guard = Some(tokio::spawn(async move {
                            tracing::info!(
                                "Reload health checker task started. Interval: {}s, Path: {}, Unhealthy Threshold: {}, Healthy Threshold: {}",
                                config_for_logging.health_check.interval_secs,
                                config_for_logging.health_check.path,
                                config_for_logging.health_check.unhealthy_threshold,
                                config_for_logging.health_check.healthy_threshold
                            );
                            if let Err(e) = health_checker.run().await {
                                tracing::error!("Reload health checker run error: {}", e);
                            }
                        }));
                    } else {
                        tracing::info!("Health checking is disabled in the new configuration.");
                    }
                    tracing::info!(
                        "Configuration reloaded and health checker (if enabled) managed."
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to reload configuration: {}. Keeping old configuration.",
                        e
                    );
                }
            }
            while notify_rx.try_recv().is_ok() {}
        }
        tracing::info!("Config watcher task is shutting down.");
    });

    // Create graceful shutdown manager
    let graceful_shutdown = Arc::new(GracefulShutdown::new());

    // Start signal handler for graceful shutdown
    let signal_handler_shutdown = graceful_shutdown.clone();
    tokio::spawn(async move {
        if let Err(e) = signal_handler_shutdown.run_signal_handler().await {
            tracing::error!("Signal handler error: {}", e);
        }
    });

    // Create a simple HTTP handler for now
    // In a full implementation, this would be replaced with a proper UnifiedServer
    use std::net::SocketAddr;

    use axon::{adapters::HttpHandler, utils::ConnectionTracker};

    let connection_tracker = Arc::new(ConnectionTracker::new());
    let http_handler = Arc::new(HttpHandler::new(
        gateway_service_holder.clone(),
        http_client.clone(),
        file_system.clone(),
        connection_tracker.clone(),
        config_holder.clone(),
    ));

    // Optionally start HTTP/3 QUIC endpoint (avoid holding locks across await)
    #[cfg(feature = "http3")]
    {
        let mut _http3_handle: Option<tokio::task::JoinHandle<()>> = None; // reserved for future graceful shutdown handling
        let (http3_enabled, tls_cfg_opt, listen_addr_for_h3) = {
            let cfg = config_holder.load();
            (
                cfg.protocols.http3_enabled,
                cfg.tls.clone(),
                cfg.listen_addr.clone(),
            )
        };
        if http3_enabled {
            if let Some(tls_cfg) = tls_cfg_opt {
                if let (Some(cert_path), Some(key_path)) = (tls_cfg.cert_path, tls_cfg.key_path) {
                    match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
                        (Ok(cert_bytes), Ok(key_bytes)) => {
                            use rustls::pki_types::{CertificateDer, PrivateKeyDer};
                            let mut cert_reader = &*cert_bytes;
                            let certs: Vec<CertificateDer> =
                                rustls_pemfile::certs(&mut cert_reader)
                                    .filter_map(|res| match res {
                                        Ok(c) => Some(c),
                                        Err(e) => {
                                            tracing::error!(error=?e, "failed parsing http3 cert");
                                            None
                                        }
                                    })
                                    .collect();
                            let mut key_reader = &*key_bytes;
                            let key: Option<PrivateKeyDer> =
                                rustls_pemfile::pkcs8_private_keys(&mut key_reader)
                                    .filter_map(|res| match res {
                                        Ok(k) => Some(PrivateKeyDer::Pkcs8(k)),
                                        Err(e) => {
                                            tracing::error!(error=?e, "failed parsing http3 key");
                                            None
                                        }
                                    })
                                    .next();
                            if let Some(key) = key {
                                if certs.is_empty() {
                                    tracing::warn!("HTTP/3 enabled but no certificates parsed");
                                } else if let Ok(addr) =
                                    listen_addr_for_h3.parse::<std::net::SocketAddr>()
                                {
                                    let mut server_config = rustls::ServerConfig::builder()
                                        .with_no_client_auth()
                                        .with_single_cert(certs, key)
                                        .map_err(|e| {
                                            eyre!("Failed building rustls config for http3: {e}")
                                        })?;
                                    server_config.alpn_protocols = vec![b"h3".to_vec()];
                                    let h3_handler = http_handler.clone();
                                    let shutdown_token = graceful_shutdown.shutdown_token();
                                    match http3::spawn_http3(
                                        addr,
                                        h3_handler,
                                        server_config,
                                        Some(shutdown_token),
                                    )
                                    .await
                                    {
                                        Ok(h) => {
                                            _http3_handle = Some(h);
                                            tracing::info!("HTTP/3 endpoint started on {addr}");
                                        }
                                        Err(e) => {
                                            tracing::error!(error=%e, "Failed to start HTTP/3 endpoint")
                                        }
                                    }
                                } else {
                                    tracing::error!("Failed to parse listen address for http3");
                                }
                            } else {
                                tracing::error!("No PKCS#8 private key found in key file");
                            }
                        }
                        (Err(e1), Err(e2)) => {
                            tracing::error!(error=?(e1,e2), "Failed reading cert & key for http3")
                        }
                        (Err(e), _) => tracing::error!(error=?e, "Failed reading cert for http3"),
                        (_, Err(e)) => tracing::error!(error=?e, "Failed reading key for http3"),
                    }
                } else {
                    tracing::warn!(
                        "HTTP/3 enabled but TLS manual cert/key paths not provided (ACME unsupported yet)"
                    );
                }
            } else {
                tracing::warn!(
                    "HTTP/3 enabled but TLS configuration missing – QUIC requires TLS; skipping http3 start"
                );
            }
        }
    }

    // Simple server that binds to the configured address
    let addr: SocketAddr = {
        let config_ref = config_holder.load();
        config_ref
            .listen_addr
            .parse()
            .context("Failed to parse listen address")?
    };

    // Show configuration info
    {
        let ch = config_holder.load();
        let protocols = &ch.protocols;

        tracing::info!(
            "Starting Axon API Gateway on {} (TLS enabled: {}, HTTP/2: {}, WebSocket: {})",
            ch.listen_addr,
            ch.tls.is_some(),
            protocols.http2_enabled,
            protocols.websocket_enabled
        );

        println!(
            "Axon API Gateway listening on {} (TLS: {}, HTTP/2: {}, WebSocket: {})",
            ch.listen_addr,
            ch.tls.is_some(),
            protocols.http2_enabled,
            protocols.websocket_enabled
        );
    }

    // Create Axum router with real request handling
    use std::convert::Infallible;

    use axum::{
        Router,
        body::Body,
        extract::{ConnectInfo, Request},
        response::Response,
        routing::any,
    };
    use tower_http::compression::CompressionLayer;

    let make_request_route = |handler: Arc<HttpHandler>| {
        any(
            move |ConnectInfo(client_addr): ConnectInfo<SocketAddr>, req: Request| {
                let handler = handler.clone();
                async move {
                    match handler.handle_request(req, Some(client_addr)).await {
                        Ok(response) => Ok::<Response<Body>, Infallible>(response),
                        Err(e) => {
                            tracing::error!("Request handling error: {:?}", e);
                            let error_response = Response::builder()
                                .status(500)
                                .body(Body::from("Internal Server Error"))
                                .unwrap_or_else(|_| {
                                    Response::new(Body::from("Internal Server Error"))
                                });
                            Ok(error_response)
                        }
                    }
                }
            },
        )
    };

    let app = Router::new()
        .layer(CompressionLayer::new())
        .route("/{*path}", make_request_route(http_handler.clone()))
        .route("/", make_request_route(http_handler.clone()));

    // Log initial routes from the config_holder
    {
        let ch = config_holder.load();
        for (prefix, route) in &ch.routes {
            tracing::info!("Configured route: {} -> {:?}", prefix, route);
        }
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind to address")?;

    tracing::info!("Axon API Gateway server starting on {}", addr);

    // Determine TLS configuration
    let tls_config = {
        let cfg = config_holder.load();
        cfg.tls.clone()
    };

    // Run the server and wait for shutdown
    let server_result = if let Some(tls) = tls_config {
        if let Some(acme) = tls.acme {
            // ACME (Let's Encrypt) mode
            use rustls_acme::{AcmeConfig, caches::DirCache};

            tracing::info!("Starting server with ACME (Let's Encrypt) support");
            let state = AcmeConfig::new(acme.domains)
                .contact([format!("mailto:{}", acme.email)])
                .cache_option(Some(DirCache::new(acme.cache_dir)))
                .directory_lets_encrypt(acme.production)
                .state();

            let local_addr = listener.local_addr().context("Failed to get local addr")?;
            let incoming = state.incoming(
                TcpListenerStream::new(listener).map(|res| res.map(|s| s.compat())),
                vec![],
            );
            let stream = incoming
                .filter_map(|res| async {
                    match res {
                        Ok(stream) => {
                            let stream = stream.compat();
                            let addr = stream
                                .get_ref()
                                .get_ref()
                                .0
                                .get_ref()
                                .peer_addr()
                                .unwrap_or_else(|_| {
                                    "0.0.0.0:0".parse().expect("valid fallback address")
                                });
                            Some(Ok::<_, std::io::Error>((stream, addr)))
                        }
                        Err(e) => {
                            tracing::debug!("TLS accept error: {}", e);
                            None
                        }
                    }
                })
                .boxed();

            let tls_listener = AxumListener { stream, local_addr }.tap_io(|_io| {});

            tokio::select! {
                result = axum::serve(
                    tls_listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                ) => {
                    result.context("Server error")
                },
                shutdown_reason = graceful_shutdown.wait_for_shutdown_signal() => {
                    tracing::info!("Shutdown signal received: {:?}", shutdown_reason);
                    Ok(())
                }
            }
        } else if let (Some(cert_path), Some(key_path)) = (tls.cert_path, tls.key_path) {
            // Manual TLS
            use std::{fs::File, io::BufReader};

            use rustls::ServerConfig;
            use rustls_pemfile::{certs, pkcs8_private_keys};
            use tls_listener::TlsListener;

            tracing::info!("Starting server with manual TLS");
            let cert_file =
                &mut BufReader::new(File::open(cert_path).context("failed to open cert file")?);
            let key_file =
                &mut BufReader::new(File::open(key_path).context("failed to open key file")?);

            let cert_chain = certs(cert_file).collect::<Result<Vec<_>, _>>()?;
            let key = pkcs8_private_keys(key_file)
                .next()
                .transpose()?
                .ok_or_else(|| eyre!("No PKCS#8 private key found in key file"))?;

            let config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, key.into())?;

            let local_addr = listener.local_addr().context("Failed to get local addr")?;
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let tls_listener_stream = TlsListener::new(acceptor, listener);

            let tls_listener = AxumListener {
                stream: tls_listener_stream,
                local_addr,
            }
            .tap_io(|_io| {});

            tokio::select! {
                result = axum::serve(
                    tls_listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                ) => {
                    result.context("Server error")
                },
                shutdown_reason = graceful_shutdown.wait_for_shutdown_signal() => {
                    tracing::info!("Shutdown signal received: {:?}", shutdown_reason);
                    Ok(())
                }
            }
        } else {
            Err(eyre!("TLS enabled but no valid config found"))
        }
    } else {
        // Plain HTTP
        tokio::select! {
            result = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>()
            ) => {
                result.context("Server error")
            },
            shutdown_reason = graceful_shutdown.wait_for_shutdown_signal() => {
                tracing::info!("Shutdown signal received: {:?}", shutdown_reason);

                // Cleanup health checker
                let mut handle_guard = health_checker_handle_arc_mutex.lock().await;
                if let Some(health_handle) = handle_guard.take() {
                    tracing::info!("Shutting down health checker...");
                    health_handle.abort();
                }

                tracing::info!("Graceful shutdown completed");
                Ok(())
            }
        }
    };

    server_result?;

    // Shutdown tracing on exit
    tracing_setup::shutdown_tracing();

    Ok(())
}

/// Validate configuration file and exit
async fn validate_config_command(config_path: &str) -> Result<()> {
    use axon::config::{ServerConfigValidator, loader::load_config};

    println!("🔍 Validating configuration file: {config_path}");

    // First check if file exists and is readable
    if !Path::new(config_path).exists() {
        eprintln!("❌ Error: Configuration file '{config_path}' not found");
        std::process::exit(1);
    }

    // Try to parse the configuration
    let config = match load_config(config_path).await {
        Ok(config) => {
            println!("✅ Configuration parsing: OK");
            config
        }
        Err(e) => {
            eprintln!("❌ Configuration parsing failed:");
            eprintln!("   {e}");
            std::process::exit(1);
        }
    };

    // Validate the configuration
    match ServerConfigValidator::validate(&config) {
        Ok(()) => {
            println!("✅ Configuration validation: OK");
            println!();
            println!("📋 Configuration Summary:");
            println!("   • Listen Address: {}", config.listen_addr);
            println!("   • Routes: {}", config.routes.len());
            println!("   • TLS Enabled: {}", config.tls.is_some());
            println!("   • Health Checks: {}", config.health_check.enabled);
            println!();
            println!("🎉 Configuration is valid and ready to use!");
            Ok(())
        }
        Err(e) => {
            eprintln!("❌ Configuration validation failed:");
            eprintln!("{e}");
            println!();
            println!("💡 Common fixes:");
            println!("   • Ensure all URLs start with http:// or https://");
            println!("   • Check that file paths exist");
            println!("   • Verify listen address format (e.g., '127.0.0.1:3000')");
            println!("   • Ensure rate limit periods use valid units (s, m, h)");
            std::process::exit(1);
        }
    }
}

/// Initialize a new configuration file
async fn init_config_command(config_path: &str) -> Result<()> {
    let path = Path::new(config_path);
    if path.exists() {
        eprintln!("❌ Error: Configuration file '{config_path}' already exists");
        std::process::exit(1);
    }

    let default_config = r#"# Axon API Gateway Configuration

# The address to listen on
listen_addr = "127.0.0.1:8080"

# Health check configuration
[health_check]
enabled = true
interval_secs = 10
path = "/health"

# Protocol configuration
[protocols]
http2_enabled = true
websocket_enabled = true

# Example Route: Proxy to a backend
[routes."/api"]
type = "proxy"
target = "http://localhost:3000"

# Example Route: Static files
[routes."/static"]
type = "static"
root = "./static"

# Example Route: Load Balancer
# [routes."/service"]
# type = "load_balance"
# targets = ["http://localhost:3001", "http://localhost:3002"]
# strategy = "round_robin"
"#;

    tokio::fs::write(path, default_config)
        .await
        .context("Failed to write config file")?;
    println!("✅ Created default configuration at: {config_path}");
    println!("   Run 'axon serve --config {config_path}' to start the server");
    Ok(())
}
