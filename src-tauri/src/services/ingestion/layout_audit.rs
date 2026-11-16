//! Ignored audit of table extraction against `expected-tables.json` (owner-checked ground truth, kept
//! outside the repo next to `bench.json`).
//! Run: CHINGUI_TEST_PAPERS=<folder> cargo test corpus_layout -- --ignored --nocapture
//! Until the table code exists this prints a baseline: which numbers of each table are already missing
//! or changed in today's page text. Tables with `checked: false` are reported but marked.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::pdf::PdfReader;

/// Numbers as printed: optional sign (hyphen, en dash or minus), digits, one decimal mark.
/// Thousands separators are not joined, so `1,000` is two tokens on both sides.
fn numeric_tokens(s: &str) -> Vec<String> {
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

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Default, PartialEq)]
pub struct GridScore {
    pub shape_ok: bool,
    pub cells: usize,
    pub cells_exact: usize,
    pub numeric_cells: usize,
    pub numeric_exact: usize,
}

/// Compares a produced grid with the truth cell by cell. Whitespace (including newlines) is
/// collapsed. A cell is numeric when the truth has a digit in it.
pub fn score_grid(got: &[Vec<String>], want: &[Vec<String>]) -> GridScore {
    let mut s = GridScore { shape_ok: got.len() == want.len() && got.iter().zip(want).all(|(a, b)| a.len() == b.len()), ..Default::default() };
    for (r, row) in want.iter().enumerate() {
        for (c, w) in row.iter().enumerate() {
            let exact = got.get(r).and_then(|g| g.get(c)).is_some_and(|g| norm(g) == norm(w));
            s.cells += 1;
            s.cells_exact += exact as usize;
            if w.chars().any(|ch| ch.is_ascii_digit()) {
                s.numeric_cells += 1;
                s.numeric_exact += exact as usize;
            }
        }
    }
    s
}

fn grid(v: &Value) -> Vec<Vec<String>> {
    v.as_array()
        .map(|rows| rows.iter().map(|r| r.as_array().map(|c| c.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()).collect())
        .unwrap_or_default()
}

/// Truth tokens that do not appear (as a multiset) in the page text.
fn missing_tokens(want: &[Vec<String>], page_text: &str) -> Vec<String> {
    let mut have: BTreeMap<String, i32> = BTreeMap::new();
    for t in numeric_tokens(page_text) {
        *have.entry(t).or_default() += 1;
    }
    let mut missing = Vec::new();
    for t in want.iter().flatten().flat_map(|c| numeric_tokens(c)) {
        match have.get_mut(&t) {
            Some(n) if *n > 0 => *n -= 1,
            _ => missing.push(t),
        }
    }
    missing
}

#[test]
fn numeric_tokens_keep_signs() {
    assert_eq!(numeric_tokens("−258 (18%) 0.65 1,000 a-3 (-47)"), ["−258", "18", "0.65", "1", "000", "3", "−47"]);
    assert_eq!(numeric_tokens("2258"), ["2258"]);
}

#[test]
fn grid_score_counts_numeric_cells() {
    let g = |r: &[&[&str]]| r.iter().map(|x| x.iter().map(|s| s.to_string()).collect()).collect::<Vec<Vec<String>>>();
    let want = g(&[&["Name", "1.5"], &["a\nb", "−2"]]);
    let got = g(&[&["Name", "1.5"], &["a b", "2"]]);
    let s = score_grid(&got, &want);
    assert_eq!((s.shape_ok, s.cells, s.cells_exact, s.numeric_cells, s.numeric_exact), (true, 4, 3, 2, 1));
}

#[test]
#[ignore]
fn corpus_layout() {
    let dir = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
    let Ok(reader) = PdfReader::new() else {
        eprintln!("skipped: libpdfium not found");
        return;
    };
    let truth: Value = serde_json::from_slice(&std::fs::read(Path::new(&dir).join("expected-tables.json")).expect("expected-tables.json")).unwrap();
    println!("{:<12} {:>7} {:>5} {:>8} | numbers missing from the page text", "table", "checked", "page", "numbers");
    for t in truth["tables"].as_array().unwrap() {
        let id = t["id"].as_str().unwrap();
        let page = t["page"].as_u64().unwrap() as u32;
        let want = grid(&t["rows"]);
        let Ok(out) = reader.extract(&Path::new(&dir).join(t["file"].as_str().unwrap()), &|| false, &mut |_, _| {}) else {
            println!("{id}: could not be read");
            continue;
        };
        let text = out.blocks.iter().filter(|b| b.page == page).map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        let total = want.iter().flatten().flat_map(|c| numeric_tokens(c)).count();
        let missing = missing_tokens(&want, &text);
        println!("{:<12} {:>7} {:>5} {:>8} | {} {}", id, t["checked"].as_bool().unwrap_or(false), page, total, missing.len(), missing.iter().take(12).cloned().collect::<Vec<_>>().join(" "));
    }
}
