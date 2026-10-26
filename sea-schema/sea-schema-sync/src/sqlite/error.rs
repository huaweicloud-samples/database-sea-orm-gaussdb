use std::num::{ParseFloatError, ParseIntError};

use crate::rusqlite_types::RusqliteError;

/// This type simplifies error handling
pub type DiscoveryResult<T> = Result<T, SqliteDiscoveryError>;

/// All the errors that can be encountered when using this module
#[derive(Debug)]
pub enum SqliteDiscoveryError {
    /// An error parsing a string from the result of an SQLite query into an rust-language integer
    ParseIntError,
    /// An error parsing a string from the result of an SQLite query into an rust-language float
    ParseFloatError,
    /// The target index was not found
    IndexNotFound(String),
    /// The error as defined in [RusqliteError]
    RusqliteError(RusqliteError),
}

impl From<ParseIntError> for SqliteDiscoveryError {
    fn from(_: ParseIntError) -> Self {
        SqliteDiscoveryError::ParseIntError
    }
}

impl From<ParseFloatError> for SqliteDiscoveryError {
    fn from(_: ParseFloatError) -> Self {
        SqliteDiscoveryError::ParseFloatError
    }
}

impl From<RusqliteError> for SqliteDiscoveryError {
    fn from(error: RusqliteError) -> Self {
        SqliteDiscoveryError::RusqliteError(error)
    }
}

impl std::error::Error for SqliteDiscoveryError {}

impl std::fmt::Display for SqliteDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SqliteDiscoveryError::ParseIntError => write!(f, "Parse Integer Error"),
            SqliteDiscoveryError::ParseFloatError => write!(f, "Parse Float Error"),
            SqliteDiscoveryError::IndexNotFound(index) => write!(f, "Index Not Found: {index}"),
            SqliteDiscoveryError::RusqliteError(e) => write!(f, "Rusqlite Error: {e:?}"),
        }
    }
}
