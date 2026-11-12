//! Provider abstraction. Providers get the API key per call and never store it.

pub mod gemini;
pub mod openai;
pub mod sse;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use futures_util::future::BoxFuture;

use crate::domain::ModelInfo;
use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRole {
    User,
    Model,
}

/// One piece of a turn. Tool results travel in a `User` turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    ToolCall { id: String, name: String, args: Value },
    ToolResult { id: String, name: String, output: Value },
}

/// A provider's own record of a model turn (Gemini: the parts with their thought signatures,
/// OpenAI: the response output items including encrypted reasoning). It is replayed as is when the
/// turn goes back to the same provider, because tool calling can fail without it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    pub provider: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub role: TurnRole,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<Replay>,
}

impl Turn {
    pub fn text(role: TurnRole, text: impl Into<String>) -> Self {
        Turn { role, parts: vec![Part::Text { text: text.into() }], replay: None }
    }
}

/// A function the model may call. `parameters` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub model: String,
    pub system: Option<String>,
    pub turns: Vec<Turn>,
    /// Empty means no tools are sent.
    pub tools: Vec<ToolSpec>,
    /// Keep `tools` declared (the history has calls) but forbid the model from calling any.
    pub no_tool_calls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    /// Ran into the output limit.
    Length,
    /// The model wants its tool calls answered.
    ToolCalls,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationEvent {
    Text(String),
    /// A complete tool call (providers buffer argument deltas).
    ToolCall { id: String, name: String, args: Value },
    /// The provider's record of the finished model turn, sent once before `Finished`.
    Replay(Value),
    Usage {
        input_tokens: u32,
        /// Includes thinking or reasoning tokens, the cost maths depends on it.
        output_tokens: u32,
        /// Part of `input_tokens` served from the provider's prompt cache.
        cached_tokens: u32,
    },
    Finished(FinishReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub tools: bool,
    /// The provider caches repeated prompt prefixes without being asked.
    pub implicit_caching: bool,
    pub vision: bool,
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<GenerationEvent>> + Send>>;

pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn list_models<'a>(&'a self, api_key: &'a str) -> BoxFuture<'a, Result<Vec<ModelInfo>>>;
    /// Dropping the stream cancels the request.
    fn generate(&self, api_key: String, request: GenerationRequest) -> EventStream;
}

#[derive(Clone)]
pub struct Providers(Arc<HashMap<&'static str, Arc<dyn Provider>>>);

impl Providers {
    pub fn new() -> Self {
        let mut map: HashMap<&'static str, Arc<dyn Provider>> = HashMap::new();
        let gemini: Arc<dyn Provider> = Arc::new(gemini::Gemini::new());
        map.insert(gemini.id(), gemini);
        let openai: Arc<dyn Provider> = Arc::new(openai::OpenAi::new());
        map.insert(openai.id(), openai);
        Providers(Arc::new(map))
    }

    /// Provider ids in a fixed order.
    pub fn ids(&self) -> Vec<&'static str> {
        let mut ids: Vec<&'static str> = self.0.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Provider>> {
        self.0
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Invalid(format!("Unknown provider: {id}")))
    }
}

/// Runs one request to the end and returns its text with the token counts (input, output,
/// cached). For short calls that need the whole reply, such as picking an order or reading details.
pub async fn collect_text(
    provider: &Arc<dyn Provider>,
    api_key: &str,
    request: GenerationRequest,
    timeout: std::time::Duration,
) -> Result<(String, (u32, u32, u32))> {
    use futures_util::StreamExt;
    let mut stream = provider.generate(api_key.to_string(), request);
    let (mut reply, mut tokens) = (String::new(), (0, 0, 0));
    let read = async {
        while let Some(ev) = stream.next().await {
            match ev? {
                GenerationEvent::Text(t) => reply.push_str(&t),
                GenerationEvent::Usage { input_tokens, output_tokens, cached_tokens } => tokens = (input_tokens, output_tokens, cached_tokens),
                _ => {}
            }
        }
        Ok::<(), AppError>(())
    };
    tokio::time::timeout(timeout, read).await.map_err(|_| AppError::Invalid("The request timed out.".into()))??;
    Ok((reply, tokens))
}

/// Providers want alternating roles. Join consecutive turns from the same role. Text that meets
/// text is joined with a blank line. A merged turn loses its replay record.
pub fn merge_turns(turns: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for t in turns {
        match out.last_mut() {
            Some(last) if last.role == t.role => {
                last.replay = None;
                let mut parts = t.parts.into_iter().peekable();
                if let (Some(Part::Text { text: tail }), Some(Part::Text { .. })) =
                    (last.parts.last_mut(), parts.peek())
                    && let Some(Part::Text { text }) = parts.next()
                {
                    tail.push_str("\n\n");
                    tail.push_str(&text);
                }
                last.parts.extend(parts);
            }
            _ => out.push(t),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_consecutive_same_role_turns() {
        let t = |role, s: &str| Turn::text(role, s);
        let merged = merge_turns(vec![
            t(TurnRole::User, "a"),
            t(TurnRole::User, "b"),
            t(TurnRole::Model, "c"),
            t(TurnRole::User, "d"),
        ]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].parts, vec![Part::Text { text: "a\n\nb".into() }]);
    }

    #[test]
    fn merging_keeps_tool_parts_and_drops_replay() {
        let call = Part::ToolCall { id: "1".into(), name: "f".into(), args: serde_json::json!({}) };
        let mut a = Turn::text(TurnRole::Model, "thinking");
        a.replay = Some(Replay { provider: "x".into(), data: serde_json::json!([]) });
        let b = Turn { role: TurnRole::Model, parts: vec![call.clone()], replay: None };
        let c = Turn::text(TurnRole::Model, "done");
        let merged = merge_turns(vec![a, b, c]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].parts.len(), 3);
        assert_eq!(merged[0].parts[1], call);
        assert!(merged[0].replay.is_none());
    }

    #[test]
    fn turns_survive_a_json_round_trip() {
        let turn = Turn {
            role: TurnRole::Model,
            parts: vec![Part::ToolCall { id: "c1".into(), name: "search".into(), args: serde_json::json!({"q": "x"}) }],
            replay: Some(Replay { provider: "openai".into(), data: serde_json::json!([{"type": "reasoning"}]) }),
        };
        let back: Turn = serde_json::from_str(&serde_json::to_string(&turn).unwrap()).unwrap();
        assert_eq!(back, turn);
    }

    #[test]
    fn unknown_provider_is_an_error() {
        assert!(Providers::new().get("nope").is_err());
        assert!(Providers::new().get("gemini").is_ok());
        assert!(Providers::new().get("openai").is_ok());
    }
}
