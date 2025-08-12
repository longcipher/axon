use std::{
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

#[cfg(feature = "http3")]
use axon::adapters::http3; // HTTP/3 spawn function
use axon::{
    adapters::{FileSystemAdapter, HealthChecker, HttpClientAdapter},
    config::{loader::load_config, models::ServerConfig},
    core::GatewayService,
    ports::http_client::HttpClient,
    tracing_setup,
    utils::graceful_shutdown::GracefulShutdown,
};
use clap::Parser;
use color_eyre::{
    Result,
    eyre::{Context, eyre},
};
use notify::{RecursiveMode, Watcher};
use tokio::sync::{Mutex as TokioMutex, mpsc};

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
    /// Start the gateway server (default)
    Serve {
        /// Configuration file to use
        #[clap(short, long, default_value = "config.toml")]
        config: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let args = Args::parse();

    // Determine the command to run
    let (command, config_path) = match args.command {
        Some(Commands::Validate { config }) => ("validate", config),
        Some(Commands::Serve { config }) => ("serve", config),
        None => ("serve", args.config), // Default to serve with config from args
    };

    match command {
        "validate" => {
            return validate_config_command(&config_path).await;
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

    tracing::info!("Loading initial configuration from {config_path}");
    let initial_server_config_data: ServerConfig = load_config(&config_path)
        .await
        .with_context(|| format!("Failed to load initial config from {config_path}"))?;

    let initial_config_arc = Arc::new(initial_server_config_data);
    let config_holder = Arc::new(RwLock::new(initial_config_arc.clone()));

    let http_client: Arc<dyn HttpClient> =
        Arc::new(HttpClientAdapter::new().context("Failed to create HTTP client adapter")?);
    let file_system = Arc::new(FileSystemAdapter::new());

    let initial_gateway_service = Arc::new(GatewayService::new(
        config_holder
            .read()
            .map_err(|e| eyre!("Failed to acquire config read lock: {}", e))?
            .clone(),
    ));
    let gateway_service_holder = Arc::new(RwLock::new(initial_gateway_service.clone()));

    let health_checker_handle_arc_mutex =
        Arc::new(TokioMutex::new(None::<tokio::task::JoinHandle<()>>));

    {
        let mut handle_guard = health_checker_handle_arc_mutex.lock().await;
        let current_config = config_holder
            .read()
            .map_err(|e| eyre!("Failed to acquire config read lock: {}", e))?
            .clone();
        if current_config.health_check.enabled {
            tracing::info!("Starting initial health checker...");

            let health_checker = HealthChecker::new(
                gateway_service_holder
                    .read()
                    .map_err(|e| eyre!("Failed to acquire gateway service read lock: {}", e))?
                    .clone(),
                http_client.clone(),
            );

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

    // File Watcher Task
    let config_path_for_watcher = config_path.clone();
    let config_holder_clone = config_holder.clone();
    let gateway_service_holder_clone = gateway_service_holder.clone();
    // Clone for use in different parts of the application
    let health_handle_for_watcher = health_checker_handle_arc_mutex.clone();
    let http_client_for_watcher = http_client.clone();
    let debounce_duration = Duration::from_secs(2);

    tokio::spawn(async move {
        let (notify_tx, mut notify_rx) = mpsc::channel::<()>(10);

        let config_file_as_path = Path::new(&config_path_for_watcher);
        let directory_to_watch = config_file_as_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let config_file_path_for_closure = config_path_for_watcher.clone();

        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    let config_file_name_to_check = Path::new(&config_file_path_for_closure)
                        .file_name()
                        .unwrap_or_default();
                    if (event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove())
                        && event
                            .paths
                            .iter()
                            .any(|p| p.file_name().unwrap_or_default() == config_file_name_to_check)
                    {
                        tracing::debug!(
                            "Config file event detected: {:?}, sending signal for reload.",
                            event.kind
                        );
                        if notify_tx.try_send(()).is_err() {
                            tracing::warn!(
                                "Config reload signal channel (internal to watcher) full or disconnected."
                            );
                        }
                    }
                }
                Err(e) => tracing::error!("File watch error: {:?}", e),
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(
                    "Failed to create file watcher: {}. Hot reloading will be disabled.",
                    e
                );
                return;
            }
        };

        if let Err(e) = watcher.watch(&directory_to_watch, RecursiveMode::NonRecursive) {
            tracing::error!(
                "Failed to watch config directory {:?}: {}. Hot reloading will be disabled.",
                directory_to_watch,
                e
            );
            return;
        }
        tracing::info!(
            "Watching for config file changes in directory: {:?} for file: {}",
            directory_to_watch,
            Path::new(&config_path_for_watcher)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );

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
            match load_config(&config_path_for_watcher).await {
                Ok(new_config_data) => {
                    let new_config_arc: Arc<ServerConfig> = Arc::new(new_config_data);
                    tracing::info!("Successfully loaded new configuration.");

                    {
                        match config_holder_clone.write() {
                            Ok(mut config_w) => {
                                *config_w = new_config_arc.clone();
                                tracing::info!("Global ServerConfig Arc updated.");
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to acquire config write lock during reload: {}",
                                    e
                                );
                                continue;
                            }
                        }
                    }

                    let new_gateway_service = Arc::new(GatewayService::new(new_config_arc.clone()));
                    {
                        match gateway_service_holder_clone.write() {
                            Ok(mut gateway_s_w) => {
                                *gateway_s_w = new_gateway_service.clone();
                                tracing::info!("Global GatewayService Arc updated.");
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to acquire gateway service write lock during reload: {}",
                                    e
                                );
                                continue;
                            }
                        }
                    }

                    let mut handle_guard = health_handle_for_watcher.lock().await;
                    if let Some(old_handle) = handle_guard.take() {
                        tracing::info!("Aborting previous health checker task...");
                        old_handle.abort();
                    }

                    if new_config_arc.health_check.enabled {
                        tracing::info!(
                            "Starting new health checker task with updated configuration..."
                        );

                        let health_checker = HealthChecker::new(
                            new_gateway_service.clone(),
                            http_client_for_watcher.clone(),
                        );
                        let config_for_logging = new_config_arc.clone();

                        *handle_guard = Some(tokio::spawn(async move {
                            tracing::info!(
                                "File Reload health checker task started. Interval: {}s, Path: {}, Unhealthy Threshold: {}, Healthy Threshold: {}",
                                config_for_logging.health_check.interval_secs,
                                config_for_logging.health_check.path,
                                config_for_logging.health_check.unhealthy_threshold,
                                config_for_logging.health_check.healthy_threshold
                            );
                            if let Err(e) = health_checker.run().await {
                                tracing::error!("File Reload health checker run error: {}", e);
                            }
                        }));
                    } else {
                        tracing::info!(
                            "Health checking is disabled in the new configuration. Not starting health checker task."
                        );
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
        tracing::info!("File watcher task is shutting down.");
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
            let cfg = config_holder
                .read()
                .map_err(|e| eyre!("Failed to acquire config read lock: {}", e))?;
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
        let config_ref = config_holder
            .read()
            .map_err(|e| eyre!("Failed to acquire config read lock: {}", e))?;
        config_ref
            .listen_addr
            .parse()
            .context("Failed to parse listen address")?
    };

    // Show configuration info
    {
        let ch = config_holder
            .read()
            .map_err(|e| eyre!("Failed to acquire config read lock: {}", e))?;
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

    let handler = http_handler.clone();
    let app = Router::new()
        .route(
            "/{*path}",
            any(
                move |ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request| {
                    let handler = handler.clone();
                    async move {
                        match handler.handle_request(req, Some(addr)).await {
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
            ),
        )
        .route(
            "/",
            any(
                move |ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request| {
                    let handler = http_handler.clone();
                    async move {
                        match handler.handle_request(req, Some(addr)).await {
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
            ),
        );

    // Log initial routes from the config_holder
    {
        let ch = config_holder
            .read()
            .map_err(|e| eyre!("Failed to acquire config read lock for logging: {}", e))?;
        for (prefix, route) in &ch.routes {
            tracing::info!("Configured route: {} -> {:?}", prefix, route);
        }
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind to address")?;

    tracing::info!("Axon API Gateway server starting on {}", addr);

    // Run the server and wait for shutdown
    let server_result = tokio::select! {
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
