//! PDF text with line boxes, read with pdfium. See docs/STATUS.md (spike result) for why lines
//! are split by position and why text outside the crop box is dropped.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use super::meta;
use super::segment::{Block, Join};
use super::Extraction;
use crate::error::{AppError, Result};
use crate::persistence::library::{Extracted, NewPage};

/// Pages with fewer letters than this count as having no text.
const MIN_CHARS_PER_PAGE: usize = 20;
/// A page is "low" quality when more than 1 in 20 characters is a replacement or private-use mark.
const LOW_QUALITY_ODD_RATIO: usize = 20;

pub struct PdfReader {
    pdfium: Pdfium,
}

/// Where to look for libpdfium: an explicit folder, next to the app bundle, then the dev copy
/// fetched by `scripts/fetch-pdfium.sh`.
fn library_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("CHINGUI_PDFIUM_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(macos) = exe.parent()
    {
        dirs.push(macos.join("../Frameworks"));
        dirs.push(macos.join("../Resources/pdfium"));
    }
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/pdfium"));
    dirs
}

impl PdfReader {
    pub fn new() -> Result<Self> {
        for dir in library_dirs() {
            let lib = Pdfium::pdfium_platform_library_name_at_path(&dir);
            if let Ok(bindings) = Pdfium::bind_to_library(lib) {
                return Ok(PdfReader { pdfium: Pdfium::new(bindings) });
            }
        }
        Err(AppError::Internal(
            "The PDF library (libpdfium) was not found. Run src-tauri/scripts/fetch-pdfium.sh.".into(),
        ))
    }

    /// `cancelled` is polled between pages. `progress` gets (pages done, page count).
    pub fn extract(
        &self,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<Extraction> {
        let doc = self.pdfium.load_pdf_from_file(path, None).map_err(open_error)?;
        let total = doc.pages().len() as u32;

        let mut pages = Vec::new();
        let mut blocks = Vec::new();
        let mut unreadable = 0u32;

        for (i, page) in doc.pages().iter().enumerate() {
            if cancelled() {
                return Err(AppError::Cancelled);
            }
            let page_no = i as u32 + 1;
            let (lines, quality) = match read_page(&page) {
                Ok(r) => r,
                Err(_) => {
                    unreadable += 1;
                    (vec![], "none")
                }
            };
            pages.push(NewPage { page_no, width: page.width().value, height: page.height().value, quality });
            blocks.extend(lines.into_iter().map(|l| Block {
                page: page_no,
                text: l.text,
                rect: Some(l.rect),
                font_size: l.font,
                join: if l.tight { Join::Tight } else { Join::Space },
            }));
            progress(page_no, total);
        }

        let info = |t| doc.metadata().get(t).map(|m| m.value().trim().to_string()).filter(|s| !s.is_empty());
        let head_text = blocks.iter().filter(|b| b.page <= 3).map(|b| b.text.as_str()).collect::<Vec<_>>().join(" ");
        let meta = Extracted {
            title: info(PdfDocumentMetadataTagType::Title)
                .and_then(|t| meta::clean_title(&t))
                .or_else(|| meta::title_from_first_page(&blocks)),
            authors: info(PdfDocumentMetadataTagType::Author).map(|a| meta::split_authors(&a)).unwrap_or_default(),
            year: meta::find_year(&head_text, current_year()),
            doi: meta::find_doi(&head_text, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        };
        Ok(Extraction { pages, blocks, meta, unreadable_pages: unreadable })
    }
}

fn current_year() -> i32 {
    // Seconds per average year, good enough for an upper bound on plausible years.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    1970 + (secs / 31_556_952) as i32 + 1
}

fn open_error(e: PdfiumError) -> AppError {
    match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            AppError::Invalid("This PDF is password protected.".into())
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError)
        | PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            AppError::Invalid("This file couldn't be read as a PDF. It may be damaged.".into())
        }
        other => AppError::Invalid(format!("This PDF couldn't be opened: {other}")),
    }
}

struct RawLine {
    text: String,
    rect: [f32; 4],
    font: f32,
    /// Starts a word the layout broke at a line end.
    tight: bool,
}

#[derive(Default)]
struct LineBuilder {
    out: Vec<RawLine>,
    cur: String,
    rect: Option<[f32; 4]>,
    font: f32,
    cur_tight: bool,
    /// The last line ended because the position jumped, not because pdfium put a newline.
    soft_split: bool,
}

impl LineBuilder {
    fn flush(&mut self) {
        if let (false, Some(rect)) = (self.cur.trim().is_empty(), self.rect) {
            self.out.push(RawLine {
                text: self.cur.trim_end().to_string(),
                rect,
                font: self.font,
                tight: self.cur_tight,
            });
        }
        self.cur.clear();
        self.rect = None;
        self.font = 0.0;
        self.cur_tight = false;
    }

    /// Does a character centred at (cx, cy) belong to the current line?
    /// `vertical` text runs up the page (rotated pages), so its band is horizontal.
    fn same_line(&self, cx: f32, cy: f32, vertical: bool) -> bool {
        match self.rect {
            None => true,
            Some(r) if vertical => cx >= r[0] && cx <= r[2],
            Some(r) => cy >= r[1] && cy <= r[3],
        }
    }

    fn start_line(&mut self, first: char) {
        // pdfium drops the hyphen when it joins "geo-" and "engineering" and gives no newline.
        let prev_alpha = self.out.last().is_some_and(|l| l.text.chars().last().is_some_and(char::is_alphabetic));
        self.cur_tight = self.soft_split && prev_alpha && first.is_lowercase();
        self.soft_split = false;
    }
}

fn read_page(page: &PdfPage) -> std::result::Result<(Vec<RawLine>, &'static str), PdfiumError> {
    let text = page.text()?;
    let mut b = LineBuilder::default();
    let mut letters = 0usize;
    let mut odd = 0usize;

    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else { continue };
        if c == '\r' || c == '\n' {
            b.flush();
            b.soft_split = false;
            continue;
        }
        if c.is_control() {
            continue;
        }
        let bounds = if c.is_whitespace() { None } else { ch.loose_bounds().ok() };
        if let Some(r) = bounds {
            let (cx, cy) = ((r.left().value + r.right().value) / 2.0, (r.bottom().value + r.top().value) / 2.0);
            let angle = ch.angle_degrees().unwrap_or(0.0).abs() % 180.0;
            let vertical = (angle - 90.0).abs() < 20.0;
            if !b.same_line(cx, cy, vertical) {
                b.flush();
                b.soft_split = true;
            }
        }
        if b.cur.is_empty() {
            if c.is_whitespace() {
                continue;
            }
            b.start_line(c);
        }
        b.cur.push(c);
        if c.is_whitespace() {
            continue;
        }
        letters += 1;
        if c == '\u{FFFD}' || ('\u{E000}'..='\u{F8FF}').contains(&c) {
            odd += 1;
        }
        if let Some(r) = bounds {
            let (l, bo, ri, t) = (r.left().value, r.bottom().value, r.right().value, r.top().value);
            b.rect = Some(match b.rect {
                None => [l, bo, ri, t],
                Some(o) => [o[0].min(l), o[1].min(bo), o[2].max(ri), o[3].max(t)],
            });
            b.font = b.font.max(ch.scaled_font_size().value);
        }
    }
    b.flush();

    // Text outside the crop box is invisible in a viewer (prepress headers, crop marks).
    let crop = page.boundaries().crop().or_else(|_| page.boundaries().media()).ok().map(|x| x.bounds);
    let mut lines = b.out;
    if let Some(c) = crop {
        let (l, bo, r, t) = (c.left().value, c.bottom().value, c.right().value, c.top().value);
        lines.retain(|x| x.rect[0] < r && x.rect[2] > l && x.rect[1] < t && x.rect[3] > bo);
    }

    let quality = if letters < MIN_CHARS_PER_PAGE {
        "none"
    } else if odd * LOW_QUALITY_ODD_RATIO > letters {
        "low"
    } else {
        "ok"
    };
    Ok((lines, quality))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One page, two lines of 18 pt Helvetica. No xref table: pdfium rebuilds it.
    fn tiny_pdf() -> Vec<u8> {
        let content = "BT /F1 18 Tf 20 150 Td (Hello Chingui world) Tj 0 -30 Td (Second line here) Tj ET";
        format!(
            "%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
             2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
             3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 200]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n\
             4 0 obj<</Length {}>>stream\n{content}\nendstream endobj\n\
             5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n\
             trailer<</Root 1 0 R/Size 6>>\n%%EOF\n",
            content.len()
        )
        .into_bytes()
    }

    fn reader() -> Option<PdfReader> {
        match PdfReader::new() {
            Ok(r) => Some(r),
            Err(_) => {
                eprintln!("skipped: libpdfium not found (run scripts/fetch-pdfium.sh)");
                None
            }
        }
    }

    #[test]
    fn reads_lines_boxes_and_page_size() {
        let Some(reader) = reader() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.pdf");
        std::fs::write(&path, tiny_pdf()).unwrap();

        let mut last = (0, 0);
        let out = reader.extract(&path, &|| false, &mut |d, t| last = (d, t)).unwrap();
        assert_eq!(last, (1, 1));
        assert_eq!((out.pages.len(), out.pages[0].quality), (1, "ok"));
        assert_eq!((out.pages[0].width, out.pages[0].height), (300.0, 200.0));
        let texts: Vec<_> = out.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["Hello Chingui world", "Second line here"]);
        let r = out.blocks[0].rect.unwrap();
        assert!(r[0] >= 19.0 && r[2] <= 300.0 && r[1] > 140.0 && r[3] < 180.0, "{r:?}");
        assert!((out.blocks[0].font_size - 18.0).abs() < 1.0);
    }

    #[test]
    fn a_page_without_text_is_flagged_not_failed() {
        let Some(reader) = reader() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blank.pdf");
        let blank = String::from_utf8(tiny_pdf()).unwrap().replace("Hello Chingui world", "").replace("Second line here", "");
        std::fs::write(&path, blank).unwrap();
        let out = reader.extract(&path, &|| false, &mut |_, _| {}).unwrap();
        assert_eq!(out.pages[0].quality, "none");
        assert!(out.blocks.is_empty());
    }

    /// Reads every PDF in $CHINGUI_TEST_PAPERS and prints what came out.
    /// Run: CHINGUI_TEST_PAPERS=~/papers cargo test corpus_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn corpus_smoke() {
        let Some(reader) = reader() else { return };
        let dir = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "pdf")).collect();
        files.sort();
        for f in files {
            let t = std::time::Instant::now();
            let out = reader.extract(&f, &|| false, &mut |_, _| {}).unwrap();
            let passages = crate::services::ingestion::segment::segment(&out.blocks);
            let none = out.pages.iter().filter(|p| p.quality == "none").count();
            println!(
                "{:>4} pages {:>3} no-text {:>5} passages {:>4.1}s | title={:?} year={:?} doi={:?} authors={} | {}",
                out.pages.len(), none, passages.len(), t.elapsed().as_secs_f32(),
                out.meta.title.as_deref().map(|t| t.chars().take(50).collect::<String>()),
                out.meta.year, out.meta.doi, out.meta.authors.len(),
                f.file_name().unwrap().to_string_lossy().chars().take(30).collect::<String>()
            );
        }
    }

    #[test]
    fn cancel_and_bad_files_are_reported() {
        let Some(reader) = reader() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.pdf");
        std::fs::write(&path, tiny_pdf()).unwrap();
        assert!(matches!(reader.extract(&path, &|| true, &mut |_, _| {}), Err(AppError::Cancelled)));

        let junk = dir.path().join("junk.pdf");
        std::fs::write(&junk, b"this is not a pdf at all").unwrap();
        let err = reader.extract(&junk, &|| false, &mut |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("couldn't be"), "{err}");
    }
}
