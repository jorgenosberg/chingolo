//! Ignored audit of metadata extraction and page quality on a folder of real papers.
//! Run: CHINGUI_TEST_PAPERS=<folder> cargo test corpus_audit -- --ignored --nocapture
//! With CHINGUI_OCR_DIR=<model folder> the metadata of scanned pages is read through text recognition.
//! The folder must hold `expected-meta.json` (see docs/STATUS.md, "Slice 9 baseline").

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::Value;

use super::ocr::OcrSlot;
use super::pdf::PdfReader;
use crate::persistence::library::Extracted;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Hit,
    Blank,
    Wrong,
    Skipped,
}

fn tokens(s: &str) -> HashSet<String> {
    let mut plain = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    plain
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn same_title(a: &str, b: &str) -> bool {
    let (a, b) = (tokens(a), tokens(b));
    let union = a.union(&b).count();
    union > 0 && a.intersection(&b).count() as f32 / union as f32 >= 0.85
}

fn title_verdict(got: &Option<String>, want: &str) -> Verdict {
    match got {
        None => Verdict::Blank,
        Some(t) if same_title(t, want) => Verdict::Hit,
        Some(_) => Verdict::Wrong,
    }
}

fn authors_verdict(got: &[String], want: &Value) -> Verdict {
    let list: Vec<&str> = want["authors"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if want["authorsUnknown"].as_bool() == Some(true) {
        return Verdict::Skipped;
    }
    match (list.first(), got.first()) {
        (None, None) => Verdict::Hit,
        (None, Some(_)) => Verdict::Wrong,
        (Some(_), None) => Verdict::Blank,
        (Some(s), Some(g)) => {
            if g.to_lowercase().contains(&s.to_lowercase()) { Verdict::Hit } else { Verdict::Wrong }
        }
    }
}

fn year_verdict(got: Option<i32>, want: &Value) -> Verdict {
    let wanted: Vec<i64> = match want {
        Value::Number(n) => n.as_i64().into_iter().collect(),
        Value::Array(a) => a.iter().filter_map(Value::as_i64).collect(),
        _ => vec![],
    };
    match (got, wanted.is_empty()) {
        (None, true) => Verdict::Hit,
        (Some(_), true) => Verdict::Wrong,
        (None, false) => Verdict::Blank,
        (Some(y), false) => if wanted.contains(&(y as i64)) { Verdict::Hit } else { Verdict::Wrong },
    }
}

fn doi_verdict(got: &Option<String>, want: &Value) -> Verdict {
    match (got, want.as_str()) {
        (None, None) => Verdict::Hit,
        (Some(_), None) => Verdict::Wrong,
        (None, Some(_)) => Verdict::Blank,
        (Some(g), Some(w)) => if g.eq_ignore_ascii_case(w) { Verdict::Hit } else { Verdict::Wrong },
    }
}

/// Verdicts for title, authors, year and DOI.
fn compare(m: &Extracted, want: &Value) -> [Verdict; 4] {
    [
        title_verdict(&m.title, want["title"].as_str().unwrap_or("")),
        authors_verdict(&m.authors, want),
        year_verdict(m.year, &want["year"]),
        doi_verdict(&m.doi, &want["doi"]),
    ]
}

#[test]
fn verdicts_distinguish_hit_blank_and_wrong() {
    let want: Value = serde_json::json!({ "title": "Iron <i>enrichment</i> and diatoms", "authors": ["Boyd"], "year": [2003, 2004], "doi": "10.1/x" });
    let got = Extracted { title: Some("iron enrichment and diatoms".into()), authors: vec!["P. W. Boyd".into()], year: Some(2004), doi: Some("10.1/X".into()), ..Default::default() };
    assert_eq!(compare(&got, &want), [Verdict::Hit; 4]);
    let bad = Extracted { title: Some("Abstract of something else entirely".into()), authors: vec!["[Personer]".into()], year: Some(1999), doi: Some("10.1/y".into()), ..Default::default() };
    assert_eq!(compare(&bad, &want), [Verdict::Wrong; 4]);
    assert_eq!(compare(&Extracted::default(), &want), [Verdict::Blank; 4]);
    let none = serde_json::json!({ "title": "T", "authors": [], "year": null, "doi": null });
    assert_eq!(compare(&Extracted::default(), &none)[1..], [Verdict::Hit; 3]);
}

#[test]
#[ignore]
fn corpus_audit() {
    let dir = std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS");
    let Ok(reader) = PdfReader::new() else {
        eprintln!("skipped: libpdfium not found");
        return;
    };
    let expected: Value = serde_json::from_slice(&std::fs::read(Path::new(&dir).join("expected-meta.json")).expect("expected-meta.json")).unwrap();
    let files = expected["files"].as_object().unwrap();
    let mut slot = std::env::var("CHINGUI_OCR_DIR").ok().map(|d| OcrSlot::new(Some(d.into())));

    let mut totals: BTreeMap<&str, BTreeMap<Verdict, u32>> = BTreeMap::new();
    let (mut pages_total, mut pages_none, mut pages_low) = (0usize, 0usize, 0usize);
    println!("{:<44} {:>5} {:>4} {:>4} | title authors year doi", "file", "pages", "none", "low");
    for (name, want) in files {
        let path = Path::new(&dir).join(name);
        let Ok(out) = reader.extract(&path, &|| false, &mut |_, _| {}) else {
            println!("{name}: could not be read");
            continue;
        };
        let none = out.pages.iter().filter(|p| p.quality == "none").count();
        let low = out.pages.iter().filter(|p| p.quality == "low").count();
        pages_total += out.pages.len();
        pages_none += none;
        pages_low += low;
        // the same read a "Refresh details" does: file properties and the first pages
        let meta = match slot.as_mut() {
            Some(slot) => reader.extract_meta(&path, slot).unwrap_or_default(),
            None => reader.extract_meta(&path, &mut OcrSlot::default()).unwrap_or_default(),
        };
        let v = compare(&meta, want);
        for (field, verdict) in ["title", "authors", "year", "doi"].iter().zip(v) {
            *totals.entry(field).or_default().entry(verdict).or_default() += 1;
        }
        let tag = |v: Verdict| match v {
            Verdict::Hit => "ok   ",
            Verdict::Blank => "blank",
            Verdict::Wrong => "WRONG",
            Verdict::Skipped => "skip ",
        };
        println!(
            "{:<44} {:>5} {:>4} {:>4} | {} {}   {} {}",
            name.chars().take(44).collect::<String>(), out.pages.len(), none, low, tag(v[0]), tag(v[1]), tag(v[2]), tag(v[3])
        );
        for (field, verdict, got) in [
            ("title", v[0], meta.title.clone()),
            ("authors", v[1], meta.authors.first().cloned()),
            ("year", v[2], meta.year.map(|y| y.to_string())),
            ("doi", v[3], meta.doi.clone()),
        ] {
            if verdict == Verdict::Wrong {
                println!("      {field}: got {:?}", got.map(|s| s.chars().take(90).collect::<String>()));
            }
        }
    }
    println!("\nfield     hit blank wrong skipped");
    for (field, t) in &totals {
        let n = |v| t.get(&v).copied().unwrap_or(0);
        println!("{field:<9} {:>3} {:>5} {:>5} {:>7}", n(Verdict::Hit), n(Verdict::Blank), n(Verdict::Wrong), n(Verdict::Skipped));
    }
    println!("pages: {pages_total} total, {pages_none} without text, {pages_low} flagged low");
}
