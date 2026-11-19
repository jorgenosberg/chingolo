//! Ignored audit of table extraction against `expected-tables.json` (owner-checked ground truth, kept
//! outside the repo next to `bench.json`).
//! Run: CHINGUI_TEST_PAPERS=<folder> cargo test corpus_layout -- --ignored --nocapture
//! Until the table code exists this prints a baseline: which numbers of each table are already missing
//! or changed in today's page text. Tables with `checked: false` are reported but marked.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::pdf::PdfReader;
use super::tables::numeric_tokens;

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
    assert_eq!(numeric_tokens("−258 (18%) 0.65 1,000 a-3 (-47)"), ["−258", "18", "0.65", "1,000", "3", "−47"]);
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

/// Prints the regions for a few pages. CHINGUI_LAYOUT_DIR=<folder with pp-doclayoutv3.onnx>
/// CHINGUI_LAYOUT_PAGES="nature02437.pdf:2,Fiskehelserapporten 2025.pdf:12"
#[test]
#[ignore]
fn layout_regions() {
    let dir = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
    let model = std::env::var("CHINGUI_LAYOUT_DIR").expect("set CHINGUI_LAYOUT_DIR");
    let pages = std::env::var("CHINGUI_LAYOUT_PAGES").expect("set CHINGUI_LAYOUT_PAGES");
    let reader = PdfReader::new().expect("libpdfium");
    let layout = super::layout::Layout::load(Path::new(&model)).unwrap();
    for spec in pages.split(',') {
        let (file, n) = spec.rsplit_once(':').unwrap();
        let r = reader.render_page(&Path::new(&dir).join(file), n.parse().unwrap()).unwrap();
        let t = std::time::Instant::now();
        let regions = layout.detect(r.img).unwrap();
        println!("{file} p{n}: {} regions in {:.1}s", regions.len(), t.elapsed().as_secs_f32());
        for g in regions {
            println!("  {:>2} {:<16} {:.2} [{:.0} {:.0} {:.0} {:.0}]", g.order, g.label, g.score, g.rect[0], g.rect[1], g.rect[2], g.rect[3]);
        }
    }
}

/// Runs layout and the rebuild on whole files and prints what was dropped.
/// CHINGUI_LAYOUT_FILES="nature02437.pdf,Fiskehelserapporten 2025.pdf"
#[test]
#[ignore]
fn layout_apply() {
    use super::layout::{Layout, apply};
    use super::segment::segment;
    let dir = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
    let model = std::env::var("CHINGUI_LAYOUT_DIR").expect("set CHINGUI_LAYOUT_DIR");
    let files = std::env::var("CHINGUI_LAYOUT_FILES").expect("set CHINGUI_LAYOUT_FILES");
    let reader = PdfReader::new().expect("libpdfium");
    let layout = Layout::load(Path::new(&model)).unwrap();
    for file in files.split(',') {
        let path = Path::new(&dir).join(file);
        let ex = reader.extract(&path, &|| false, &mut |_, _| {}).unwrap();
        let pages: Vec<u32> = ex.pages.iter().filter(|p| p.quality == "ok").map(|p| p.page_no).collect();
        let t = std::time::Instant::now();
        let found = reader.detect_layout(&path, &pages, &layout, &|| false, &mut |_, _| {}).unwrap();
        let secs = t.elapsed().as_secs_f32();
        let before = segment(&ex.blocks);
        let kept: std::collections::HashSet<String> = {
            let (blocks, _) = apply(ex.blocks.clone(), &found);
            blocks.iter().map(|b| format!("{}|{}", b.page, b.text)).collect()
        };
        let dropped: Vec<String> = ex.blocks.iter().filter(|b| !kept.contains(&format!("{}|{}", b.page, b.text))).map(|b| format!("p{} {}", b.page, b.text.chars().take(70).collect::<String>())).collect();
        let (blocks, stats) = apply(ex.blocks, &found);
        let after = segment(&blocks);
        println!("== {file}: {} pages in {secs:.1}s ({:.2}s/page), passages {} -> {}, {stats:?}", pages.len(), secs / pages.len().max(1) as f32, before.len(), after.len());
        for d in dropped.iter().take(25) {
            println!("   dropped: {d}");
        }
    }
}

/// The table pipeline on the ground-truth tables: layout finds the regions, the table model the
/// cells, the text layer the characters. Prints, per truth table, the best matching region.
/// CHINGUI_LAYOUT_DIR=<pp-doclayoutv3 folder> CHINGUI_TABLES_DIR=<slanet_plus folder>
/// CHINGUI_TABLE_IDS=nature-t1,sci-t1 (optional, default every table that is not a scan)
#[test]
#[ignore]
fn table_grids() {
    use super::layout::Layout;
    use super::tables::Tables;
    let dir = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
    let layout = Layout::load(Path::new(&std::env::var("CHINGUI_LAYOUT_DIR").expect("set CHINGUI_LAYOUT_DIR"))).unwrap();
    let tables = Tables::load(Path::new(&std::env::var("CHINGUI_TABLES_DIR").expect("set CHINGUI_TABLES_DIR"))).unwrap();
    let only: Option<Vec<String>> = std::env::var("CHINGUI_TABLE_IDS").ok().map(|s| s.split(',').map(str::to_string).collect());
    let reader = PdfReader::new().expect("libpdfium");
    let truth: Value = serde_json::from_slice(&std::fs::read(Path::new(&dir).join("expected-tables.json")).expect("expected-tables.json")).unwrap();
    for t in truth["tables"].as_array().unwrap() {
        let id = t["id"].as_str().unwrap();
        let scan = t["kind"].as_str().unwrap_or("").starts_with("scan");
        if only.as_ref().map_or(scan, |o| !o.iter().any(|x| x == id)) {
            continue;
        }
        let page = t["page"].as_u64().unwrap() as u32;
        let path = Path::new(&dir).join(t["file"].as_str().unwrap());
        let want = grid(&t["rows"]);
        let started = std::time::Instant::now();
        let found = reader.detect_layout(&path, &[page], &layout, &|| false, &mut |_, _| {}).unwrap();
        let regions: Vec<(u32, [f32; 4])> = found.iter().flat_map(|p| p.regions.iter().filter(|r| r.label == "table").map(move |r| (p.page, r.rect))).collect();
        let reads = reader.read_tables(&path, &regions, &tables, &|| false, &mut |_, _| {}).unwrap();
        let secs = started.elapsed().as_secs_f32();
        println!("== {id} (checked: {}) {} table regions on p{page}, {secs:.1}s", t["checked"].as_bool().unwrap_or(false), reads.len());
        let mut best: Option<(usize, GridScore, String)> = None;
        for (i, r) in reads.iter().enumerate() {
            match &r.filled {
                Err(e) => println!("   region {i}: failed: {e}"),
                Ok(f) => {
                    let rows = f.cells.iter().map(|c| c.row + 1).max().unwrap_or(0);
                    let cols = f.cells.iter().map(|c| c.col + 1).max().unwrap_or(0);
                    let mut got = vec![vec![String::new(); cols]; rows];
                    for c in &f.cells {
                        got[c.row][c.col] = c.text.clone();
                    }
                    let s = score_grid(&got, &want);
                    println!(
                        "   region {i}: {rows}x{cols} (truth {}x{}), chars {}/{}, numbers_ok {}, sound {}, cells exact {}/{}, numeric exact {}/{}",
                        want.len(), want.first().map_or(0, Vec::len), f.assigned, f.glyphs, f.numbers_ok, f.sound(), s.cells_exact, s.cells, s.numeric_exact, s.numeric_cells
                    );
                    if !f.numbers_ok {
                        println!("      numbers only in the region: {:?}, only in the cells: {:?}", f.number_diff.0, f.number_diff.1);
                    }
                    let line = got.iter().map(|r| r.join(" | ")).collect::<Vec<_>>().join("\n      ");
                    if best.as_ref().is_none_or(|b| s.numeric_exact > b.1.numeric_exact) {
                        best = Some((i, s, line));
                    }
                }
            }
        }
        if let Some((i, _, text)) = best {
            if std::env::var("CHINGUI_TABLE_TEXT").is_ok() {
                println!("   best region {i}:\n      {text}");
            }
            if let Ok(f) = &reads[i].filled {
                let mut got = vec![vec![String::new(); want.first().map_or(0, Vec::len)]; want.len()];
                let mut ver = vec![vec![None; want.first().map_or(0, Vec::len)]; want.len()];
                for c in &f.cells {
                    if let Some(slot) = got.get_mut(c.row).and_then(|r| r.get_mut(c.col)) {
                        *slot = c.text.clone();
                        ver[c.row][c.col] = c.verified;
                    }
                }
                let (mut wrong, mut wrong_flagged) = (0, 0);
                for (r, row) in want.iter().enumerate() {
                    for (c, w) in row.iter().enumerate() {
                        if norm(&got[r][c]) != norm(w) {
                            wrong += 1;
                            wrong_flagged += (ver[r][c] == Some(false)) as usize;
                            println!("   differs r{r} c{c}: want {:?} got {:?}, second reader: {:?}", norm(w), norm(&got[r][c]), ver[r][c]);
                        }
                    }
                }
                let flagged = f.cells.iter().filter(|c| c.verified == Some(false)).count();
                let checked = f.cells.iter().filter(|c| c.verified.is_some()).count();
                println!("   second reader: {checked} cells compared, {flagged} unverified; {wrong} cells differ from the truth, {wrong_flagged} of them flagged");
            }
        }
    }
}

/// Why tables came out `unsure` in a real library: re-reads every unsure table of a copy of the
/// app database and prints the text no cell took and the numbers that differ.
/// CHINGUI_DB=<copy of chingui.sqlite> CHINGUI_LIBRARY=<app library folder> CHINGUI_LAYOUT_DIR=.. CHINGUI_TABLES_DIR=..
#[test]
#[ignore]
fn unsure_tables() {
    use super::tables::Tables;
    let db = rusqlite::Connection::open(std::env::var("CHINGUI_DB").expect("set CHINGUI_DB")).unwrap();
    let lib = std::env::var("CHINGUI_LIBRARY").expect("set CHINGUI_LIBRARY");
    let tables = Tables::load(Path::new(&std::env::var("CHINGUI_TABLES_DIR").expect("set CHINGUI_TABLES_DIR"))).unwrap();
    let reader = PdfReader::new().expect("libpdfium");
    let mut stmt = db
        .prepare(
            "SELECT d.original_name, d.content_hash, t.page_no, t.ordinal, t.left, t.bottom, t.right, t.top
               FROM tables t JOIN documents d ON d.id = t.document_id
              WHERE t.status = 'unsure' AND t.note NOT LIKE '0 of 0%' ORDER BY d.original_name, t.page_no, t.ordinal",
        )
        .unwrap();
    let rows: Vec<(String, String, u32, u32, [f32; 4])> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?])))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let only = std::env::var("CHINGUI_ONLY").ok();
    for (name, hash, page, ordinal, rect) in rows {
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let path = Path::new(&lib).join(&hash[..2]).join(format!("{hash}.pdf"));
        let reads = reader.read_tables(&path, &[(page, rect)], &tables, &|| false, &mut |_, _| {}).unwrap();
        match &reads[0].filled {
            Ok(f) => println!(
                "== {} p{page} #{ordinal}: {} {}+{}+{}/{} chars, numbers: missing {:?} extra {:?}\n   left out: {}",
                name.chars().take(30).collect::<String>(),
                if f.sound() && !super::tables::has_duplicate_positions(&f.cells) { "NOW-OK" } else { "still unsure" },
                f.assigned,
                f.placed,
                f.noted,
                f.glyphs,
                f.number_diff.0,
                f.number_diff.1,
                f.left_out.chars().take(160).collect::<String>().replace('\n', " | ")
            ),
            Err(e) => println!("== {name} p{page} #{ordinal}: failed {e}"),
        }
    }
}
