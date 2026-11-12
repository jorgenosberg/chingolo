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
    /// Journal for papers.
    pub journal: Option<String>,
    /// Publisher or institute, mostly for reports.
    pub publisher: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    /// Report series and number, such as "NIVA-rapport 7132-2017".
    pub series: Option<String>,
    pub isbn: Option<String>,
    pub issn: Option<String>,
    /// Fields the user changed. A refresh leaves these alone.
    pub meta_edited: Vec<String>,
    /// Where each detected field came from: "file", "crossref" or "ai".
    pub meta_source: std::collections::HashMap<String, String>,
    pub page_count: Option<u32>,
    pub status: DocStatus,
    /// Why the document failed, or a note such as how many pages could not be read.
    pub error: Option<String>,
    pub passage_count: u32,
    /// Passages that have a vector for meaning search. Stale vectors are purged when the
    /// model changes, so this counts the current ones.
    pub embedded_count: u32,
    /// Pages with little or no readable text.
    pub weak_pages: u32,
    /// Why the last Crossref or model lookup failed. Cleared when one finishes.
    pub enrich_error: Option<String>,
    /// Projects this document is attached to.
    pub project_ids: Vec<Id>,
    #[specta(type = Number)]
    pub created_at: i64,
    #[specta(type = Number)]
    pub updated_at: i64,
}

/// "Løvik", "van Dorst", "Boyd": the surname of "Jarl Eivind Løvik", "Renee Mina van Dorst" or
/// "Boyd, P. W.".
pub fn surname(name: &str) -> String {
    if let Some((s, _)) = name.split_once(',') {
        return s.trim().to_string();
    }
    const PARTICLES: &[&str] = &["van", "von", "de", "der", "den", "da", "di", "du", "la", "le", "dos", "ter", "ten", "af", "al", "bin"];
    let toks: Vec<&str> = name.split_whitespace().collect();
    let mut start = toks.len().saturating_sub(1);
    while start > 1 && PARTICLES.contains(&toks[start - 1]) {
        start -= 1;
    }
    toks[start..].join(" ")
}

/// How a source is named in prose: "Boyd 2004", "Yang & Albright 1992", "Boyd et al. 2004". A
/// report without personal authors uses its publisher. Empty when neither is known.
pub fn short_label(authors: &[String], year: Option<i32>, publisher: Option<&str>) -> String {
    let who = match authors {
        [] => publisher.map(str::trim).unwrap_or_default().to_string(),
        [a] => surname(a),
        [a, b] => format!("{} & {}", surname(a), surname(b)),
        [a, ..] => format!("{} et al.", surname(a)),
    };
    match (who.is_empty(), year) {
        (true, _) => String::new(),
        (false, Some(y)) => format!("{who} {y}"),
        (false, None) => who,
    }
}

impl Document {
    /// See `short_label`.
    pub fn label(&self) -> String {
        short_label(&self.authors, self.year, self.publisher.as_deref())
    }
}

/// What a conversation may read, as chosen in the composer. Stored as JSON in
/// `conversations.source_scope`. `Project` means the conversation's own project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Scope {
    Project,
    Library,
    Documents { ids: Vec<Id> },
}

/// A scope resolved against a conversation, ready for queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocFilter {
    All,
    Project(Id),
    Documents(Vec<Id>),
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
    pub journal: Option<String>,
    pub publisher: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub series: Option<String>,
    pub isbn: Option<String>,
    pub issn: Option<String>,
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
    #[test]
    fn sources_are_named_by_surname_and_year() {
        let a = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(short_label(&a(&["P. W. Boyd"]), Some(2004), None), "Boyd 2004");
        assert_eq!(short_label(&a(&["C. Z. Yang", "L. J. Albright"]), Some(1992), None), "Yang & Albright 1992");
        assert_eq!(short_label(&a(&["Philip W. Boyd", "Tim Jickells", "Cliff Law"]), Some(2004), None), "Boyd et al. 2004");
        assert_eq!(short_label(&a(&["Hallegraeff, Gustaaf M.", "Anderson, Don M."]), None, None), "Hallegraeff & Anderson");
        assert_eq!(short_label(&[], Some(2025), Some("Veterinærinstituttet")), "Veterinærinstituttet 2025");
        assert_eq!(short_label(&[], Some(2025), None), "");
        assert_eq!(surname("Renee Mina van Dorst"), "van Dorst");
        assert_eq!(surname("Jan-Erik Thrane"), "Thrane");
        assert_eq!(surname("Boyd"), "Boyd");
    }

    use super::*;

    #[test]
    fn folds_norwegian_letters_and_case() {
        assert_eq!(fold("Økosystemet ved Ærø og Åre"), "okosystemet ved aero og are");
    }

    #[test]
    fn scope_json_round_trips_and_reads_the_old_value() {
        let old: Scope = serde_json::from_str("{\"kind\":\"library\"}").unwrap();
        assert_eq!(old, Scope::Library);
        for s in [Scope::Project, Scope::Library, Scope::Documents { ids: vec!["a".into(), "b".into()] }] {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<Scope>(&json).unwrap(), s);
        }
        assert_eq!(serde_json::to_string(&Scope::Project).unwrap(), "{\"kind\":\"project\"}");
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
