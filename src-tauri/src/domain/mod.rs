//! Plain data types and validation. No IO, no provider or UI details.

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use crate::error::{AppError, Result};

pub type Id = String;

pub const DEFAULT_CONVERSATION_TITLE: &str = "New conversation";

pub fn new_id() -> Id {
    uuid::Uuid::now_v7().to_string()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Trim and require a non-empty name of reasonable length.
pub fn clean_name(raw: &str, what: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AppError::Invalid(format!("{what} can't be empty.")));
    }
    if s.chars().count() > 200 {
        return Err(AppError::Invalid(format!("{what} is too long.")));
    }
    Ok(s.to_string())
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: Id,
    pub name: String,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub id: Id,
    pub project_id: Id,
    pub title: String,
    pub position: i32,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: Id,
    pub project_id: Option<Id>,
    pub chapter_id: Option<Id>,
    pub title: String,
    pub provider: String,
    pub model_id: Option<String>,
    #[specta(type = Option<Number>)]
    pub archived_at: Option<i64>,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Streaming,
    Complete,
    Error,
    Cancelled,
    Interrupted,
}

impl MessageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageStatus::Streaming => "streaming",
            MessageStatus::Complete => "complete",
            MessageStatus::Error => "error",
            MessageStatus::Cancelled => "cancelled",
            MessageStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "streaming" => Some(MessageStatus::Streaming),
            "complete" => Some(MessageStatus::Complete),
            "error" => Some(MessageStatus::Error),
            "cancelled" => Some(MessageStatus::Cancelled),
            "interrupted" => Some(MessageStatus::Interrupted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Id,
    pub conversation_id: Id,
    pub role: Role,
    pub content: String,
    pub status: MessageStatus,
    pub provider: Option<String>,
    pub model_id: Option<String>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub error: Option<String>,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConversationFilter {
    /// Only conversations in this project.
    pub project_id: Option<Id>,
    /// Only conversations that belong to no project.
    #[serde(default)]
    pub unassigned_only: bool,
    #[serde(default)]
    pub include_archived: bool,
    /// Matches titles and message text.
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub onboarded: bool,
    pub privacy_acknowledged: bool,
    pub default_model: Option<String>,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub onboarded: Option<bool>,
    pub privacy_acknowledged: Option<bool>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub input_token_limit: Option<u32>,
    pub output_token_limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_trims_and_rejects_empty() {
        assert_eq!(clean_name("  Thesis ", "Name").unwrap(), "Thesis");
        assert!(clean_name("   ", "Name").is_err());
        assert!(clean_name(&"x".repeat(201), "Name").is_err());
    }

    #[test]
    fn uuid_v7_ids_sort_by_creation() {
        let a = new_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_id();
        assert!(a < b);
    }
}
