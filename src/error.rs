use http::StatusCode;
use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Failed to load config from {path}: {source}")]
    ConfigLoadError { path: String, source: String },

    #[error("Configuration validation error: {message}")]
    ConfigValidationError { message: String },

    #[error("Failed to connect to upstream {url}: {source}")]
    UpstreamConnectionError { url: String, source: String },

    #[error("Upstream timeout: {url}")]
    UpstreamTimeoutError { url: String },

    #[error("Invalid request: {message}")]
    InvalidRequestError { message: String },

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("YAML parsing error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] http::Error),

    #[error("TLS error: {0}")]
    TlsError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl AppError {
    pub fn to_status_code(&self) -> StatusCode {
        match self {
            AppError::ConfigLoadError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::ConfigValidationError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::UpstreamConnectionError { .. } => StatusCode::BAD_GATEWAY,
            AppError::UpstreamTimeoutError { .. } => StatusCode::GATEWAY_TIMEOUT,
            AppError::InvalidRequestError { .. } => StatusCode::BAD_REQUEST,
            AppError::IoError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::YamlError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::HttpError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::TlsError(_) => StatusCode::BAD_GATEWAY,
            AppError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn is_user_error(&self) -> bool {
        matches!(
            self,
            AppError::InvalidRequestError { .. } | AppError::ConfigValidationError { .. }
        )
    }

    pub fn config_load(path: impl fmt::Display, source: impl fmt::Display) -> Self {
        AppError::ConfigLoadError {
            path: path.to_string(),
            source: source.to_string(),
        }
    }

    pub fn config_validation(message: impl fmt::Display) -> Self {
        AppError::ConfigValidationError {
            message: message.to_string(),
        }
    }

    pub fn upstream_connection(url: impl fmt::Display, source: impl fmt::Display) -> Self {
        AppError::UpstreamConnectionError {
            url: url.to_string(),
            source: source.to_string(),
        }
    }

    pub fn upstream_timeout(url: impl fmt::Display) -> Self {
        AppError::UpstreamTimeoutError {
            url: url.to_string(),
        }
    }

    pub fn invalid_request(message: impl fmt::Display) -> Self {
        AppError::InvalidRequestError {
            message: message.to_string(),
        }
    }

    pub fn internal(message: impl fmt::Display) -> Self {
        AppError::InternalError(message.to_string())
    }

    pub fn tls(message: impl fmt::Display) -> Self {
        AppError::TlsError(message.to_string())
    }
}
