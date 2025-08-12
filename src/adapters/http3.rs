//! Minimal HTTP/3 (QUIC) listener that bridges requests into the existing
//! `HttpHandler` (HTTP/1.1 & HTTP/2 path). This is an early implementation
//! focused on getting a functional baseline that compiles; streaming bodies,
//! flow control awareness, metrics, and graceful shutdown will follow.
//!
//! Current limitations (to be addressed):
//! - Request & response bodies fully buffered (no streaming)
//! - No per-request timeout / cancellation wiring
//! - Lacks protocol–specific metrics (counts / latency histograms)
//! - No integration with global graceful shutdown yet
//! - Limited error classification / backoff

use std::{net::SocketAddr, sync::Arc, time::Instant};

use axum::{BoxError, body::Body as AxumBody};
use eyre::{Result, WrapErr};
use futures_util::StreamExt; // for map on ReceiverStream
use http_body_util::BodyExt; // for frame() on Body
use hyper::{Request, Response};
use quinn::{Endpoint, ServerConfig as QuinnServerConfig};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::{
    adapters::http_handler::HttpHandler,
    metrics::{increment_request_total, record_request_duration},
    utils::graceful_shutdown::ShutdownToken,
};

/// Spawn an HTTP/3 QUIC endpoint using a fully prepared rustls server config
/// (already containing ALPN h3). Returns a background task handle.
pub async fn spawn_http3(
    listen_addr: SocketAddr,
    handler: Arc<HttpHandler>,
    rustls_config: rustls::ServerConfig,
    mut shutdown: Option<ShutdownToken>,
) -> Result<JoinHandle<()>> {
    let mut transport = quinn::TransportConfig::default();
    // Conservative initial concurrency – later make configurable (Http3Config)
    transport.max_concurrent_bidi_streams(100u32.into());

    // Convert rustls::ServerConfig into quinn crypto config
    let crypto = Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(rustls_config)
            .wrap_err("convert rustls config to quinn QuicServerConfig")?,
    );
    let mut server_config = QuinnServerConfig::with_crypto(crypto);
    server_config.transport = Arc::new(transport);

    let endpoint = Endpoint::server(server_config, listen_addr)
        .wrap_err("Failed to build QUIC server endpoint")?;

    info!(%listen_addr, "HTTP/3 QUIC endpoint listening");
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = async {
                    if let Some(tok) = shutdown.as_mut() {
                        tok.wait_for_shutdown().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    info!("Shutdown signal received – closing HTTP/3 endpoint");
                    endpoint.close(0u32.into(), b"shutdown");
                    break;
                }
                connecting = endpoint.accept() => {
                    let connecting = match connecting {
                        Some(c) => c,
                        None => {
                            warn!("quinn endpoint closed – stopping HTTP/3 accept loop");
                            break;
                        }
                    };
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        match connecting.await {
                            Ok(connection) => {
                                let remote = connection.remote_address();
                                if let Err(e) = handle_connection(connection, handler.clone()).await {
                                    error!(error=%e, %remote, "http3 connection error");
                                }
                            }
                            Err(e) => warn!(error=%e, "QUIC handshake failed"),
                        }
                    });
                }
            }
        }
    });

    Ok(task)
}

async fn handle_connection(quinn_conn: quinn::Connection, handler: Arc<HttpHandler>) -> Result<()> {
    // Establish h3 server connection. Current h3 API returns a Connection directly.
    let mut h3_conn =
        h3::server::Connection::<_, bytes::Bytes>::new(h3_quinn::Connection::new(quinn_conn))
            .await
            .wrap_err("h3 connection handshake failed")?;

    loop {
        let accepted = h3_conn.accept().await; // Result<Option<(Request<()>, RequestStream)>, Error>
        let resolver = match accepted {
            Ok(opt) => match opt {
                Some(r) => r,
                None => break,
            },
            Err(e) => {
                error!(error=%e, "h3 accept error – ending connection loop");
                break;
            }
        };
        let handler_clone = handler.clone();
        tokio::spawn(async move {
            match resolver.resolve_request().await {
                Ok((req_head, mut req_stream)) => {
                    let start = Instant::now();
                    use bytes::Bytes;
                    use tokio::sync::mpsc;
                    use tokio_stream::wrappers::ReceiverStream;

                    // Channel for streaming request body chunks
                    let (tx, rx) = mpsc::channel::<Result<Bytes, eyre::Report>>(32);
                    use std::sync::Mutex as StdMutex;
                    let req_stream_shared = Arc::new(tokio::sync::Mutex::new(req_stream));
                    let req_stream_for_body = req_stream_shared.clone();
                    tokio::spawn(async move {
                        loop {
                            let mut guard = req_stream_for_body.lock().await;
                            match guard.recv_data().await {
                                Ok(Some(mut buf_impl)) => {
                                    use bytes::{Buf, BytesMut};
                                    let mut bytes_mut =
                                        BytesMut::with_capacity(buf_impl.remaining());
                                    while buf_impl.has_remaining() {
                                        let chunk_part = buf_impl.chunk();
                                        bytes_mut.extend_from_slice(chunk_part);
                                        let len = chunk_part.len();
                                        buf_impl.advance(len);
                                    }
                                    let bytes = bytes_mut.freeze();
                                    if tx.send(Ok(bytes)).await.is_err() {
                                        break;
                                    }
                                }
                                Ok(None) => {
                                    break;
                                }
                                Err(e) => {
                                    let report: eyre::Report =
                                        eyre::eyre!(e).wrap_err("recv h3 body stream");
                                    let _ = tx.send(Err(report)).await; // best-effort
                                    break;
                                }
                            }
                        }
                        // drop tx -> end of stream
                    });

                    // Build synthetic hyper (axum) request with streaming body
                    let (parts, _) = req_head.into_parts();
                    let method_str = parts.method.as_str().to_string();
                    let path_str = parts.uri.path().to_string();
                    let mut builder = Request::builder()
                        .method(parts.method.clone())
                        .uri(parts.uri.clone());
                    for (k, v) in parts.headers.iter() {
                        builder = builder.header(k, v);
                    }

                    let stream = ReceiverStream::new(rx).map(|res| {
                        match res {
                            Ok(bytes) => Ok::<Bytes, BoxError>(bytes),
                            Err(e) => {
                                error!(error=?e, "error streaming HTTP/3 request body");
                                // Provide empty chunk then end; convert error into empty data for now
                                Ok(Bytes::new())
                            }
                        }
                    });
                    let body = AxumBody::from_stream(stream);

                    let hyper_req: Request<AxumBody> = match builder.body(body) {
                        Ok(r) => r,
                        Err(e) => {
                            error!(error=%e, "build synthetic hyper request from h3");
                            return;
                        }
                    };

                    let hyper_resp = match handler_clone.handle_request(hyper_req, None).await {
                        Ok(r) => r,
                        Err(e) => {
                            error!(error=%e, "handler error for h3 request");
                            match Response::builder()
                                .status(500)
                                .body(AxumBody::from("Internal Server Error"))
                            {
                                Ok(r) => r,
                                Err(build_err) => {
                                    error!(error=%build_err, "failed to build 500 response for h3 request");
                                    return;
                                }
                            }
                        }
                    };

                    let status_for_metrics = hyper_resp.status().as_u16();
                    let (resp_parts, mut resp_body) = hyper_resp.into_parts();
                    let mut h3_resp_builder = http::Response::builder().status(resp_parts.status);
                    for (k, v) in resp_parts.headers.iter() {
                        h3_resp_builder = h3_resp_builder.header(k, v);
                    }
                    let h3_resp = match h3_resp_builder.body(()) {
                        Ok(r) => r,
                        Err(e) => {
                            error!(error=%e, "build h3 response headers");
                            return;
                        }
                    };
                    let mut req_stream = req_stream_shared.lock().await;
                    if let Err(e) = req_stream.send_response(h3_resp).await {
                        error!(error=%e, "send h3 response headers");
                        return;
                    }
                    // Stream response body in chunks to avoid buffering entire body in memory.
                    while let Some(frame) = resp_body.frame().await {
                        match frame {
                            Ok(f) => {
                                if let Some(data) = f.data_ref() {
                                    if !data.is_empty() {
                                        if let Err(e) = req_stream.send_data(data.clone()).await {
                                            error!(error=%e, "send h3 response data frame");
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!(error=%e, "error reading response body frame for h3");
                                break;
                            }
                        }
                    }
                    // Record metrics after response fully sent
                    increment_request_total(&path_str, &method_str, status_for_metrics, "http3");
                    record_request_duration(&path_str, &method_str, "http3", start.elapsed());
                }
                Err(e) => error!(error=%e, "HTTP/3 request resolve error"),
            }
        });
    }
    Ok(())
}

// (Tests for HTTP/3 integration will be added once the functional path is
// validated; initial attempt removed due to evolving upstream crate APIs.)
