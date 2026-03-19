use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

const VERSION: &str = "1.1.0";
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

fn retrotls_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".retrotls")
    } else {
        PathBuf::from(".retrotls")
    }
}

fn pid_file_path() -> PathBuf {
    retrotls_home().join("retrotls.pid")
}

fn write_pid_file(pid: u32) -> Result<(), std::io::Error> {
    let pid_path = pid_file_path();
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, pid.to_string())
}

fn read_pid_file() -> Option<u32> {
    let pid_path = pid_file_path();
    let content = std::fs::read_to_string(&pid_path).ok()?;
    content.trim().parse().ok()
}

fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_file_path());
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        use std::process::Command;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

#[derive(Parser)]
#[command(name = "retrotls")]
#[command(about = "Ultra-lightweight HTTP to HTTPS bridge proxy")]
struct Cli {
    #[arg(value_name = "COMMAND")]
    command: Option<String>,

    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(long)]
    check: bool,

    #[arg(long)]
    version: bool,

    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,

    #[arg(long)]
    foreground: bool,
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

    #[error("Daemon already running (PID: {0})")]
    AlreadyRunning(u32),

    #[error("Daemon not running")]
    NotRunning,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

fn daemonize() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::process::Command;
        use std::os::unix::process::CommandExt;
        
        let args: Vec<String> = std::env::args()
            .skip(1)
            .filter(|arg| arg != "--foreground" && arg != "-f")
            .chain(std::iter::once("--foreground".to_string()))
            .collect();
        
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        
        cmd.spawn()?;
        std::process::exit(0);
    }
    
    #[cfg(not(unix))]
    {
        Ok(())
    }
}

fn stop_daemon() -> Result<(), AppError> {
    let pid = read_pid_file().ok_or(AppError::NotRunning)?;
    
    if !is_process_running(pid) {
        remove_pid_file();
        return Err(AppError::NotRunning);
    }
    
    #[cfg(unix)]
    {
        unsafe {
            if libc::kill(pid as i32, libc::SIGTERM) != 0 {
                return Err(AppError::ConfigValidation(format!(
                    "Failed to send signal to process {}",
                    pid
                )));
            }
        }
    }
    
    #[cfg(windows)]
    {
        use std::process::Command;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }
    
    println!("RetroTLS daemon stopped (PID: {})", pid);
    remove_pid_file();
    Ok(())
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

    if let Some(cmd) = cli.command.as_deref() {
        match cmd {
            "stop" => {
                return stop_daemon();
            }
            "start" => {
                // continue to start daemon
            }
            _ => {
                eprintln!("Unknown command: {}", cmd);
                eprintln!("Usage: retrotls [start|stop] [options]");
                std::process::exit(1);
            }
        }
    }

    let config_path = cli.config.unwrap_or_else(default_config_path);

    if cli.check {
        let config = load_config(&config_path)?;
        config.validate()?;
        println!("Configuration valid");
        return Ok(());
    }

    // Check if already running
    if let Some(pid) = read_pid_file() {
        if is_process_running(pid) {
            return Err(AppError::AlreadyRunning(pid));
        }
        remove_pid_file();
    }

    // Daemonize unless --foreground is specified
    if !cli.foreground {
        write_pid_file(std::process::id())?;
        daemonize()?;
        return Ok(());
    }

    // Write PID file for foreground mode too (for stop command)
    write_pid_file(std::process::id())?;

    let log_level = cli.log_level.as_deref().unwrap_or(DEFAULT_LOG_LEVEL);
    init_logging(log_level);

    info!("RetroTLS v{VERSION} starting...");

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

    remove_pid_file();
    info!("Shutdown complete");
    Ok(())
}

fn log_file_path() -> PathBuf {
    retrotls_home().join("logs").join("retrotls.log")
}

fn init_logging(level: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_LEVEL));

    let log_path = log_file_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(true)
                .with_level(true)
                .with_writer(std::sync::Arc::new(file))
                .try_init();
        }
        Err(e) => {
            eprintln!("Failed to open log file {}: {}", log_path.display(), e);
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(true)
                .with_level(true)
                .try_init();
        }
    };
}

fn default_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".retrotls")
            .join("config.yaml");
    }

    PathBuf::from(".retrotls/config.yaml")
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
