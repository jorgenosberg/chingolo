//! Plain data types and validation. No IO, no provider or UI details.

pub mod library;
pub mod usage;

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
    /// Input tokens of the latest reply: roughly how much a different model would have to re-read
    /// without cached input. None if there is no reply, or the latest one failed.
    pub switch_cost_tokens: Option<u32>,
    /// Title of the conversation this one was forked from, as it was at fork time.
    pub forked_from_title: Option<String>,
    /// Last copied message of a fork. Messages up to and including it are shown collapsed.
    pub fork_boundary_message_id: Option<Id>,
    /// What the conversation may read. `None` is the default: its project if that has ready
    /// documents, else the whole library.
    pub source_scope: Option<library::Scope>,
    /// Text typed in the composer and not sent yet. Empty if none.
    pub draft: String,
    /// False for a conversation nobody has written in yet.
    pub has_messages: bool,
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
    pub cached_input_tokens: Option<u32>,
    pub error: Option<String>,
    /// The app's check of each source the reply cites, one entry per handle. Empty for replies
    /// that cite nothing.
    #[serde(default)]
    pub citations: Vec<Citation>,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

/// How a cited source held up when the app checked it. A handle that matches no source is never
/// stored: it is removed from the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CitationStatus {
    Ok,
    /// The claim shares few words with the passage it cites.
    Weak,
    /// The reply quotes the source, and the quote is not in it.
    QuoteMismatch,
}

/// A source a reply cites (`[S3]`), with what the UI needs to show it and open it.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub handle: u32,
    pub status: CitationStatus,
    /// Some(true) if the reply quotes this source and the quote is in it, Some(false) if not.
    pub quote_verified: Option<bool>,
    /// May be gone if the document was read again, then the page is the fallback.
    pub passage_id: Id,
    pub document_id: Id,
    pub title: String,
    /// Author and year such as "Boyd et al. 2004", empty if the document had neither. Replies
    /// saved before it existed don't have it.
    #[serde(default)]
    pub label: String,
    /// First page, 0 for Word documents.
    pub page: u32,
    pub snippet: String,
}

/// What a saved item holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum SavedKind {
    /// A whole reply, markdown with its `[S<n>]` handles.
    Reply,
    /// Text selected in a reply.
    Selection,
    /// Text selected in the reader.
    Passage,
}

/// A source copied into a saved item. Opens the reader even if the conversation or the document
/// record is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedSource {
    pub handle: u32,
    /// Empty if a selection in the reader matched no stored passage.
    pub passage_id: String,
    pub document_id: Id,
    pub title: String,
    pub label: String,
    /// First page, 0 for Word documents.
    pub page: u32,
    pub text: String,
    pub doi: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedNote {
    pub id: Id,
    pub body: String,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

/// A label on saved items. Shared by all projects. `color` is one of a fixed set of names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedTag {
    pub id: Id,
    pub name: String,
    pub color: String,
}

/// A tag with the number of saved items that have it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedTagInfo {
    pub id: Id,
    pub name: String,
    pub color: String,
    pub item_count: u32,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SavedItem {
    pub id: Id,
    pub kind: SavedKind,
    pub project_id: Option<Id>,
    /// A folder of the item's project. Starts as the chat's folder.
    pub chapter_id: Option<Id>,
    /// None once the conversation is deleted.
    pub conversation_id: Option<Id>,
    pub message_id: Option<Id>,
    /// The conversation's title when the item was saved.
    pub conversation_title: String,
    pub body: String,
    /// Oldest first, like comments under a post.
    pub notes: Vec<SavedNote>,
    pub sources: Vec<SavedSource>,
    /// By name.
    pub tags: Vec<SavedTag>,
    /// When it was marked as used in the user's own text.
    #[specta(type = Option<Number>)]
    pub used_at: Option<i64>,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SavedFilter {
    All,
    Project {
        id: Id,
    },
    /// Items with no project, or whose project was deleted.
    Unassigned,
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

/// One message that matched a chat search.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MessageHit {
    pub conversation_id: Id,
    pub conversation_title: String,
    pub project_id: Option<Id>,
    pub archived: bool,
    pub message_id: Id,
    pub role: Role,
    #[specta(type = Number)]
    pub created_at: i64,
    /// Matching text around the hit. Matched words sit between U+0002 and U+0003.
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub onboarded: bool,
    pub privacy_acknowledged: bool,
    pub default_model: Option<String>,
    /// The provider that `default_model` belongs to. New chats start with it.
    pub default_provider: Option<String>,
    /// Provider ids that have an API key saved in the Keychain.
    pub api_keys: Vec<String>,
    /// Why the Keychain couldn't be read, if it couldn't. `api_keys` is then incomplete.
    pub keychain_error: Option<String>,
    /// The user chose "Not now" on the search model download notice.
    pub embedding_notice_dismissed: bool,
    /// Search results are reordered by the chat model (sends the top passages to its provider).
    pub rerank_enabled: bool,
    /// Model for reranking and the provider it belongs to. Without them the chat model is used.
    pub rerank_model: Option<String>,
    pub rerank_provider: Option<String>,
    /// Look documents up online by DOI, arXiv id, PubMed id or ISBN for exact details. Only the
    /// identifier is sent. (The name is from when Crossref was the only service.)
    pub crossref_enabled: bool,
    /// Search Crossref by title for documents with no identifier. Sends the title and first
    /// author's surname.
    pub title_search_enabled: bool,
    /// A model reads the first pages of papers whose title, authors or year are missing.
    pub metadata_ai_enabled: bool,
    /// Model for that and the provider it belongs to. Without them the default chat model is used.
    pub metadata_model: Option<String>,
    pub metadata_provider: Option<String>,
    /// Models the user starred, as `provider/id`, in the order they chose. Empty means the pickers
    /// show the curated default list.
    pub favorite_models: Vec<String>,
    /// The version of the running app.
    pub app_version: String,
    /// The version whose "What's new" the user has seen. None for installs that predate it.
    pub last_seen_version: Option<String>,
    /// The reporting server the user connected to. The token is in the Keychain.
    pub reporting_url: Option<String>,
    pub has_reporting_token: bool,
    /// Send daily usage counts to the reporting server.
    pub usage_stats: bool,
    /// Send crash reports to the reporting server.
    pub crash_reports: bool,
    /// The server turned the token down on the last send.
    pub reporting_error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub onboarded: Option<bool>,
    pub last_seen_version: Option<String>,
    pub privacy_acknowledged: Option<bool>,
    pub default_model: Option<String>,
    pub default_provider: Option<String>,
    pub embedding_notice_dismissed: Option<bool>,
    pub rerank_enabled: Option<bool>,
    /// An empty string goes back to the chat model.
    pub rerank_model: Option<String>,
    pub rerank_provider: Option<String>,
    pub crossref_enabled: Option<bool>,
    pub title_search_enabled: Option<bool>,
    pub metadata_ai_enabled: Option<bool>,
    /// An empty string goes back to the default chat model.
    pub metadata_model: Option<String>,
    pub metadata_provider: Option<String>,
    /// The whole list, replacing the saved one. An empty list clears it.
    pub favorite_models: Option<Vec<String>>,
    /// Both need a connected reporting server.
    pub usage_stats: Option<bool>,
    pub crash_reports: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// The provider id this model belongs to, for example `gemini` or `openai`.
    pub provider: String,
    pub id: String,
    pub display_name: String,
    pub input_token_limit: Option<u32>,
    pub output_token_limit: Option<u32>,
    /// From the price table. `None` when the table doesn't know the model.
    pub facts: Option<ModelFacts>,
}

/// What the price table (a trimmed models.dev copy) says about a model. Prices are USD per 1M tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelFacts {
    /// The provider's marketing name, for example "Gemini 3.8 Flash".
    pub name: Option<String>,
    pub input_price: f64,
    pub output_price: f64,
    pub cached_price: Option<f64>,
    pub release_date: Option<String>,
    pub knowledge_cutoff: Option<String>,
    pub reasoning: bool,
    pub tool_calls: bool,
    pub image_input: bool,
    pub pdf_input: bool,
    /// The provider has announced this model's retirement.
    pub deprecated: bool,
    /// Above this many input tokens the whole request is billed at a higher rate.
    pub price_jump_over: Option<u32>,
    /// How much more input costs above `price_jump_over` (2.0 means double).
    pub price_jump_factor: Option<f64>,
}

/// A provider whose model list couldn't be loaded.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub provider: String,
    pub message: String,
}

/// Models from every provider that has a key. One failing provider doesn't hide the others.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    pub models: Vec<ModelInfo>,
    pub errors: Vec<ProviderError>,
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
