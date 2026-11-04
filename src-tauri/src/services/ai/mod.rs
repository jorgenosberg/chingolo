//! Provider abstraction. Providers get the API key per call and never store it.

pub mod gemini;
pub mod sse;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::Stream;
use futures_util::future::BoxFuture;

use crate::domain::ModelInfo;
use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRole {
    User,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: TurnRole,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub model: String,
    pub system: Option<String>,
    pub turns: Vec<Turn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationEvent {
    Text(String),
    Usage { input_tokens: u32, output_tokens: u32 },
    /// The provider's finish reason, e.g. "STOP" or "MAX_TOKENS".
    Finished(String),
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<GenerationEvent>> + Send>>;

pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
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
        Providers(Arc::new(map))
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Provider>> {
        self.0
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Invalid(format!("Unknown provider: {id}")))
    }
}

/// Providers want alternating roles. Join consecutive turns from the same role.
pub fn merge_turns(turns: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for t in turns {
        match out.last_mut() {
            Some(last) if last.role == t.role => {
                last.text.push_str("\n\n");
                last.text.push_str(&t.text);
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
        let t = |role, s: &str| Turn { role, text: s.into() };
        let merged = merge_turns(vec![
            t(TurnRole::User, "a"),
            t(TurnRole::User, "b"),
            t(TurnRole::Model, "c"),
            t(TurnRole::User, "d"),
        ]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "a\n\nb");
    }

    #[test]
    fn unknown_provider_is_an_error() {
        assert!(Providers::new().get("nope").is_err());
        assert!(Providers::new().get("gemini").is_ok());
    }
}
