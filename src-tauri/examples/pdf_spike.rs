//! Spike: extract text and character boxes from PDFs with pdfium.
//! Usage: cargo run --example pdf_spike -- <out_dir> <pdf>...
//! Writes <out_dir>/<file stem>.json per PDF and prints a one-line summary each.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let out_dir = PathBuf::from(args.next().expect("out dir"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let lib = Pdfium::pdfium_platform_library_name_at_path("vendor/pdfium/");
    let pdfium = Pdfium::new(Pdfium::bind_to_library(lib).expect("bind pdfium"));

    for path in args {
        let path = PathBuf::from(path);
        let started = std::time::Instant::now();
        match extract(&pdfium, &path) {
            Ok((doc, summary)) => {
                let stem = path.file_stem().unwrap().to_string_lossy().to_string();
                let out = out_dir.join(format!("{stem}.json"));
                std::fs::write(&out, serde_json::to_vec(&doc).unwrap()).unwrap();
                println!(
                    "{summary} | {:.1}s | {}",
                    started.elapsed().as_secs_f32(),
                    path.file_name().unwrap().to_string_lossy()
                );
            }
            Err(e) => println!("ERROR {e} | {}", path.display()),
        }
    }
}

fn r4(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn extract(pdfium: &Pdfium, path: &Path) -> Result<(serde_json::Value, String), PdfiumError> {
    let doc = pdfium.load_pdf_from_file(path, None)?;
    let mut pages = Vec::new();
    let (mut total_chars, mut empty_pages, mut low_pages) = (0usize, 0usize, 0usize);

    for (i, page) in doc.pages().iter().enumerate() {
        let text = page.text()?;
        let mut lines: Vec<serde_json::Value> = Vec::new();
        let (mut cur, mut rect): (String, Option<[f32; 4]>) = (String::new(), None);
        let mut chars_on_page = 0usize;
        let mut odd = 0usize;

        let mut flush = |cur: &mut String,
                         rect: &mut Option<[f32; 4]>,
                         lines: &mut Vec<serde_json::Value>| {
            if let (false, Some(r)) = (cur.trim().is_empty(), *rect) {
                lines.push(json!({ "text": cur.trim_end(), "rect": [r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])] }));
            }
            cur.clear();
            *rect = None;
        };

        for ch in text.chars().iter() {
            let Some(c) = ch.unicode_char() else { continue };
            if c == '\r' || c == '\n' {
                flush(&mut cur, &mut rect, &mut lines);
                continue;
            }
            if c.is_control() {
                continue;
            }
            if !c.is_whitespace()
                && let (Some(r), Ok(b)) = (rect, ch.loose_bounds())
            {
                // pdfium joins hyphenated breaks ("geo-" + "engineering") into one run with no
                // newline. Start a new line when the char's centre leaves the current line's band.
                let (cx, cy) = (
                    (b.left().value + b.right().value) / 2.0,
                    (b.bottom().value + b.top().value) / 2.0,
                );
                let angle = ch.angle_degrees().unwrap_or(0.0).abs() % 180.0;
                let vertical = (angle - 90.0).abs() < 20.0;
                let same_line = if vertical {
                    cx >= r[0] && cx <= r[2]
                } else {
                    cy >= r[1] && cy <= r[3]
                };
                if !same_line {
                    flush(&mut cur, &mut rect, &mut lines);
                }
            }
            cur.push(c);
            if !c.is_whitespace() {
                chars_on_page += 1;
                if c == '\u{FFFD}' || ('\u{E000}'..='\u{F8FF}').contains(&c) {
                    odd += 1;
                }
                if let Ok(b) = ch.loose_bounds() {
                    let (l, bo, ri, t) = (
                        b.left().value,
                        b.bottom().value,
                        b.right().value,
                        b.top().value,
                    );
                    rect = Some(match rect {
                        None => [l, bo, ri, t],
                        Some(r) => [r[0].min(l), r[1].min(bo), r[2].max(ri), r[3].max(t)],
                    });
                }
            }
        }
        flush(&mut cur, &mut rect, &mut lines);

        total_chars += chars_on_page;
        // Same idea as the planned quality flag: no text, or many private-use/replacement chars.
        let quality = if chars_on_page < 20 {
            empty_pages += 1;
            "none"
        } else if odd * 20 > chars_on_page {
            low_pages += 1;
            "low"
        } else {
            "ok"
        };

        let media = page.boundaries().media().map(|b| b.bounds).ok();
        let crop = page.boundaries().crop().map(|b| b.bounds).ok();
        let bx = |b: Option<PdfRect>| {
            b.map(|b| {
                json!([
                    r4(b.left().value),
                    r4(b.bottom().value),
                    r4(b.right().value),
                    r4(b.top().value)
                ])
            })
        };
        pages.push(json!({
            "page": i + 1,
            "width": r4(page.width().value),
            "height": r4(page.height().value),
            "rotation": page.rotation().map(|r| r.as_degrees()).unwrap_or(0.0),
            "mediaBox": bx(media),
            "cropBox": bx(crop),
            "chars": chars_on_page,
            "quality": quality,
            "lines": lines,
        }));
    }

    let n = pages.len();
    let doc_json = json!({ "file": path.file_name().unwrap().to_string_lossy(), "pages": pages });
    let verdict = if total_chars == 0 { "NO TEXT" } else { "text" };
    Ok((
        doc_json,
        format!("{verdict:7} {n:4}p {total_chars:7} chars, {empty_pages} empty, {low_pages} low"),
    ))
}
