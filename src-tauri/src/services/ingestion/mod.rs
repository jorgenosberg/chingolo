//! Turns imported files into searchable passages: read the file, guess metadata, split into
//! passages, index. The queue that runs this in the background is added on top of these parts.

pub mod activity;
#[cfg(test)]
mod audit;
pub mod crossref;
pub mod details_ai;
pub mod detect;
pub mod docx;
pub mod enrich;
pub mod import;
pub mod layout;
#[cfg(test)]
mod layout_audit;
pub mod lookup;
pub mod meta;
pub mod ocr;
pub mod pdf;
pub mod queue;
pub mod segment;
pub mod tables;
pub mod vision;

use crate::persistence::library::{Extracted, NewPage};
use segment::Block;

/// What reading a file produced, before it is stored.
#[derive(Debug)]
pub struct Extraction {
    /// Page geometry and quality. Empty for DOCX.
    pub pages: Vec<NewPage>,
    pub blocks: Vec<Block>,
    pub meta: Extracted,
    /// Pages pdfium could not read at all.
    pub unreadable_pages: u32,
}
