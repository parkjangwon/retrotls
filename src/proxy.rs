use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{CONNECTION, HOST, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rustls::ClientConfig as RustlsClientConfig;
use tracing::{info, warn};

use crate::config::{Config, TlsMinVersion};
use crate::error::AppError;
use crate::http;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type BoxBody = http_body_util::combinators::BoxBody<Bytes, BoxError>;

pub struct Proxy {
    pub config: Config,
    pub client: Client<hyper_rustls::HttpsConnector<HttpConnector>, BoxBody>,
    pub tls_config: Arc<RustlsClientConfig>,
}

impl Proxy {
    pub fn new(config: Config) -> Result<Self, AppError> {
        let tls_config = Arc::new(build_tls_config(&config)?);

        let mut http_connector = HttpConnector::new();
        http_connector.enforce_http(false);
        http_connector.set_nodelay(true);
        http_connector.set_connect_timeout(Some(Duration::from_millis(config.connect_timeout_ms)));

        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*tls_config).clone())
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .wrap_connector(http_connector);

        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_millis(config.pool_idle_timeout_ms))
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .build(https_connector);

        Ok(Self {
            config,
            client,
            tls_config,
        })
    }

    pub async fn handle_request(
        &self,
        req: Request<Incoming>,
        client_addr: SocketAddr,
        bind_addr: SocketAddr,
        upstream: &str,
    ) -> Result<Response<BoxBody>, AppError> {
        let started_at = Instant::now();

        let (parts, body) = req.into_parts();
        let method = parts.method.clone();
        let path = parts.uri.path().to_owned();
        let query = parts.uri.query().map(ToOwned::to_owned);

        let upstream_uri = match http::build_upstream_url(upstream, &path, query.as_deref()) {
            Ok(uri) => uri,
            Err(err) => {
                warn!(error = %err, path = %path, "invalid upstream request url");
                return Ok(error_response(StatusCode::BAD_REQUEST, "bad request"));
            }
        };

        let mut req_builder = Request::builder()
            .method(method.clone())
            .uri(upstream_uri)
            .version(parts.version);

        if let Some(headers) = req_builder.headers_mut() {
            copy_request_headers(headers, &parts.headers, upstream)?;
            apply_forwarding_headers(headers, &parts.headers, client_addr, bind_addr)?;
        }

        let upstream_body = body.map_err(|e| -> BoxError { Box::new(e) }).boxed();
        let upstream_req = match req_builder.body(upstream_body) {
            Ok(request) => request,
            Err(err) => {
                warn!(error = %err, "failed to build upstream request");
                return Ok(error_response(StatusCode::BAD_REQUEST, "bad request"));
            }
        };

        let upstream_res = match tokio::time::timeout(
            Duration::from_millis(self.config.request_timeout_ms),
            self.client.request(upstream_req),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                warn!(error = %err, "upstream connection failed");
                return Ok(map_upstream_client_error(err));
            }
            Err(_) => {
                return Ok(error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "gateway timeout",
                ));
            }
        };

        let status = upstream_res.status();
        let version = upstream_res.version();
        let upstream_headers = upstream_res.headers().clone();
        let response_body = upstream_res
            .into_body()
            .map_err(|e| -> BoxError { Box::new(e) })
            .boxed();

        let mut response_builder = Response::builder().status(status).version(version);
        if let Some(headers) = response_builder.headers_mut() {
            copy_response_headers(headers, &upstream_headers);
        }

        let response = match response_builder.body(response_body) {
            Ok(resp) => resp,
            Err(err) => {
                warn!(error = %err, "failed to build downstream response");
                error_response(StatusCode::BAD_GATEWAY, "bad gateway")
            }
        };

        let latency_ms = started_at.elapsed().as_millis();
        info!(
            client_addr = %client_addr,
            bind_addr = %bind_addr,
            upstream = %upstream,
            method = %method,
            path = %path,
            status = response.status().as_u16(),
            latency_ms = latency_ms,
            "proxy access"
        );

        Ok(response)
    }
}

fn build_tls_config(config: &Config) -> Result<RustlsClientConfig, AppError> {
    let mut root_store = rustls::RootCertStore::empty();

    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = root_store.add(cert);
    }

    let mut tls_config = RustlsClientConfig::builder_with_protocol_versions(match config.min_tls_version {
        TlsMinVersion::V1_2 => &[&rustls::version::TLS12, &rustls::version::TLS13],
        TlsMinVersion::V1_3 => &[&rustls::version::TLS13],
    })
    .with_root_certificates(root_store)
    .with_no_client_auth();

    if config.insecure_disable_certificate_verification {
        tls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoCertificateVerification));
    }

    Ok(tls_config)
}

fn copy_request_headers(
    dst: &mut HeaderMap<HeaderValue>,
    src: &HeaderMap<HeaderValue>,
    upstream: &str,
) -> Result<(), AppError> {
    for (name, value) in src {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        dst.append(name.clone(), value.clone());
    }

    let host = match Uri::try_from(upstream) {
        Ok(uri) => uri
            .authority()
            .map(|a| a.as_str().to_owned())
            .unwrap_or_else(|| upstream.to_owned()),
        Err(_) => upstream.to_owned(),
    };

    if let Ok(host_header) = HeaderValue::from_str(&host) {
        dst.insert(HOST, host_header);
    }

    Ok(())
}

fn apply_forwarding_headers(
    dst: &mut HeaderMap<HeaderValue>,
    src: &HeaderMap<HeaderValue>,
    client_addr: SocketAddr,
    bind_addr: SocketAddr,
) -> Result<(), AppError> {
    let forwarded_for = append_csv_header_value(src.get("x-forwarded-for"), &client_addr.ip().to_string());
    let forwarded_proto = HeaderValue::from_static("http");
    let forwarded_host = src
        .get(HOST)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_str(&bind_addr.to_string()).unwrap_or_else(|_| HeaderValue::from_static("unknown")));

    dst.insert("x-forwarded-for", forwarded_for);
    dst.insert("x-forwarded-proto", forwarded_proto);
    dst.insert("x-forwarded-host", forwarded_host);

    Ok(())
}

fn copy_response_headers(dst: &mut HeaderMap<HeaderValue>, src: &HeaderMap<HeaderValue>) {
    let connection_tokens = connection_header_tokens(src.get(CONNECTION));

    for (name, value) in src {
        let lower = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop(&lower) || connection_tokens.contains(&lower) {
            continue;
        }
        dst.append(name.clone(), value.clone());
    }
}

fn map_upstream_client_error(err: hyper_util::client::legacy::Error) -> Response<BoxBody> {
    if err.is_timeout() {
        return error_response(StatusCode::GATEWAY_TIMEOUT, "gateway timeout");
    }
    error_response(StatusCode::BAD_GATEWAY, "bad gateway")
}

fn append_csv_header_value(existing: Option<&HeaderValue>, value: &str) -> HeaderValue {
    let mut out = existing
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_default();

    if !out.is_empty() {
        out.push_str(", ");
    }
    out.push_str(value);

    HeaderValue::from_str(&out).unwrap_or_else(|_| HeaderValue::from_static("unknown"))
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) || name.eq_ignore_ascii_case(PROXY_AUTHENTICATE.as_str())
        || name.eq_ignore_ascii_case(PROXY_AUTHORIZATION.as_str())
        || name.eq_ignore_ascii_case(TE.as_str())
        || name.eq_ignore_ascii_case(TRAILER.as_str())
        || name.eq_ignore_ascii_case(TRANSFER_ENCODING.as_str())
        || name.eq_ignore_ascii_case(UPGRADE.as_str())
}

fn connection_header_tokens(header: Option<&HeaderValue>) -> Vec<String> {
    let Some(value) = header else {
        return Vec::new();
    };

    let Ok(text) = value.to_str() else {
        return Vec::new();
    };

    text.split(',')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| !token.is_empty())
        .collect()
}

fn error_response(status: StatusCode, message: &'static str) -> Response<BoxBody> {
    let body = Full::new(Bytes::from_static(message.as_bytes()))
        .map_err(|never: Infallible| match never {})
        .boxed();

    Response::builder()
        .status(status)
        .body(body)
        .unwrap_or_else(|_| {
            let fallback = Full::new(Bytes::from_static(b"internal server error"))
                .map_err(|never: Infallible| match never {})
                .boxed();
            Response::new(fallback)
        })
}

#[derive(Debug)]
struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}
