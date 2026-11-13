//! Estimated cost of an API call.
//!
//! Prices are a trimmed copy of models.dev (MIT, open data), USD per 1M tokens. A copy is bundled
//! at build time (`data/prices.json`), and the app fetches a fresh one about once a week into its
//! data folder, using whichever is newer. Models that aren't in the table have no cost, and the
//! usage screen says so. Google has no usage API for API keys, so the real bill is on AI Studio's
//! Usage page.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::domain::{ModelFacts, now_ms};
use crate::error::{AppError, Result};
use crate::persistence::{Db, settings};

const BUNDLED: &str = include_str!("../../data/prices.json");
pub const FEED: &str = "https://models.dev/api.json";
/// The refreshed copy, in the app's data folder.
const SAVED: &str = "prices.json";
const CHECKED_KEY: &str = "prices_checked_at";
pub const AUTO_KEY: &str = "prices_auto_update";
const REFRESH_EVERY_MS: i64 = 7 * 24 * 3_600_000;
const MAX_FEED_BYTES: usize = 40_000_000;
/// Variants that bill differently from the base model, so a prefix match must not reach them.
const NOT_BASE: &[&str] = &["tts", "image", "live", "audio", "robotics", "translate", "embedding"];

#[derive(Debug, Deserialize)]
struct Tier {
    over: u32,
    input: f64,
    output: f64,
    cache_read: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Price {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    #[serde(default)]
    tiers: Vec<Tier>,
    /// Context window in tokens, when the feed has one.
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    knowledge: Option<String>,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    tool_call: bool,
    #[serde(default)]
    image_input: bool,
    #[serde(default)]
    pdf_input: bool,
    #[serde(default)]
    deprecated: bool,
}

#[derive(Debug, Deserialize)]
pub struct Table {
    pub source: String,
    #[serde(rename = "fetchedAt")]
    pub fetched_at: String,
    providers: HashMap<String, HashMap<String, Price>>,
}

impl Table {
    pub fn parse(json: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    fn find(&self, provider: &str, model: &str) -> Option<&Price> {
        let model = model.strip_prefix("models/").unwrap_or(model);
        let models = self.providers.get(provider)?;
        if let Some(p) = models.get(model) {
            return Some(p);
        }
        // A dated or "-latest" variant of a base model. Never the special variants.
        models
            .iter()
            .filter(|(id, _)| {
                model.strip_prefix(id.as_str()).is_some_and(|rest| {
                    rest.starts_with('-') && !NOT_BASE.iter().any(|w| rest.contains(w))
                })
            })
            .max_by_key(|(id, _)| id.len())
            .map(|(_, p)| p)
    }

    /// The model's context window, for providers whose model list doesn't say.
    pub fn context_limit(&self, provider: &str, model: &str) -> Option<u32> {
        self.find(provider, model)?.context.and_then(|c| u32::try_from(c).ok())
    }

    /// What the table knows about a model, matched like `cost` (a dated variant finds its base).
    pub fn facts(&self, provider: &str, model: &str) -> Option<ModelFacts> {
        let p = self.find(provider, model)?;
        Some(ModelFacts {
            name: p.name.clone(),
            input_price: p.input,
            output_price: p.output,
            cached_price: p.cache_read,
            release_date: p.release_date.clone(),
            knowledge_cutoff: p.knowledge.clone(),
            reasoning: p.reasoning,
            tool_calls: p.tool_call,
            image_input: p.image_input,
            pdf_input: p.pdf_input,
            deprecated: p.deprecated,
        })
    }

    /// `input` includes the cached part. `output` includes thinking tokens.
    pub fn cost(&self, provider: &str, model: &str, input: u32, output: u32, cached: u32) -> Option<f64> {
        let price = self.find(provider, model)?;
        let (pin, pout, pcached) = match price.tiers.iter().filter(|t| input > t.over).max_by_key(|t| t.over) {
            Some(t) => (t.input, t.output, t.cache_read.unwrap_or(t.input)),
            None => (price.input, price.output, price.cache_read.unwrap_or(price.input)),
        };
        let cached = cached.min(input);
        let fresh = (input - cached) as f64;
        Some((fresh * pin + cached as f64 * pcached + output as f64 * pout) / 1_000_000.0)
    }
}

fn bundled() -> Arc<Table> {
    static TABLE: OnceLock<Arc<Table>> = OnceLock::new();
    TABLE
        .get_or_init(|| Arc::new(Table::parse(BUNDLED).expect("data/prices.json is valid")))
        .clone()
}

static FRESH: RwLock<Option<Arc<Table>>> = RwLock::new(None);

/// The newest prices we have: a refreshed copy if there is one, else the bundled one.
pub fn current() -> Arc<Table> {
    FRESH.read().ok().and_then(|g| g.clone()).unwrap_or_else(bundled)
}

/// The model's context window from the price table, if known.
pub fn context_limit(provider: &str, model: &str) -> Option<u32> {
    current().context_limit(provider, model)
}

/// The model's facts from the price table, if known.
pub fn facts(provider: &str, model: &str) -> Option<ModelFacts> {
    current().facts(provider, model)
}

/// Estimated cost in USD, or `None` when the model has no price in the table.
pub fn cost(provider: &str, model: &str, input: u32, output: u32, cached: u32) -> Option<f64> {
    current().cost(provider, model, input, output, cached)
}

fn trim_provider(provider: &Value) -> Value {
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let mut out = serde_json::Map::new();
    let Some(models) = provider.get("models").and_then(Value::as_object) else { return Value::Object(out) };
    for (id, model) in models {
        let Some(cost) = model.get("cost") else { continue };
        let Some(input) = num(cost, "input") else { continue };
        let takes = |kind: &str| {
            model
                .pointer("/modalities/input")
                .and_then(Value::as_array)
                .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some(kind)))
        };
        let tiers: Vec<Value> = cost
            .get("tiers")
            .and_then(Value::as_array)
            .map(|tiers| {
                tiers
                    .iter()
                    .filter(|t| t.pointer("/tier/type").and_then(Value::as_str) == Some("context"))
                    .filter_map(|t| {
                        Some(json!({
                            "over": t.pointer("/tier/size")?.as_u64()?,
                            "input": num(t, "input")?,
                            "output": num(t, "output").unwrap_or(0.0),
                            "cache_read": num(t, "cache_read"),
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(
            id.clone(),
            json!({
                "input": input,
                "output": num(cost, "output").unwrap_or(0.0),
                "cache_read": num(cost, "cache_read"),
                "tiers": tiers,
                "context": model.pointer("/limit/context").and_then(Value::as_u64),
                "name": model.get("name").and_then(Value::as_str),
                "release_date": model.get("release_date").and_then(Value::as_str),
                "knowledge": model.get("knowledge").and_then(Value::as_str),
                "reasoning": model.get("reasoning").and_then(Value::as_bool).unwrap_or(false),
                "tool_call": model.get("tool_call").and_then(Value::as_bool).unwrap_or(false),
                "image_input": takes("image"),
                "pdf_input": takes("pdf"),
                "deprecated": model.get("status").and_then(Value::as_str) == Some("deprecated"),
            }),
        );
    }
    Value::Object(out)
}

/// Keeps what Chingui needs from the models.dev feed. Refuses data that looks broken, so a
/// changed format can't replace good prices with nothing.
pub fn trim(feed: &Value, date: &str) -> std::result::Result<(String, Table), String> {
    let doc = json!({
        "source": FEED,
        "fetchedAt": date,
        "providers": { "gemini": trim_provider(&feed["google"]), "openai": trim_provider(&feed["openai"]) },
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let table = Table::parse(&text).map_err(|e| e.to_string())?;
    let gemini_models = table.providers.get("gemini").map_or(0, HashMap::len);
    let embeds = table.cost("gemini", "gemini-embedding-2", 1_000_000, 0, 0).unwrap_or(0.0) > 0.0;
    if gemini_models < 10 || !embeds {
        return Err(format!("the feed looks wrong ({gemini_models} Gemini models, embedding priced: {embeds})"));
    }
    Ok((text, table))
}

/// Downloads and trims the feed at `url`. Returns the file text and the parsed table.
pub async fn fetch_table(client: &reqwest::Client, url: &str) -> Result<(String, Table)> {
    let net = |e: reqwest::Error| AppError::Internal(format!("Couldn't fetch prices: {e}"));
    let resp = client.get(url).timeout(Duration::from_secs(60)).send().await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!("Couldn't fetch prices: HTTP {}", resp.status().as_u16())));
    }
    let bytes = resp.bytes().await.map_err(net)?;
    if bytes.len() > MAX_FEED_BYTES {
        return Err(AppError::Internal("The price feed is far bigger than expected.".into()));
    }
    let feed: Value = serde_json::from_slice(&bytes).map_err(|e| AppError::Internal(format!("Bad price feed: {e}")))?;
    trim(&feed, &today()).map_err(|e| AppError::Internal(format!("Bad price feed: {e}")))
}

/// Uses the refreshed copy from a previous run if it is newer than the bundled one.
pub fn load_saved(dir: &Path) {
    if let Some(table) = newer_saved(dir) {
        log::info!("prices: using the copy from {} in the data folder", table.fetched_at);
        if let Ok(mut fresh) = FRESH.write() {
            *fresh = Some(Arc::new(table));
        }
    }
}

fn newer_saved(dir: &Path) -> Option<Table> {
    let text = std::fs::read_to_string(dir.join(SAVED)).ok()?;
    let table = Table::parse(&text).ok()?;
    (table.fetched_at > bundled().fetched_at).then_some(table)
}

async fn refresh(client: &reqwest::Client, dir: &Path) -> Result<()> {
    let (text, table) = fetch_table(client, FEED).await?;
    let tmp = dir.join(format!("{SAVED}.part"));
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, dir.join(SAVED))).map_err(|e| AppError::Internal(format!("Couldn't save prices: {e}")))?;
    log::info!("prices refreshed ({})", table.fetched_at);
    if let Ok(mut fresh) = FRESH.write() {
        *fresh = Some(Arc::new(table));
    }
    Ok(())
}

/// Whether the weekly download is on. It is unless the user turned it off.
pub fn auto_update_enabled(conn: &rusqlite::Connection) -> Result<bool> {
    Ok(settings::get(conn, AUTO_KEY)?.as_deref() != Some("false"))
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).build().expect("http client")
}

/// Fetches the prices now, whatever the schedule says, and remembers the time on success.
pub async fn update_now(db: &Db, dir: &Path) -> Result<()> {
    refresh(&http_client(), dir).await?;
    let _ = db.call(|c| settings::set(c, CHECKED_KEY, &now_ms().to_string())).await;
    Ok(())
}

/// Fetches when automatic updates are on and the last success is over a week old. A failure is
/// logged and left for the next check.
pub async fn update_if_due(db: &Db, dir: &Path) {
    let state = db
        .call(|c| Ok((auto_update_enabled(c)?, settings::get(c, CHECKED_KEY)?)))
        .await;
    let Ok((enabled, last)) = state else { return };
    let last = last.and_then(|v| v.parse::<i64>().ok());
    if enabled && last.is_none_or(|t| now_ms() - t >= REFRESH_EVERY_MS) {
        if let Err(e) = update_now(db, dir).await {
            log::warn!("prices not refreshed: {e}");
        }
    }
}

/// Checks at startup and then daily. The request carries nothing of the user's.
pub async fn keep_fresh(db: Db, dir: std::path::PathBuf) {
    loop {
        update_if_due(&db, &dir).await;
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

/// Today as `YYYY-MM-DD` (UTC).
fn today() -> String {
    civil_date(now_ms().div_euclid(86_400_000))
}

/// Days since 1970-01-01 to a date (Howard Hinnant's algorithm).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "source": "test", "fetchedAt": "2026-01-01",
        "providers": { "gemini": {
            "base":        { "input": 1.0, "output": 4.0, "cache_read": 0.1, "tiers": [] },
            "base-live":   { "input": 9.0, "output": 9.0 },
            "pro":         { "input": 2.0, "output": 12.0, "cache_read": 0.2,
                             "tiers": [ { "over": 200000, "input": 4.0, "output": 18.0, "cache_read": 0.4 } ] },
            "emb":         { "input": 0.2, "output": 0 }
        } } }"#;

    fn table() -> Table {
        Table::parse(FIXTURE).unwrap()
    }

    fn near(a: Option<f64>, b: f64) {
        let a = a.expect("a price");
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    #[test]
    fn input_output_and_cached_are_priced_separately() {
        // 600k fresh * 1.0 + 400k cached * 0.1 + 100k output * 4.0 = 0.6 + 0.04 + 0.4
        near(table().cost("gemini", "base", 1_000_000, 100_000, 400_000), 1.04);
    }

    #[test]
    fn embeddings_bill_input_only_and_cached_defaults_to_input() {
        near(table().cost("gemini", "emb", 3_000_000, 0, 0), 0.60);
        near(table().cost("gemini", "emb", 1_000_000, 0, 500_000), 0.20);
    }

    #[test]
    fn long_prompts_use_the_tier() {
        near(table().cost("gemini", "pro", 200_000, 0, 0), 0.40);
        near(table().cost("gemini", "pro", 200_001, 0, 0), 0.800004);
    }

    #[test]
    fn variants_match_the_base_but_special_models_do_not() {
        let t = table();
        assert!(t.cost("gemini", "models/base-latest", 1, 1, 0).is_some());
        assert!(t.cost("gemini", "base-001", 1, 1, 0).is_some());
        near(t.cost("gemini", "base-live", 1_000_000, 0, 0), 9.0); // its own entry
        assert_eq!(t.cost("gemini", "base-tts-preview", 1, 1, 0), None);
        assert_eq!(t.cost("gemini", "base-image-preview", 1, 1, 0), None);
    }

    #[test]
    fn unknown_models_and_providers_have_no_price() {
        assert_eq!(table().cost("gemini", "something-new", 10, 10, 0), None);
        assert_eq!(table().cost("anthropic", "base", 10, 10, 0), None);
    }

    fn feed(gemini_models: usize) -> Value {
        let mut models = serde_json::Map::new();
        for i in 0..gemini_models {
            models.insert(format!("gemini-m{i}"), json!({ "cost": { "input": 1.0, "output": 2.0, "cache_read": 0.1 } }));
        }
        models.insert("gemini-embedding-2".into(), json!({ "cost": { "input": 0.2, "input_audio": 6.5 } }));
        models.insert(
            "gemini-pro-x".into(),
            json!({ "cost": { "input": 2.0, "output": 12.0, "tiers": [
                { "input": 4.0, "output": 18.0, "cache_read": 0.4, "tier": { "type": "context", "size": 200000 } },
                { "input": 9.0, "output": 9.0, "tier": { "type": "other", "size": 5 } } ] } }),
        );
        models.insert("gemini-free-thing".into(), json!({ "name": "no cost field" }));
        json!({ "google": { "models": models }, "openai": { "models": { "text-embedding-3-small": { "cost": { "input": 0.02 } } } } })
    }

    #[test]
    fn trimming_keeps_prices_context_tiers_and_drops_the_rest() {
        let (text, t) = trim(&feed(12), "2026-02-03").unwrap();
        assert_eq!(t.fetched_at, "2026-02-03");
        near(t.cost("gemini", "gemini-embedding-2", 1_000_000, 0, 0), 0.2);
        near(t.cost("gemini", "gemini-pro-x", 200_001, 0, 0), 0.800004);
        near(t.cost("openai", "text-embedding-3-small", 1_000_000, 0, 0), 0.02);
        assert_eq!(t.cost("gemini", "gemini-free-thing", 1, 1, 0), None);
        assert!(!text.contains("input_audio"));
    }

    #[test]
    fn trimming_keeps_what_a_model_picker_shows() {
        let mut f = feed(12);
        f["google"]["models"]["gemini-pro-x"].as_object_mut().unwrap().extend(
            json!({
                "name": "Gemini Pro X", "release_date": "2026-05-01", "knowledge": "2025-12",
                "reasoning": true, "tool_call": true, "status": "deprecated",
                "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let (_, t) = trim(&f, "2026-02-03").unwrap();
        let facts = t.facts("gemini", "gemini-pro-x").unwrap();
        assert_eq!(facts.name.as_deref(), Some("Gemini Pro X"));
        assert_eq!(facts.release_date.as_deref(), Some("2026-05-01"));
        assert_eq!(facts.knowledge_cutoff.as_deref(), Some("2025-12"));
        assert!(facts.reasoning && facts.tool_calls && facts.image_input && facts.pdf_input && facts.deprecated);
        assert_eq!((facts.input_price, facts.output_price), (2.0, 12.0));
        // A model with none of those fields gets plain defaults.
        let plain = t.facts("gemini", "gemini-m0").unwrap();
        assert_eq!(plain.name, None);
        assert!(!plain.reasoning && !plain.image_input && !plain.deprecated);
        assert_eq!(plain.cached_price, Some(0.1));
    }

    #[test]
    fn facts_follow_the_same_matching_as_cost() {
        let t = table();
        assert!(t.facts("gemini", "base").is_some());
        assert!(t.facts("gemini", "models/base-001").is_some(), "a dated variant finds its base");
        assert!(t.facts("gemini", "base-tts-preview").is_none(), "special variants don't");
        assert!(t.facts("gemini", "something-new").is_none());
    }

    #[test]
    fn a_feed_that_looks_wrong_is_refused() {
        assert!(trim(&feed(2), "2026-02-03").is_err(), "too few models");
        assert!(trim(&json!({ "unexpected": true }), "2026-02-03").is_err());
    }

    #[test]
    fn automatic_updates_are_on_until_turned_off() {
        let conn = crate::persistence::test_conn();
        assert!(auto_update_enabled(&conn).unwrap());
        settings::set_bool(&conn, AUTO_KEY, false).unwrap();
        assert!(!auto_update_enabled(&conn).unwrap());
        settings::set_bool(&conn, AUTO_KEY, true).unwrap();
        assert!(auto_update_enabled(&conn).unwrap());
    }

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_762), "2026-11-05");
        assert_eq!(civil_date(11_016), "2000-02-29");
    }

    #[test]
    fn a_saved_copy_is_used_only_when_newer_than_the_bundled_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(newer_saved(dir.path()).is_none());
        let (old, _) = trim(&feed(12), "2000-01-01").unwrap();
        std::fs::write(dir.path().join(SAVED), old).unwrap();
        assert!(newer_saved(dir.path()).is_none());
        let (new, _) = trim(&feed(12), "2999-01-01").unwrap();
        std::fs::write(dir.path().join(SAVED), new).unwrap();
        assert_eq!(newer_saved(dir.path()).unwrap().fetched_at, "2999-01-01");
        std::fs::write(dir.path().join(SAVED), "not json").unwrap();
        assert!(newer_saved(dir.path()).is_none());
    }

    #[test]
    fn fetching_reads_a_feed_and_reports_http_errors() {
        use std::io::{Read, Write};
        let body = feed(12).to_string();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for status in [200, 500] {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let b = if status == 200 { body.as_str() } else { "" };
                let _ = write!(s, "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}", b.len());
            }
        });
        let client = reqwest::Client::new();
        let (_, t) = tauri::async_runtime::block_on(fetch_table(&client, &url)).unwrap();
        near(t.cost("gemini", "gemini-embedding-2", 1_000_000, 0, 0), 0.2);
        let err = tauri::async_runtime::block_on(fetch_table(&client, &url)).unwrap_err().to_string();
        assert!(err.contains("HTTP 500"), "{err}");
    }

    /// Rewrites `data/prices.json`, the copy bundled with the app, from the live feed. The app
    /// refreshes its own copy weekly, so this only matters for new builds. Run from `src-tauri`:
    /// `cargo test update_bundled_prices -- --ignored --nocapture`, then commit the diff.
    #[test]
    #[ignore]
    fn update_bundled_prices() {
        let client = reqwest::Client::new();
        let (text, table) = tauri::async_runtime::block_on(fetch_table(&client, FEED)).expect("fetch prices");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/prices.json");
        std::fs::write(&path, text).expect("write prices.json");
        println!("wrote {} (fetched {})", path.display(), table.fetched_at);
    }

    #[test]
    fn context_limits_come_from_the_table() {
        let t = Table::parse(
            r#"{ "source": "s", "fetchedAt": "d", "providers": { "openai": {
                "gpt-x": { "input": 1.0, "output": 2.0, "context": 400000 },
                "gpt-y": { "input": 1.0, "output": 2.0 } } } }"#,
        )
        .unwrap();
        assert_eq!(t.context_limit("openai", "gpt-x"), Some(400_000));
        assert_eq!(t.context_limit("openai", "gpt-x-2026-01-01"), Some(400_000));
        assert_eq!(t.context_limit("openai", "gpt-y"), None);
        assert_eq!(t.context_limit("openai", "nope"), None);
    }

    #[test]
    fn the_bundled_file_parses_and_prices_the_embedding_model() {
        let t = bundled();
        assert!(!t.fetched_at.is_empty());
        assert!(t.cost("gemini", "gemini-embedding-2", 1_000_000, 0, 0).unwrap() > 0.0);
    }
}
