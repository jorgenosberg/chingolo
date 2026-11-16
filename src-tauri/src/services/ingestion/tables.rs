//! Tables. SLANet+ (through oar-ocr, ONNX Runtime) only finds the cell grid in a table crop. The
//! characters in each cell are the PDF's own: a character goes to the cell that holds its centre.
//! The checks here flag, they never repair: a character that lands in no cell, or a number that is
//! cut in two, marks the table "structure unsure" and the caller keeps today's plain lines.
//! See docs/STATUS.md ("Next batch") for the rules.

use std::path::{Path, PathBuf};

use oar_ocr::core::traits::adapter::{AdapterBuilder, ModelAdapter};
use oar_ocr::core::traits::task::ImageTaskInput;
use oar_ocr::domain::adapters::{SLANetWirelessAdapterBuilder, TableStructureRecognitionAdapter};
use oar_ocr::processors::parse_cell_grid_info;

use super::ocr::OcrModel;
use crate::error::{AppError, Result};
use crate::services::embedding::model::ManifestFile;

pub const FILES: &[ManifestFile] = &[
    ManifestFile {
        name: "slanet_plus.onnx",
        remote: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/slanet_plus.onnx",
        size: 7_782_138,
        sha256: "3a96a71719247c5d94992fca31266b598c54740388de371f0c75077e2a9e0b55",
    },
    ManifestFile {
        name: "table_structure_dict_ch.txt",
        remote: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/table_structure_dict_ch.txt",
        size: 578,
        sha256: "68d344a84b726e043f390122240ff2b2ced2949b2a80ce9b61ae955054d190ef",
    },
];

pub fn model_dir(app_data: &Path) -> PathBuf {
    app_data.join("models").join("tables-slanetplus-v0.3.0")
}

/// The download of the table model; same behaviour as the OCR and layout models.
pub fn model(app_data: &Path) -> OcrModel {
    OcrModel::with_files(model_dir(app_data), FILES)
}

pub fn is_ready(dir: &Path) -> bool {
    crate::services::embedding::model::is_complete(dir, FILES)
}

fn fail(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(format!("Table structure failed: {e}"))
}

/// One cell of the grid, in the pixels of the crop that was recognised (top-left origin).
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub rect: [f32; 4],
}

pub struct Tables {
    adapter: TableStructureRecognitionAdapter,
}

impl Tables {
    pub fn load(dir: &Path) -> Result<Self> {
        let adapter = SLANetWirelessAdapterBuilder::new()
            .dict_path(dir.join("table_structure_dict_ch.txt"))
            .build(&dir.join("slanet_plus.onnx"))
            .map_err(fail)?;
        Ok(Tables { adapter })
    }

    /// The cell grid of one table crop. Fails (for this table only) when the model's cell count and
    /// its structure tokens disagree.
    pub fn grid(&self, crop: image::RgbImage) -> Result<Vec<GridCell>> {
        let out = self.adapter.execute(ImageTaskInput::new(vec![crop]), None).map_err(fail)?;
        let (Some(tokens), Some(boxes)) = (out.structures.first(), out.bboxes.first()) else {
            return Err(fail("no structure returned"));
        };
        let info = parse_cell_grid_info(tokens);
        if info.len() != boxes.len() || info.is_empty() {
            return Err(fail(format!("{} cells in the structure but {} boxes", info.len(), boxes.len())));
        }
        Ok(info
            .into_iter()
            .zip(boxes)
            .map(|(g, b)| {
                let xs = b.iter().step_by(2).copied();
                let ys = b.iter().skip(1).step_by(2).copied();
                GridCell {
                    row: g.row,
                    col: g.col,
                    row_span: g.row_span.max(1),
                    col_span: g.col_span.max(1),
                    rect: [
                        xs.clone().fold(f32::MAX, f32::min),
                        ys.clone().fold(f32::MAX, f32::min),
                        xs.fold(f32::MIN, f32::max),
                        ys.fold(f32::MIN, f32::max),
                    ],
                }
            })
            .collect())
    }
}

/// Loads the table model on first use, like `LayoutSlot`.
#[derive(Default)]
pub struct TableSlot {
    dir: Option<PathBuf>,
    loaded: Option<Tables>,
}

impl TableSlot {
    pub fn set_dir(&mut self, dir: Option<PathBuf>) {
        if self.dir != dir {
            self.dir = dir;
            self.loaded = None;
        }
    }

    pub fn get(&mut self) -> Option<&Tables> {
        if self.loaded.is_none() {
            let dir = self.dir.as_ref().filter(|d| is_ready(d))?;
            match Tables::load(dir) {
                Ok(t) => self.loaded = Some(t),
                Err(e) => {
                    log::warn!("table model failed to load: {e}");
                    self.dir = None;
                    return None;
                }
            }
        }
        self.loaded.as_ref()
    }
}

/// Numbers as printed: optional sign (hyphen, en dash or minus), digits, one decimal mark.
/// A comma or dot between digits is one decimal mark (Norwegian style), so `1,000` is one token.
pub fn numeric_tokens(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i + 1 < chars.len() && matches!(chars[i], '.' | ',') && chars[i + 1].is_ascii_digit() {
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
        let signed = start > 0 && matches!(chars[start - 1], '−' | '-' | '–') && (start < 2 || chars[start - 2].is_whitespace() || matches!(chars[start - 2], '(' | '['));
        let mut tok: String = chars[start..i].iter().collect();
        if signed {
            tok.insert(0, '−');
        }
        out.push(tok);
    }
    out
}

/// One character of the text layer with its box in PDF user space `[left, bottom, right, top]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub ch: char,
    pub rect: [f32; 4],
}

impl Glyph {
    fn centre(&self) -> (f32, f32) {
        ((self.rect[0] + self.rect[2]) / 2.0, (self.rect[1] + self.rect[3]) / 2.0)
    }

    fn height(&self) -> f32 {
        self.rect[3] - self.rect[1]
    }
}

/// A cell with its box in PDF user space and the text that landed in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub rect: [f32; 4],
    pub text: String,
}

fn contains(r: &[f32; 4], (x, y): (f32, f32)) -> bool {
    x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
}

fn area(r: &[f32; 4]) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

/// Reads glyphs in reading order: top line first, left to right inside a line, a space where the
/// gap is wider than a quarter of the type size, lines joined by a space.
fn text_of(glyphs: &[Glyph]) -> String {
    let mut g = glyphs.to_vec();
    g.sort_by(|a, b| b.centre().1.total_cmp(&a.centre().1));
    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    for x in g {
        match lines.last_mut() {
            Some(l) if (l[0].centre().1 - x.centre().1).abs() <= 0.5 * l[0].height().max(x.height()) => l.push(x),
            _ => lines.push(vec![x]),
        }
    }
    let mut out = Vec::with_capacity(lines.len());
    for mut l in lines {
        l.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        let mut s = String::new();
        for (i, x) in l.iter().enumerate() {
            if i > 0 && x.rect[0] - l[i - 1].rect[2] > 0.25 * l[i - 1].height().max(x.height()) {
                s.push(' ');
            }
            s.push(x.ch);
        }
        out.push(s);
    }
    out.join(" ")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filled {
    pub cells: Vec<Cell>,
    /// Characters whose centre is inside the table region.
    pub glyphs: usize,
    /// Of those, the ones that landed in a cell.
    pub assigned: usize,
    /// The numbers in the region are the numbers in the cells (none cut in two, none lost).
    pub numbers_ok: bool,
}

impl Filled {
    /// Every character is in exactly one cell and no number was cut. Anything else keeps the
    /// plain lines.
    pub fn sound(&self) -> bool {
        self.glyphs > 0 && self.assigned == self.glyphs && self.numbers_ok
    }
}

/// Puts the text layer's characters into the cells. A character belongs to the smallest cell that
/// holds its centre. Spaces carry no box in the text layer and are not needed: they come back from
/// the gaps between characters.
pub fn fill(grid: &[(GridCell, [f32; 4])], region: [f32; 4], glyphs: &[Glyph]) -> Filled {
    let inside: Vec<&Glyph> = glyphs.iter().filter(|g| !g.ch.is_whitespace() && contains(&region, g.centre())).collect();
    let mut per_cell: Vec<Vec<Glyph>> = vec![Vec::new(); grid.len()];
    let mut assigned = 0;
    for g in &inside {
        let c = g.centre();
        let best = grid
            .iter()
            .enumerate()
            .filter(|(_, (_, r))| contains(r, c))
            .min_by(|(_, (_, a)), (_, (_, b))| area(a).total_cmp(&area(b)))
            .map(|(i, _)| i);
        if let Some(i) = best {
            per_cell[i].push(**g);
            assigned += 1;
        }
    }
    let cells: Vec<Cell> = grid
        .iter()
        .zip(&per_cell)
        .map(|((g, rect), gl)| Cell { row: g.row, col: g.col, row_span: g.row_span, col_span: g.col_span, rect: *rect, text: text_of(gl) })
        .collect();
    let mut want = numeric_tokens(&text_of(&inside.iter().map(|g| **g).collect::<Vec<_>>()));
    let mut got: Vec<String> = cells.iter().flat_map(|c| numeric_tokens(&c.text)).collect();
    want.sort();
    got.sort();
    Filled { cells, glyphs: inside.len(), assigned, numbers_ok: want == got }
}

/// Passages are cut at about this many characters, on row boundaries.
const CHUNK_CHARS: usize = 2000;

/// The table as Markdown grids, one per chunk of rows. Row 0 is the header and repeats in every
/// chunk. Cells covered by a span are left empty. The caption, if any, starts each chunk.
pub fn to_markdown(caption: Option<&str>, cells: &[Cell]) -> Vec<String> {
    let rows = cells.iter().map(|c| c.row + c.row_span).max().unwrap_or(0);
    let cols = cells.iter().map(|c| c.col + c.col_span).max().unwrap_or(0);
    if rows == 0 || cols == 0 {
        return vec![];
    }
    let mut grid = vec![vec![String::new(); cols]; rows];
    for c in cells {
        grid[c.row][c.col] = c.text.replace('|', "\\|").split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let line = |r: &[String]| format!("| {} |", r.join(" | "));
    let head = format!("{}\n| {} |", line(&grid[0]), vec!["---"; cols].join(" | "));
    let top = |chunk: &str| match caption {
        Some(c) if !c.trim().is_empty() => format!("{}\n\n{chunk}", c.trim()),
        _ => chunk.to_string(),
    };
    let mut out = Vec::new();
    let mut cur = head.clone();
    for r in &grid[1..] {
        let l = line(r);
        if cur.len() > head.len() && cur.len() + l.len() + 1 > CHUNK_CHARS {
            out.push(top(&cur));
            cur = head.clone();
        }
        cur.push('\n');
        cur.push_str(&l);
    }
    out.push(top(&cur));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word laid out left to right from `x`, glyphs 5 wide and 10 high, baseline `y`.
    fn word(s: &str, x: f32, y: f32) -> Vec<Glyph> {
        s.chars().enumerate().map(|(i, ch)| Glyph { ch, rect: [x + i as f32 * 5.0, y, x + i as f32 * 5.0 + 5.0, y + 10.0] }).collect()
    }

    fn gc(row: usize, col: usize) -> GridCell {
        GridCell { row, col, row_span: 1, col_span: 1, rect: [0.0; 4] }
    }

    /// Two columns, two rows. The cells are 50 wide and 30 high, row 0 on top.
    fn grid() -> Vec<(GridCell, [f32; 4])> {
        vec![
            (gc(0, 0), [0.0, 40.0, 50.0, 70.0]),
            (gc(0, 1), [50.0, 40.0, 100.0, 70.0]),
            (gc(1, 0), [0.0, 0.0, 50.0, 30.0]),
            (gc(1, 1), [50.0, 0.0, 100.0, 30.0]),
        ]
    }

    const REGION: [f32; 4] = [0.0, 0.0, 100.0, 70.0];

    #[test]
    fn characters_go_to_the_cell_that_holds_their_centre() {
        let mut g = word("Site", 2.0, 50.0);
        g.extend(word("n", 52.0, 50.0));
        g.extend(word("A", 2.0, 10.0));
        g.extend(word("12.5", 52.0, 10.0));
        let f = fill(&grid(), REGION, &g);
        assert_eq!(f.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Site", "n", "A", "12.5"]);
        assert!(f.sound());
    }

    #[test]
    fn a_wrapped_cell_reads_top_line_first_and_gaps_become_spaces() {
        let mut g = word("mg", 2.0, 55.0);
        g.extend(word("per", 22.0, 55.0));
        g.extend(word("litre", 2.0, 45.0));
        let f = fill(&grid(), REGION, &g);
        assert_eq!(f.cells[0].text, "mg per litre");
    }

    #[test]
    fn a_character_outside_every_cell_makes_the_table_unsure() {
        let mut g = word("a", 2.0, 50.0);
        g.extend(word("x", 2.0, 33.0)); // in the gap between the rows, inside the region
        let f = fill(&grid(), REGION, &g);
        assert_eq!((f.glyphs, f.assigned), (2, 1));
        assert!(!f.sound());
    }

    #[test]
    fn characters_outside_the_region_are_not_counted() {
        let mut g = word("a", 2.0, 50.0);
        g.extend(word("body text", 2.0, 300.0));
        let f = fill(&grid(), REGION, &g);
        assert_eq!((f.glyphs, f.assigned), (1, 1));
        assert!(f.sound());
    }

    #[test]
    fn a_number_cut_across_two_cells_is_flagged() {
        // "1,000" printed across the column line: the region reads one number, the cells two.
        let g = word("1,000", 40.0, 50.0);
        let f = fill(&grid(), REGION, &g);
        assert_eq!(f.assigned, f.glyphs);
        assert!(!f.numbers_ok);
        assert!(!f.sound());
    }

    #[test]
    fn markdown_has_a_header_and_splits_on_rows_with_the_header_repeated() {
        let cells: Vec<Cell> = (0..40)
            .flat_map(|r| {
                (0..2).map(move |c| Cell { row: r, col: c, row_span: 1, col_span: 1, rect: [0.0; 4], text: if r == 0 { format!("H{c}") } else { format!("{}", "x".repeat(60)) + &r.to_string() } })
            })
            .collect();
        let md = to_markdown(Some("Table 1. Things"), &cells);
        assert!(md.len() > 1);
        for chunk in &md {
            assert!(chunk.starts_with("Table 1. Things\n\n| H0 | H1 |\n| --- | --- |\n"));
            assert!(chunk.len() <= CHUNK_CHARS + 200);
        }
        assert_eq!(md.iter().map(|c| c.matches(&"x".repeat(60)).count()).sum::<usize>(), 39 * 2);
    }

    #[test]
    fn markdown_escapes_pipes_and_leaves_spanned_positions_empty() {
        let cells = vec![
            Cell { row: 0, col: 0, row_span: 1, col_span: 2, rect: [0.0; 4], text: "Wide".into() },
            Cell { row: 1, col: 0, row_span: 1, col_span: 1, rect: [0.0; 4], text: "a|b".into() },
            Cell { row: 1, col: 1, row_span: 1, col_span: 1, rect: [0.0; 4], text: "2".into() },
        ];
        assert_eq!(to_markdown(None, &cells), ["| Wide |  |\n| --- | --- |\n| a\\|b | 2 |"]);
    }
}
