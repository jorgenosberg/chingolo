//! Opt-in reports to a server the user points the app at (usage counts, crashes, bug reports).
//! The contract is in `.context/REPORTING.md`. Off by default. Nothing here knows an address:
//! the URL and token come from Settings and the Keychain. Bodies never carry file names, titles,
//! chat text or keys.

use std::sync::OnceLock;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::domain::{new_id, now_ms};
use crate::error::{AppError, Result};
use crate::persistence::Db;
use crate::persistence::settings as repo;
use crate::security;

pub const URL_KEY: &str = "reporting_url";
pub const USAGE_KEY: &str = "usage_stats";
pub const CRASH_KEY: &str = "crash_reports";
pub const ERROR_KEY: &str = "reporting_error";
const INSTALL_ID_KEY: &str = "install_id";
const SCHEMA: u32 = 1;

/// What a send came to. Clients treat 5xx and network errors as "try later".
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Outcome {
    Sent,
    Retry,
    /// 401. Shown in Settings so the user can fix the token.
    BadToken,
    /// 413 or 422: our bug, resending won't help.
    Drop,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub base: String,
    pub token: String,
}

/// `https://` only, plus `http://` for a local test server. No trailing slash.
pub fn normalize_url(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://").filter(|r| is_local(r)))
        .ok_or_else(|| {
            AppError::Invalid(
                "The address must start with https:// (http:// only for localhost).".into(),
            )
        })?;
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return Err(AppError::Invalid("That isn't a valid address.".into()));
    }
    Ok(url.to_string())
}

fn is_local(rest: &str) -> bool {
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host == "localhost" || host == "127.0.0.1"
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

/// The saved URL and token, or None if either is missing.
pub async fn target(db: &Db) -> Option<Target> {
    let base = db
        .call(|c| repo::get(c, URL_KEY))
        .await
        .ok()
        .flatten()
        .filter(|u| !u.is_empty())?;
    let token = security::get_reporting_token().await.ok().flatten()?;
    Some(Target { base, token })
}

/// `GET /v1/ping`. The error text is for the Settings page.
pub async fn ping(base: &str, token: &str) -> Result<()> {
    let res = client()
        .get(format!("{base}/v1/ping"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| {
            log::warn!("reporting ping failed: {e}");
            AppError::Provider("Couldn't reach the server. Check the address.".into())
        })?;
    match res.status().as_u16() {
        204 | 200 => Ok(()),
        401 => Err(AppError::Provider(
            "The server didn't accept that token.".into(),
        )),
        s => Err(AppError::Provider(format!("The server answered {s}."))),
    }
}

/// Posts one body (already holding the envelope). Records a bad token for Settings, clears it on
/// success.
pub async fn post(db: &Db, t: &Target, path: &str, body: &Value) -> Outcome {
    let outcome = match client()
        .post(format!("{}{path}", t.base))
        .bearer_auth(&t.token)
        .json(body)
        .send()
        .await
    {
        Ok(res) => match res.status().as_u16() {
            200..=299 => Outcome::Sent,
            401 => Outcome::BadToken,
            413 | 422 => {
                log::warn!("reporting {path} rejected with {}", res.status());
                Outcome::Drop
            }
            s => {
                log::info!("reporting {path} answered {s}, will retry");
                Outcome::Retry
            }
        },
        Err(e) => {
            log::info!("reporting {path} failed: {e}, will retry");
            Outcome::Retry
        }
    };
    match outcome {
        Outcome::BadToken => set_error(db, Some("The server didn't accept the saved token.")).await,
        Outcome::Sent => set_error(db, None).await,
        _ => {}
    }
    outcome
}

async fn set_error(db: &Db, msg: Option<&'static str>) {
    let _ = db
        .call(move |c| repo::set(c, ERROR_KEY, msg.unwrap_or("")))
        .await;
}

/// The fields every POST carries. `install_id` is made here on first use, so a bug report sent
/// with both toggles off still has one.
pub fn envelope(c: &Connection) -> Result<Value> {
    let install_id = match repo::get(c, INSTALL_ID_KEY)?.filter(|i| !i.is_empty()) {
        Some(i) => i,
        None => {
            let id = new_id();
            repo::set(c, INSTALL_ID_KEY, &id)?;
            id
        }
    };
    Ok(json!({
        "schema": SCHEMA,
        "install_id": install_id,
        "app_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "os_version": os_version(),
        "arch": std::env::consts::ARCH,
        "sent_at": rfc3339_now(),
    }))
}

/// `envelope` with extra fields merged in.
pub async fn body(db: &Db, extra: Value) -> Result<Value> {
    let mut env = db.call(|c| envelope(c)).await?;
    if let (Some(env), Value::Object(extra)) = (env.as_object_mut(), extra) {
        env.extend(extra);
    }
    Ok(env)
}

#[cfg(target_os = "macos")]
fn os_version() -> String {
    let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    format!("{}.{}", v.majorVersion, v.minorVersion)
}

#[cfg(not(target_os = "macos"))]
fn os_version() -> String {
    String::new()
}

fn rfc3339_now() -> String {
    rfc3339(now_ms())
}

fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let date = crate::services::pricing::civil_date(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{date}T{:02}:{:02}:{:02}Z", t / 3600, t % 3600 / 60, t % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_conn;
    use crate::services::testserver::serve;

    #[test]
    fn urls_must_be_https_or_local() {
        assert_eq!(
            normalize_url(" https://a.example.com/ ").unwrap(),
            "https://a.example.com"
        );
        assert!(normalize_url("http://localhost:8080").is_ok());
        assert!(normalize_url("http://127.0.0.1:1/").is_ok());
        assert!(normalize_url("http://a.example.com").is_err());
        assert!(normalize_url("http://localhost.evil.com").is_err());
        assert!(normalize_url("ftp://a.example.com").is_err());
        assert!(normalize_url("https://").is_err());
        assert!(normalize_url("https://a b.com").is_err());
    }

    #[test]
    fn dates_format_as_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(1_795_392_000_000 + 3_661_000),
            "2026-11-23T01:01:01Z"
        );
    }

    #[test]
    fn the_envelope_has_only_the_contract_fields_and_a_stable_install_id() {
        let c = test_conn();
        let a = envelope(&c).unwrap();
        let b = envelope(&c).unwrap();
        let mut keys: Vec<_> = a.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "app_version",
                "arch",
                "install_id",
                "os",
                "os_version",
                "schema",
                "sent_at"
            ]
        );
        assert_eq!(a["install_id"], b["install_id"]);
        assert_eq!(a["schema"], 1);
    }

    #[tokio::test]
    async fn ping_maps_statuses() {
        let (base, seen) = serve(vec![(204, String::new()), (401, String::new())]);
        assert!(ping(&base, "tok").await.is_ok());
        assert!(ping(&base, "bad").await.is_err());
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.starts_with("GET /v1/ping"));
        assert!(
            seen[0]
                .0
                .to_lowercase()
                .contains("authorization: bearer tok")
        );
    }
}
