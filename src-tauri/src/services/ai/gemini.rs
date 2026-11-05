use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::sse::SseParser;
use super::{EventStream, GenerationEvent, GenerationRequest, Provider, TurnRole, merge_turns};
use crate::domain::ModelInfo;
use crate::error::{AppError, Result};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
/// Give up if the connection goes quiet this long mid-stream.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const RETRIES_ON_503: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Models that accept generateContent but are not text chat models.
const SKIP_IDS: &[&str] = &["-tts", "-image", "-live", "native-audio", "embedding", "robotics"];

pub struct Gemini {
    client: reqwest::Client,
}

impl Gemini {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("http client");
        Gemini { client }
    }
}

impl Provider for Gemini {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn list_models<'a>(&'a self, api_key: &'a str) -> BoxFuture<'a, Result<Vec<ModelInfo>>> {
        Box::pin(async move {
            let mut models = Vec::new();
            let mut page_token: Option<String> = None;
            loop {
                let mut req = self
                    .client
                    .get(format!("{BASE}/models"))
                    .header("x-goog-api-key", api_key)
                    .query(&[("pageSize", "200")])
                    .timeout(Duration::from_secs(20));
                if let Some(t) = &page_token {
                    req = req.query(&[("pageToken", t.as_str())]);
                }
                let resp = req.send().await.map_err(net_error)?;
                let body = check_status(resp).await?.text().await.map_err(net_error)?;
                let page: ModelsPage = serde_json::from_str(&body)
                    .map_err(|e| AppError::Provider(format!("Unexpected Gemini response: {e}")))?;
                models.extend(page.models.into_iter().filter_map(ApiModel::into_info));
                match page.next_page_token.filter(|t| !t.is_empty()) {
                    Some(t) => page_token = Some(t),
                    None => break,
                }
            }
            Ok(models)
        })
    }

    fn generate(&self, api_key: String, request: GenerationRequest) -> EventStream {
        let (tx, mut rx) = mpsc::channel::<Result<GenerationEvent>>(64);
        let client = self.client.clone();
        tokio::spawn(async move {
            // The task also ends when the receiver is dropped (cancellation).
            tokio::select! {
                _ = run_stream(client, api_key, request, tx.clone()) => {}
                _ = tx.closed() => {}
            }
        });
        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }
}

async fn run_stream(
    client: reqwest::Client,
    api_key: String,
    request: GenerationRequest,
    tx: mpsc::Sender<Result<GenerationEvent>>,
) {
    if let Err(e) = stream_inner(&client, &api_key, request, &tx).await {
        let _ = tx.send(Err(e)).await;
    }
}

async fn stream_inner(
    client: &reqwest::Client,
    api_key: &str,
    request: GenerationRequest,
    tx: &mpsc::Sender<Result<GenerationEvent>>,
) -> Result<()> {
    let url = format!("{BASE}/models/{}:streamGenerateContent", request.model);
    let body = build_body(&request);
    let mut attempt = 0;
    let resp = loop {
        let resp = client
            .post(&url)
            .query(&[("alt", "sse")])
            .header("x-goog-api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(net_error)?;
        // "High demand" 503s are common and usually clear within seconds. Nothing has streamed yet.
        if resp.status().as_u16() == 503 && attempt < RETRIES_ON_503 {
            attempt += 1;
            tokio::time::sleep(RETRY_DELAY).await;
            continue;
        }
        break check_status(resp).await?;
    };

    let mut parser = SseParser::default();
    let mut bytes = resp.bytes_stream();
    loop {
        let next = tokio::time::timeout(IDLE_TIMEOUT, bytes.next())
            .await
            .map_err(|_| AppError::Provider("Gemini stopped responding.".into()))?;
        let (payloads, ended) = match next {
            Some(chunk) => (parser.push(&chunk.map_err(net_error)?), false),
            None => (parser.finish(), true),
        };
        for p in payloads {
            if p.trim() == "[DONE]" {
                continue;
            }
            let value: Value = serde_json::from_str(&p)
                .map_err(|e| AppError::Provider(format!("Unexpected Gemini response: {e}")))?;
            for ev in parse_chunk(&value)? {
                if tx.send(Ok(ev)).await.is_err() {
                    return Ok(());
                }
            }
        }
        if ended {
            return Ok(());
        }
    }
}

pub fn build_body(req: &GenerationRequest) -> Value {
    let contents: Vec<Value> = merge_turns(req.turns.clone())
        .into_iter()
        .map(|t| {
            let role = match t.role {
                TurnRole::User => "user",
                TurnRole::Model => "model",
            };
            json!({ "role": role, "parts": [{ "text": t.text }] })
        })
        .collect();
    let mut body = json!({ "contents": contents });
    if let Some(system) = &req.system {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    body
}

/// Turns one streamed JSON chunk into events. Thought summaries are not shown.
pub fn parse_chunk(v: &Value) -> Result<Vec<GenerationEvent>> {
    if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(AppError::Provider(format!(
            "Gemini blocked this request ({reason}). Try rephrasing."
        )));
    }
    let mut out = Vec::new();
    if let Some(cand) = v.pointer("/candidates/0") {
        if let Some(parts) = cand.pointer("/content/parts").and_then(Value::as_array) {
            for part in parts {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    out.push(GenerationEvent::Text(text.to_string()));
                }
            }
        }
        if let Some(reason) = cand.get("finishReason").and_then(Value::as_str)
            && !reason.is_empty()
        {
            if let Some(u) = usage(v) {
                out.push(u);
            }
            out.push(GenerationEvent::Finished(reason.to_string()));
            return Ok(out);
        }
    }
    if let Some(u) = usage(v) {
        out.push(u);
    }
    Ok(out)
}

fn usage(v: &Value) -> Option<GenerationEvent> {
    let m = v.get("usageMetadata")?;
    let n = |k: &str| m.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    Some(GenerationEvent::Usage {
        input_tokens: n("promptTokenCount"),
        output_tokens: n("candidatesTokenCount") + n("thoughtsTokenCount"),
        cached_tokens: n("cachedContentTokenCount"),
    })
}

fn net_error(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::Provider("The request to Gemini timed out.".into())
    } else if e.is_connect() {
        AppError::Provider("Couldn't reach Gemini. Check your internet connection.".into())
    } else {
        // reqwest errors can embed the URL, which never contains the key (it is sent as a header).
        AppError::Provider(format!(
            "Network error talking to Gemini: {}",
            error_chain(&e)
        ))
    }
}

/// reqwest's top-level message hides the real cause (TLS, closed connection, ...).
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(AppError::Provider(describe_error(status.as_u16(), &body)))
}

pub fn describe_error(status: u16, body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned));
    match (status, message) {
        (400 | 401 | 403, Some(m)) if m.to_lowercase().contains("api key") => {
            format!("Gemini rejected the API key: {m}")
        }
        (429, m) => format!(
            "Gemini rate limit or quota reached.{}",
            m.map(|m| format!(" {m}")).unwrap_or_default()
        ),
        (s, Some(m)) => format!("Gemini error {s}: {m}"),
        (s, None) => format!("Gemini error {s}."),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsPage {
    #[serde(default)]
    models: Vec<ApiModel>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiModel {
    name: String,
    display_name: Option<String>,
    input_token_limit: Option<u32>,
    output_token_limit: Option<u32>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

impl ApiModel {
    fn into_info(self) -> Option<ModelInfo> {
        let id = self.name.strip_prefix("models/")?.to_string();
        let can_chat = self
            .supported_generation_methods
            .iter()
            .any(|m| m == "generateContent");
        if !can_chat || !id.starts_with("gemini") || SKIP_IDS.iter().any(|s| id.contains(s)) {
            return None;
        }
        Some(ModelInfo {
            display_name: self.display_name.unwrap_or_else(|| id.clone()),
            id,
            input_token_limit: self.input_token_limit,
            output_token_limit: self.output_token_limit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai::{Turn, sse::SseParser};

    // Shape follows the documented GenerateContentResponse. Not yet checked against the live API.
    const STREAM: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hel\"}]},\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":7}}\r\n\r\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"},{\"text\":\"hidden\",\"thought\":true}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3}}\r\n\r\n",
    );

    fn events(stream: &str) -> Vec<GenerationEvent> {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        for payload in p.push(stream.as_bytes()) {
            let v: Value = serde_json::from_str(&payload).unwrap();
            out.extend(parse_chunk(&v).unwrap());
        }
        out
    }

    #[test]
    fn parses_text_usage_and_finish_and_skips_thoughts() {
        let ev = events(STREAM);
        let text: String = ev
            .iter()
            .filter_map(|e| match e {
                GenerationEvent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello");
        assert!(ev.contains(&GenerationEvent::Usage { input_tokens: 7, output_tokens: 5, cached_tokens: 0 }));
        assert_eq!(ev.last(), Some(&GenerationEvent::Finished("STOP".into())));
    }

    #[test]
    fn blocked_prompt_is_an_error() {
        let v = json!({ "promptFeedback": { "blockReason": "SAFETY" } });
        assert!(parse_chunk(&v).is_err());
    }

    #[test]
    fn body_has_roles_and_system_instruction() {
        let req = GenerationRequest {
            model: "m".into(),
            system: Some("be brief".into()),
            turns: vec![
                Turn { role: TurnRole::User, text: "hi".into() },
                Turn { role: TurnRole::User, text: "again".into() },
                Turn { role: TurnRole::Model, text: "yo".into() },
            ],
        };
        let b = build_body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(b["contents"].as_array().unwrap().len(), 2);
        assert_eq!(b["contents"][0]["parts"][0]["text"], "hi\n\nagain");
        assert_eq!(b["contents"][1]["role"], "model");
    }

    #[test]
    fn error_messages_are_readable() {
        let body = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}"#;
        assert!(describe_error(400, body).starts_with("Gemini rejected the API key"));
        assert!(describe_error(429, "").contains("rate limit"));
        assert_eq!(describe_error(500, "not json"), "Gemini error 500.");
    }

    #[test]
    fn model_list_keeps_chat_models_only() {
        let page: ModelsPage = serde_json::from_value(json!({
            "models": [
                { "name": "models/gemini-x-flash", "displayName": "X Flash", "inputTokenLimit": 1000000,
                  "supportedGenerationMethods": ["generateContent", "countTokens"] },
                { "name": "models/gemini-embedding-001", "supportedGenerationMethods": ["embedContent"] },
                { "name": "models/gemini-x-tts", "supportedGenerationMethods": ["generateContent"] },
                { "name": "models/gemma-3", "supportedGenerationMethods": ["generateContent"] }
            ]
        }))
        .unwrap();
        let kept: Vec<_> = page.models.into_iter().filter_map(ApiModel::into_info).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "gemini-x-flash");
        assert_eq!(kept[0].input_token_limit, Some(1_000_000));
    }
}
