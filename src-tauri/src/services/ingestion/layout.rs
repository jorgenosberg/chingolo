//! Page layout with PP-DocLayoutV3 through oar-ocr (ONNX Runtime): which parts of a page are body
//! text, headers, footers, tables, figures, and in what order they read. Regions only say where
//! things are. The characters in them still come from the PDF's own text layer (or OCR on scans).

use std::path::{Path, PathBuf};

use oar_ocr::core::traits::adapter::{AdapterBuilder, ModelAdapter};
use oar_ocr::core::traits::task::ImageTaskInput;
use oar_ocr::domain::adapters::{
    LayoutDetectionAdapter, LayoutDetectionAdapterBuilder, LayoutModelConfig,
};
use oar_ocr::domain::tasks::LayoutDetectionConfig;

use super::ocr::OcrModel;
use super::segment::{Block, Join, segment};
use crate::error::{AppError, Result};
use crate::persistence::library::NewPassage;
use crate::services::embedding::model::ManifestFile;

pub const FILES: &[ManifestFile] = &[ManifestFile {
    name: "pp-doclayoutv3.onnx",
    remote: "https://github.com/GreatV/oar-ocr/releases/download/v0.6.0/pp-doclayoutv3.onnx",
    size: 129_955_811,
    sha256: "1a7ec3812d239ad14debb87e38273012558fea27ac10b44b61260e3a88358e39",
}];

pub fn model_dir(app_data: &Path) -> PathBuf {
    app_data.join("models").join("layout-ppdoclayoutv3-v0.6.0")
}

/// The download of the layout model; same behaviour as the OCR models.
pub fn model(app_data: &Path) -> OcrModel {
    OcrModel::with_files(model_dir(app_data), FILES)
}

/// One region. The box is in image pixels, top-left origin.
#[derive(Debug, Clone)]
pub struct Region {
    /// The model's own label, e.g. `text`, `table`, `header`, `footnote`.
    pub label: String,
    pub rect: [f32; 4],
    pub score: f32,
    /// Position in the model's reading order, 0-based.
    pub order: u32,
}

impl Region {
    /// Running heads, page numbers and the like: not part of what the document says.
    pub fn is_furniture(&self) -> bool {
        matches!(
            self.label.as_str(),
            "header" | "header_image" | "footer" | "footer_image" | "number"
        )
    }
}

/// Loads the layout model on first use, like `OcrSlot`.
#[derive(Default)]
pub struct LayoutSlot {
    dir: Option<PathBuf>,
    loaded: Option<Layout>,
}

impl LayoutSlot {
    pub fn set_dir(&mut self, dir: Option<PathBuf>) {
        if self.dir != dir {
            self.dir = dir;
            self.loaded = None;
        }
    }

    pub fn get(&mut self) -> Option<&Layout> {
        if self.loaded.is_none() {
            let dir = self.dir.as_ref().filter(|d| is_ready(d))?;
            match Layout::load(dir) {
                Ok(l) => self.loaded = Some(l),
                Err(e) => {
                    log::warn!("layout model failed to load: {e}");
                    self.dir = None;
                    return None;
                }
            }
        }
        self.loaded.as_ref()
    }
}

pub fn is_ready(dir: &Path) -> bool {
    crate::services::embedding::model::is_complete(dir, FILES)
}

pub struct Layout {
    adapter: LayoutDetectionAdapter,
}

fn fail(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Page layout failed: {e}"))
}

impl Layout {
    pub fn load(dir: &Path) -> Result<Self> {
        let adapter = LayoutDetectionAdapterBuilder::new()
            .model_config(LayoutModelConfig::pp_doclayoutv3())
            .with_config(LayoutDetectionConfig::with_pp_doclayoutv3_defaults())
            .build(&dir.join("pp-doclayoutv3.onnx"))
            .map_err(fail)?;
        Ok(Layout { adapter })
    }

    /// Regions in reading order, from the layout model alone (no text recognition, no table models,
    /// which fail a whole page when the wired table model is missing).
    pub fn detect(&self, img: image::RgbImage) -> Result<Vec<Region>> {
        let out = self
            .adapter
            .execute(ImageTaskInput::new(vec![img]), None)
            .map_err(fail)?;
        let elements = out.elements.into_iter().next().unwrap_or_default();
        Ok(elements
            .into_iter()
            .enumerate()
            .map(|(i, e)| Region {
                label: e.element_type.to_lowercase(),
                rect: [
                    e.bbox.x_min(),
                    e.bbox.y_min(),
                    e.bbox.x_max(),
                    e.bbox.y_max(),
                ],
                score: e.score,
                order: i as u32,
            })
            .collect())
    }
}

/// Regions of one page, in PDF user space (the same space as the line boxes).
#[derive(Debug, Clone)]
pub struct PageRegions {
    pub page: u32,
    pub regions: Vec<Region>,
}

/// What a region is for passage building: passages never run from one kind into another, so a
/// table, a chart's labels and a footnote each stay apart from the body text around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Body,
    Table,
    Figure,
    Aside,
}

fn kind(label: &str) -> Kind {
    match label {
        "table" => Kind::Table,
        "chart" | "image" | "figure_title" | "chart_title" | "seal" => Kind::Figure,
        "footnote" | "vision_footnote" => Kind::Aside,
        _ => Kind::Body,
    }
}

/// Furniture below this score is kept: better a running head in the text than a lost paragraph.
const MIN_FURNITURE_SCORE: f32 = 0.5;
/// A line belongs to the region that holds at least this share of its area.
const MIN_OVERLAP: f32 = 0.5;
/// A page where fewer lines than this find a region keeps its lines as they were.
const MIN_COVERAGE: f32 = 0.8;

fn area(r: &[f32; 4]) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    area(&[
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ])
}

/// The region a line belongs to: the smallest one holding most of it. Inline formula boxes sit
/// inside text and are not regions of their own.
fn region_of(rect: &[f32; 4], regions: &[Region]) -> Option<usize> {
    let a = area(rect);
    if a <= 0.0 {
        return None;
    }
    regions
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.label.contains("formula") && overlap(rect, &r.rect) / a >= MIN_OVERLAP)
        .min_by(|(_, x), (_, y)| area(&x.rect).total_cmp(&area(&y.rect)))
        .map(|(i, _)| i)
}

#[derive(Debug, Default, PartialEq)]
pub struct ApplyStats {
    pub lines: usize,
    pub dropped: usize,
    pub unplaced: usize,
    pub pages_kept: usize,
}

/// Puts lines in the layout's reading order, drops running heads, footers and page numbers, and
/// marks where one kind of region ends and another starts. No characters change: a line is kept
/// whole or dropped whole. Pages without regions, or where too few lines find one, are left alone.
pub fn apply(blocks: Vec<Block>, pages: &[PageRegions]) -> (Vec<Block>, ApplyStats) {
    let mut stats = ApplyStats::default();
    let mut out = Vec::with_capacity(blocks.len());
    let mut i = 0;
    while i < blocks.len() {
        let page = blocks[i].page;
        let end = blocks[i..]
            .iter()
            .position(|b| b.page != page)
            .map_or(blocks.len(), |n| i + n);
        let lines = &blocks[i..end];
        i = end;
        stats.lines += lines.len();
        let regions = pages
            .iter()
            .find(|p| p.page == page)
            .map(|p| p.regions.as_slice())
            .unwrap_or(&[]);
        let placed: Vec<Option<usize>> = lines
            .iter()
            .map(|b| b.rect.and_then(|r| region_of(&r, regions)))
            .collect();
        let found = placed.iter().filter(|p| p.is_some()).count();
        if regions.is_empty() || (found as f32) < MIN_COVERAGE * lines.len() as f32 {
            out.extend_from_slice(lines);
            stats.pages_kept += 1;
            continue;
        }
        // lines stay in the order they came in within a region; regions go in the model's order
        let mut order: Vec<usize> = (0..lines.len()).collect();
        order.sort_by_key(|&j| {
            (
                placed[j].map_or(usize::MAX, |r| regions[r].order as usize),
                j,
            )
        });
        let mut prev: Option<Kind> = None;
        for j in order {
            let Some(r) = placed[j].map(|r| &regions[r]) else {
                stats.unplaced += 1;
                out.push(lines[j].clone());
                continue;
            };
            if r.is_furniture() && r.score >= MIN_FURNITURE_SCORE {
                stats.dropped += 1;
                continue;
            }
            let mut b = lines[j].clone();
            let k = kind(&r.label);
            if prev.is_some_and(|p| p != k) {
                b.join = Join::Break;
            }
            prev = Some(k);
            out.push(b);
        }
    }
    (out, stats)
}

/// Passages from the lines `apply` kept, with each read table's grid passages put where its first
/// line stood. The table's own lines are left out. `tables` are (page, region box, passages).
pub fn weave(
    blocks: &[Block],
    pages: &[PageRegions],
    tables: &[(u32, [f32; 4], Vec<NewPassage>)],
) -> Vec<NewPassage> {
    let mut out = Vec::new();
    let mut run: Vec<Block> = Vec::new();
    let mut placed = vec![false; tables.len()];
    for b in blocks {
        let regions = pages
            .iter()
            .find(|p| p.page == b.page)
            .map(|p| p.regions.as_slice())
            .unwrap_or(&[]);
        let hit = b
            .rect
            .and_then(|r| region_of(&r, regions))
            .map(|i| regions[i].rect)
            .and_then(|rect| {
                tables
                    .iter()
                    .position(|(p, t, _)| *p == b.page && *t == rect)
            });
        match hit {
            Some(t) => {
                if !placed[t] {
                    placed[t] = true;
                    out.extend(segment(&run));
                    run.clear();
                    out.extend(tables[t].2.iter().cloned());
                }
            }
            None => run.push(b.clone()),
        }
    }
    out.extend(segment(&run));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(label: &str, order: u32, rect: [f32; 4]) -> Region {
        Region {
            label: label.into(),
            rect,
            score: 0.9,
            order,
        }
    }

    fn line(text: &str, rect: [f32; 4]) -> Block {
        Block {
            page: 1,
            text: text.into(),
            rect: Some(rect),
            font_size: 10.0,
            join: Join::Space,
        }
    }

    fn page(regions: Vec<Region>) -> Vec<PageRegions> {
        vec![PageRegions { page: 1, regions }]
    }

    #[test]
    fn lines_follow_the_layout_order_and_furniture_is_dropped() {
        // PDF user space, y up. Right column is listed first by the file, the layout reads left first.
        let regions = page(vec![
            region("header", 0, [0.0, 280.0, 200.0, 300.0]),
            region("text", 1, [0.0, 100.0, 90.0, 270.0]),
            region("text", 2, [110.0, 100.0, 200.0, 270.0]),
            region("number", 3, [90.0, 0.0, 110.0, 20.0]),
        ]);
        let blocks = vec![
            line("Journal 2025", [10.0, 285.0, 100.0, 295.0]),
            line("right 1", [120.0, 250.0, 190.0, 260.0]),
            line("left 1", [10.0, 250.0, 80.0, 260.0]),
            line("left 2", [10.0, 235.0, 80.0, 245.0]),
            line("12", [95.0, 5.0, 105.0, 15.0]),
        ];
        let (out, stats) = apply(blocks, &regions);
        assert_eq!(
            out.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
            ["left 1", "left 2", "right 1"]
        );
        assert_eq!((stats.dropped, stats.unplaced, stats.pages_kept), (2, 0, 0));
    }

    #[test]
    fn a_table_is_kept_apart_from_the_text_around_it() {
        let regions = page(vec![
            region("text", 0, [0.0, 200.0, 200.0, 290.0]),
            region("table", 1, [0.0, 100.0, 200.0, 190.0]),
            region("text", 2, [0.0, 10.0, 200.0, 90.0]),
        ]);
        let blocks = vec![
            line("before", [10.0, 250.0, 90.0, 260.0]),
            line("cell", [10.0, 150.0, 90.0, 160.0]),
            line("after", [10.0, 50.0, 90.0, 60.0]),
        ];
        let (out, _) = apply(blocks, &regions);
        let joins: Vec<Join> = out.iter().map(|b| b.join).collect();
        assert_eq!(joins, [Join::Space, Join::Break, Join::Break]);
    }

    #[test]
    fn a_read_table_replaces_its_lines_and_sits_in_reading_order() {
        let regions = page(vec![
            region("text", 0, [0.0, 200.0, 200.0, 290.0]),
            region("table", 1, [0.0, 100.0, 200.0, 190.0]),
            region("text", 2, [0.0, 10.0, 200.0, 90.0]),
        ]);
        let blocks = vec![
            line("The text before the table.", [10.0, 250.0, 90.0, 260.0]),
            line("cell one", [10.0, 150.0, 90.0, 160.0]),
            line("cell two", [10.0, 130.0, 90.0, 140.0]),
            line("The text after the table.", [10.0, 50.0, 90.0, 60.0]),
        ];
        let table = NewPassage {
            page_start: 1,
            page_end: 1,
            text: "| a | b |".into(),
            boxes: None,
        };
        let out = weave(
            &blocks,
            &regions,
            &[(1, [0.0, 100.0, 200.0, 190.0], vec![table])],
        );
        assert_eq!(
            out.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            [
                "The text before the table.",
                "| a | b |",
                "The text after the table."
            ]
        );
    }

    #[test]
    fn low_coverage_or_no_regions_leaves_the_page_alone() {
        let blocks = vec![
            line("a", [10.0, 10.0, 50.0, 20.0]),
            line("b", [10.0, 30.0, 50.0, 40.0]),
        ];
        let (out, stats) = apply(blocks.clone(), &page(vec![]));
        assert_eq!((out.len(), stats.pages_kept), (2, 1));
        let far = page(vec![region("text", 0, [500.0, 500.0, 600.0, 600.0])]);
        let (out, stats) = apply(blocks, &far);
        assert_eq!((out.len(), stats.pages_kept, stats.dropped), (2, 1, 0));
    }

    #[test]
    fn weak_furniture_and_inline_formulas_do_not_remove_text() {
        let mut head = region("header", 0, [0.0, 100.0, 200.0, 200.0]);
        head.score = 0.3;
        let regions = page(vec![
            head,
            region("inline_formula", 1, [10.0, 140.0, 40.0, 160.0]),
        ]);
        let (out, stats) = apply(vec![line("kept", [10.0, 145.0, 90.0, 155.0])], &regions);
        assert_eq!((out.len(), stats.dropped), (1, 0));
    }
}
