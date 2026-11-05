//! Turns extracted blocks (PDF lines or DOCX paragraphs) into passages of a useful size.

use serde_json::json;

use crate::persistence::library::NewPassage;

/// A passage is closed once it reaches this many characters (about 300 tokens).
const TARGET: usize = 1200;
/// No passage is longer than this. Longer blocks are cut at sentence ends.
const HARD_MAX: usize = 1800;
/// A passage shorter than this keeps going across a page break instead of ending at it.
const MIN_BEFORE_PAGE_END: usize = 300;
/// Fragments with fewer letters or digits than this (page numbers, stray marks) are dropped.
const MIN_ALNUM: usize = 8;

/// How a block attaches to the previous one inside a passage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    Space,
    /// A word the layout broke across lines ("geo" + "engineering"): no space.
    Tight,
    Newline,
}

#[derive(Debug, Clone)]
pub struct Block {
    /// PDF page, 1-based. 0 for DOCX.
    pub page: u32,
    pub text: String,
    /// [left, bottom, right, top] in PDF user space. None for DOCX.
    pub rect: Option<[f32; 4]>,
    /// Largest character size in the block, in points. 0 if unknown.
    pub font_size: f32,
    pub join: Join,
}

struct Open {
    text: String,
    page_start: u32,
    page_end: u32,
    /// (page, rects) in reading order
    boxes: Vec<(u32, Vec<[f32; 4]>)>,
}

impl Open {
    fn new(page: u32) -> Self {
        Open { text: String::new(), page_start: page, page_end: page, boxes: vec![] }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn push(&mut self, b: &Block, text: &str, first_piece: bool) {
        if !self.text.is_empty() {
            let join = if first_piece { b.join } else { Join::Space };
            match join {
                Join::Space => self.text.push(' '),
                Join::Newline => self.text.push('\n'),
                Join::Tight => {}
            }
        }
        self.text.push_str(text);
        self.page_end = b.page;
        if let Some(r) = b.rect {
            match self.boxes.last_mut() {
                Some((p, rects)) if *p == b.page => rects.push(r),
                _ => self.boxes.push((b.page, vec![r])),
            }
        }
    }

    fn finish(self) -> Option<NewPassage> {
        let text = self.text.trim().to_string();
        if text.chars().filter(|c| c.is_alphanumeric()).count() < MIN_ALNUM {
            return None;
        }
        let boxes = (!self.boxes.is_empty()).then(|| {
            json!(self.boxes.iter().map(|(page, rects)| json!({ "page": page, "rects": rects })).collect::<Vec<_>>())
                .to_string()
        });
        Some(NewPassage { page_start: self.page_start, page_end: self.page_end, text, boxes })
    }
}

/// Cuts text longer than `max` characters at sentence ends, else at spaces, else anywhere.
fn split_long(text: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while chars.len() - start > max {
        let window = &chars[start..start + max];
        let cut = window
            .iter()
            .rposition(|c| matches!(c, '.' | '!' | '?'))
            .filter(|&i| i > max / 2)
            .or_else(|| window.iter().rposition(|c| c.is_whitespace()).filter(|&i| i > max / 2))
            .map_or(max, |i| i + 1);
        out.push(chars[start..start + cut].iter().collect::<String>().trim().to_string());
        start += cut;
    }
    out.push(chars[start..].iter().collect::<String>().trim().to_string());
    out.retain(|s| !s.is_empty());
    out
}

pub fn segment(blocks: &[Block]) -> Vec<NewPassage> {
    let mut out = Vec::new();
    let mut cur: Option<Open> = None;

    for b in blocks {
        let text = b.text.trim();
        if text.is_empty() {
            continue;
        }
        // A page break ends the passage unless it is still very short.
        if let Some(o) = &cur
            && b.page != o.page_end
            && o.len() >= MIN_BEFORE_PAGE_END
        {
            out.extend(cur.take().and_then(Open::finish));
        }
        for (i, piece) in split_long(text, HARD_MAX).iter().enumerate() {
            let o = cur.get_or_insert_with(|| Open::new(b.page));
            if o.len() + piece.chars().count() + 1 > HARD_MAX && o.len() > 0 {
                out.extend(cur.take().and_then(Open::finish));
            }
            let o = cur.get_or_insert_with(|| Open::new(b.page));
            o.push(b, piece, i == 0);
            if o.len() >= TARGET {
                out.extend(cur.take().and_then(Open::finish));
            }
        }
    }
    out.extend(cur.and_then(Open::finish));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(page: u32, text: &str) -> Block {
        Block { page, text: text.into(), rect: Some([10.0, 10.0, 100.0, 20.0]), font_size: 10.0, join: Join::Space }
    }

    fn lines(page: u32, n: usize, width: usize) -> Vec<Block> {
        (0..n).map(|i| line(page, &format!("{}{}", "word ".repeat(width / 5 - 1), i))).collect()
    }

    #[test]
    fn closes_passages_near_target_size() {
        let out = segment(&lines(1, 60, 80));
        assert!(out.len() > 2);
        for p in &out[..out.len() - 1] {
            let n = p.text.chars().count();
            assert!((TARGET..=HARD_MAX).contains(&n), "passage of {n} chars");
        }
    }

    #[test]
    fn page_break_ends_a_long_passage_but_not_a_short_one() {
        let mut blocks = lines(1, 20, 80); // about 1,600 chars, closed on page 1
        blocks.push(line(2, "Short tail on page two, long enough to count as text."));
        let out = segment(&blocks);
        assert_eq!((out[0].page_start, out[0].page_end), (1, 1));
        assert_eq!((out.last().unwrap().page_start, out.last().unwrap().page_end), (2, 2));

        // a short passage at the end of page 1 continues onto page 2
        let blocks = vec![line(1, "A short heading line on page one"), line(2, "and the paragraph that follows it on page two")];
        let out = segment(&blocks);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].page_start, out[0].page_end), (1, 2));
        let boxes: serde_json::Value = serde_json::from_str(out[0].boxes.as_ref().unwrap()).unwrap();
        assert_eq!(boxes.as_array().unwrap().len(), 2, "one box group per page");
        assert_eq!(boxes[1]["page"], 2);
    }

    #[test]
    fn broken_words_are_joined_without_a_space() {
        let mut second = line(1, "engineering schemes are proposed for the open ocean");
        second.join = Join::Tight;
        let out = segment(&[line(1, "proposed geo"), second]);
        assert!(out[0].text.starts_with("proposed geoengineering schemes"), "{}", out[0].text);
    }

    #[test]
    fn drops_page_numbers_and_stray_marks() {
        let out = segment(&[line(1, "12"), line(1, "~~~~n~~~~~,")]);
        assert!(out.is_empty());
    }

    #[test]
    fn long_paragraph_is_cut_at_sentence_ends() {
        let sentence = "This is one fairly ordinary sentence about algae. ";
        let para = sentence.repeat(120); // about 6,000 chars
        let blocks = vec![Block { page: 0, text: para, rect: None, font_size: 0.0, join: Join::Newline }];
        let out = segment(&blocks);
        assert!(out.len() >= 3);
        for p in &out {
            assert!(p.text.chars().count() <= HARD_MAX);
            assert!(p.text.ends_with('.'), "cut mid-sentence: {:?}", &p.text[p.text.len().saturating_sub(20)..]);
            assert!(p.boxes.is_none());
        }
    }

    #[test]
    fn handles_multibyte_text() {
        let out = segment(&[line(1, &"Økosystemet ved Ærø og Åre er sårbart. ".repeat(60))]);
        assert!(out.iter().all(|p| p.text.chars().count() <= HARD_MAX));
        assert!(out.len() >= 2);
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(segment(&[]).is_empty());
        assert!(segment(&[line(1, "   ")]).is_empty());
    }
}
