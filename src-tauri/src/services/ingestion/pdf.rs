//! PDF text with line boxes, read with pdfium. See docs/STATUS.md (spike result) for why lines
//! are split by position and why text outside the crop box is dropped.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use pdfium_render::prelude::*;
use regex::Regex;

use super::detect::{Info, detect};
use super::layout::{Layout, PageRegions, Region};
use super::ocr::{self, Ocr, OcrSlot};
use super::segment::{Block, Join};
use super::tables::{self, Filled, Glyph, Tables};
use super::vision;
use super::Extraction;
use crate::error::{AppError, Result};
use crate::persistence::library::{Extracted, NewPage};

/// Pages with fewer letters than this count as having no text.
const MIN_CHARS_PER_PAGE: usize = 20;
/// Pages read for details. NINA reports put their citation line on page 4, and the edition year
/// rule reads the copyright page, which is page 5 in the Manual. The other rules stop at 4.
const META_PAGES: usize = 6;
/// A page is "low" quality when more than 1 in 20 characters is a replacement or private-use mark,
/// or when several different words in it are garbled (see `is_garbled`).
const LOW_QUALITY_ODD_RATIO: usize = 20;

/// Shapes that real words don't have but a broken font encoding makes: a digit in front of a letter
/// run (`0DQXDORQ` for "Manual"), a digit inside one, or a symbol between two letters (`c~nv~Z~t~~`).
static GARBLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d\p{L}{4,}|\p{L}{2,}\d\p{L}{2,}|\p{L}[~^|{}\\]\p{L}").unwrap());

const ELEMENTS: &str = "H He Li Be B C N O F Ne Na Mg Al Si P S Cl Ar K Ca Sc Ti V Cr Mn Fe Co Ni Cu Zn Ga Ge As Se Br Kr Rb Sr Y Zr Nb Mo Tc Ru Rh Pd Ag Cd In Sn Sb Te I Xe Cs Ba La Ce Pr Nd Pm Sm Eu Gd Tb Dy Ho Er Tm Yb Lu Hf Ta W Re Os Ir Pt Au Hg Tl Pb Bi Po At Rn Fr Ra Ac Th Pa U Np Pu Am Cm Bk Cf Es Fm Md No Lr";

/// A chemical formula such as `NaH2PO4`: element symbols with counts, at least one count. Digits
/// inside a word are normal there, unlike in `DUPIXO0DULQH`, whose letters are no elements.
fn is_formula(w: &str) -> bool {
    let w = w.trim_start_matches(|c: char| c.is_ascii_digit());
    let (mut chars, mut counted) = (w.chars().peekable(), false);
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            counted = true;
            continue;
        }
        if !c.is_ascii_uppercase() {
            return false;
        }
        let mut sym = c.to_string();
        if let Some(l) = chars.next_if(char::is_ascii_lowercase) {
            sym.push(l);
        }
        if !ELEMENTS.split(' ').any(|e| e == sym) {
            return false;
        }
    }
    counted
}

/// The garbled words in some lines of text, and the word count. Words are runs of 3 or more
/// letters (with what sticks to them) after trimming punctuation. A leading number on a
/// capitalised word is a footnote or affiliation mark ("11Département", "3-4April"), not garble.
pub fn garble_words<'a>(lines: impl IntoIterator<Item = &'a str>) -> (Vec<&'a str>, usize) {
    let (mut bad, mut all) = (Vec::new(), 0);
    for w in lines.into_iter().flat_map(str::split_whitespace) {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        if w.chars().filter(|c| c.is_alphabetic()).count() < 3 {
            continue;
        }
        all += 1;
        let core = w.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '\u{2013}');
        let mut cs = core.chars();
        let capitalised = cs.next().is_some_and(char::is_uppercase) && cs.clone().count() >= 2 && cs.all(char::is_lowercase);
        if !(capitalised && core.len() < w.len()) && GARBLE.is_match(w) && !is_formula(w) {
            bad.push(w);
        }
    }
    (bad, all)
}

/// Recognised lines beat a garbled text layer when they have fewer garbled words and lose no more
/// than a tenth of the layer's good words (recognition reads a table or a figure label differently,
/// but a page that comes back much shorter has lost something).
fn reads_better(layer: &[RawLine], ocr: &[RawLine]) -> bool {
    let (bad_layer, all_layer) = garble_words(layer.iter().map(|l| l.text.as_str()));
    let (bad_ocr, all_ocr) = garble_words(ocr.iter().map(|l| l.text.as_str()));
    bad_ocr.len() < bad_layer.len() && all_ocr * 10 >= all_layer.saturating_sub(bad_layer.len()) * 9
}

/// How many different garbled words a page needs to count as having a broken text layer. One name
/// with a digit in it ("PTX2sa"), even repeated, is not garble.
pub const GARBLED_DISTINCT: usize = 3;

/// True when the page's text layer is broken: several different garbled words.
pub fn is_garbled<'a>(lines: impl IntoIterator<Item = &'a str>) -> bool {
    let (mut bad, _) = garble_words(lines);
    bad.sort_unstable();
    bad.dedup();
    bad.len() >= GARBLED_DISTINCT
}

pub struct PdfReader {
    pdfium: Pdfium,
}

/// Where to look for libpdfium: an explicit folder, next to the app bundle, then the dev copy
/// fetched by `scripts/fetch-pdfium.sh`.
fn library_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("CHINGOLO_PDFIUM_DIR") {
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
    #[cfg(test)]
    pub fn extract(
        &self,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<Extraction> {
        self.extract_with(path, &mut OcrSlot::default(), cancelled, progress)
    }

    /// Like `extract`, and pages with no text layer go through text recognition when the models are there.
    pub fn extract_with(
        &self,
        path: &Path,
        ocr: &mut OcrSlot,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<Extraction> {
        self.read(path, ocr, cancelled, progress, usize::MAX)
    }

    /// Runs layout detection on the given pages (with or without text), and returns the regions in PDF user space.
    /// `pages` are the page numbers to look at. `progress` gets (pages done, pages to do).
    pub fn detect_layout(
        &self,
        path: &Path,
        pages: &[u32],
        layout: &Layout,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<Vec<PageRegions>> {
        let doc = self.pdfium.load_pdf_from_file(path, None).map_err(open_error)?;
        let mut out = Vec::with_capacity(pages.len());
        for (done, &no) in pages.iter().enumerate() {
            if cancelled() {
                return Err(AppError::Cancelled);
            }
            // A page that can't be drawn or read still counts as done, with no regions, so it
            // isn't tried again at every start.
            let regions = doc
                .pages()
                .get(no as i32 - 1)
                .map_err(|e| AppError::Internal(e.to_string()))
                .and_then(|page| {
                    let Rendered { img, scale, crop, rotation } = render(&page)?;
                    let found = layout.detect(img)?;
                    Ok(found.into_iter().map(|r| Region { rect: ocr::to_user_space(r.rect, scale, crop, rotation), ..r }).collect())
                })
                .unwrap_or_else(|e| {
                    log::warn!("layout failed on page {no}: {e}");
                    vec![]
                });
            out.push(PageRegions { page: no, regions });
            progress(done as u32 + 1, pages.len() as u32);
        }
        Ok(out)
    }

    /// Reads the tables in the given regions (PDF user space, grouped by page). The grid comes from
    /// the table model, the text from the page's own characters. A table that fails gets an error
    /// string and nothing else is affected.
    pub fn read_tables(
        &self,
        path: &Path,
        regions: &[(u32, [f32; 4])],
        tables: &Tables,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<Vec<TableRead>> {
        let doc = self.pdfium.load_pdf_from_file(path, None).map_err(open_error)?;
        let mut out = Vec::with_capacity(regions.len());
        let mut i = 0;
        while i < regions.len() {
            if cancelled() {
                return Err(AppError::Cancelled);
            }
            let no = regions[i].0;
            let end = regions[i..].iter().position(|r| r.0 != no).map_or(regions.len(), |n| i + n);
            let page = doc.pages().get(no as i32 - 1).map_err(|e| AppError::Internal(e.to_string()));
            let drawn = page.as_ref().map_err(|e| e.to_string()).and_then(|p| render(p).map(|r| (r, page_glyphs(p))).map_err(|e| e.to_string()));
            for &(_, region) in &regions[i..end] {
                let filled = match &drawn {
                    Ok((r, glyphs)) => read_table(r, glyphs, region, tables),
                    Err(e) => Err(e.clone()),
                };
                out.push(TableRead { filled });
            }
            i = end;
            progress(i as u32, regions.len() as u32);
        }
        Ok(out)
    }

    /// One page drawn for layout detection (1-based page number).
    #[cfg(test)]
    pub(super) fn render_page(&self, path: &Path, page: u32) -> Result<Rendered> {
        let doc = self.pdfium.load_pdf_from_file(path, None).map_err(open_error)?;
        let page = doc.pages().get((page - 1) as i32).map_err(|e| AppError::Internal(e.to_string()))?;
        render(&page)
    }

    /// Only the details: the file properties and the first pages, which is where titles,
    /// authors and citation lines are. Nothing is stored, so nothing needs indexing again.
    pub fn extract_meta(&self, path: &Path, ocr: &mut OcrSlot) -> Result<Extracted> {
        Ok(self.read(path, ocr, &|| false, &mut |_, _| {}, META_PAGES)?.meta)
    }

    fn read(
        &self,
        path: &Path,
        ocr: &mut OcrSlot,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(u32, u32),
        max_pages: usize,
    ) -> Result<Extraction> {
        let doc = self.pdfium.load_pdf_from_file(path, None).map_err(open_error)?;
        let total = (doc.pages().len() as usize).min(max_pages) as u32;

        let mut pages = Vec::new();
        let mut blocks = Vec::new();
        let mut unreadable = 0u32;

        for (i, page) in doc.pages().iter().enumerate().take(max_pages) {
            if cancelled() {
                return Err(AppError::Cancelled);
            }
            let page_no = i as u32 + 1;
            let mut failed = false;
            let (mut lines, mut quality) = match read_page(&page) {
                Ok(r) => r,
                Err(_) => {
                    failed = true;
                    (vec![], "none")
                }
            };
            let mut ocr_read = false;
            // A page with no text, or a text layer that is garbled, is read by recognition when the
            // models are there. For a garbled layer the new text is kept only if it is better.
            if (quality == "none" || quality == "low")
                && let Some(engine) = ocr.get()
            {
                match ocr_page(&page, engine) {
                    Ok(l)
                        if l.iter().map(|x| x.text.chars().filter(|c| c.is_alphanumeric()).count()).sum::<usize>() >= MIN_CHARS_PER_PAGE
                            && (quality == "none" || reads_better(&lines, &l)) =>
                    {
                        lines = l;
                        quality = "ok";
                        ocr_read = true;
                        failed = false;
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("text recognition failed on page {page_no}: {e}"),
                }
            }
            unreadable += u32::from(failed);
            pages.push(NewPage { page_no, width: page.width().value, height: page.height().value, quality, ocr: ocr_read });
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
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let found = Info {
            title: info(PdfDocumentMetadataTagType::Title),
            author: info(PdfDocumentMetadataTagType::Author),
            subject: info(PdfDocumentMetadataTagType::Subject),
        };
        let meta = detect(&found, &blocks, &file_name, current_year());
        Ok(Extraction { pages, blocks, meta, unreadable_pages: unreadable })
    }
}

pub fn current_year() -> i32 {
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

/// A page drawn the way a viewer shows it, with what is needed to map pixels back to user space.
/// A table region of a page and what reading it gave: the cells with their text, or why not.
#[derive(Debug)]
pub struct TableRead {
    pub filled: std::result::Result<Filled, String>,
}

/// Room around a table region for the model, in pixels. Layout boxes are tight.
const TABLE_PAD_PX: f32 = 6.0;

fn read_table(r: &Rendered, glyphs: &[Glyph], region: [f32; 4], tables: &Tables) -> std::result::Result<Filled, String> {
    let [x0, y0, x1, y1] = ocr::to_pixels(region, r.scale, r.crop, r.rotation);
    let (x0, y0) = ((x0 - TABLE_PAD_PX).max(0.0).floor(), (y0 - TABLE_PAD_PX).max(0.0).floor());
    let (x1, y1) = ((x1 + TABLE_PAD_PX).min(r.img.width() as f32).ceil(), (y1 + TABLE_PAD_PX).min(r.img.height() as f32).ceil());
    let (w, h) = ((x1 - x0).max(0.0) as u32, (y1 - y0).max(0.0) as u32);
    if w < 16 || h < 16 {
        return Err("the table region is empty".into());
    }
    let crop = image::imageops::crop_imm(&r.img, x0 as u32, y0 as u32, w, h).to_image();
    let grid = tables.grid(crop).map_err(|e| e.to_string())?;
    let pixels: Vec<[f32; 4]> = grid.iter().map(|c| [c.rect[0] + x0, c.rect[1] + y0, c.rect[2] + x0, c.rect[3] + y0]).collect();
    let cells: Vec<_> = grid.into_iter().zip(&pixels).map(|(c, px)| (c, ocr::to_user_space(*px, r.scale, r.crop, r.rotation))).collect();
    let mut filled = tables::fill(&cells, region, glyphs);
    verify_cells(&r.img, &pixels, &mut filled.cells);
    Ok(filled)
}

/// Room around a cell when Vision reads it, in pixels, and the white border added so text at the
/// edge of the crop is not cut off.
const CELL_PAD_PX: i64 = 3;
const CELL_BORDER_PX: u32 = 10;
const CELL_MIN_HEIGHT_PX: u32 = 96;

/// Asks Apple Vision to read each cell that has digits or symbols and marks the cell by whether the
/// two readers agree. Nothing in the cell changes. A cell Vision fails on stays unmarked.
fn verify_cells(img: &image::RgbImage, pixels: &[[f32; 4]], cells: &mut [tables::Cell]) {
    if !vision::AVAILABLE {
        return;
    }
    for (cell, px) in cells.iter_mut().zip(pixels) {
        if !tables::worth_checking(&cell.text) {
            continue;
        }
        let (x0, y0) = ((px[0] as i64 - CELL_PAD_PX).max(0), (px[1] as i64 - CELL_PAD_PX).max(0));
        let (x1, y1) = ((px[2] as i64 + CELL_PAD_PX).min(img.width() as i64), (px[3] as i64 + CELL_PAD_PX).min(img.height() as i64));
        if x1 - x0 < 4 || y1 - y0 < 4 {
            continue;
        }
        let part = image::imageops::crop_imm(img, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image();
        // Vision skips very small text, so short crops are enlarged first
        let part = if part.height() < CELL_MIN_HEIGHT_PX {
            let k = (CELL_MIN_HEIGHT_PX as f32 / part.height() as f32).min(4.0);
            image::imageops::resize(&part, (part.width() as f32 * k) as u32, (part.height() as f32 * k) as u32, image::imageops::FilterType::Lanczos3)
        } else {
            part
        };
        let mut framed = image::RgbImage::from_pixel(part.width() + 2 * CELL_BORDER_PX, part.height() + 2 * CELL_BORDER_PX, image::Rgb([255, 255, 255]));
        image::imageops::overlay(&mut framed, &part, CELL_BORDER_PX as i64, CELL_BORDER_PX as i64);
        // a reading with nothing in it says nothing, it is not a disagreement
        let lines: Vec<String> = vision::read_lines(&framed).unwrap_or_default().into_iter().filter(|l| l.chars().any(char::is_alphanumeric)).collect();
        if !lines.is_empty() {
            cell.verified = Some(tables::readers_agree(&cell.text, &lines.join(" ")));
            if cell.verified == Some(false) && std::env::var("CHINGOLO_VISION_DEBUG").is_ok() {
                println!("      vision r{}c{}: layer {:?} vision {:?}", cell.row, cell.col, cell.text, lines);
            }
        }
    }
}

/// Every character of the page with a box, in PDF user space. Spaces have no box: the character
/// after one remembers it.
fn page_glyphs(page: &PdfPage) -> Vec<Glyph> {
    let Ok(text) = page.text() else { return vec![] };
    let mut out = Vec::new();
    let mut space = false;
    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else { continue };
        if c.is_whitespace() || c == '\r' || c == '\n' {
            space = true;
            continue;
        }
        if c.is_control() {
            continue;
        }
        if let Ok(r) = ch.loose_bounds() {
            out.push(Glyph { ch: c, rect: [r.left().value, r.bottom().value, r.right().value, r.top().value], space_before: std::mem::take(&mut space) });
        }
    }
    out
}

pub(super) struct Rendered {
    pub img: image::RgbImage,
    pub scale: (f32, f32),
    pub crop: [f32; 4],
    pub rotation: ocr::Rotation,
}

pub(super) fn render(page: &PdfPage) -> Result<Rendered> {
    let (w, h) = (page.width().value, page.height().value);
    let scale = ocr::RENDER_SCALE.min(ocr::MAX_SIDE_PX / w.max(h));
    let draw = |e: PdfiumError| AppError::Internal(format!("couldn't draw the page: {e}"));
    let bitmap = page
        .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(scale).rotate_if_landscape(PdfPageRenderRotation::None, false))
        .map_err(draw)?;
    let img = bitmap.as_image().map_err(draw)?.to_rgb8();
    let crop = page.boundaries().crop().or_else(|_| page.boundaries().media()).map(|x| x.bounds);
    let crop = crop.map(|c| [c.left().value, c.bottom().value, c.right().value, c.top().value]).unwrap_or([0.0, 0.0, w, h]);
    let rotation = match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 90,
        Ok(PdfPageRenderRotation::Degrees180) => 180,
        Ok(PdfPageRenderRotation::Degrees270) => 270,
        _ => 0,
    };
    Ok(Rendered { scale: (img.width() as f32 / w, img.height() as f32 / h), img, crop, rotation })
}

/// Reads the page with text recognition and puts the lines back in PDF user space.
fn ocr_page(page: &PdfPage, engine: &Ocr) -> Result<Vec<RawLine>> {
    let Rendered { img, scale, crop, rotation } = render(page)?;
    Ok(engine
        .recognize(img)?
        .into_iter()
        .map(|l| RawLine {
            font: (l.rect[3] - l.rect[1]) / scale.1 * 0.8,
            rect: ocr::to_user_space(l.rect, scale, crop, rotation),
            text: l.text,
            tight: false,
        })
        .collect())
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
    } else if odd * LOW_QUALITY_ODD_RATIO > letters || is_garbled(lines.iter().map(|l| l.text.as_str())) {
        "low"
    } else {
        "ok"
    };
    Ok((lines, quality))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(text: &str) -> Vec<RawLine> {
        vec![RawLine { text: text.into(), rect: [0.0; 4], font: 10.0, tight: false }]
    }

    #[test]
    fn recognised_text_replaces_a_garbled_layer_only_when_it_is_better_and_not_much_shorter() {
        let layer = raw("Collec8on Loca8on classifica8on of samples from the first survey and the second survey of the bay");
        assert!(reads_better(&layer, &raw("Collection Location classification of samples from the first survey and the second survey of the bay")));
        assert!(!reads_better(&layer, &raw("Collection Location classification of samples")), "a third of the words are gone");
        assert!(!reads_better(&layer, &raw("Collec8on Loca8on classifica8on of samples from the first survey and the second survey of the bay")), "no better");
    }

    #[test]
    fn broken_font_encodings_are_garble_and_chemistry_and_footnote_marks_are_not() {
        let bad = |t: &str| garble_words([t]).0.len();
        assert_eq!(bad("0DQXDORQ +DUPIXO0DULQH 0LFURDOJDH"), 3, "the Manual's cover: Manual on Harmful Marine Microalgae shifted by three");
        assert_eq!(bad("Collec8on Loca8on and classifica8on"), 3, "a broken ti ligature");
        assert_eq!(bad("and c~nv~Z~t~~ increase"), 1);
        assert_eq!(bad("11Département 3-4April 1Introduction of the ocean"), 0, "numbers glued to a capitalised word");
        assert_eq!(bad("NaH2PO4 Na2SiO3 NH4VO3 CH2OHCHOH 1CH2OHCHOH"), 0, "formulas");
        assert_eq!(bad("measured 2.5 mg at 1500 m depth in 1993 (CO2) and 10.1038/nature02437"), 0);
        assert!(is_garbled(["0DQXDORQ +DUPIXO0DULQH 0LFURDOJDH"]));
        assert!(!is_garbled(["ekstrem2høy ekstrem2høy ekstrem2høy"]), "one odd word, even repeated, is not a broken layer");
    }

    /// One page, two lines of 18 pt Helvetica. No xref table: pdfium rebuilds it.
    fn tiny_pdf() -> Vec<u8> {
        let content = "BT /F1 18 Tf 20 150 Td (Hello Chingolo world) Tj 0 -30 Td (Second line here) Tj ET";
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

    /// The tiny page with its box starting at (ox, oy) and turned by `rotate` degrees.
    fn placed_pdf(ox: i32, oy: i32, rotate: u16) -> Vec<u8> {
        let content = format!("BT /F1 18 Tf {} {} Td (Hello Chingolo world) Tj 0 -30 Td (Second line here) Tj ET", 20 + ox, 150 + oy);
        format!(
            "%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
             2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
             3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[{ox} {oy} {} {}]/Rotate {rotate}/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n\
             4 0 obj<</Length {}>>stream\n{content}\nendstream endobj\n\
             5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n\
             trailer<</Root 1 0 R/Size 6>>\n%%EOF\n",
            ox + 300,
            oy + 200,
            content.len()
        )
        .into_bytes()
    }

    /// Dark pixels of the rendered page, mapped back to user space, must sit on the text layer's
    /// boxes for every page rotation and for a box that doesn't start at the origin. This is what
    /// keeps highlights on scanned pages in the right place.
    #[test]
    fn rendered_pixels_map_back_onto_the_text_layer() {
        let Some(reader) = reader() else { return };
        let dir = tempfile::tempdir().unwrap();
        for (ox, oy) in [(0, 0), (100, 70)] {
            for rotate in [0u16, 90, 180, 270] {
                let path = dir.path().join(format!("p{ox}-{rotate}.pdf"));
                std::fs::write(&path, placed_pdf(ox, oy, rotate)).unwrap();
                let doc = reader.pdfium.load_pdf_from_file(&path, None).unwrap();
                let page = doc.pages().get(0).unwrap();
                let (lines, _) = read_page(&page).unwrap();
                let want = lines.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, l| {
                    [a[0].min(l.rect[0]), a[1].min(l.rect[1]), a[2].max(l.rect[2]), a[3].max(l.rect[3])]
                });
                let r = render(&page).unwrap();
                assert_eq!(r.rotation, rotate);
                let mut px = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                for (x, y, p) in r.img.enumerate_pixels() {
                    if p.0.iter().map(|&c| c as u32).sum::<u32>() < 300 {
                        px = [px[0].min(x as f32), px[1].min(y as f32), px[2].max(x as f32 + 1.0), px[3].max(y as f32 + 1.0)];
                    }
                }
                let got = ocr::to_user_space(px, r.scale, r.crop, r.rotation);
                // Glyph boxes include padding above and below the ink, so allow a few points.
                for i in 0..4 {
                    assert!((got[i] - want[i]).abs() < 6.0, "origin ({ox},{oy}) rotate {rotate}: got {got:?}, text layer {want:?}");
                }
            }
        }
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
        assert_eq!(texts, ["Hello Chingolo world", "Second line here"]);
        let r = out.blocks[0].rect.unwrap();
        assert!(r[0] >= 19.0 && r[2] <= 300.0 && r[1] > 140.0 && r[3] < 180.0, "{r:?}");
        assert!((out.blocks[0].font_size - 18.0).abs() < 1.0);
    }

    #[test]
    fn a_page_without_text_is_flagged_not_failed() {
        let Some(reader) = reader() else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blank.pdf");
        let blank = String::from_utf8(tiny_pdf()).unwrap().replace("Hello Chingolo world", "").replace("Second line here", "");
        std::fs::write(&path, blank).unwrap();
        let out = reader.extract(&path, &|| false, &mut |_, _| {}).unwrap();
        assert_eq!(out.pages[0].quality, "none");
        assert!(out.blocks.is_empty());
    }

    /// Reads every PDF in $CHINGOLO_TEST_PAPERS and prints what came out.
    /// Run: CHINGOLO_TEST_PAPERS=~/papers cargo test corpus_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn corpus_smoke() {
        let Some(reader) = reader() else { return };
        let dir = std::env::var("CHINGOLO_TEST_PAPERS").expect("set CHINGOLO_TEST_PAPERS");
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

    /// Reads the scans in $CHINGOLO_TEST_PAPERS with the models in $CHINGOLO_OCR_DIR and prints what came out.
    /// Run: CHINGOLO_TEST_PAPERS=~/papers CHINGOLO_OCR_DIR=~/models cargo test ocr_scans -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ocr_scans() {
        let Some(reader) = reader() else { return };
        let papers = std::env::var("CHINGOLO_TEST_PAPERS").expect("set CHINGOLO_TEST_PAPERS");
        let models = std::env::var("CHINGOLO_OCR_DIR").expect("set CHINGOLO_OCR_DIR");
        let mut slot = OcrSlot::new(Some(PathBuf::from(models)));
        for name in ["383495a0.pdf", "Fs97-18-135-eng.pdf"] {
            let t = std::time::Instant::now();
            let out = reader.extract_with(&Path::new(&papers).join(name), &mut slot, &|| false, &mut |_, _| {}).unwrap();
            let read = out.pages.iter().filter(|p| p.ocr).count();
            let none = out.pages.iter().filter(|p| p.quality == "none").count();
            let passages = crate::services::ingestion::segment::segment(&out.blocks);
            println!("{name}: {} pages, {read} read by OCR, {none} still without text, {} passages, {:.0}s", out.pages.len(), passages.len(), t.elapsed().as_secs_f32());
            println!("  page 1 size {}x{}", out.pages[0].width, out.pages[0].height);
            for b in out.blocks.iter().take(4) {
                println!("  p{} {:?} font {:.1} | {}", b.page, b.rect.map(|r| r.map(|v| v.round())), b.font_size, b.text.chars().take(70).collect::<String>());
            }
            for p in passages.iter().skip(2).take(2) {
                println!("  passage p{}: {}", p.page_start, p.text.chars().take(300).collect::<String>().replace('\n', " "));
            }
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
