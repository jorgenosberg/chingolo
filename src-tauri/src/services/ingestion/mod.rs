//! Turns imported files into searchable passages: read the file, guess metadata, split into
//! passages, index. The queue that runs this in the background is added on top of these parts.

#[cfg(test)]
mod audit;
pub mod docx;
pub mod import;
pub mod meta;
pub mod ocr;
pub mod pdf;
pub mod queue;
pub mod segment;

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
