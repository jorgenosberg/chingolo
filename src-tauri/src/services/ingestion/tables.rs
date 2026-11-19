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
use crate::persistence::library::NewPassage;
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
            return Err(fail(format!("{} cells in the structure ({} tokens) but {} boxes", info.len(), tokens.len(), boxes.len())));
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
    /// The text layer has a space or a line break right before this character.
    pub space_before: bool,
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
    /// A second reader (Apple Vision) read the cell's pixels and agreed (`Some(true)`) or not
    /// (`Some(false)`) on its digits and symbols. `None` when it wasn't compared.
    pub verified: Option<bool>,
}

fn contains(r: &[f32; 4], (x, y): (f32, f32)) -> bool {
    x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
}

fn area(r: &[f32; 4]) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

/// Reads glyphs in reading order: top line first, left to right inside a line, a space where the
/// text layer has one (or the gap is wider than a quarter of the type size), lines joined by a space.
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
            if i > 0 && (x.space_before || x.rect[0] - l[i - 1].rect[2] > 0.25 * l[i - 1].height().max(x.height())) {
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
    /// Numbers the region reads that the cells don't, and the other way round (for the log).
    pub number_diff: (Vec<String>, Vec<String>),
}

impl Filled {
    /// Every character is in exactly one cell and no number was cut. Anything else keeps the
    /// plain lines.
    pub fn sound(&self) -> bool {
        self.glyphs > 0 && self.assigned == self.glyphs && self.numbers_ok
    }
}

/// How far outside a cell a character may sit and still belong to it, in character heights. The
/// model's cell boxes often clip the first or last letter of a cell.
const SLACK: f32 = 0.75;

fn distance(r: &[f32; 4], (x, y): (f32, f32)) -> f32 {
    let dx = (r[0] - x).max(0.0).max(x - r[2]);
    let dy = (r[1] - y).max(0.0).max(y - r[3]);
    dx.hypot(dy)
}

/// The next character of the text layer carries on the word: no space before it, on the same line,
/// and close behind the last one.
fn continues_word(prev: &Glyph, next: &Glyph) -> bool {
    let h = prev.height().max(next.height());
    let gap = next.rect[0] - prev.rect[2];
    !next.space_before && (next.centre().1 - prev.centre().1).abs() <= 0.5 * h && (-0.5 * h..=0.25 * h).contains(&gap)
}

/// Puts the text layer's characters into the cells. A character belongs to the nearest cell, the
/// smallest one when it sits inside several, and only if it is within `SLACK` character heights.
/// Spaces carry no box in the text layer: each character remembers whether one came before it.
pub fn fill(grid: &[(GridCell, [f32; 4])], region: [f32; 4], glyphs: &[Glyph]) -> Filled {
    let inside: Vec<&Glyph> = glyphs.iter().filter(|g| !g.ch.is_whitespace() && contains(&region, g.centre())).collect();
    let nearest = |g: &Glyph| -> Option<usize> {
        let c = g.centre();
        grid.iter()
            .enumerate()
            .map(|(i, (_, r))| (i, distance(r, c), area(r)))
            .filter(|(_, d, _)| *d <= SLACK * g.height())
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
            .map(|(i, _, _)| i)
    };
    // A word (no space, no wide gap) goes to one cell, the one most of its characters are nearest
    // to, so a box that clips the start of a word doesn't split it.
    let mut per_cell: Vec<Vec<Glyph>> = vec![Vec::new(); grid.len()];
    let mut assigned = 0;
    let mut i = 0;
    while i < inside.len() {
        let mut end = i + 1;
        while end < inside.len() && continues_word(inside[end - 1], inside[end]) {
            end += 1;
        }
        let mut votes = vec![0usize; grid.len()];
        for g in &inside[i..end] {
            if let Some(c) = nearest(g) {
                votes[c] += 1;
            }
        }
        let best = votes.iter().enumerate().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0))).filter(|(_, v)| **v > 0).map(|(c, _)| c);
        if let Some(c) = best {
            per_cell[c].extend(inside[i..end].iter().map(|g| **g));
            assigned += end - i;
        }
        i = end;
    }
    let cells: Vec<Cell> = grid
        .iter()
        .zip(&per_cell)
        .map(|((g, rect), gl)| Cell { row: g.row, col: g.col, row_span: g.row_span, col_span: g.col_span, rect: *rect, text: text_of(gl), verified: None })
        .collect();
    // Signs are left out: whether a minus belongs to a number depends on the space before it,
    // which the region text loses where cells meet. No character changes cell by mistake here
    // without the digits showing it.
    let unsigned = |t: String| t.trim_start_matches('−').to_string();
    let mut want: Vec<String> = numeric_tokens(&text_of(&inside.iter().map(|g| **g).collect::<Vec<_>>())).into_iter().map(unsigned).collect();
    let mut got: Vec<String> = cells.iter().flat_map(|c| numeric_tokens(&c.text)).map(unsigned).collect();
    want.sort();
    got.sort();
    let number_diff = (minus(&want, &got), minus(&got, &want));
    Filled { cells, glyphs: inside.len(), assigned, numbers_ok: number_diff.0.is_empty() && number_diff.1.is_empty(), number_diff }
}

/// `a` without the items of `b`, both sorted, counting repeats.
fn minus(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = b.iter().peekable();
    for x in a {
        while rest.next_if(|y| *y < x).is_some() {}
        if rest.next_if(|y| *y == x).is_none() {
            out.push(x.clone());
        }
    }
    out
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
    // cells the second reader did not confirm, by (row, column) of the grid
    let unconfirmed: Vec<(usize, usize)> = cells.iter().filter(|c| c.verified == Some(false)).map(|c| (c.row, c.col)).collect();
    let top = |chunk: &str, rows: &[usize]| {
        let mut text = match caption {
            Some(c) if !c.trim().is_empty() => format!("{}\n\n{chunk}", c.trim()),
            _ => chunk.to_string(),
        };
        let mut bad: Vec<String> = unconfirmed
            .iter()
            .filter(|(r, _)| *r == 0 || rows.contains(r))
            .map(|(r, c)| format!("row {}, column {}", r + 1, c + 1))
            .collect();
        bad.dedup();
        if !bad.is_empty() {
            text.push_str(&format!("\n\nNot confirmed by a second reading (treat these numbers and signs as uncertain): {}.", bad.join("; ")));
        }
        text
    };
    let mut out = Vec::new();
    let mut cur = head.clone();
    let mut rows_in: Vec<usize> = vec![];
    for (i, r) in grid.iter().enumerate().skip(1) {
        let l = line(r);
        if cur.len() > head.len() && cur.len() + l.len() + 1 > CHUNK_CHARS {
            out.push(top(&cur, &rows_in));
            cur = head.clone();
            rows_in.clear();
        }
        cur.push('\n');
        cur.push_str(&l);
        rows_in.push(i);
    }
    out.push(top(&cur, &rows_in));
    out
}

/// What the two readers must agree on in a cell: its digits, a decimal or thousands mark between
/// digits, a minus or dash touching a digit, the percent, comparison and degree signs, Greek letters,
/// and the prefix of a unit after a number (`mm` against `µm`). Vision reads µ as `u` (and the text
/// layer may store it as `m`), so `u`, µ and Greek mu count as one prefix, `m` as another. Everything
/// else is left out: spaces, plain letters (the readers differ on them often enough to be noise), and
/// the lines, dots and bullets Vision reads in cell borders. Dashes and minus signs count as one.
fn signature(text: &str) -> String {
    let chars: Vec<char> = text
        .chars()
        .map(|c| match c {
            '-' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2212}' => '\u{2212}',
            '\u{3bc}' | 'u' => '\u{b5}',
            c => c,
        })
        .collect();
    let digit = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(char::is_ascii_digit);
    let at = |i: usize| chars.get(i).copied().unwrap_or(' ');
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let keep = c.is_ascii_digit()
            || matches!(c, '%' | '<' | '>' | '\u{b1}' | '\u{b0}' | '\u{2264}' | '\u{2265}' | 'α' | 'β' | 'γ' | 'δ' | 'Δ' | 'σ' | 'Σ' | 'ε')
            || (matches!(c, '.' | ',') && digit(i.checked_sub(1)) && digit(Some(i + 1)))
            || (c == '\u{2212}' && digit(Some(i + 1)));
        if keep {
            out.push(c);
        }
        // a unit prefix after a number: "2 mm", "5 µg", "3 um" (a space may sit between)
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            if matches!(at(j), 'm' | '\u{b5}') && matches!(at(j + 1), 'm' | 'g' | 'l' | 'L' | 'M') {
                out.push(if at(j) == 'm' { 'm' } else { '\u{b5}' });
            }
        }
    }
    out
}

/// True when the cell has digits or symbols the second reader should be asked about.
pub fn worth_checking(text: &str) -> bool {
    !signature(text).is_empty()
}

/// The text layer's reading of a cell and Vision's agree on its digits and symbols.
pub fn readers_agree(layer: &str, vision: &str) -> bool {
    signature(layer) == signature(vision)
}

/// A passage made by `to_markdown`: a header row, then the `---` row.
pub fn is_grid(text: &str) -> bool {
    let mut lines = text.lines().skip_while(|l| l.trim().is_empty());
    let first = lines.next().is_some_and(|l| l.starts_with("| "));
    first && lines.next().is_some_and(|l| l.starts_with("| ---"))
}

/// True when two cells claim the same row and column. The grid is then not trustworthy.
pub fn has_duplicate_positions(cells: &[Cell]) -> bool {
    let mut seen = std::collections::HashSet::new();
    !cells.iter().all(|c| seen.insert((c.row, c.col)))
}

/// The passages of one table: Markdown chunks, each boxed by the table region. (Cell boxes per
/// chunk would let the reader highlight single cells, later.)
pub fn passages(page: u32, region: [f32; 4], cells: &[Cell]) -> Vec<NewPassage> {
    let boxes = serde_json::json!([{ "page": page, "rects": [region] }]).to_string();
    to_markdown(None, cells)
        .into_iter()
        .map(|text| NewPassage { page_start: page, page_end: page, text, boxes: Some(boxes.clone()) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word laid out left to right from `x`, glyphs 5 wide and 10 high, baseline `y`.
    fn word(s: &str, x: f32, y: f32) -> Vec<Glyph> {
        s.chars().enumerate().map(|(i, ch)| Glyph { ch, rect: [x + i as f32 * 5.0, y, x + i as f32 * 5.0 + 5.0, y + 10.0], space_before: false }).collect()
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
    fn readers_disagree_on_a_lost_minus_and_a_lost_micro_sign() {
        // the text layer stores a minus as a 2, and a µ as an m
        assert!(!readers_agree("2258", "\u{2212}258"));
        assert!(!readers_agree("12 mm", "12 \u{b5}m"));
        assert!(!readers_agree("0.2\u{2013}2 mm", "0.2-2 um"));
        assert!(!readers_agree(">20 mm", ">20 \u{b5}m"));
        assert!(readers_agree("2\u{2013}20 \u{b5}m", "2-20 um"));
        assert!(readers_agree("4 mm", "4 mm"));
        // dashes, the two mu characters, spaces and letters do not count
        assert!(readers_agree("\u{2212}258", "-258"));
        assert!(readers_agree("5 \u{3bc}mol", "5 \u{b5}mol"));
        assert!(readers_agree("12.5", "12.5 "));
        assert!(readers_agree("Site A", "Site A"));
        assert!(readers_agree("Temperature", "Temperatuur"));
        assert!(!readers_agree("12.5", "12,5"));
        // lines and dots Vision reads in a cell border, and a bullet, are not text
        assert!(readers_agree("7,44", "- 7,44 ..."));
        assert!(readers_agree("5 mg", "5 mg"));
        assert!(readers_agree("2022-07-29", "\u{2022} 2022-07-29 ---"));
        assert!(!readers_agree("258", "\u{2212}258"));
        assert!(worth_checking("n = 5") && !worth_checking("Temperature"));
    }

    #[test]
    fn unconfirmed_cells_are_listed_under_the_chunk_that_holds_them() {
        let mut cells: Vec<Cell> = (0..30)
            .flat_map(|r| (0..2).map(move |c| Cell { row: r, col: c, row_span: 1, col_span: 1, rect: [0.0; 4], text: if r == 0 { format!("H{c}") } else { "y".repeat(60) + &r.to_string() }, verified: None }))
            .collect();
        cells[2 * 25 + 1].verified = Some(false); // row 25, column 1
        cells[2 * 3].verified = Some(true);
        let md = to_markdown(None, &cells);
        assert!(md.len() > 1);
        let with: Vec<_> = md.iter().filter(|c| c.contains("Not confirmed by a second reading")).collect();
        assert_eq!(with.len(), 1);
        assert!(with[0].ends_with("row 26, column 2."));
        assert!(with[0].contains(&format!("{}25", "y".repeat(60))));
    }

    #[test]
    fn grid_passages_are_told_from_prose() {
        let cells = [(0, 0, "a"), (0, 1, "b"), (1, 0, "1"), (1, 1, "2")]
            .map(|(row, col, t)| Cell { row, col, row_span: 1, col_span: 1, rect: [0.0; 4], text: t.into(), verified: None });
        assert!(is_grid(&to_markdown(None, &cells)[0]));
        assert!(!is_grid("Plain text | with a pipe\nand more"));
    }

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
    fn a_letter_clipped_by_the_cell_box_still_belongs_to_it() {
        // The model's box for the right cell starts at 52 and cut the S (centre 49.5).
        let cells = vec![(gc(0, 0), [0.0, 0.0, 40.0, 30.0]), (gc(0, 1), [52.0, 0.0, 100.0, 30.0])];
        let g = word("Summer", 47.0, 10.0);
        let f = fill(&cells, [0.0, 0.0, 100.0, 30.0], &g);
        assert_eq!(f.cells[1].text, "Summer");
        assert!(f.sound());
    }

    #[test]
    fn a_word_clipped_by_a_cell_box_stays_whole() {
        // "2023-08-21" starts 2 characters left of the right cell's box: per character the first
        // two would be nearer the left cell, as a word they follow the other eight.
        let cells = vec![(gc(0, 0), [0.0, 0.0, 40.0, 30.0]), (gc(0, 1), [52.0, 0.0, 100.0, 30.0])];
        let g = word("2023-08-21", 38.0, 10.0);
        let f = fill(&cells, [0.0, 0.0, 100.0, 30.0], &g);
        assert_eq!((f.cells[0].text.as_str(), f.cells[1].text.as_str()), ("", "2023-08-21"));
    }

    #[test]
    fn two_columns_that_touch_still_split_at_the_gap() {
        let cells = vec![(gc(0, 0), [0.0, 0.0, 40.0, 30.0]), (gc(0, 1), [52.0, 0.0, 100.0, 30.0])];
        let mut g = word("KVOP", 20.0, 10.0);
        g.extend(word("2021", 54.0, 10.0)); // no space in the text layer, but a visible gap
        let f = fill(&cells, [0.0, 0.0, 100.0, 30.0], &g);
        assert_eq!((f.cells[0].text.as_str(), f.cells[1].text.as_str()), ("KVOP", "2021"));
    }

    #[test]
    fn the_text_layers_own_spaces_are_kept() {
        let mut g = word("Navn", 2.0, 50.0);
        let mut p = word("pa", 21.0, 50.0); // no visible gap at all
        p[0].space_before = true;
        g.extend(p);
        let f = fill(&grid(), REGION, &g);
        assert_eq!(f.cells[0].text, "Navn pa");
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
    fn a_character_far_from_every_cell_makes_the_table_unsure() {
        // rows 30 apart, so a character between them is 15 from both, over the slack of 7.5
        let cells = vec![(gc(0, 0), [0.0, 60.0, 100.0, 90.0]), (gc(1, 0), [0.0, 0.0, 100.0, 30.0])];
        let mut g = word("a", 2.0, 70.0);
        g.extend(word("x", 2.0, 40.0));
        let f = fill(&cells, [0.0, 0.0, 100.0, 90.0], &g);
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
    fn a_number_across_the_column_line_stays_whole_in_the_cell_with_most_of_it() {
        // "1,000" printed across the column line: 2 characters left of it, 3 right of it.
        let g = word("1,000", 40.0, 50.0);
        let f = fill(&grid(), REGION, &g);
        assert_eq!((f.cells[0].text.as_str(), f.cells[1].text.as_str()), ("", "1,000"));
        assert!(f.sound());
    }

    #[test]
    fn markdown_has_a_header_and_splits_on_rows_with_the_header_repeated() {
        let cells: Vec<Cell> = (0..40)
            .flat_map(|r| {
                (0..2).map(move |c| Cell { row: r, col: c, row_span: 1, col_span: 1, rect: [0.0; 4], text: if r == 0 { format!("H{c}") } else { format!("{}", "x".repeat(60)) + &r.to_string() }, verified: None })
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
            Cell { row: 0, col: 0, row_span: 1, col_span: 2, rect: [0.0; 4], text: "Wide".into(), verified: None },
            Cell { row: 1, col: 0, row_span: 1, col_span: 1, rect: [0.0; 4], text: "a|b".into(), verified: None },
            Cell { row: 1, col: 1, row_span: 1, col_span: 1, rect: [0.0; 4], text: "2".into(), verified: None },
        ];
        assert_eq!(to_markdown(None, &cells), ["| Wide |  |\n| --- | --- |\n| a\\|b | 2 |"]);
    }
}
