use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use clap::Parser;
use http::header::HOST;
use http::{Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Method;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use serde::Deserialize;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const VERSION: &str = "0.1.0";
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

#[derive(Parser)]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(long)]
    check: bool,

    #[arg(long)]
    version: bool,

    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default)]
    access_log: bool,
    #[serde(default)]
    shutdown_timeout_secs: Option<u64>,
    listeners: Vec<ListenerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
struct ListenerConfig {
    listen: SocketAddr,
    upstream: String,
}

#[derive(Debug, Clone)]
struct Upstream {
    base: Uri,
    authority: String,
    host: String,
    port: u16,
}

struct Proxy {
    tls_connector: TlsConnector,
    access_log: bool,
}

#[derive(Error, Debug)]
enum AppError {
    #[error("Configuration file not found: {path}")]
    ConfigNotFound { path: PathBuf },

    #[error("Failed to read configuration file {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to parse configuration file {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: serde_yaml::Error,
    },

    #[error("Configuration error: {0}")]
    ConfigValidation(String),

    #[error("Failed to bind listener on {addr}: {source}")]
    BindFailed {
        addr: SocketAddr,
        source: std::io::Error,
    },

    #[error("Listener task failed: {0}")]
    ListenerTask(String),
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("Fatal error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let cli = Cli::parse();

    if cli.version {
        println!("{VERSION}");
        return Ok(());
    }

    let config_path = cli.config.unwrap_or_else(default_config_path);

    if cli.check {
        let config = load_config(&config_path)?;
        config.validate()?;
        println!("Configuration valid");
        return Ok(());
    }

    let log_level = cli.log_level.as_deref().unwrap_or(DEFAULT_LOG_LEVEL);
    init_logging(log_level);

    let config = load_config(&config_path)?;
    config.validate()?;

    let proxy = Arc::new(Proxy::new(config.access_log));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let in_flight_notify = Arc::new(Notify::new());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut handles = Vec::with_capacity(config.listeners.len());
    for listener in &config.listeners {
        let parsed_upstream = parse_upstream(&listener.upstream)?;
        let handle = spawn_listener(
            listener.listen,
            parsed_upstream,
            Arc::clone(&proxy),
            Arc::clone(&in_flight),
            Arc::clone(&in_flight_notify),
            shutdown_rx.clone(),
        )
        .await?;
        handles.push(handle);
    }

    wait_for_shutdown_signal().await;
    info!("Shutdown signal received");
    let _ = shutdown_tx.send(true);

    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(join_err) => return Err(AppError::ListenerTask(join_err.to_string())),
        }
    }

    let shutdown_timeout = Duration::from_secs(
        config
            .shutdown_timeout_secs
            .unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT_SECS),
    );

    if let Err(_) = timeout(
        shutdown_timeout,
        wait_for_in_flight(Arc::clone(&in_flight), Arc::clone(&in_flight_notify)),
    )
    .await
    {
        warn!(
            "Timed out waiting for in-flight requests to finish after {:?}",
            shutdown_timeout
        );
    }

    info!("Shutdown complete");
    Ok(())
}

fn init_logging(level: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_LEVEL));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_level(true)
        .try_init();
}

fn default_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("retrotls")
            .join("config.yaml");
    }

    PathBuf::from(".config/retrotls/config.yaml")
}

const DEFAULT_CONFIG_CONTENT: &str = r#"access_log: true
listeners:
  - listen: "127.0.0.1:8080"
    upstream: "https://api.example.com"
"#;

fn create_default_config(path: &PathBuf) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::ConfigValidation(format!(
                "Failed to create config directory {}: {}",
                parent.display(),
                e
            ))
        })?;
    }

    std::fs::write(path, DEFAULT_CONFIG_CONTENT).map_err(|e| {
        AppError::ConfigValidation(format!(
            "Failed to write default config to {}: {}",
            path.display(),
            e
        ))
    })?;

    Ok(())
}

fn load_config(path: &PathBuf) -> Result<Config, AppError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("Configuration file not found: {}", path.display());
            eprintln!("Creating default configuration file...");
            create_default_config(path)?;
            eprintln!("Created: {}", path.display());
            eprintln!("Please edit the configuration file and restart RetroTLS.");
            std::process::exit(0);
        }
        Err(source) => {
            return Err(AppError::ConfigRead {
                path: path.clone(),
                source,
            });
        }
    };

    serde_yaml::from_str(&content).map_err(|source| AppError::ConfigParse {
        path: path.clone(),
        source,
    })
}

impl Config {
    fn validate(&self) -> Result<(), AppError> {
        if self.listeners.is_empty() {
            return Err(AppError::ConfigValidation(
                "at least one listener is required".to_string(),
            ));
        }

        for (idx, listener) in self.listeners.iter().enumerate() {
            if listener.upstream.trim().is_empty() {
                return Err(AppError::ConfigValidation(format!(
                    "listeners[{idx}].upstream cannot be empty"
                )));
            }

            parse_upstream(&listener.upstream)?;
        }

        Ok(())
    }
}

impl Proxy {
    fn new(access_log: bool) -> Self {
        let mut root_store = RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs();
        for cert in certs.certs {
            let _ = root_store.add(cert);
        }

        let client_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Self {
            tls_connector: TlsConnector::from(Arc::new(client_config)),
            access_log,
        }
    }

    async fn handle(
        &self,
        req: Request<Incoming>,
        upstream: &Upstream,
        client_addr: SocketAddr,
        local_addr: SocketAddr,
    ) -> Response<Full<Bytes>> {
        let started = Instant::now();
        let method = req.method().clone();
        let path = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());

        let result = self.forward(req, upstream).await;
        let mut response = match result {
            Ok(resp) => resp,
            Err(err) => {
                error!("proxy request failed: {}", err);
                simple_response(StatusCode::BAD_GATEWAY, "Bad Gateway")
            }
        };

        if self.access_log {
            info!(
                "{} {} -> {} {} {} {}ms upstream={}",
                client_addr,
                local_addr,
                method,
                path,
                response.status().as_u16(),
                started.elapsed().as_millis(),
                upstream.base,
            );
        }

        if response.headers().get("connection").is_none() {
            response.headers_mut().remove("connection");
        }

        response
    }

    async fn forward(
        &self,
        req: Request<Incoming>,
        upstream: &Upstream,
    ) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
        let (parts, body) = req.into_parts();
        let body_bytes = body.collect().await?.to_bytes();

        let target_uri = build_target_uri(upstream, parts.uri.path_and_query());

        let stream = TcpStream::connect((upstream.host.as_str(), upstream.port)).await?;
        let server_name = ServerName::try_from(upstream.host.clone())?;
        let tls_stream = self
            .tls_connector
            .connect(server_name, stream)
            .await
            .map_err(|e| format!("tls connect failed: {e}"))?;

        let io = TokioIo::new(tls_stream);
        let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                warn!("upstream connection closed with error: {}", err);
            }
        });

        let mut outbound = Request::builder()
            .method(parts.method)
            .uri(target_uri)
            .version(parts.version)
            .body(Full::new(body_bytes))?;

        *outbound.headers_mut() = parts.headers;
        outbound
            .headers_mut()
            .insert(HOST, upstream.authority.parse()?);

        let upstream_resp = sender.send_request(outbound).await?;
        let (resp_parts, resp_body) = upstream_resp.into_parts();
        let resp_bytes = resp_body.collect().await?.to_bytes();

        Ok(Response::from_parts(resp_parts, Full::new(resp_bytes)))
    }
}

async fn spawn_listener(
    addr: SocketAddr,
    upstream: Upstream,
    proxy: Arc<Proxy>,
    in_flight: Arc<AtomicUsize>,
    in_flight_notify: Arc<Notify>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<JoinHandle<Result<(), AppError>>, AppError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| AppError::BindFailed { addr, source })?;

    info!("Listening on {} -> {}", addr, upstream.base);

    Ok(tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        break;
                    }
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, peer_addr)) => {
                            let proxy = Arc::clone(&proxy);
                            let upstream = upstream.clone();
                            let in_flight = Arc::clone(&in_flight);
                            let in_flight_notify = Arc::clone(&in_flight_notify);
                            let local_addr = addr;

                            tokio::spawn(async move {
                                let _guard = InFlightGuard::new(Arc::clone(&in_flight), Arc::clone(&in_flight_notify));
                                if let Err(err) = serve_connection(stream, peer_addr, local_addr, upstream, proxy).await {
                                    warn!("connection error from {}: {}", peer_addr, err);
                                }
                            });
                        }
                        Err(err) => {
                            warn!("accept failed on {}: {}", addr, err);
                        }
                    }
                }
            }
        }

        Ok(())
    }))
}

async fn serve_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    upstream: Upstream,
    proxy: Arc<Proxy>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let io = TokioIo::new(stream);
    let service = service_fn(move |req: Request<Incoming>| {
        let proxy = Arc::clone(&proxy);
        let upstream = upstream.clone();
        async move {
            let response = proxy.handle(req, &upstream, peer_addr, local_addr).await;
            Ok::<_, Infallible>(response)
        }
    });

    AutoBuilder::new(TokioExecutor::new())
        .serve_connection(io, service)
        .await?;

    Ok(())
}

fn parse_upstream(raw: &str) -> Result<Upstream, AppError> {
    let base = Uri::from_str(raw)
        .map_err(|_| AppError::ConfigValidation(format!("invalid upstream URI: {raw}")))?;

    if base.scheme_str() != Some("https") {
        return Err(AppError::ConfigValidation(format!(
            "upstream must use https scheme: {raw}"
        )));
    }

    let authority = base
        .authority()
        .ok_or_else(|| AppError::ConfigValidation(format!("upstream missing authority: {raw}")))?;

    let authority_str = authority.as_str().to_string();
    let host = authority.host().to_string();
    let port = authority.port_u16().unwrap_or(443);

    Ok(Upstream {
        base,
        authority: authority_str,
        host,
        port,
    })
}

fn build_target_uri(upstream: &Upstream, path_and_query: Option<&http::uri::PathAndQuery>) -> Uri {
    let pq = path_and_query
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    Uri::builder()
        .scheme("https")
        .authority(upstream.authority.as_str())
        .path_and_query(pq)
        .build()
        .unwrap_or_else(|_| Uri::from_static("https://invalid.local/"))
}

fn simple_response(status: StatusCode, message: &'static str) -> Response<Full<Bytes>> {
    let body = if status == StatusCode::BAD_GATEWAY {
        Bytes::from_static(b"Bad Gateway")
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        Bytes::from_static(b"Service Unavailable")
    } else if status == StatusCode::METHOD_NOT_ALLOWED {
        Bytes::from_static(b"Method Not Allowed")
    } else {
        Bytes::from(message.as_bytes().to_vec())
    };

    Response::builder()
        .status(status)
        .body(Full::new(body))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"internal error"))))
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }

    let _ = tokio::signal::ctrl_c().await;
}

async fn wait_for_in_flight(in_flight: Arc<AtomicUsize>, notify: Arc<Notify>) {
    loop {
        if in_flight.load(Ordering::SeqCst) == 0 {
            break;
        }
        notify.notified().await;
    }
}

struct InFlightGuard {
    in_flight: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl InFlightGuard {
    fn new(in_flight: Arc<AtomicUsize>, notify: Arc<Notify>) -> Self {
        in_flight.fetch_add(1, Ordering::SeqCst);
        Self { in_flight, notify }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

#[allow(dead_code)]
fn _method_allowed(_method: &Method) -> bool {
    true
}
