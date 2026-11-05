//! Embeddings from Gemini's API (`gemini-embedding-2`). Passages leave the machine, so the engine
//! only uses this after the user agrees. The key is read from the Keychain for every call and
//! never kept.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::DIMENSIONS;
use super::engine::{EncodeUsage, Encoder, normalize};
use crate::error::{AppError, Result};
use crate::services::ai::gemini::{describe_error, net_error};

pub const MODEL: &str = "gemini-embedding-2";
/// Stored as the vector revision. Changing the output size must rebuild the index.
pub const REVISION: &str = "768d";
const BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const RETRIES: u32 = 3;
const RETRY_STEP: Duration = Duration::from_secs(2);
const TIMEOUT: Duration = Duration::from_secs(60);

pub type KeySource = Arc<dyn Fn() -> Result<Option<String>> + Send + Sync>;

pub struct GeminiEncoder {
    client: reqwest::Client,
    base: String,
    key: KeySource,
    retry_step: Duration,
    last: Option<EncodeUsage>,
}

impl GeminiEncoder {
    pub fn new(key: KeySource) -> Self {
        Self::with_base(BASE.into(), key, RETRY_STEP)
    }

    pub fn with_base(base: String, key: KeySource, retry_step: Duration) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("http client");
        GeminiEncoder { client, base, key, retry_step, last: None }
    }

    async fn call(&self, key: &str, texts: &[String]) -> Result<(Vec<Vec<f32>>, EncodeUsage)> {
        let url = format!("{}/models/{MODEL}:batchEmbedContents", self.base);
        let body = build_body(texts);
        let mut attempt = 0;
        loop {
            let resp = self
                .client
                .post(&url)
                .header("x-goog-api-key", key)
                .timeout(TIMEOUT)
                .json(&body)
                .send()
                .await
                .map_err(net_error)?;
            let status = resp.status().as_u16();
            // Rate limits and "high demand" usually clear within seconds.
            if matches!(status, 429 | 500 | 503) && attempt < RETRIES {
                attempt += 1;
                log::warn!("gemini embeddings: HTTP {status}, retry {attempt} of {RETRIES}");
                tokio::time::sleep(self.retry_step * attempt).await;
                continue;
            }
            let text = resp.text().await.map_err(net_error)?;
            if !(200..300).contains(&status) {
                return Err(AppError::Provider(describe_error(status, &text)));
            }
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| AppError::Provider(format!("Unexpected Gemini response: {e}")))?;
            let vectors = parse_response(&value, texts.len())?;
            let usage = match response_tokens(&value) {
                Some(tokens) => EncodeUsage { tokens, estimated: false },
                None => EncodeUsage { tokens: estimate_tokens(texts), estimated: true },
            };
            log::debug!(
                "gemini embeddings: {} texts, response keys {:?}, tokens {} (estimated: {})",
                texts.len(),
                value.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                usage.tokens,
                usage.estimated
            );
            return Ok((vectors, usage));
        }
    }
}

/// Prompt tokens the response reports, if it reports any: a total for the call, or one count per
/// embedding. The API reference doesn't show the batch response, so both are tried.
fn response_tokens(value: &Value) -> Option<u32> {
    let count = |v: &Value| {
        ["promptTokenCount", "totalTokenCount", "tokenCount"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_u64))
    };
    if let Some(n) = value.get("usageMetadata").and_then(count) {
        return Some(n as u32);
    }
    let per_item: Option<Vec<u64>> = value
        .get("embeddings")?
        .as_array()?
        .iter()
        .map(|e| e.get("usageMetadata").and_then(count).or_else(|| e.get("statistics").and_then(count)))
        .collect();
    per_item.map(|v| v.iter().sum::<u64>() as u32)
}

/// About 4 characters per token. Only used when the response has no count.
fn estimate_tokens(texts: &[String]) -> u32 {
    texts.iter().map(|t| t.chars().count().div_ceil(4) as u32).sum()
}

impl Encoder for GeminiEncoder {
    fn encode(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let key = (self.key)()?.ok_or_else(|| {
            AppError::Invalid("Add your Gemini API key in Settings to use meaning search.".into())
        })?;
        // Runs on the embedding worker or a blocking thread, never inside the async runtime.
        let (vectors, usage) = tauri::async_runtime::block_on(self.call(&key, texts))?;
        self.last = Some(usage);
        Ok(vectors)
    }

    fn last_usage(&self) -> Option<EncodeUsage> {
        self.last
    }
}

fn build_body(texts: &[String]) -> Value {
    let requests: Vec<Value> = texts
        .iter()
        .map(|t| {
            json!({
                "model": format!("models/{MODEL}"),
                "content": { "parts": [{ "text": t }] },
                "outputDimensionality": DIMENSIONS,
            })
        })
        .collect();
    json!({ "requests": requests })
}

/// One vector per text, or an error. A wrong count or size is an error rather than something to
/// store, because a bad vector would sit in the index silently.
fn parse_response(value: &Value, expected: usize) -> Result<Vec<Vec<f32>>> {
    let bad = |what: &str| AppError::Provider(format!("Unexpected Gemini embedding response: {what}"));
    let items: Vec<&Value> = match value.get("embeddings").and_then(Value::as_array) {
        Some(list) => list.iter().collect(),
        None => value.get("embedding").into_iter().collect(),
    };
    if items.len() != expected {
        return Err(bad(&format!("expected {expected} vectors, got {}", items.len())));
    }
    items
        .into_iter()
        .map(|item| {
            let values = item.get("values").and_then(Value::as_array).ok_or_else(|| bad("no values"))?;
            if values.len() != DIMENSIONS {
                return Err(bad(&format!("expected {DIMENSIONS} dimensions, got {}", values.len())));
            }
            let v: Option<Vec<f32>> = values.iter().map(|x| x.as_f64().map(|f| f as f32)).collect();
            Ok(normalize(v.ok_or_else(|| bad("a value was not a number"))?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    fn vector(seed: f32) -> Value {
        json!({ "values": (0..DIMENSIONS).map(|i| seed + i as f32).collect::<Vec<_>>() })
    }

    #[test]
    fn body_has_one_request_per_text_with_the_output_size() {
        let body = build_body(&["a".into(), "b".into()]);
        let reqs = body["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1]["content"]["parts"][0]["text"], "b");
        assert_eq!(reqs[0]["model"], "models/gemini-embedding-2");
        assert_eq!(reqs[0]["outputDimensionality"], 768);
    }

    #[test]
    fn parses_vectors_and_normalizes_them() {
        let out = parse_response(&json!({ "embeddings": [vector(1.0), vector(2.0)] }), 2).unwrap();
        assert_eq!(out.len(), 2);
        let norm: f32 = out[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn reads_token_counts_from_either_place_or_gives_none() {
        assert_eq!(response_tokens(&json!({ "usageMetadata": { "promptTokenCount": 42 } })), Some(42));
        let per_item = json!({ "embeddings": [
            { "values": [], "usageMetadata": { "promptTokenCount": 5 } },
            { "values": [], "statistics": { "tokenCount": 7 } },
        ]});
        assert_eq!(response_tokens(&per_item), Some(12));
        assert_eq!(response_tokens(&json!({ "embeddings": [{ "values": [] }] })), None);
        assert_eq!(estimate_tokens(&["abcdefgh".into(), "abc".into()]), 3);
    }

    #[test]
    fn a_call_without_counts_is_estimated_and_flagged() {
        let (base, _) = serve(vec![(200, json!({ "embeddings": [vector(1.0)] }).to_string())]);
        let mut e = encoder(base, Some("k"));
        e.encode(&["twelve chars".into()]).unwrap();
        assert_eq!(e.last_usage(), Some(EncodeUsage { tokens: 3, estimated: true }));
    }

    #[test]
    fn accepts_the_single_embedding_shape() {
        assert_eq!(parse_response(&json!({ "embedding": vector(1.0) }), 1).unwrap().len(), 1);
    }

    #[test]
    fn rejects_wrong_counts_and_sizes() {
        assert!(parse_response(&json!({ "embeddings": [vector(1.0)] }), 2).is_err());
        assert!(parse_response(&json!({ "embeddings": [{ "values": [1.0, 2.0] }] }), 1).is_err());
        assert!(parse_response(&json!({ "nope": true }), 1).is_err());
    }

    type Seen = Arc<Mutex<Vec<(String, String)>>>;

    /// Answers each request with the next canned (status, body), and records (head, body).
    fn serve(replies: Vec<(u16, String)>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, len) = loop {
                    let n = s.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len = text[..i]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = s.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                log.lock().unwrap().push((head, String::from_utf8_lossy(&buf[head_end..]).to_string()));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(reply.as_bytes()).unwrap();
            }
        });
        (base, seen)
    }

    fn encoder(base: String, key: Option<&str>) -> GeminiEncoder {
        let key = key.map(str::to_owned);
        GeminiEncoder::with_base(base, Arc::new(move || Ok(key.clone())), Duration::ZERO)
    }

    #[test]
    fn sends_the_key_as_a_header_and_returns_vectors() {
        let (base, seen) = serve(vec![(200, json!({ "embeddings": [vector(1.0)] }).to_string())]);
        let out = encoder(base, Some("secret-key")).encode(&["hello".into()]).unwrap();
        assert_eq!(out.len(), 1);
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.to_lowercase().contains("x-goog-api-key: secret-key"));
        assert!(!seen[0].0.contains("key="), "the key must never be in the URL");
        assert!(seen[0].1.contains("hello"));
    }

    #[test]
    fn retries_a_rate_limit_then_succeeds() {
        let ok = json!({ "embeddings": [vector(1.0)] }).to_string();
        let (base, seen) = serve(vec![(429, "{}".into()), (503, "{}".into()), (200, ok)]);
        assert_eq!(encoder(base, Some("k")).encode(&["x".into()]).unwrap().len(), 1);
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn gives_up_after_the_retries_and_says_why() {
        let (base, _) = serve(vec![(429, r#"{"error":{"message":"quota"}}"#.into()); 4]);
        let err = encoder(base, Some("k")).encode(&["x".into()]).unwrap_err().to_string();
        assert!(err.contains("rate limit"), "{err}");
    }

    #[test]
    fn a_rejected_key_is_reported_without_retrying() {
        let body = r#"{"error":{"message":"API key not valid. Please pass a valid API key."}}"#;
        let (base, seen) = serve(vec![(400, body.into())]);
        let err = encoder(base, Some("bad")).encode(&["x".into()]).unwrap_err().to_string();
        assert!(err.contains("rejected the API key"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn no_key_is_a_clear_error_and_no_request() {
        let err = encoder("http://127.0.0.1:1".into(), None).encode(&["x".into()]).unwrap_err().to_string();
        assert!(err.contains("Add your Gemini API key"), "{err}");
    }

    /// Live check of the real API shape. Run by hand:
    /// `GEMINI_API_KEY=... cargo test live_gemini_embeddings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_gemini_embeddings() {
        let key = std::env::var("GEMINI_API_KEY").expect("set GEMINI_API_KEY");
        let mut e = GeminiEncoder::new(Arc::new(move || Ok(Some(key.clone()))));
        let texts = [
            format!("{}Coral bleaching follows sustained heat stress.", super::super::DOC_PREFIX),
            format!("{}Gill tissue was stained with alcian blue.", super::super::DOC_PREFIX),
            format!("{}why do reefs lose their colour", super::super::QUERY_PREFIX),
        ];
        let v = e.encode(&texts).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].len(), DIMENSIONS);
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        println!("query vs bleaching {:.3}, query vs staining {:.3}", dot(&v[2], &v[0]), dot(&v[2], &v[1]));
        assert!(dot(&v[2], &v[0]) > dot(&v[2], &v[1]));
    }
}
