use thiserror::Error;

pub type Result<T> = std::result::Result<T, RizonetError>;

#[derive(Debug, Error)]
pub enum RizonetError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("runtime bootstrap failed: {0}")]
    Runtime(String),

    #[error("webview error: {0}")]
    WebView(#[from] wry::Error),

    #[error("window error: {0}")]
    Window(#[from] tao::error::OsError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },

    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("semver parse error: {0}")]
    SemVer(#[from] semver::Error),

    #[error("updater error: {0}")]
    Updater(String),

    #[error("plugin error ({plugin}): {message}")]
    Plugin { plugin: String, message: String },

    #[error("{0}")]
    Other(String),
}
