// placeholder — filled in Task 2
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CharradissaError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("approval timeout for {id}")]
    ApprovalTimeout { id: String },
    #[error("approval rejected: {reason}")]
    ApprovalRejected { reason: String },
    #[error("tool error: {0}")]
    Tool(String),
    #[error("dispatch error: {0}")]
    Dispatch(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CharradissaError>;
