use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("invalid provider URL: {0}")]
    InvalidUrl(String),
    #[error("unsupported website: {0}")]
    UnsupportedWebsite(String),
    #[error("provider and tab URL do not match ({provider} vs {url})")]
    ProviderMismatch { provider: String, url: String },
    #[error("tool request rejected: {0}")]
    InvalidToolRequest(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}
