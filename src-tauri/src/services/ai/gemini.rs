use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::sse::SseParser;
use super::{
    Capabilities, EventStream, FinishReason, GenerationEvent, GenerationRequest, Part, Provider, Turn, TurnRole,
    merge_turns,
};
use crate::domain::ModelInfo;
use crate::error::{AppError, Result};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
/// Give up if the connection goes quiet this long mid-stream.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const RETRIES_ON_503: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Models that accept generateContent but are not text chat models.
const SKIP_IDS: &[&str] = &["-tts", "-image", "-live", "native-audio", "embedding", "robotics"];

/// Tool call ids we made up because Gemini sent none. They are not sent back.
const LOCAL_ID: &str = "local-call-";

pub struct Gemini {
    client: reqwest::Client,
    base: String,
    retry_delay: Duration,
}

impl Gemini {
    pub fn new() -> Self {
        Self::build(BASE.to_string(), RETRY_DELAY)
    }

    #[cfg(test)]
    fn with_base(base: String, retry_delay: Duration) -> Self {
        Self::build(base, retry_delay)
    }

    fn build(base: String, retry_delay: Duration) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("http client");
        Gemini { client, base, retry_delay }
    }
}

impl Provider for Gemini {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { tools: true, implicit_caching: true, vision: true }
    }

    fn list_models<'a>(&'a self, api_key: &'a str) -> BoxFuture<'a, Result<Vec<ModelInfo>>> {
        Box::pin(async move {
            let mut models = Vec::new();
            let mut page_token: Option<String> = None;
            loop {
                let mut req = self
                    .client
                    .get(format!("{}/models", self.base))
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
        let (client, base, retry_delay) = (self.client.clone(), self.base.clone(), self.retry_delay);
        tokio::spawn(async move {
            // The task also ends when the receiver is dropped (cancellation).
            tokio::select! {
                _ = run_stream(client, base, retry_delay, api_key, request, tx.clone()) => {}
                _ = tx.closed() => {}
            }
        });
        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }
}

async fn run_stream(
    client: reqwest::Client,
    base: String,
    retry_delay: Duration,
    api_key: String,
    request: GenerationRequest,
    tx: mpsc::Sender<Result<GenerationEvent>>,
) {
    if let Err(e) = stream_inner(&client, &base, retry_delay, &api_key, request, &tx).await {
        let _ = tx.send(Err(e)).await;
    }
}

async fn stream_inner(
    client: &reqwest::Client,
    base: &str,
    retry_delay: Duration,
    api_key: &str,
    request: GenerationRequest,
    tx: &mpsc::Sender<Result<GenerationEvent>>,
) -> Result<()> {
    let url = format!("{base}/models/{}:streamGenerateContent", request.model);
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
            log::warn!("gemini 503 (high demand), retry {attempt} of {RETRIES_ON_503}");
            tokio::time::sleep(retry_delay).await;
            continue;
        }
        break check_status(resp).await?;
    };

    let mut parser = SseParser::default();
    let mut state = ChunkState::default();
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
            for ev in parse_chunk(&value, &mut state)? {
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
        .iter()
        .map(|t| {
            let role = match t.role {
                TurnRole::User => "user",
                TurnRole::Model => "model",
            };
            json!({ "role": role, "parts": turn_parts(t) })
        })
        .collect();
    let mut body = json!({ "contents": contents });
    if let Some(system) = &req.system {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if !req.tools.is_empty() {
        let declarations: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "parametersJsonSchema": t.parameters }))
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    body
}

/// A model turn that came from Gemini goes back exactly as received, because Gemini 3 rejects a
/// tool call whose thought signature is missing.
fn turn_parts(t: &Turn) -> Vec<Value> {
    if t.role == TurnRole::Model
        && let Some(r) = &t.replay
        && r.provider == "gemini"
        && let Some(parts) = r.data.as_array()
    {
        return parts.clone();
    }
    t.parts
        .iter()
        .map(|p| match p {
            Part::Text { text } => json!({ "text": text }),
            Part::ToolCall { id, name, args } => {
                json!({ "functionCall": with_id(json!({ "name": name, "args": args }), id) })
            }
            Part::ToolResult { id, name, output } => {
                let response = if output.is_object() { output.clone() } else { json!({ "result": output }) };
                json!({ "functionResponse": with_id(json!({ "name": name, "response": response }), id) })
            }
        })
        .collect()
}

fn with_id(mut v: Value, id: &str) -> Value {
    if !id.starts_with(LOCAL_ID) {
        v["id"] = json!(id);
    }
    v
}

/// What the stream has produced so far, needed to build the replay record and the finish reason.
#[derive(Default)]
pub struct ChunkState {
    parts: Vec<Value>,
    calls: u32,
}

/// Turns one streamed JSON chunk into events. Thought summaries are not shown.
pub fn parse_chunk(v: &Value, state: &mut ChunkState) -> Result<Vec<GenerationEvent>> {
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
                // Kept as received, including empty parts that only carry a thought signature.
                state.parts.push(part.clone());
                if let Some(call) = part.get("functionCall") {
                    let name = call
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| AppError::Provider("Gemini sent a tool call without a name.".into()))?;
                    let id = match call.get("id").and_then(Value::as_str).filter(|i| !i.is_empty()) {
                        Some(id) => id.to_string(),
                        None => format!("{LOCAL_ID}{}", state.calls),
                    };
                    state.calls += 1;
                    out.push(GenerationEvent::ToolCall {
                        id,
                        name: name.to_string(),
                        args: call.get("args").cloned().unwrap_or_else(|| json!({})),
                    });
                } else if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    out.push(GenerationEvent::Text(text.to_string()));
                }
            }
        }
        if let Some(reason) = cand.get("finishReason").and_then(Value::as_str)
            && !reason.is_empty()
        {
            if !state.parts.is_empty() {
                out.push(GenerationEvent::Replay(Value::Array(std::mem::take(&mut state.parts))));
            }
            if let Some(u) = usage(v) {
                out.push(u);
            }
            out.push(GenerationEvent::Finished(finish_reason(reason, state.calls)));
            return Ok(out);
        }
    }
    if let Some(u) = usage(v) {
        out.push(u);
    }
    Ok(out)
}

/// Gemini reports STOP even when the turn ends in a tool call.
fn finish_reason(reason: &str, calls: u32) -> FinishReason {
    match reason {
        "STOP" if calls > 0 => FinishReason::ToolCalls,
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        other => FinishReason::Other(other.to_string()),
    }
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

pub(crate) fn net_error(e: reqwest::Error) -> AppError {
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
            provider: "gemini".to_string(),
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
    use crate::services::ai::{Replay, ToolSpec, sse::SseParser};
    use crate::services::testserver::{Reply, serve_with};

    // Shape follows the documented GenerateContentResponse. The live test checks it against the API.
    const STREAM: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hel\"}]},\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":7}}\r\n\r\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"},{\"text\":\"hidden\",\"thought\":true}]},\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":3}}\r\n\r\n",
    );

    const TOOL_STREAM: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"\",\"thoughtSignature\":\"sig-a\"}]}}]}\r\n\r\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"id\":\"fc1\",\"name\":\"add\",\"args\":{\"a\":1,\"b\":2}},\"thoughtSignature\":\"sig-b\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":9,\"candidatesTokenCount\":1}}\r\n\r\n",
    );

    fn events(stream: &str) -> Vec<GenerationEvent> {
        let mut p = SseParser::default();
        let mut state = ChunkState::default();
        let mut out = Vec::new();
        for payload in p.push(stream.as_bytes()) {
            let v: Value = serde_json::from_str(&payload).unwrap();
            out.extend(parse_chunk(&v, &mut state).unwrap());
        }
        out
    }

    fn request(turns: Vec<Turn>) -> GenerationRequest {
        GenerationRequest { model: "m".into(), system: None, turns, tools: Vec::new() }
    }

    fn add_tool() -> ToolSpec {
        ToolSpec {
            name: "add".into(),
            description: "Adds two numbers".into(),
            parameters: json!({ "type": "object", "properties": { "a": { "type": "number" } } }),
        }
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
        assert_eq!(ev.last(), Some(&GenerationEvent::Finished(FinishReason::Stop)));
    }

    #[test]
    fn a_tool_call_ends_the_turn_with_tool_calls_and_keeps_signatures() {
        let ev = events(TOOL_STREAM);
        assert!(ev.contains(&GenerationEvent::ToolCall {
            id: "fc1".into(),
            name: "add".into(),
            args: json!({ "a": 1, "b": 2 }),
        }));
        assert_eq!(ev.last(), Some(&GenerationEvent::Finished(FinishReason::ToolCalls)));
        let replay = ev.iter().find_map(|e| match e {
            GenerationEvent::Replay(v) => Some(v.clone()),
            _ => None,
        });
        let replay = replay.expect("replay record");
        // The empty part that only carries a signature is kept, and parts are not merged.
        assert_eq!(replay.as_array().unwrap().len(), 2);
        assert_eq!(replay[0]["thoughtSignature"], "sig-a");
        assert_eq!(replay[1]["thoughtSignature"], "sig-b");
    }

    #[test]
    fn max_tokens_and_other_reasons_map() {
        assert_eq!(finish_reason("MAX_TOKENS", 0), FinishReason::Length);
        assert_eq!(finish_reason("SAFETY", 0), FinishReason::Other("SAFETY".into()));
    }

    #[test]
    fn blocked_prompt_is_an_error() {
        let v = json!({ "promptFeedback": { "blockReason": "SAFETY" } });
        assert!(parse_chunk(&v, &mut ChunkState::default()).is_err());
    }

    #[test]
    fn body_has_roles_and_system_instruction() {
        let mut req = request(vec![
            Turn::text(TurnRole::User, "hi"),
            Turn::text(TurnRole::User, "again"),
            Turn::text(TurnRole::Model, "yo"),
        ]);
        req.system = Some("be brief".into());
        let b = build_body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(b["contents"].as_array().unwrap().len(), 2);
        assert_eq!(b["contents"][0]["parts"][0]["text"], "hi\n\nagain");
        assert_eq!(b["contents"][1]["role"], "model");
        assert!(b.get("tools").is_none());
    }

    #[test]
    fn body_declares_tools_and_replays_the_model_turn() {
        let signed = json!([{ "functionCall": { "id": "fc1", "name": "add", "args": { "a": 1 } }, "thoughtSignature": "sig-b" }]);
        let mut model = Turn {
            role: TurnRole::Model,
            parts: vec![Part::ToolCall { id: "fc1".into(), name: "add".into(), args: json!({ "a": 1 }) }],
            replay: Some(Replay { provider: "gemini".into(), data: signed.clone() }),
        };
        let result = Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult { id: "fc1".into(), name: "add".into(), output: json!(3) }],
            replay: None,
        };
        let mut req = request(vec![Turn::text(TurnRole::User, "1+2?"), model.clone(), result.clone()]);
        req.tools = vec![add_tool()];
        let b = build_body(&req);
        assert_eq!(b["tools"][0]["functionDeclarations"][0]["name"], "add");
        assert!(b["tools"][0]["functionDeclarations"][0].get("parametersJsonSchema").is_some());
        assert_eq!(b["contents"][1]["parts"], signed, "model turn replayed as received");
        // The call comes before its response, and a plain value is wrapped in an object.
        assert_eq!(b["contents"][2]["role"], "user");
        assert_eq!(b["contents"][2]["parts"][0]["functionResponse"]["response"], json!({ "result": 3 }));
        assert_eq!(b["contents"][2]["parts"][0]["functionResponse"]["id"], "fc1");

        // Another provider's replay is ignored and the turn is rebuilt from its parts.
        model.replay = Some(Replay { provider: "openai".into(), data: json!([]) });
        let b = build_body(&request(vec![model, result]));
        assert_eq!(b["contents"][0]["parts"][0]["functionCall"]["name"], "add");
    }

    #[test]
    fn invented_tool_call_ids_are_not_sent_back() {
        let ev = events("data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"add\",\"args\":{}}}]},\"finishReason\":\"STOP\"}]}\r\n\r\n");
        let id = ev
            .iter()
            .find_map(|e| match e {
                GenerationEvent::ToolCall { id, .. } => Some(id.clone()),
                _ => None,
            })
            .unwrap();
        assert!(id.starts_with(LOCAL_ID));
        let b = build_body(&request(vec![Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult { id, name: "add".into(), output: json!({ "sum": 3 }) }],
            replay: None,
        }]));
        assert!(b["contents"][0]["parts"][0]["functionResponse"].get("id").is_none());
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

    async fn collect(g: &Gemini, key: &str, req: GenerationRequest) -> Vec<Result<GenerationEvent>> {
        g.generate(key.to_string(), req).collect().await
    }

    #[tokio::test]
    async fn streams_a_reply_with_the_key_in_a_header_only() {
        let (base, seen) = serve_with(vec![Reply::sse(STREAM)]);
        let g = Gemini::with_base(base, Duration::ZERO);
        let out = collect(&g, "secret-key", request(vec![Turn::text(TurnRole::User, "hi")])).await;
        assert!(out.iter().all(Result::is_ok));
        assert!(out.iter().any(|e| matches!(e, Ok(GenerationEvent::Text(t)) if t == "Hel")));
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.to_lowercase().contains("x-goog-api-key: secret-key"));
        assert!(!seen[0].0.contains("secret-key\r\n\r\n") && !seen[0].0.lines().next().unwrap().contains("secret-key"));
        assert!(seen[0].0.starts_with("POST /models/m:streamGenerateContent?alt=sse"));
    }

    #[tokio::test]
    async fn retries_a_503_before_any_text() {
        let (base, seen) = serve_with(vec![
            Reply::json(503, r#"{"error":{"message":"high demand"}}"#),
            Reply::sse(STREAM),
        ]);
        let g = Gemini::with_base(base, Duration::ZERO);
        let out = collect(&g, "k", request(vec![Turn::text(TurnRole::User, "hi")])).await;
        assert!(out.iter().all(Result::is_ok));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_tool_round_trip_sends_the_call_then_its_result() {
        let (base, seen) = serve_with(vec![Reply::sse(TOOL_STREAM), Reply::sse(STREAM)]);
        let g = Gemini::with_base(base, Duration::ZERO);
        let mut req = request(vec![Turn::text(TurnRole::User, "1+2?")]);
        req.tools = vec![add_tool()];
        let first: Vec<GenerationEvent> = collect(&g, "k", req.clone()).await.into_iter().map(|e| e.unwrap()).collect();
        let (mut call, mut replay) = (None, None);
        for e in first {
            match e {
                GenerationEvent::ToolCall { id, name, args } => call = Some((id, name, args)),
                GenerationEvent::Replay(v) => replay = Some(v),
                _ => {}
            }
        }
        let (id, name, args) = call.unwrap();
        req.turns.push(Turn {
            role: TurnRole::Model,
            parts: vec![Part::ToolCall { id: id.clone(), name: name.clone(), args }],
            replay: Some(Replay { provider: "gemini".into(), data: replay.unwrap() }),
        });
        req.turns.push(Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult { id, name, output: json!({ "sum": 3 }) }],
            replay: None,
        });
        assert!(collect(&g, "k", req).await.iter().all(Result::is_ok));
        let seen = seen.lock().unwrap();
        let body: Value = serde_json::from_str(&seen[1].1).unwrap();
        assert_eq!(body["contents"][1]["parts"][1]["thoughtSignature"], "sig-b");
        assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["response"]["sum"], 3);
    }

    /// Live check of the tool round trip. Needs GEMINI_API_KEY and GEMINI_TEST_MODEL (a model id
    /// from the model list, never hardcoded in the app).
    /// `cargo test live_gemini_tools -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs GEMINI_API_KEY and network"]
    async fn live_gemini_tools() {
        let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY");
        let model = std::env::var("GEMINI_TEST_MODEL").expect("GEMINI_TEST_MODEL");
        let g = Gemini::new();
        let mut req = GenerationRequest {
            model,
            system: Some("Use the add tool for any addition. Do not add numbers yourself.".into()),
            turns: vec![Turn::text(TurnRole::User, "What is 1234 plus 4321?")],
            tools: vec![ToolSpec {
                name: "add".into(),
                description: "Adds two numbers".into(),
                parameters: json!({
                    "type": "object",
                    "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
                    "required": ["a", "b"]
                }),
            }],
        };
        let mut call = None;
        let mut replay = None;
        for e in collect(&g, &key, req.clone()).await {
            match e.unwrap() {
                GenerationEvent::ToolCall { id, name, args } => call = Some((id, name, args)),
                GenerationEvent::Replay(v) => replay = Some(v),
                ev => println!("first: {ev:?}"),
            }
        }
        let (id, name, args) = call.expect("the model should call the tool");
        println!("call {name} {args}; replay {replay:?}");
        let sum = args["a"].as_f64().unwrap() + args["b"].as_f64().unwrap();
        req.turns.push(Turn {
            role: TurnRole::Model,
            parts: vec![Part::ToolCall { id: id.clone(), name: name.clone(), args }],
            replay: replay.map(|data| Replay { provider: "gemini".into(), data }),
        });
        req.turns.push(Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult { id, name, output: json!({ "sum": sum }) }],
            replay: None,
        });
        let mut text = String::new();
        for e in collect(&g, &key, req).await {
            match e.unwrap() {
                GenerationEvent::Text(t) => text.push_str(&t),
                ev @ (GenerationEvent::Usage { .. } | GenerationEvent::Finished(_)) => println!("second: {ev:?}"),
                _ => {}
            }
        }
        println!("answer: {text}");
        assert!(text.replace([',', ' '], "").contains("5555"), "answer was: {text}");
    }
}
