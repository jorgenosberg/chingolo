//! OpenAI through the Responses API (`POST /v1/responses`). Chat Completions can't do tool
//! calling with reasoning on newer models, and the Responses API is the one OpenAI recommends.
//! Nothing is stored on OpenAI's side (`store: false`), so the conversation is sent every time and
//! the model's own output items (including encrypted reasoning) are replayed with tool results.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::sse::SseParser;
use super::{
    Capabilities, EventStream, FinishReason, GenerationEvent, GenerationRequest, Part, Provider,
    Turn, TurnRole, merge_turns,
};
use crate::domain::ModelInfo;
use crate::error::{AppError, Result};
use crate::services::pricing;

const BASE: &str = "https://api.openai.com/v1";
/// Reasoning models can think for a long time without sending an event, so this is longer than
/// Gemini's.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const RETRIES: u32 = 2;
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// A `Retry-After` longer than this is not waited for.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);
/// Parts of a model id that mark a model that isn't a text chat model.
const SKIP_IDS: &[&str] = &[
    "audio",
    "realtime",
    "tts",
    "transcribe",
    "diarize",
    "image",
    "search",
    "embedding",
    "moderation",
    "instruct",
];

pub struct OpenAi {
    client: reqwest::Client,
    base: String,
    retry_delay: Duration,
}

impl OpenAi {
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
        OpenAi {
            client,
            base,
            retry_delay,
        }
    }
}

impl Provider for OpenAi {
    fn id(&self) -> &'static str {
        "openai"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tools: true,
            implicit_caching: true,
            vision: true,
        }
    }

    fn list_models<'a>(&'a self, api_key: &'a str) -> BoxFuture<'a, Result<Vec<ModelInfo>>> {
        Box::pin(async move {
            let resp = self
                .client
                .get(format!("{}/models", self.base))
                .bearer_auth(api_key)
                .timeout(Duration::from_secs(20))
                .send()
                .await
                .map_err(net_error)?;
            let body = check_status(resp).await?.text().await.map_err(net_error)?;
            let value: Value = serde_json::from_str(&body)
                .map_err(|e| AppError::Provider(format!("Unexpected OpenAI response: {e}")))?;
            let ids = value
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    AppError::Provider("Unexpected OpenAI response: no model list.".into())
                })?
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str));
            let mut models: Vec<ModelInfo> = ids
                .filter(|id| is_chat_model(id))
                .map(|id| {
                    let facts = pricing::facts("openai", id);
                    ModelInfo {
                        provider: "openai".to_string(),
                        id: id.to_string(),
                        // The list has no names, the price table does.
                        display_name: facts
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_else(|| id.to_string()),
                        // The model list has no limits, the price table does.
                        input_token_limit: pricing::context_limit("openai", id),
                        output_token_limit: None,
                        facts,
                    }
                })
                .collect();
            models.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(models)
        })
    }

    fn generate(&self, api_key: String, request: GenerationRequest) -> EventStream {
        let (tx, mut rx) = mpsc::channel::<Result<GenerationEvent>>(64);
        let (client, base, retry_delay) =
            (self.client.clone(), self.base.clone(), self.retry_delay);
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

/// Text models only: `gpt-*` and the `o<digit>` reasoning models, minus audio, image and the like.
fn is_chat_model(id: &str) -> bool {
    let family = id.starts_with("gpt-")
        || id
            .strip_prefix('o')
            .and_then(|r| r.chars().next())
            .is_some_and(|c| c.is_ascii_digit());
    family && !SKIP_IDS.iter().any(|s| id.contains(s))
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
    let body = build_body(&request);
    let mut attempt = 0;
    let resp = loop {
        let resp = client
            .post(format!("{base}/responses"))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(net_error)?;
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            break resp;
        }
        let wait = retry_after(&resp);
        let text = resp.text().await.unwrap_or_default();
        // Nothing has streamed yet, so a retry can't repeat anything the user saw.
        if is_retryable(status, &text) && attempt < RETRIES {
            attempt += 1;
            let wait = wait.map_or(retry_delay, |w| w.min(MAX_RETRY_WAIT));
            log::warn!(
                "openai {status}, retry {attempt} of {RETRIES} in {} ms",
                wait.as_millis()
            );
            tokio::time::sleep(wait).await;
            continue;
        }
        return Err(AppError::Provider(describe_error(status, &text)));
    };

    let mut parser = SseParser::default();
    let mut state = StreamState::default();
    let mut bytes = resp.bytes_stream();
    loop {
        let next = tokio::time::timeout(IDLE_TIMEOUT, bytes.next())
            .await
            .map_err(|_| AppError::Provider("OpenAI stopped responding.".into()))?;
        let (payloads, ended) = match next {
            Some(chunk) => (parser.push(&chunk.map_err(net_error)?), false),
            None => (parser.finish(), true),
        };
        for p in payloads {
            if p.trim() == "[DONE]" {
                continue;
            }
            let value: Value = serde_json::from_str(&p)
                .map_err(|e| AppError::Provider(format!("Unexpected OpenAI response: {e}")))?;
            for ev in parse_event(&value, &mut state)? {
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
    let mut input: Vec<Value> = Vec::new();
    for turn in merge_turns(req.turns.clone()) {
        input.extend(turn_items(&turn));
    }
    let mut body = json!({ "model": req.model, "input": input, "stream": true, "store": false });
    if let Some(system) = &req.system {
        body["instructions"] = json!(system);
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                    "strict": false,
                })
            })
            .collect();
        body["tools"] = json!(tools);
        if req.no_tool_calls {
            body["tool_choice"] = json!("none");
        }
    }
    body
}

/// A model turn that came from OpenAI goes back as the output items it produced, so encrypted
/// reasoning travels with its tool calls.
fn turn_items(t: &Turn) -> Vec<Value> {
    if t.role == TurnRole::Model
        && let Some(r) = &t.replay
        && r.provider == "openai"
        && let Some(items) = r.data.as_array()
    {
        return items.clone();
    }
    let role = match t.role {
        TurnRole::User => "user",
        TurnRole::Model => "assistant",
    };
    t.parts
        .iter()
        .map(|p| match p {
            Part::Text { text } => json!({ "role": role, "content": text }),
            Part::ToolCall { id, name, args } => {
                json!({ "type": "function_call", "call_id": id, "name": name, "arguments": args.to_string() })
            }
            Part::ToolResult { id, output, .. } => {
                let output = match output {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                json!({ "type": "function_call_output", "call_id": id, "output": output })
            }
        })
        .collect()
}

struct PendingCall {
    call_id: String,
    name: String,
    args: String,
}

/// Function calls arrive as an item and then argument deltas. They are sent on once complete.
#[derive(Default)]
pub struct StreamState {
    pending: HashMap<String, PendingCall>,
    done: HashSet<String>,
    calls: u32,
}

impl StreamState {
    fn finish_call(
        &mut self,
        key: &str,
        full_args: Option<&str>,
    ) -> Result<Option<GenerationEvent>> {
        if self.done.contains(key) {
            return Ok(None);
        }
        let Some(call) = self.pending.remove(key) else {
            return Ok(None);
        };
        let raw = full_args.unwrap_or(&call.args);
        let args: Value = if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw).map_err(|_| {
                AppError::Provider(format!(
                    "OpenAI sent tool call arguments for {} that are not valid JSON.",
                    call.name
                ))
            })?
        };
        self.done.insert(key.to_string());
        self.calls += 1;
        Ok(Some(GenerationEvent::ToolCall {
            id: call.call_id,
            name: call.name,
            args,
        }))
    }
}

fn item_key(v: &Value) -> String {
    v.get("item_id")
        .or_else(|| v.pointer("/item/id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| v.get("output_index").map(|i| format!("index-{i}")))
        .unwrap_or_default()
}

/// Turns one streamed event into events. Event names follow the OpenAI streaming guide.
pub fn parse_event(v: &Value, state: &mut StreamState) -> Result<Vec<GenerationEvent>> {
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str).unwrap_or_default() {
        "response.output_text.delta" => {
            if let Some(d) = v.get("delta").and_then(Value::as_str)
                && !d.is_empty()
            {
                out.push(GenerationEvent::Text(d.to_string()));
            }
        }
        "response.output_item.added" => {
            let item = v.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                let text = |k: &str| {
                    item.get(k)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                state.pending.insert(
                    item_key(v),
                    PendingCall {
                        call_id: text("call_id"),
                        name: text("name"),
                        args: text("arguments"),
                    },
                );
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(call) = state.pending.get_mut(&item_key(v))
                && let Some(d) = v.get("delta").and_then(Value::as_str)
            {
                call.args.push_str(d);
            }
        }
        "response.function_call_arguments.done" => {
            let args = v.get("arguments").and_then(Value::as_str);
            out.extend(state.finish_call(&item_key(v), args)?);
        }
        // Not every stream is documented to send the events above, so a finished item is a backup.
        "response.output_item.done" => {
            let item = v.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                out.extend(
                    state
                        .finish_call(&item_key(v), item.get("arguments").and_then(Value::as_str))?,
                );
            }
        }
        "response.completed" => {
            let resp = v.get("response").unwrap_or(&Value::Null);
            if let Some(items) = resp.get("output").and_then(Value::as_array)
                && !items.is_empty()
            {
                out.push(GenerationEvent::Replay(Value::Array(items.clone())));
            }
            out.extend(usage(resp));
            let reason = if state.calls > 0 {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            };
            out.push(GenerationEvent::Finished(reason));
        }
        "response.incomplete" => {
            let resp = v.get("response").unwrap_or(&Value::Null);
            out.extend(usage(resp));
            let reason = match resp
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
            {
                Some("max_output_tokens") => FinishReason::Length,
                Some(other) => FinishReason::Other(other.to_string()),
                None => FinishReason::Other("incomplete".to_string()),
            };
            out.push(GenerationEvent::Finished(reason));
        }
        "response.failed" => {
            let message = v
                .pointer("/response/error/message")
                .and_then(Value::as_str)
                .unwrap_or("it failed");
            return Err(AppError::Provider(format!(
                "OpenAI could not finish the reply: {message}"
            )));
        }
        "error" => {
            let message = v
                .get("message")
                .or_else(|| v.pointer("/error/message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(AppError::Provider(format!("OpenAI error: {message}")));
        }
        _ => {}
    }
    Ok(out)
}

/// `output_tokens` already includes reasoning tokens, so they are not added again.
fn usage(resp: &Value) -> Option<GenerationEvent> {
    let u = resp.get("usage").filter(|u| !u.is_null())?;
    let n = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0) as u32;
    Some(GenerationEvent::Usage {
        input_tokens: n("/input_tokens"),
        output_tokens: n("/output_tokens"),
        cached_tokens: n("/input_tokens_details/cached_tokens"),
    })
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs: f64 = resp
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Overloaded servers and plain rate limits clear up. Running out of credit does not.
fn is_retryable(status: u16, body: &str) -> bool {
    match status {
        503 => true,
        429 => !is_billing_limit(body),
        _ => false,
    }
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/code")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn is_billing_limit(body: &str) -> bool {
    let code = error_code(body);
    code.contains("insufficient_quota")
        || code.contains("credit_balance_exhausted")
        || code.contains("spend_limit")
}

pub(crate) fn net_error(e: reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::Provider("The request to OpenAI timed out.".into())
    } else if e.is_connect() {
        AppError::Provider("Couldn't reach OpenAI. Check your internet connection.".into())
    } else {
        // The key is sent as a header, so it is never in the URL that reqwest may print.
        AppError::Provider(format!(
            "Network error talking to OpenAI: {}",
            error_chain(&e)
        ))
    }
}

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
    let message = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let with = |text: &str| match &message {
        Some(m) => format!("{text} {m}"),
        None => text.to_string(),
    };
    match status {
        401 => with("OpenAI rejected the API key."),
        403 => with("OpenAI refused the request. It may not be available in your region."),
        429 if is_billing_limit(body) => {
            with("The OpenAI account is out of credit or over its spend limit.")
        }
        429 => with("OpenAI rate limit reached."),
        s => match message {
            Some(m) => format!("OpenAI error {s}: {m}"),
            None => format!("OpenAI error {s}."),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai::{Replay, ToolSpec, sse::SseParser};
    use crate::services::testserver::{Reply, serve_with};

    // Event names and fields follow the OpenAI streaming guide. The live test checks them.
    const TEXT_STREAM: &str = concat!(
        "event: response.created\r\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\r\n\r\n",
        "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"Hel\"}\r\n\r\n",
        "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"lo\"}\r\n\r\n",
        "event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":4},\"output_tokens\":8,\"output_tokens_details\":{\"reasoning_tokens\":5},\"total_tokens\":18}}}\r\n\r\n",
    );

    const TOOL_STREAM: &str = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\r\n\r\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"add\",\"arguments\":\"\"}}\r\n\r\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":1,\"delta\":\"{\\\"a\\\":1,\"}\r\n\r\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"output_index\":1,\"delta\":\"\\\"b\\\":2}\"}\r\n\r\n",
        "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"output_index\":1,\"arguments\":\"{\\\"a\\\":1,\\\"b\\\":2}\"}\r\n\r\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"add\",\"arguments\":\"{\\\"a\\\":1,\\\"b\\\":2}\"}}\r\n\r\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"reasoning\",\"id\":\"rs_1\",\"encrypted_content\":\"enc\"},{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"add\",\"arguments\":\"{\\\"a\\\":1,\\\"b\\\":2}\"}],\"usage\":{\"input_tokens\":12,\"output_tokens\":3}}}\r\n\r\n",
    );

    fn events(stream: &str) -> Vec<GenerationEvent> {
        let mut p = SseParser::default();
        let mut state = StreamState::default();
        let mut out = Vec::new();
        for payload in p.push(stream.as_bytes()) {
            let v: Value = serde_json::from_str(&payload).unwrap();
            out.extend(parse_event(&v, &mut state).unwrap());
        }
        out
    }

    fn request(turns: Vec<Turn>) -> GenerationRequest {
        GenerationRequest {
            model: "m".into(),
            system: None,
            turns,
            tools: Vec::new(),
            no_tool_calls: false,
        }
    }

    fn add_tool() -> ToolSpec {
        ToolSpec {
            name: "add".into(),
            description: "Adds two numbers".into(),
            parameters: json!({ "type": "object", "properties": { "a": { "type": "number" } } }),
        }
    }

    #[test]
    fn parses_text_usage_and_finish() {
        let ev = events(TEXT_STREAM);
        let text: String = ev
            .iter()
            .filter_map(|e| match e {
                GenerationEvent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello");
        // Reasoning tokens are already inside output_tokens and must not be added again.
        assert!(ev.contains(&GenerationEvent::Usage {
            input_tokens: 10,
            output_tokens: 8,
            cached_tokens: 4
        }));
        assert_eq!(
            ev.last(),
            Some(&GenerationEvent::Finished(FinishReason::Stop))
        );
    }

    #[test]
    fn a_tool_call_is_sent_once_with_joined_arguments_and_the_replay_keeps_reasoning() {
        let ev = events(TOOL_STREAM);
        let calls: Vec<_> = ev
            .iter()
            .filter(|e| matches!(e, GenerationEvent::ToolCall { .. }))
            .collect();
        assert_eq!(calls.len(), 1, "the done events must not repeat the call");
        assert_eq!(
            calls[0],
            &GenerationEvent::ToolCall {
                id: "call_1".into(),
                name: "add".into(),
                args: json!({ "a": 1, "b": 2 })
            }
        );
        assert_eq!(
            ev.last(),
            Some(&GenerationEvent::Finished(FinishReason::ToolCalls))
        );
        let replay = ev
            .iter()
            .find_map(|e| match e {
                GenerationEvent::Replay(v) => Some(v.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(replay[0]["encrypted_content"], "enc");
        assert_eq!(replay.as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_call_is_still_found_when_only_the_item_done_event_arrives() {
        let ev = events(concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_9\",\"call_id\":\"c9\",\"name\":\"add\",\"arguments\":\"\"}}\r\n\r\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_9\",\"call_id\":\"c9\",\"name\":\"add\",\"arguments\":\"{}\"}}\r\n\r\n",
        ));
        assert_eq!(
            ev,
            vec![GenerationEvent::ToolCall {
                id: "c9".into(),
                name: "add".into(),
                args: json!({})
            }]
        );
    }

    #[test]
    fn broken_tool_arguments_are_an_error() {
        let mut state = StreamState::default();
        let added = json!({ "type": "response.output_item.added", "item": { "type": "function_call", "id": "f", "call_id": "c", "name": "add" } });
        parse_event(&added, &mut state).unwrap();
        let done = json!({ "type": "response.function_call_arguments.done", "item_id": "f", "arguments": "{\"a\":" });
        assert!(parse_event(&done, &mut state).is_err());
    }

    #[test]
    fn incomplete_and_failed_responses() {
        let mut state = StreamState::default();
        let ev = parse_event(
            &json!({ "type": "response.incomplete", "response": { "incomplete_details": { "reason": "max_output_tokens" },
                "usage": { "input_tokens": 1, "output_tokens": 2 } } }),
            &mut state,
        )
        .unwrap();
        assert_eq!(
            ev.last(),
            Some(&GenerationEvent::Finished(FinishReason::Length))
        );
        let filtered = parse_event(
            &json!({ "type": "response.incomplete", "response": { "incomplete_details": { "reason": "content_filter" } } }),
            &mut state,
        )
        .unwrap();
        assert_eq!(
            filtered,
            vec![GenerationEvent::Finished(FinishReason::Other(
                "content_filter".into()
            ))]
        );
        let failed = json!({ "type": "response.failed", "response": { "error": { "message": "boom" }, "usage": null } });
        assert!(
            parse_event(&failed, &mut state)
                .unwrap_err()
                .to_string()
                .contains("boom")
        );
        assert!(parse_event(&json!({ "type": "error", "message": "bad" }), &mut state).is_err());
    }

    #[test]
    fn body_does_not_store_and_declares_tools() {
        let mut req = request(vec![
            Turn::text(TurnRole::User, "hi"),
            Turn::text(TurnRole::Model, "yo"),
        ]);
        req.system = Some("be brief".into());
        req.tools = vec![add_tool()];
        let b = build_body(&req);
        assert_eq!(b["store"], false);
        assert_eq!(b["stream"], true);
        assert_eq!(b["instructions"], "be brief");
        assert_eq!(b["input"][0], json!({ "role": "user", "content": "hi" }));
        assert_eq!(
            b["input"][1],
            json!({ "role": "assistant", "content": "yo" })
        );
        assert_eq!(b["tools"][0]["type"], "function");
        assert_eq!(b["tools"][0]["name"], "add");
        assert!(b["tools"][0]["parameters"].is_object());
        assert!(build_body(&request(vec![]))["tools"].is_null());
        assert!(b.get("tool_choice").is_none());
        req.no_tool_calls = true;
        let b = build_body(&req);
        assert_eq!(b["tool_choice"], "none");
        assert_eq!(b["tools"][0]["name"], "add");
    }

    #[test]
    fn body_replays_openai_output_and_answers_calls_with_matching_ids() {
        let output = json!([
            { "type": "reasoning", "id": "rs_1", "encrypted_content": "enc" },
            { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "add", "arguments": "{}" },
        ]);
        let model = Turn {
            role: TurnRole::Model,
            parts: vec![Part::ToolCall {
                id: "call_1".into(),
                name: "add".into(),
                args: json!({ "a": 1 }),
            }],
            replay: Some(Replay {
                provider: "openai".into(),
                data: output.clone(),
            }),
        };
        let result = Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult {
                id: "call_1".into(),
                name: "add".into(),
                output: json!({ "sum": 3 }),
            }],
            replay: None,
        };
        let b = build_body(&request(vec![
            Turn::text(TurnRole::User, "1+2?"),
            model.clone(),
            result.clone(),
        ]));
        assert_eq!(b["input"][1], output[0]);
        assert_eq!(b["input"][2], output[1]);
        assert_eq!(b["input"][3]["type"], "function_call_output");
        assert_eq!(b["input"][3]["call_id"], "call_1");
        assert_eq!(b["input"][3]["output"], "{\"sum\":3}");

        // Another provider's replay is ignored and the call is rebuilt from its parts.
        let mut other = model;
        other.replay = Some(Replay {
            provider: "gemini".into(),
            data: json!([]),
        });
        let b = build_body(&request(vec![other, result]));
        assert_eq!(b["input"][0]["type"], "function_call");
        assert_eq!(b["input"][0]["call_id"], "call_1");
        assert_eq!(b["input"][0]["arguments"], "{\"a\":1}");
    }

    #[test]
    fn error_messages_are_readable() {
        let key = r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error","code":"invalid_api_key"}}"#;
        assert!(describe_error(401, key).starts_with("OpenAI rejected the API key."));
        assert!(describe_error(401, key).contains("Incorrect API key"));
        assert!(describe_error(403, "").contains("region"));
        let credit = r#"{"error":{"message":"You exceeded your current quota","code":"insufficient_quota"}}"#;
        assert!(describe_error(429, credit).contains("out of credit"));
        assert!(
            describe_error(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#)
                .contains("rate limit")
        );
        assert_eq!(describe_error(500, "not json"), "OpenAI error 500.");
        assert!(!is_retryable(429, credit));
        assert!(is_retryable(429, "{}"));
        assert!(is_retryable(503, "{}"));
        assert!(!is_retryable(500, "{}"));
    }

    #[test]
    fn model_list_keeps_text_chat_models() {
        for keep in ["gpt-5.4", "gpt-4o-mini", "o3", "o4-mini", "gpt-5.4-pro"] {
            assert!(is_chat_model(keep), "{keep}");
        }
        for skip in [
            "gpt-image-2",
            "gpt-realtime-2.1",
            "gpt-4o-audio-preview",
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "gpt-4o-search-preview",
            "gpt-3.5-turbo-instruct",
            "text-embedding-3-small",
            "omni-moderation-latest",
            "dall-e-3",
            "whisper-1",
            "tts-1",
            "babbage-002",
        ] {
            assert!(!is_chat_model(skip), "{skip}");
        }
    }

    async fn collect(
        g: &OpenAi,
        key: &str,
        req: GenerationRequest,
    ) -> Vec<Result<GenerationEvent>> {
        g.generate(key.to_string(), req).collect().await
    }

    #[tokio::test]
    async fn streams_a_reply_with_the_key_in_a_header_only() {
        let (base, seen) = serve_with(vec![Reply::sse(TEXT_STREAM)]);
        let g = OpenAi::with_base(base, Duration::ZERO);
        let out = collect(
            &g,
            "sk-secret",
            request(vec![Turn::text(TurnRole::User, "hi")]),
        )
        .await;
        assert!(out.iter().all(Result::is_ok));
        assert!(
            out.iter()
                .any(|e| matches!(e, Ok(GenerationEvent::Text(t)) if t == "Hel"))
        );
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.starts_with("POST /responses "));
        assert!(
            seen[0]
                .0
                .to_lowercase()
                .contains("authorization: bearer sk-secret")
        );
        assert!(!seen[0].0.lines().next().unwrap().contains("sk-secret"));
        assert!(!seen[0].1.contains("sk-secret"));
    }

    #[tokio::test]
    async fn retries_a_rate_limit_and_an_overload_then_succeeds() {
        let (base, seen) = serve_with(vec![
            Reply::json(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#)
                .header("Retry-After", "0"),
            Reply::json(503, r#"{"error":{"code":"server_is_overloaded"}}"#),
            Reply::sse(TEXT_STREAM),
        ]);
        let g = OpenAi::with_base(base, Duration::ZERO);
        let out = collect(&g, "k", request(vec![Turn::text(TurnRole::User, "hi")])).await;
        assert!(out.iter().all(Result::is_ok));
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn running_out_of_credit_is_not_retried() {
        let credit = r#"{"error":{"message":"You exceeded your current quota","code":"insufficient_quota"}}"#;
        let (base, seen) = serve_with(vec![Reply::json(429, credit)]);
        let g = OpenAi::with_base(base, Duration::ZERO);
        let out = collect(&g, "k", request(vec![Turn::text(TurnRole::User, "hi")])).await;
        assert_eq!(seen.lock().unwrap().len(), 1);
        let err = out.into_iter().find_map(|e| e.err()).unwrap().to_string();
        assert!(err.contains("out of credit"), "{err}");
    }

    #[tokio::test]
    async fn a_tool_round_trip_replays_reasoning_and_answers_the_call() {
        let (base, seen) = serve_with(vec![Reply::sse(TOOL_STREAM), Reply::sse(TEXT_STREAM)]);
        let g = OpenAi::with_base(base, Duration::ZERO);
        let mut req = request(vec![Turn::text(TurnRole::User, "1+2?")]);
        req.tools = vec![add_tool()];
        let first: Vec<GenerationEvent> = collect(&g, "k", req.clone())
            .await
            .into_iter()
            .map(|e| e.unwrap())
            .collect();
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
            parts: vec![Part::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args,
            }],
            replay: Some(Replay {
                provider: "openai".into(),
                data: replay.unwrap(),
            }),
        });
        req.turns.push(Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult {
                id,
                name,
                output: json!({ "sum": 3 }),
            }],
            replay: None,
        });
        assert!(collect(&g, "k", req).await.iter().all(Result::is_ok));
        let seen = seen.lock().unwrap();
        let body: Value = serde_json::from_str(&seen[1].1).unwrap();
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][1]["encrypted_content"], "enc");
        assert_eq!(body["input"][3]["call_id"], "call_1");
    }

    #[tokio::test]
    async fn lists_chat_models_with_the_key_as_a_bearer_token() {
        let list = json!({ "object": "list", "data": [
            { "id": "gpt-4o-mini" }, { "id": "text-embedding-3-small" }, { "id": "o3" }, { "id": "gpt-image-2" },
        ]});
        let (base, seen) = serve_with(vec![Reply::json(200, list.to_string())]);
        let models = OpenAi::with_base(base, Duration::ZERO)
            .list_models("sk-secret")
            .await
            .unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["gpt-4o-mini", "o3"]
        );
        assert!(
            models[0].input_token_limit.is_some(),
            "context comes from the price table"
        );
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.starts_with("GET /models "));
        assert!(
            seen[0]
                .0
                .to_lowercase()
                .contains("authorization: bearer sk-secret")
        );
    }

    #[tokio::test]
    async fn a_rejected_key_says_so() {
        let body = r#"{"error":{"message":"Incorrect API key provided","code":"invalid_api_key"}}"#;
        let (base, _) = serve_with(vec![Reply::json(401, body)]);
        let err = OpenAi::with_base(base, Duration::ZERO)
            .list_models("bad")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("OpenAI rejected the API key."), "{err}");
    }

    /// Live check of the tool round trip. Needs OPENAI_API_KEY and OPENAI_TEST_MODEL (a model id
    /// from the model list, never hardcoded in the app).
    /// `cargo test live_openai_tools -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs OPENAI_API_KEY and network"]
    async fn live_openai_tools() {
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let model = std::env::var("OPENAI_TEST_MODEL").expect("OPENAI_TEST_MODEL");
        let g = OpenAi::new();
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
                    "required": ["a", "b"],
                    "additionalProperties": false
                }),
            }],
            no_tool_calls: false,
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
            parts: vec![Part::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args,
            }],
            replay: replay.map(|data| Replay {
                provider: "openai".into(),
                data,
            }),
        });
        req.turns.push(Turn {
            role: TurnRole::User,
            parts: vec![Part::ToolResult {
                id,
                name,
                output: json!({ "sum": sum }),
            }],
            replay: None,
        });
        let mut text = String::new();
        for e in collect(&g, &key, req).await {
            match e.unwrap() {
                GenerationEvent::Text(t) => text.push_str(&t),
                ev @ (GenerationEvent::Usage { .. } | GenerationEvent::Finished(_)) => {
                    println!("second: {ev:?}")
                }
                _ => {}
            }
        }
        println!("answer: {text}");
        assert!(
            text.replace([',', ' '], "").contains("5555"),
            "answer was: {text}"
        );
    }
}
