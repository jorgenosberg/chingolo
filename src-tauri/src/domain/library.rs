//! Library types: documents, passages and search results.

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Pdf,
    Docx,
}

impl DocKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DocKind::Pdf => "pdf",
            DocKind::Docx => "docx",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pdf" => Some(DocKind::Pdf),
            "docx" => Some(DocKind::Docx),
            _ => None,
        }
    }

    /// From a file extension, case-insensitive.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Self::parse(&ext.to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DocStatus {
    Queued,
    Processing,
    Ready,
    /// Imported fine, but no text could be read (a scan without OCR).
    NoText,
    Error,
    Cancelled,
}

impl DocStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DocStatus::Queued => "queued",
            DocStatus::Processing => "processing",
            DocStatus::Ready => "ready",
            DocStatus::NoText => "no_text",
            DocStatus::Error => "error",
            DocStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(DocStatus::Queued),
            "processing" => Some(DocStatus::Processing),
            "ready" => Some(DocStatus::Ready),
            "no_text" => Some(DocStatus::NoText),
            "error" => Some(DocStatus::Error),
            "cancelled" => Some(DocStatus::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: Id,
    pub content_hash: String,
    pub original_name: String,
    pub kind: DocKind,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub page_count: Option<u32>,
    pub status: DocStatus,
    /// Why the document failed, or a note such as how many pages could not be read.
    pub error: Option<String>,
    pub passage_count: u32,
    /// Passages that have a vector for meaning search. Stale vectors are purged when the
    /// model changes, so this counts the current ones.
    pub embedded_count: u32,
    /// Projects this document is attached to.
    pub project_ids: Vec<Id>,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

/// Line boxes of a passage on one page, in PDF user space: `[left, bottom, right, top]`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PageBoxes {
    pub page: u32,
    pub rects: Vec<[f32; 4]>,
}

/// A stored passage with what the reader needs to show it.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PassageView {
    pub id: Id,
    pub document_id: Id,
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
    /// Empty when the document has no boxes (DOCX) or they couldn't be read.
    pub boxes: Vec<PageBoxes>,
}

/// What the app saved when it first showed a passage in a conversation. It outlives the document.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub handle: u32,
    pub title: String,
    /// First page, 0 for Word documents.
    pub page: u32,
    pub text: String,
}

/// Fields the user can correct. `None` leaves a field alone, an empty string clears it.
#[derive(Debug, Clone, Default, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    /// Zero clears the year.
    pub year: Option<i32>,
    pub doi: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub passage_id: Id,
    pub document_id: Id,
    /// The document's title, or its file name if it has none.
    pub document_title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
    pub matched_by: MatchKind,
}

/// Why a passage is in the results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// It contains words from the query.
    Keyword,
    /// It is close in meaning to the query but shares no ranked keyword match.
    Meaning,
    Both,
    /// Not a match itself: it sits next to one.
    Context,
}

/// Lowercase and fold the Norwegian letters the full-text tokenizer leaves alone.
/// `unicode61 remove_diacritics` already maps å to a, so folding å here keeps both consistent.
/// Used on indexed text and on queries.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'æ' => out.push_str("ae"),
            'ø' => out.push('o'),
            'å' => out.push('a'),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_norwegian_letters_and_case() {
        assert_eq!(fold("Økosystemet ved Ærø og Åre"), "okosystemet ved aero og are");
    }

    #[test]
    fn status_and_kind_round_trip() {
        for s in [
            DocStatus::Queued,
            DocStatus::Processing,
            DocStatus::Ready,
            DocStatus::NoText,
            DocStatus::Error,
            DocStatus::Cancelled,
        ] {
            assert_eq!(DocStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(DocKind::from_extension("PDF"), Some(DocKind::Pdf));
        assert_eq!(DocKind::from_extension("txt"), None);
    }
}
