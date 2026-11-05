use serde::Serialize;
use specta::Type;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("Not found")]
    NotFound,
    #[error("No API key is set for {0}. Add one in Settings.")]
    NoApiKey(String),
    #[error("Keychain error: {0}")]
    Keychain(String),
    #[error("{0}")]
    Provider(String),
    #[error("Cancelled")]
    Cancelled,
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, AppError>;

/// What the frontend sees: a plain message string.
#[derive(Debug, Serialize, Type)]
#[serde(transparent)]
pub struct CmdError(String);

impl From<AppError> for CmdError {
    fn from(e: AppError) -> Self {
        // Provider and keychain errors never contain the API key (it is only sent as a header).
        CmdError(e.to_string())
    }
}

pub type CmdResult<T> = std::result::Result<T, CmdError>;
