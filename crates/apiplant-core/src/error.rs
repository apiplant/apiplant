//! Error type shared across apiplant crates.

use std::path::PathBuf;

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong while loading an app or serving a request.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid TOML in {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid schema for resource `{resource}`: {message}")]
    Schema { resource: String, message: String },

    #[error("database error: {0}")]
    Database(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("{0}")]
    Message(String),
}

impl Error {
    /// The HTTP status this error maps to.
    pub fn status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::Forbidden(_) => 403,
            Error::Unauthorized(_) => 401,
            Error::BadRequest(_) | Error::Schema { .. } => 400,
            _ => 500,
        }
    }
}
