use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("stack underflow: expected {expected} value(s), got {actual}")]
    StackUnderflow { expected: usize, actual: usize },

    #[error("type error: expected {expected}, got {actual}")]
    Type { expected: &'static str, actual: &'static str },

    #[error("unbound word: {0}")]
    Unbound(String),

    #[error("arity mismatch: function expects {expected} arg(s), got {actual}")]
    Arity { expected: usize, actual: usize },

    #[error("parse error at {line}:{col}: {msg}")]
    Parse { line: usize, col: usize, msg: String },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
