//! Checks the `[S<n>]` handles in a finished reply against the passages the app handed out.
//!
//! - A handle that matches no source is removed from the text. The model can't make one up.
//! - A quoted span (four words or more) must appear in a cited passage, or the citation is marked
//!   as a quote mismatch.
//! - A claim that shares almost no words with the passage it cites is marked weak. This is a
//!   rough word-overlap test: it flags, it never removes, and it can't judge meaning. It also can't
//!   compare across languages (a Norwegian answer citing an English paper), so it skips those.

use std::collections::{HashMap, HashSet};

use crate::domain::library::fold;
use crate::domain::{Citation, CitationStatus};
use crate::persistence::library::COMMON;
use crate::persistence::sources::Source;

const SNIPPET_CHARS: usize = 400;
const MIN_QUOTE_WORDS: usize = 4;
/// Claims with fewer content words than this are too short to judge.
const MIN_CLAIM_WORDS: usize = 4;
/// Share of the claim's content words that must appear in the passage.
const WEAK_BELOW: f32 = 0.2;

pub struct Checked {
    /// The reply with unknown handles removed and grouped handles written as `[S1][S2]`.
    pub text: String,
    /// One entry per cited handle, in order of first use.
    pub citations: Vec<Citation>,
    /// How many handles were removed because no source matches them.
    pub removed: usize,
}

struct Token {
    start: usize,
    end: usize,
    handles: Vec<u32>,
}

pub fn check(text: &str, sources: &HashMap<u32, Source>) -> Checked {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut removed = 0;
    let mut order: Vec<u32> = Vec::new();
    let mut found: HashMap<u32, Citation> = HashMap::new();

    for t in find_tokens(text) {
        out.push_str(&text[last..t.start]);
        last = t.end;
        let valid: Vec<u32> = t.handles.iter().copied().filter(|h| sources.contains_key(h)).collect();
        removed += t.handles.len() - valid.len();
        if valid.is_empty() {
            if out.ends_with(' ') {
                out.pop();
            }
            continue;
        }
        for h in &valid {
            out.push_str(&format!("[S{h}]"));
        }

        let claim = claim_before(text, t.start);
        let quotes: Vec<String> = quoted_spans(&claim)
            .iter()
            .map(|q| norm(q))
            .filter(|q| q.split(' ').count() >= MIN_QUOTE_WORDS)
            .collect();
        let passages: Vec<(u32, String)> =
            valid.iter().map(|h| (*h, format!(" {} ", norm(&sources[h].text)))).collect();
        let in_passage = |q: &str, p: &str| p.contains(&format!(" {q} "));
        let all_quotes_found = quotes.iter().all(|q| passages.iter().any(|(_, p)| in_passage(q, p)));
        let claim_stems = stems(&claim);

        for (h, passage) in &passages {
            let (status, quote_verified) = if !quotes.is_empty() && !all_quotes_found {
                (CitationStatus::QuoteMismatch, Some(false))
            } else {
                let quoted_here = quotes.iter().any(|q| in_passage(q, passage));
                let supported = claim_stems.len() < MIN_CLAIM_WORDS || language(&claim) != language(&sources[h].text) || {
                    let theirs = stems(passage);
                    claim_stems.intersection(&theirs).count() as f32 / claim_stems.len() as f32 >= WEAK_BELOW
                };
                let status = if supported { CitationStatus::Ok } else { CitationStatus::Weak };
                (status, quoted_here.then_some(true))
            };
            let src = &sources[h];
            let entry = found.entry(*h).or_insert_with(|| {
                order.push(*h);
                Citation {
                    handle: *h,
                    status: CitationStatus::Ok,
                    quote_verified: None,
                    passage_id: src.passage_id.clone(),
                    document_id: src.document_id.clone(),
                    title: src.title.clone(),
                    page: src.page,
                    snippet: src.text.chars().take(SNIPPET_CHARS).collect(),
                }
            });
            // The same handle can be cited more than once: keep the worst result.
            if severity(status) > severity(entry.status) {
                entry.status = status;
            }
            entry.quote_verified = match (entry.quote_verified, quote_verified) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), _) | (_, Some(true)) => Some(true),
                _ => None,
            };
        }
    }
    out.push_str(&text[last..]);
    Checked { text: out, citations: order.into_iter().filter_map(|h| found.remove(&h)).collect(), removed }
}

fn severity(s: CitationStatus) -> u8 {
    match s {
        CitationStatus::Ok => 0,
        CitationStatus::Weak => 1,
        CitationStatus::QuoteMismatch => 2,
    }
}

/// `[S3]`, `[S3, S5]` and `[S3; S5]`. Anything else in brackets is left alone.
fn find_tokens(text: &str) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut from = 0;
    while let Some(off) = text[from..].find('[') {
        let start = from + off;
        let Some(close) = text[start + 1..].find(']') else { break };
        let inner = &text[start + 1..start + 1 + close];
        match (inner.len() <= 40).then(|| parse_group(inner)).flatten() {
            Some(handles) => {
                let end = start + close + 2;
                // `[S4][S2]` is one citation of two sources, like `[S4, S2]`.
                match out.last_mut() {
                    Some(prev) if text[prev.end..start].trim().is_empty() => {
                        prev.end = end;
                        prev.handles.extend(handles);
                    }
                    _ => out.push(Token { start, end, handles }),
                }
                from = end;
            }
            None => from = start + 1,
        }
    }
    out
}

fn parse_group(inner: &str) -> Option<Vec<u32>> {
    inner
        .split([',', ';'])
        .map(|p| p.trim().strip_prefix(['S', 's'])?.parse::<u32>().ok())
        .collect()
}

fn strip_tokens(s: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for t in find_tokens(s) {
        out.push_str(&s[last..t.start]);
        last = t.end;
    }
    out.push_str(&s[last..]);
    out
}

/// The sentence (or list line) a citation ends. A citation placed after the full stop belongs to
/// the sentence before it.
fn claim_before(text: &str, start: usize) -> String {
    let head = text[..start].trim_end().trim_end_matches(['.', '!', '?']);
    let chars: Vec<(usize, char)> = head.char_indices().collect();
    let mut begin = 0;
    for (k, (idx, c)) in chars.iter().enumerate().rev() {
        let sentence_end = matches!(c, '.' | '!' | '?') && chars.get(k + 1).is_some_and(|(_, n)| n.is_whitespace());
        if *c == '\n' || sentence_end {
            begin = idx + c.len_utf8();
            break;
        }
    }
    strip_tokens(&head[begin..])
}

/// Text between straight or curly double quotes.
fn quoted_spans(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for c in s.chars() {
        if cur.is_none() {
            if c == '"' || c == '“' {
                cur = Some(String::new());
            }
        } else if c == '"' || c == '”' {
            out.extend(cur.take());
        } else if let Some(buf) = cur.as_mut() {
            buf.push(c);
        }
    }
    out
}

/// Lowercase letters and digits separated by single spaces, so a quote still matches when the PDF
/// text differs in punctuation, line breaks or hyphenation.
fn norm(s: &str) -> String {
    let s = s.replace("-\r\n", "").replace("-\n", "").replace('\u{ad}', "");
    let mut out = String::with_capacity(s.len());
    for c in fold(&s).chars() {
        if matches!(c, '-' | '\'' | '’') {
            continue;
        }
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Content words cut to five letters, so "limitation" and "limits" meet.
#[derive(PartialEq)]
enum Lang {
    Norwegian,
    English,
    Unknown,
}

const NORWEGIAN_WORDS: &[&str] = &[
    "og", "ikke", "som", "det", "til", "med", "av", "pa", "er", "for", "har", "kan", "den", "de", "om", "fra", "ved", "men", "mot", "blir",
    "eller", "ogsa", "denne", "disse", "hvor", "ble", "ved", "sa", "seg",
];
const ENGLISH_WORDS: &[&str] = &[
    "the", "and", "of", "to", "is", "in", "that", "with", "as", "are", "was", "by", "on", "this", "which", "from", "be", "or", "it", "were", "have", "has",
];

/// Norwegian or English by which common function words the text uses more. Too few of either is unknown.
fn language(s: &str) -> Lang {
    let folded = fold(s);
    let (mut no, mut en) = (0, 0);
    for w in folded.split(|c: char| !c.is_alphanumeric()) {
        no += NORWEGIAN_WORDS.contains(&w) as u32;
        en += ENGLISH_WORDS.contains(&w) as u32;
    }
    match (no, en) {
        (n, e) if n >= 3 && n > e => Lang::Norwegian,
        (n, e) if e >= 3 && e > n => Lang::English,
        _ => Lang::Unknown,
    }
}

fn stems(s: &str) -> HashSet<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4 && !COMMON.contains(w))
        .map(|w| w.chars().take(5).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(texts: &[&str]) -> HashMap<u32, Source> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let h = i as u32 + 1;
                (
                    h,
                    Source {
                        handle: h,
                        passage_id: format!("p{h}"),
                        document_id: "d".into(),
                        title: format!("Paper {h}"),
                        page: 3,
                        text: (*t).into(),
                    },
                )
            })
            .collect()
    }

    const IRON: &str = "Iron limitation controls plankton growth in open ocean waters.";

    #[test]
    fn unknown_handles_are_removed_and_counted() {
        let c = check("Iron limits growth [S1]. Nothing here [S9]. Also [S1, S9] and [S8].", &sources(&[IRON]));
        assert_eq!(c.text, "Iron limits growth [S1]. Nothing here. Also [S1] and.");
        assert_eq!(c.removed, 3);
        assert_eq!(c.citations.len(), 1, "one entry per handle");
        assert_eq!((c.citations[0].handle, c.citations[0].title.as_str(), c.citations[0].page), (1, "Paper 1", 3));
    }

    #[test]
    fn grouped_handles_are_written_as_separate_tokens() {
        let c = check("Iron limits plankton growth [S1; S2].", &sources(&[IRON, IRON]));
        assert_eq!(c.text, "Iron limits plankton growth [S1][S2].");
        assert_eq!(c.removed, 0);
    }

    #[test]
    fn adjacent_tokens_are_one_group() {
        let c = check("Iron limits plankton growth [S1] [S2].", &sources(&[IRON, IRON]));
        assert_eq!(c.text, "Iron limits plankton growth [S1][S2].");
        let c = check("Nothing [S9][S9] here.", &sources(&[IRON]));
        assert_eq!((c.text.as_str(), c.removed), ("Nothing here.", 2));
    }

    #[test]
    fn text_without_handles_or_with_other_brackets_is_untouched() {
        let t = "See [1] and [the figure] and [S] and [Sx] and [S1 S2].";
        let c = check(t, &sources(&[IRON]));
        assert_eq!(c.text, t);
        assert!(c.citations.is_empty());
        assert_eq!(c.removed, 0);
    }

    #[test]
    fn a_quote_in_the_passage_is_verified_across_hyphenation_and_case() {
        let src = sources(&["The result was clear: geo-\nengineering is NOT a substitute for cutting emissions."]);
        let c = check("The authors say \"geoengineering is not a substitute for cutting emissions\" [S1].", &src);
        assert_eq!(c.citations[0].status, CitationStatus::Ok);
        assert_eq!(c.citations[0].quote_verified, Some(true));
    }

    #[test]
    fn a_quote_that_is_not_in_the_source_is_flagged() {
        let c = check("They wrote “iron is the only thing that limits growth anywhere” [S1].", &sources(&[IRON]));
        assert_eq!(c.citations[0].status, CitationStatus::QuoteMismatch);
        assert_eq!(c.citations[0].quote_verified, Some(false));
    }

    #[test]
    fn short_quotes_are_not_checked_and_one_source_can_carry_the_quote() {
        let c = check("It is \"iron limited\" [S1].", &sources(&[IRON]));
        assert_eq!(c.citations[0].quote_verified, None);
        let c = check("They say \"controls plankton growth in open ocean\" [S1][S2].", &sources(&[IRON, "Unrelated text about plankton growth and ocean waters."]));
        assert_eq!(c.citations[0].quote_verified, Some(true));
        assert_eq!(c.citations[1].status, CitationStatus::Ok, "the other source isn't blamed for a quote it doesn't hold");
        assert_eq!(c.citations[1].quote_verified, None);
    }

    #[test]
    fn a_claim_with_almost_no_overlap_is_flagged_weak_but_kept() {
        let claim = "Chinook salmon died during a Heterosigma bloom near Wellington harbour";
        let c = check(&format!("{claim} [S1]."), &sources(&[IRON]));
        assert_eq!(c.citations[0].status, CitationStatus::Weak);
        assert!(c.text.contains("[S1]"), "flagged, not removed");
        let c = check("Iron limitation controls plankton growth in open ocean waters [S1].", &sources(&[IRON]));
        assert_eq!(c.citations[0].status, CitationStatus::Ok);
    }

    #[test]
    fn a_norwegian_claim_on_an_english_passage_is_not_flagged_weak() {
        let passage = "Of the roughly 5000 species of marine phytoplankton, about 300 can form blooms that discolour the water, and some 80 species are able to produce potent toxins.";
        let claim = "Omtrent 300 arter kan danne oppblomstringer som misfarger vannet, og rundt 80 arter kan produsere kraftige toksiner";
        let c = check(&format!("{claim} [S1]."), &sources(&[passage]));
        assert_eq!(c.citations[0].status, CitationStatus::Ok);
    }

    #[test]
    fn a_norwegian_claim_on_a_norwegian_passage_is_still_checked() {
        let passage = "Det er ikke kjent hvor mye laks som dør av algeoppblomstring i oppdrettsanlegg langs kysten, men det er en stor risiko for fisk i merd.";
        let c = check("Blåskjell og kråkebolle er viktige arter som lever på havbunnen og som ikke har noe med dette å gjøre [S1].", &sources(&[passage]));
        assert_eq!(c.citations[0].status, CitationStatus::Weak);
    }

    #[test]
    fn short_claims_are_not_judged() {
        let c = check("Salmon died [S1].", &sources(&[IRON]));
        assert_eq!(c.citations[0].status, CitationStatus::Ok);
    }

    #[test]
    fn a_citation_after_the_full_stop_belongs_to_the_sentence_before() {
        let src = sources(&[IRON]);
        let c = check("Earlier text is unrelated to anything. Iron limitation controls plankton growth in waters. [S1]", &src);
        assert_eq!(c.citations[0].status, CitationStatus::Ok, "judged on the sentence before the stop");
        let c = check("- Iron limitation controls plankton growth in waters [S1]\n- Salmon farms cage many fish in fjords [S1]", &src);
        assert_eq!(c.citations[0].status, CitationStatus::Weak, "a list line is its own claim and the worst result wins");
    }

    #[test]
    fn norwegian_letters_fold_when_comparing() {
        let src = sources(&["Næringssalter begrenser veksten av planteplankton i åpent hav."]);
        let c = check("Forfatterne skriver \"næringssalter begrenser veksten av planteplankton\" [S1].", &src);
        assert_eq!(c.citations[0].quote_verified, Some(true));
    }
}
