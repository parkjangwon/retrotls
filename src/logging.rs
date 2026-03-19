use std::fmt::Display;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static ACCESS_LOG_ENABLED: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone)]
pub struct AccessLogEvent {
    pub timestamp: DateTime<Utc>,
    pub client_addr: SocketAddr,
    pub local_bind: SocketAddr,
    pub method: String,
    pub path: String,
    pub upstream: String,
    pub status_code: u16,
    pub latency_ms: u64,
}

pub fn init_logging(level: &str, access_log_enabled: bool) {
    ACCESS_LOG_ENABLED.store(access_log_enabled, Ordering::Relaxed);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_ansi(true);

    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .try_init();
}

pub fn log_access(event: &AccessLogEvent) {
    if !ACCESS_LOG_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    let ts = event.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);

    info!(
        target: "retrotls::logging",
        "{} {} -> {} {} {} {} {}ms upstream={}",
        ts,
        event.client_addr,
        event.local_bind,
        event.method,
        event.path,
        event.status_code,
        event.latency_ms,
        event.upstream,
    );
}

pub fn log_startup(bind_addr: SocketAddr, upstream_mappings: &[(String, String)]) {
    info!(target: "retrotls::logging", "RetroTLS starting on {}", bind_addr);

    if upstream_mappings.is_empty() {
        info!(target: "retrotls::logging", "No upstream mappings configured");
        return;
    }

    for (source, upstream) in upstream_mappings {
        info!(
            target: "retrotls::logging",
            "Upstream mapping: {} -> {}",
            source,
            upstream,
        );
    }
}

pub fn log_error(context: &str, err: impl Display) {
    error!(target: "retrotls::logging", "{}: {}", context, err);
}

pub fn log_shutdown(reason: &str) {
    info!(target: "retrotls::logging", "RetroTLS shutting down: {}", reason);
}
