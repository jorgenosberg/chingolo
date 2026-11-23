//! Crash reports. The panic hook can't use the network (release builds abort), so it writes a
//! small JSON file to `<app data>/crashes/`. The next launch sends the pending files if crash
//! reports are on, and deletes them once the server has them. Frontend errors take the same
//! path, except they are sent right away.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Value, json};

use crate::domain::{new_id, now_ms};
use crate::persistence::Db;
use crate::persistence::settings as repo;
use crate::services::reporting::{self, Outcome, rfc3339};

/// Files kept while crash reports are off, or while the server can't be reached.
const MAX_PENDING: usize = 20;
/// Frontend errors sent per launch, so a render loop can't flood the server.
const MAX_FRONTEND: usize = 10;
const MAX_TEXT: usize = 2_000;
const MAX_BACKTRACE: usize = 20_000;

pub fn dir(app_data: &Path) -> PathBuf {
    app_data.join("crashes")
}

/// Replaces `/Users/<name>` with `~`, for text that is code locations or log lines.
pub fn scrub_home(s: &str) -> String {
    static HOME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/Users/[^/\s]+").expect("regex"));
    HOME.replace_all(s, "~").into_owned()
}

/// For free text such as a panic message, which can quote a path or a file name. File names can
/// hold spaces, so everything from an absolute path to the end of its line is replaced. The text
/// is also cut short.
fn scrub_message(s: &str) -> String {
    static PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/(?:Users|Volumes|private|var|tmp|home)/.*").expect("regex"));
    cut(&PATH.replace_all(s, "<path>"), MAX_TEXT)
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Writes one pending report. Called from the panic hook, so it never panics and does no I/O
/// beyond the file.
pub fn write_crash(dir: &Path, kind: &str, message: &str, location: &str, backtrace: &str) {
    let report = json!({
        "kind": kind,
        "message": scrub_message(message),
        "location": scrub_home(location),
        "backtrace": cut(&scrub_home(backtrace), MAX_BACKTRACE),
        "occurred_at": rfc3339(now_ms()),
        "app_version": env!("CARGO_PKG_VERSION"),
    });
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join(format!("{}.json", new_id())), report.to_string());
    prune(dir);
}

/// Keeps the newest `MAX_PENDING` files. Ids are UUIDv7, so names sort by time.
fn prune(dir: &Path) {
    let mut files = pending(dir);
    while files.len() > MAX_PENDING {
        let _ = std::fs::remove_file(files.remove(0));
    }
}

fn pending(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

pub fn install_hook(dir: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        let message = match info.payload().downcast_ref::<&str>() {
            Some(s) => (*s).to_string(),
            None => info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "panic".into()),
        };
        let location = info.location().map(|l| l.to_string()).unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        write_crash(&dir, "rust_panic", &message, &location, &backtrace);
    }));
}

/// At launch. With crash reports off the files are kept (up to the cap), so turning it on later
/// still sends the recent ones.
pub async fn send_pending(db: Db, dir: PathBuf) {
    prune(&dir);
    let on = db
        .call(|c| repo::get_bool(c, reporting::CRASH_KEY))
        .await
        .unwrap_or(false);
    if !on {
        return;
    }
    if let Some(target) = reporting::target(&db).await {
        send_files(&db, &target, &dir).await;
    }
}

async fn send_files(db: &Db, target: &reporting::Target, dir: &Path) {
    for path in pending(dir) {
        let Some(report) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let Ok(body) = reporting::body(db, report).await else {
            return;
        };
        match reporting::post(db, target, "/v1/crashes", &body).await {
            Outcome::Sent | Outcome::Drop => {
                let _ = std::fs::remove_file(&path);
            }
            Outcome::Retry | Outcome::BadToken => return,
        }
    }
}

/// An error from the webview. Sent now if crash reports are on, kept for the next launch if the
/// server can't be reached, dropped if they are off.
pub async fn report_frontend(
    db: &Db,
    dir: &Path,
    message: &str,
    stack: Option<&str>,
    location: Option<&str>,
) {
    static SENT: AtomicUsize = AtomicUsize::new(0);
    static SEEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
    log::warn!("frontend error: {}", scrub_message(message));
    if !db
        .call(|c| repo::get_bool(c, reporting::CRASH_KEY))
        .await
        .unwrap_or(false)
    {
        return;
    }
    let message = scrub_message(message);
    let fresh = SEEN
        .lock()
        .map(|mut s| s.insert(message.clone()))
        .unwrap_or(false);
    if !fresh || SENT.fetch_add(1, Ordering::Relaxed) >= MAX_FRONTEND {
        return;
    }
    let Some(target) = reporting::target(db).await else {
        return;
    };
    let report = json!({
        "kind": "frontend",
        "message": message,
        "location": scrub_home(location.unwrap_or("")),
        "backtrace": cut(&scrub_home(stack.unwrap_or("")), MAX_BACKTRACE),
        "occurred_at": rfc3339(now_ms()),
        "app_version": env!("CARGO_PKG_VERSION"),
    });
    let Ok(body) = reporting::body(db, report.clone()).await else {
        return;
    };
    if reporting::post(db, &target, "/v1/crashes", &body).await == Outcome::Retry {
        write_crash_json(dir, &report);
    }
}

fn write_crash_json(dir: &Path, report: &Value) {
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(dir.join(format!("{}.json", new_id())), report.to_string());
        prune(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::testserver::serve;

    #[test]
    fn paths_and_home_folders_are_scrubbed() {
        assert_eq!(
            scrub_home("at /Users/anna/.cargo/x.rs:1"),
            "at ~/.cargo/x.rs:1"
        );
        assert_eq!(
            scrub_message("cannot open /Users/anna/Papers/My Thesis.pdf now"),
            "cannot open <path>"
        );
        assert_eq!(
            scrub_message("index 3 out of range"),
            "index 3 out of range"
        );
        assert_eq!(scrub_message(&"x".repeat(5000)).len(), MAX_TEXT);
    }

    #[test]
    fn crash_files_are_capped_and_hold_only_the_report_fields() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..(MAX_PENDING + 3) {
            write_crash(
                tmp.path(),
                "rust_panic",
                &format!("boom {i}"),
                "src/a.rs:1:1",
                "bt",
            );
        }
        let files = pending(tmp.path());
        assert_eq!(files.len(), MAX_PENDING);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "app_version",
                "backtrace",
                "kind",
                "location",
                "message",
                "occurred_at"
            ]
        );
    }

    #[tokio::test]
    async fn files_are_deleted_after_a_204_and_kept_after_a_5xx() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("crashes");
        write_crash(&dir, "rust_panic", "boom", "src/a.rs:1:1", "bt");
        let db = Db::open(&tmp.path().join("t.sqlite")).unwrap();
        let (base, seen) = serve(vec![(503, String::new()), (204, String::new())]);
        let target = reporting::Target {
            base,
            token: "tok".into(),
        };
        send_files(&db, &target, &dir).await;
        assert_eq!(pending(&dir).len(), 1);
        send_files(&db, &target, &dir).await;
        assert!(pending(&dir).is_empty());
        let seen = seen.lock().unwrap();
        assert!(seen[1].0.starts_with("POST /v1/crashes"));
        let sent: Value = serde_json::from_str(&seen[1].1).unwrap();
        assert_eq!(sent["kind"], "rust_panic");
        assert_eq!(sent["schema"], 1);
        assert!(sent["install_id"].is_string());
        assert_eq!(sent["message"], "boom");
    }
}
