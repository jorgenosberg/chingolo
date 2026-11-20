//! API keys live in the macOS Keychain only. They are never logged, stored in SQLite or sent to
//! the frontend. The UI can set, delete and ask whether a key exists.

use crate::error::{AppError, Result};

const SERVICE: &str = "app.chingolo.desktop";

fn entry(provider: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &format!("{provider}-api-key"))
        .map_err(|e| AppError::Keychain(e.to_string()))
}

/// For worker threads. Async code uses `get_key`.
pub fn get_key_blocking(provider: &str) -> Result<Option<String>> {
    match entry(provider)?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::Keychain(e.to_string())),
    }
}

fn set_key_blocking(provider: &str, key: &str) -> Result<()> {
    entry(provider)?
        .set_password(key)
        .map_err(|e| AppError::Keychain(e.to_string()))
}

fn delete_key_blocking(provider: &str) -> Result<()> {
    match entry(provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Keychain(e.to_string())),
    }
}

// Keychain calls can block on a system prompt, so keep them off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

pub async fn get_key(provider: &str) -> Result<Option<String>> {
    let p = provider.to_string();
    blocking(move || get_key_blocking(&p)).await
}

pub async fn set_key(provider: &str, key: &str) -> Result<()> {
    let (p, k) = (provider.to_string(), key.to_string());
    blocking(move || set_key_blocking(&p, &k)).await
}

pub async fn delete_key(provider: &str) -> Result<()> {
    let p = provider.to_string();
    blocking(move || delete_key_blocking(&p)).await
}
