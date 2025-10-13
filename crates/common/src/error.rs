// crates/common/src/error.rs
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    
    #[error("Authentication failed: {0}")]
    Authentication(String),
    
    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),
    
    #[error("Invalid credentials: {0}")]
    InvalidCredentials(String),
    
    #[error("Venue error: {0}")]
    Venue(String),
    
    #[error("Risk check failed: {0}")]
    RiskCheck(String),
    
    #[error("Order rejected: {0}")]
    OrderRejected(String),
    
    #[error("Position not found: {0}")]
    PositionNotFound(String),
    
    #[error("Model error: {0}")]
    Model(String),
    
    #[error("Feature computation error: {0}")]
    Feature(String),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Database error: {0}")]
    Database(String),
    
    #[error("Channel send error")]
    ChannelSend,
    
    #[error("Timeout: {0}")]
    Timeout(String),
    
    #[error("Invalid data: {0}")]
    InvalidData(String),
    
    #[error("Not found: {0}")]
    NotFound(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Http(_) | Error::WebSocket(_) | Error::RateLimit(_) | Error::Timeout(_)
        )
    }
    
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Error::RiskCheck(_) | Error::Authentication(_) | Error::InvalidCredentials(_)
        )
    }
}