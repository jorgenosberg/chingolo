//! DOCX text: paragraphs from word/document.xml, title and author from docProps/core.xml.
//! DOCX has no pages, so passages are located by their order only.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;
use zip::ZipArchive;

use super::meta;
use super::segment::{Block, Join};
use super::Extraction;
use crate::error::{AppError, Result};
use crate::persistence::library::Extracted;

/// A real document.xml is a few MB. Anything near this is a zip bomb or garbage.
const MAX_XML_BYTES: u64 = 200 * 1024 * 1024;

fn read_entry(zip: &mut ZipArchive<File>, name: &str) -> Result<Option<String>> {
    let Ok(entry) = zip.by_name(name) else { return Ok(None) };
    let mut s = String::new();
    entry
        .take(MAX_XML_BYTES + 1)
        .read_to_string(&mut s)
        .map_err(|_| AppError::Invalid("This Word file is damaged.".into()))?;
    if s.len() as u64 > MAX_XML_BYTES {
        return Err(AppError::Invalid("This Word file is too large to read.".into()));
    }
    Ok(Some(s))
}

fn bad_xml<E>(_: E) -> AppError {
    AppError::Invalid("This Word file is damaged.".into())
}

/// Appends the text of an entity reference such as `&amp;` or `&#233;`.
fn push_ref(out: &mut String, r: &quick_xml::events::BytesRef) {
    match r.as_ref() {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        _ => {
            if let Ok(Some(c)) = r.resolve_char_ref() {
                out.push(c);
            }
        }
    }
}

struct Paragraph {
    text: String,
    is_title: bool,
}

fn paragraphs(xml: &str) -> Result<Vec<Paragraph>> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let (mut cur, mut in_t, mut is_title) = (String::new(), false, false);
    loop {
        match reader.read_event().map_err(bad_xml)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "t" => in_t = true,
                "p" => {
                    cur.clear();
                    is_title = false;
                }
                "pStyle" => is_title |= is_title_style(&e),
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" | "br" | "cr" => cur.push(' '),
                "pStyle" => is_title |= is_title_style(&e),
                _ => {}
            },
            Event::Text(t) if in_t => cur.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_t => push_ref(&mut cur, &r),
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "p" => {
                    let text = cur.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() {
                        out.push(Paragraph { text, is_title });
                    }
                    cur.clear();
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn is_title_style(e: &quick_xml::events::BytesStart) -> bool {
    e.attributes()
        .flatten()
        .any(|a| a.key.local_name().as_ref() == "val" && a.value.as_ref().eq_ignore_ascii_case("Title"))
}

/// Text of the first element with this local name (core.xml has one of each).
fn first_element_text(xml: &str, name: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let (mut inside, mut out) = (false, String::new());
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) if e.local_name().as_ref() == name => inside = true,
            Event::Text(t) if inside => out.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if inside => push_ref(&mut out, &r),
            Event::End(e) if inside && e.local_name().as_ref() == name => break,
            Event::Eof => break,
            _ => {}
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

pub fn extract(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<Extraction> {
    let file = File::open(path).map_err(|e| AppError::Invalid(format!("Couldn't open this file: {e}")))?;
    let mut zip = ZipArchive::new(file).map_err(|_| AppError::Invalid("This file couldn't be read as a Word document.".into()))?;
    let body = read_entry(&mut zip, "word/document.xml")?
        .ok_or_else(|| AppError::Invalid("This file couldn't be read as a Word document.".into()))?;
    if cancelled() {
        return Err(AppError::Cancelled);
    }
    let paras = paragraphs(&body)?;
    let core = read_entry(&mut zip, "docProps/core.xml")?.unwrap_or_default();

    let title = first_element_text(&core, "title")
        .and_then(|t| meta::clean_title(&t))
        .or_else(|| paras.iter().find(|p| p.is_title).and_then(|p| meta::clean_title(&p.text)));
    let authors = first_element_text(&core, "creator").map(|a| meta::split_authors(&a)).unwrap_or_default();
    let head: String = paras.iter().take(30).map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ");

    let blocks = paras
        .into_iter()
        .map(|p| Block { page: 0, text: p.text, rect: None, font_size: 0.0, join: Join::Newline })
        .collect();
    Ok(Extraction {
        pages: vec![],
        blocks,
        meta: Extracted {
            title,
            authors,
            doi: meta::find_doi(&head, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            ..Default::default()
        },
        unreadable_pages: 0,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn make_docx(dir: &Path, document: &str, core: Option<&str>) -> std::path::PathBuf {
        let path = dir.join("t.docx");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document.as_bytes()).unwrap();
        if let Some(core) = core {
            zip.start_file("docProps/core.xml", opts).unwrap();
            zip.write_all(core.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    #[test]
    fn reads_paragraphs_entities_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let doc = format!(
            r#"<w:document {W}><w:body>
              <w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Algae and Ærø</w:t></w:r></w:p>
              <w:p><w:r><w:t xml:space="preserve">Fish &amp; </w:t></w:r><w:r><w:t>chips</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>&#233;t&#xE9;</w:t></w:r></w:p>
              <w:p></w:p>
              <w:p><w:r><w:t>See doi:10.1016/j.hal.2022.102310.</w:t></w:r></w:p>
            </w:body></w:document>"#
        );
        let core = r#"<cp:coreProperties xmlns:cp="x" xmlns:dc="y"><dc:title>Microsoft Word - draft.doc</dc:title><dc:creator>A. Author; B. Other</dc:creator></cp:coreProperties>"#;
        let path = make_docx(dir.path(), &doc, Some(core));
        let out = extract(&path, &|| false).unwrap();
        let texts: Vec<_> = out.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["Algae and Ærø", "Fish & chips été", "See doi:10.1016/j.hal.2022.102310."]);
        assert!(out.pages.is_empty() && out.blocks.iter().all(|b| b.page == 0 && b.rect.is_none()));
        // the property title is a leftover file name, so the Title-styled paragraph wins
        assert_eq!(out.meta.title.as_deref(), Some("Algae and Ærø"));
        assert_eq!(out.meta.authors, ["A. Author", "B. Other"]);
        assert_eq!(out.meta.doi.as_deref(), Some("10.1016/j.hal.2022.102310"));
    }

    #[test]
    fn empty_document_has_no_blocks_and_junk_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = make_docx(dir.path(), &format!(r#"<w:document {W}><w:body/></w:document>"#), None);
        assert!(extract(&path, &|| false).unwrap().blocks.is_empty());

        let junk = dir.path().join("junk.docx");
        std::fs::write(&junk, b"not a zip").unwrap();
        assert!(extract(&junk, &|| false).unwrap_err().to_string().contains("Word document"));

        let no_body = dir.path().join("nobody.docx");
        let mut zip = zip::ZipWriter::new(File::create(&no_body).unwrap());
        zip.start_file("other.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.finish().unwrap();
        assert!(extract(&no_body, &|| false).is_err());
        assert!(matches!(extract(&path, &|| true), Err(AppError::Cancelled)));
    }
}
