//! Opt-in usage counts. Events come from a fixed enum, so no text of the user's can get in. The
//! only extras are the provider id, the model id and a short bucket. Nothing is stored while the
//! setting is off. Rows are cumulative per local day; a flush sends all of them and deletes the
//! days before today once the server has acknowledged. See `.context/REPORTING.md`.

use std::time::Duration;

use rusqlite::{Connection, params};
use serde_json::json;

use crate::error::Result;
use crate::persistence::Db;
use crate::persistence::settings as repo;
use crate::services::reporting::{self, Outcome};

const FIRST_FLUSH: Duration = Duration::from_secs(60);
const FLUSH_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, specta::Type, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    AppOpened,
    ChatSent,
    DocumentImported,
    /// Bucket: read, layout, details or no_text.
    ImportFailed,
    /// Bucket: reply, selection or passage.
    SavedItemCreated,
    ReaderOpened,
    /// Bucket: tool rounds in one reply.
    ToolRounds,
    RerankUsed,
    /// Bucket: auth, rate_limit, server, network, too_long or other.
    ProviderError,
    ContextOver80Percent,
    ContextNearPriceJump,
    ContextOverPriceJump,
    /// Bucket: the size class a conversation has reached (counted once per class).
    ConversationPeakTokens,
    /// A Gemini request over 4,096 input tokens. Bucket: hit or miss.
    GeminiCacheOver4k,
    /// Bucket: after_warning or no_warning.
    HandoffUsed,
    ForkUsed,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::AppOpened => "app_opened",
            Event::ChatSent => "chat_sent",
            Event::DocumentImported => "document_imported",
            Event::ImportFailed => "import_failed",
            Event::SavedItemCreated => "saved_item_created",
            Event::ReaderOpened => "reader_opened",
            Event::ToolRounds => "tool_rounds",
            Event::RerankUsed => "rerank_used",
            Event::ProviderError => "provider_error",
            Event::ContextOver80Percent => "context_over_80_percent",
            Event::ContextNearPriceJump => "context_near_price_jump",
            Event::ContextOverPriceJump => "context_over_price_jump",
            Event::ConversationPeakTokens => "conversation_peak_tokens",
            Event::GeminiCacheOver4k => "gemini_cache_over_4k",
            Event::HandoffUsed => "handoff_used",
            Event::ForkUsed => "fork_used",
        }
    }
}

/// Flags that mean a warning showed in the chat header.
pub const WARNING_FLAGS: [&str; 3] = [
    "context_over_80_percent",
    "context_near_price_jump",
    "context_over_price_jump",
];

pub fn size_bucket(tokens: u32) -> &'static str {
    match tokens {
        0..10_000 => "<10k",
        10_000..50_000 => "10-50k",
        50_000..100_000 => "50-100k",
        100_000..200_000 => "100-200k",
        _ => "200k+",
    }
}

/// Buckets from the user-facing text our provider code produces. The messages are ours
/// (`describe_error` and `net_error` in `services/ai`), plus the provider's own wording for an
/// input that is too long. Gemini: "The input token count (N) exceeds the maximum number of
/// tokens allowed (M)". OpenAI: "maximum context length" or "exceeds the context window"
/// (not checked against a live rejection).
pub fn classify_error(message: &str) -> &'static str {
    let m = message.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    if has(&[
        "input token count",
        "maximum number of tokens",
        "more than the max tokens",
        "maximum context length",
        "context window",
        "context_length_exceeded",
    ]) {
        "too_long"
    } else if has(&["rejected the api key", "no api key"]) {
        "auth"
    } else if has(&["rate limit", "quota", "out of credit"]) {
        "rate_limit"
    } else if has(&[
        "couldn't reach",
        "timed out",
        "network error",
        "stopped responding",
        "connection ended",
    ]) {
        "network"
    } else if has(&["error 5", "overloaded", "unavailable"]) {
        "server"
    } else {
        "other"
    }
}

pub fn enabled(c: &Connection) -> bool {
    repo::get_bool(c, reporting::USAGE_KEY).unwrap_or(false)
}

/// Adds one to today's row. Does nothing while usage statistics are off. Errors are logged, a
/// missed count never fails what the user was doing.
pub fn count_conn(c: &Connection, event: Event, provider: &str, model: &str, bucket: &str) {
    if !enabled(c) {
        return;
    }
    let done = c.execute(
        "INSERT INTO usage_counts (day, event, provider, model, bucket, count)
         VALUES (date('now', 'localtime'), ?1, ?2, ?3, ?4, 1)
         ON CONFLICT(day, event, provider, model, bucket) DO UPDATE SET count = count + 1",
        params![event.name(), provider, model, bucket],
    );
    if let Err(e) = done {
        log::warn!("couldn't count {}: {e}", event.name());
    }
}

pub async fn count(db: &Db, event: Event, provider: &str, model: &str, bucket: &str) {
    let (p, m, b) = (provider.to_string(), model.to_string(), bucket.to_string());
    let _ = db
        .call(move |c| {
            count_conn(c, event, &p, &m, &b);
            Ok(())
        })
        .await;
}

/// True the first time a conversation reaches `flag`, and only while counting is on.
pub fn flag_once(c: &Connection, conversation_id: &str, flag: &str) -> Result<bool> {
    if !enabled(c) {
        return Ok(false);
    }
    Ok(c.execute(
        "INSERT OR IGNORE INTO conversation_flags (conversation_id, flag) VALUES (?1, ?2)",
        params![conversation_id, flag],
    )? > 0)
}

/// Whether a warning has shown in this conversation.
pub fn was_warned(c: &Connection, conversation_id: &str) -> bool {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_flags WHERE conversation_id = ?1 AND flag IN (?2, ?3, ?4))",
        params![conversation_id, WARNING_FLAGS[0], WARNING_FLAGS[1], WARNING_FLAGS[2]],
        |r| r.get(0),
    )
    .unwrap_or(false)
}

/// Counts the events a finished reply's input size gives rise to, once per conversation each.
pub fn count_context(
    c: &Connection,
    conversation_id: &str,
    provider: &str,
    model: &str,
    input_tokens: u32,
    limit: Option<u32>,
    price_jump_over: Option<u32>,
) {
    let once = |flag: &str, event: Event, bucket: &str| {
        if flag_once(c, conversation_id, flag).unwrap_or(false) {
            count_conn(c, event, provider, model, bucket);
        }
    };
    let size = size_bucket(input_tokens);
    once(&format!("peak:{size}"), Event::ConversationPeakTokens, size);
    if limit.is_some_and(|l| u64::from(input_tokens) * 100 >= u64::from(l) * 80) {
        once("context_over_80_percent", Event::ContextOver80Percent, "");
    }
    if let Some(over) = price_jump_over {
        if u64::from(input_tokens) * 100 >= u64::from(over) * 85 {
            once("context_near_price_jump", Event::ContextNearPriceJump, "");
        }
        if input_tokens > over {
            once("context_over_price_jump", Event::ContextOverPriceJump, "");
        }
    }
}

pub fn clear(c: &Connection) -> Result<()> {
    c.execute("DELETE FROM usage_counts", [])?;
    c.execute("DELETE FROM conversation_flags", [])?;
    Ok(())
}

/// Sends every row as a running total. Returns what the server said.
pub async fn flush(db: &Db) -> Option<Outcome> {
    if !db.call(|c| Ok(enabled(c))).await.unwrap_or(false) {
        return None;
    }
    let target = reporting::target(db).await?;
    flush_to(db, &target).await
}

async fn flush_to(db: &Db, target: &reporting::Target) -> Option<Outcome> {
    let rows = db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT day, event, provider, model, bucket, count FROM usage_counts ORDER BY day, event",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let mut row = json!({
                        "day": r.get::<_, String>(0)?,
                        "event": r.get::<_, String>(1)?,
                        "count": r.get::<_, i64>(5)?,
                    });
                    for (i, key) in [(2, "provider"), (3, "model"), (4, "bucket")] {
                        let v: String = r.get(i)?;
                        if !v.is_empty() {
                            row[key] = v.into();
                        }
                    }
                    Ok(row)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .ok()?;
    if rows.is_empty() {
        return None;
    }
    let body = reporting::body(db, json!({ "counts": rows })).await.ok()?;
    let outcome = reporting::post(db, target, "/v1/usage", &body).await;
    if matches!(outcome, Outcome::Sent | Outcome::Drop) {
        // Today's rows stay: they are running totals and the next send replaces them.
        let _ = db
            .call(move |c| {
                if outcome == Outcome::Drop {
                    c.execute("DELETE FROM usage_counts", [])?;
                } else {
                    c.execute(
                        "DELETE FROM usage_counts WHERE day < date('now', 'localtime')",
                        [],
                    )?;
                }
                Ok(())
            })
            .await;
    }
    Some(outcome)
}

/// A shortly after launch, then every few hours while the app is open.
pub async fn keep_flushing(db: Db) {
    tokio::time::sleep(FIRST_FLUSH).await;
    loop {
        flush(&db).await;
        tokio::time::sleep(FLUSH_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_conn;
    use crate::services::testserver::serve;

    fn on(c: &Connection) {
        repo::set_bool(c, reporting::USAGE_KEY, true).unwrap();
    }

    fn total(c: &Connection) -> i64 {
        c.query_row(
            "SELECT COALESCE(SUM(count), 0) FROM usage_counts",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn nothing_is_stored_while_off_and_counts_add_up_while_on() {
        let c = test_conn();
        count_conn(&c, Event::ChatSent, "gemini", "m", "");
        assert_eq!(total(&c), 0);
        on(&c);
        count_conn(&c, Event::ChatSent, "gemini", "m", "");
        count_conn(&c, Event::ChatSent, "gemini", "m", "");
        count_conn(&c, Event::ChatSent, "openai", "m", "");
        assert_eq!(total(&c), 3);
        let rows: i64 = c
            .query_row("SELECT COUNT(*) FROM usage_counts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[test]
    fn long_chat_events_count_once_per_conversation() {
        let c = test_conn();
        on(&c);
        c.execute(
            "INSERT INTO conversations (id, title, provider, created_at, updated_at) VALUES ('c1', 't', 'gemini', 0, 0)",
            [],
        )
        .unwrap();
        let go = |tokens| {
            count_context(
                &c,
                "c1",
                "gemini",
                "m",
                tokens,
                Some(100_000),
                Some(200_000),
            )
        };
        go(85_000);
        go(90_000);
        go(170_000);
        go(250_000);
        let n = |event: &str| -> i64 {
            c.query_row(
                "SELECT COALESCE(SUM(count), 0) FROM usage_counts WHERE event = ?1",
                [event],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(n("context_over_80_percent"), 1);
        assert_eq!(n("context_near_price_jump"), 1);
        assert_eq!(n("context_over_price_jump"), 1);
        // 50-100k, then 100-200k, then 200k+: each class once.
        assert_eq!(n("conversation_peak_tokens"), 3);
        assert!(was_warned(&c, "c1"));
        assert!(!was_warned(&c, "other"));
    }

    #[test]
    fn errors_and_sizes_are_bucketed() {
        assert_eq!(
            classify_error(
                "Gemini error 400: The input token count (35868) exceeds the maximum number of tokens allowed (32768)."
            ),
            "too_long"
        );
        assert_eq!(classify_error("OpenAI rate limit reached."), "rate_limit");
        assert_eq!(classify_error("OpenAI rejected the API key."), "auth");
        assert_eq!(
            classify_error("Couldn't reach Gemini. Check your internet connection."),
            "network"
        );
        assert_eq!(classify_error("Gemini error 503: overloaded"), "server");
        assert_eq!(classify_error("something odd"), "other");
        assert_eq!(size_bucket(9_999), "<10k");
        assert_eq!(size_bucket(10_000), "10-50k");
        assert_eq!(size_bucket(200_000), "200k+");
    }

    #[tokio::test]
    async fn a_flush_sends_running_totals_and_drops_only_past_days_after_a_204() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.sqlite")).unwrap();
        db.call(|c| {
            on(c);
            c.execute(
                "INSERT INTO usage_counts VALUES ('2020-01-01', 'chat_sent', 'gemini', 'm', '', 4),
                 (date('now', 'localtime'), 'chat_sent', '', '', '', 2)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let (base, seen) = serve(vec![(503, String::new()), (204, String::new())]);
        let t = reporting::Target {
            base,
            token: "tok".into(),
        };
        let rows = |db: &Db| {
            let db = db.clone();
            async move {
                db.call(|c| {
                    Ok(c.query_row("SELECT COUNT(*) FROM usage_counts", [], |r| {
                        r.get::<_, i64>(0)
                    })?)
                })
                .await
                .unwrap()
            }
        };
        assert_eq!(flush_to(&db, &t).await, Some(Outcome::Retry));
        assert_eq!(rows(&db).await, 2);
        assert_eq!(flush_to(&db, &t).await, Some(Outcome::Sent));
        assert_eq!(rows(&db).await, 1);
        let seen = seen.lock().unwrap();
        assert!(seen[1].0.starts_with("POST /v1/usage"));
        let sent: serde_json::Value = serde_json::from_str(&seen[1].1).unwrap();
        let counts = sent["counts"].as_array().unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0]["day"], "2020-01-01");
        assert_eq!(counts[0]["provider"], "gemini");
        assert_eq!(counts[0]["count"], 4);
        // empty extras are left out
        assert!(counts[1].get("provider").is_none());
        assert_eq!(sent["schema"], 1);
    }
}
