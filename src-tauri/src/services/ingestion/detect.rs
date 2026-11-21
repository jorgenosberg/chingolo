//! Reads title, authors, year, DOI and the rest from the first pages of a PDF. Every rule is
//! strict: a blank is better than a wrong guess, and the user can fill it in (or a lookup can).
//! The rules work on lines with their type size, because that is what a title page is made of.

use std::sync::LazyLock;

use regex::Regex;

use super::meta;
use super::segment::{Block, Join};
use crate::persistence::library::Extracted;

/// Pages searched for details. NINA reports put their citation line on page 4.
const PAGES: u32 = 4;

/// File properties that may hold the title, authors or a DOI.
#[derive(Debug, Default, Clone)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
}

#[derive(Debug, Clone)]
struct Line {
    page: u32,
    size: f32,
    text: String,
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid pattern")
}

fn normalize(s: &str) -> String {
    let s: String = s
        .chars()
        .filter(|&c| c != '\u{ad}' && c != '\u{200b}')
        .map(|c| match c {
            '\u{2011}' | '\u{2010}' => '-',
            '\u{a0}' => ' ',
            c => c,
        })
        .collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The lines of the first pages. A line that continues a word broken across lines is joined.
fn lines_of(blocks: &[Block]) -> Vec<Line> {
    let mut out: Vec<Line> = vec![];
    for b in blocks.iter().filter(|b| (1..=PAGES).contains(&b.page)) {
        let t = normalize(&b.text);
        if t.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(prev) if b.join == Join::Tight && prev.page == b.page => prev.text.push_str(&t),
            _ => out.push(Line {
                page: b.page,
                size: b.font_size,
                text: t,
            }),
        }
    }
    out
}

// ---------------------------------------------------------------- names

const PARTICLES: &[&str] = &[
    "van", "von", "de", "der", "den", "da", "di", "du", "la", "le", "dos", "ter", "ten", "af",
    "al", "bin",
];

static INSTITUTION: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(university|universitetet|institute|institutt|instituttet|department|departments|laboratory|school|centre|center|college|foundation|council|agency|survey|society|museum|service|program|programme|division|research|sciences|science|ministry|association|group|inc|ltd|report|rapport)\b",
    )
});
static ET_AL: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bet al\.?"));
static PAREN: LazyLock<Regex> = LazyLock::new(|| re(r"\([^)]*\)"));
static TRAIL_MARKS: LazyLock<Regex> = LazyLock::new(|| re(r"[\s\d'’`*†‡§]+$"));
static TRAIL_LETTERS: LazyLock<Regex> = LazyLock::new(|| re(r"(?:\s+[a-z])+$"));
static TRAIL_ORG: LazyLock<Regex> = LazyLock::new(|| re(r"\s+(?:HI|NINA|NIVA|NTNU|UiO|UiB)$"));
static TRAIL_I: LazyLock<Regex> = LazyLock::new(|| re(r"([a-z])I$"));
static TOKEN_OK: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^(?:\p{Lu}\.?|(?:\p{Lu}\.)+\p{Lu}?\.?|\p{Lu}[\p{L}'’-]+\.?|\p{Lu}(?:-\p{Lu})+\.?)$")
});

fn is_initial(tok: &str) -> bool {
    tok.chars().next().is_some_and(char::is_uppercase)
        && tok
            .trim_end_matches('.')
            .chars()
            .filter(|c| c.is_alphabetic())
            .count()
            <= 1
        || tok.contains('.') && tok.split('.').all(|p| p.chars().count() <= 1)
}

fn name_ok(name: &str) -> bool {
    if INSTITUTION.is_match(name) {
        return false;
    }
    let toks: Vec<&str> = name.split(' ').collect();
    (2..=6).contains(&toks.len())
        && toks
            .iter()
            .all(|t| TOKEN_OK.is_match(t) || PARTICLES.contains(t))
        && toks
            .iter()
            .any(|t| !is_initial(t) && !PARTICLES.contains(t))
}

/// Names from a line or a run of lines ("A. Author, B. Other and C. Third"). Strict: one piece
/// that isn't a name rejects the whole text, so an affiliation line never passes for authors.
/// Affiliation marks glued to the names ("Tricka,1", "Esenkulova a,*") are removed.
fn parse_names(raw: &str) -> Option<Vec<String>> {
    let s = ET_AL
        .replace_all(&normalize(raw), "")
        .replace(" og ", ", ")
        .replace(" and ", ", ")
        .replace(" & ", ", ")
        .replace(';', ",");
    let mut names: Vec<String> = vec![];
    for piece in s.split(',') {
        let p = PAREN.replace_all(piece, "").trim().to_string();
        let p = p
            .strip_prefix("and ")
            .or_else(|| p.strip_prefix("og "))
            .unwrap_or(&p)
            .trim()
            .to_string();
        let p = TRAIL_MARKS.replace(&p, "").to_string();
        let p = TRAIL_LETTERS.replace(&p, "").to_string();
        let p = TRAIL_ORG.replace(&p, "").to_string();
        let p = TRAIL_I.replace(&p, "$1").trim().to_string();
        if p.chars().count() <= 1 {
            continue;
        }
        if !name_ok(&p) {
            return None;
        }
        if !names.contains(&p) {
            names.push(p);
        }
    }
    if names.is_empty() {
        return None;
    }
    strip_affiliation_letters(&mut names);
    Some(names)
}

/// PNAS and Elsevier print affiliation letters right after the surname: "Tricka", "Billb",
/// "Cochlanb". When every surname in a list of three or more ends in a to e, that is what they are.
fn strip_affiliation_letters(names: &mut [String]) {
    let end = |n: &String| n.chars().last().filter(|c| ('a'..='e').contains(c));
    if names.len() >= 3
        && names
            .iter()
            .all(|n| end(n).is_some() && n.chars().count() >= 4)
    {
        for n in names.iter_mut() {
            n.pop();
        }
    }
}

/// A list of names is believable as the authors if it has several, or one with an initial
/// ("C. Z. Yang"). A lone "Washington Sea Grant" is not.
fn believable(names: &[String]) -> bool {
    names.len() >= 2 || names.first().is_some_and(|n| n.split(' ').any(is_initial))
}

// ---------------------------------------------------------------- headline

struct Headline {
    title: String,
    start: usize,
    end: usize,
}

fn all_caps(s: &str) -> bool {
    s.chars().any(char::is_alphabetic) && !s.chars().any(char::is_lowercase)
}

fn letters_ratio(s: &str) -> f32 {
    let (mut l, mut n) = (0, 0);
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        n += 1;
        l += usize::from(c.is_alphabetic());
    }
    if n == 0 { 0.0 } else { l as f32 / n as f32 }
}

static CITATION_MARK: LazyLock<Regex> = LazyLock::new(|| re(r"\(\d+(?:,\s*\d+)*\)"));

/// One line of running text set in large type: the abstract's first sentence (PNAS) or a pull
/// quote ("The work of John Martin (1, 2) sharply"). Not a title.
fn prose(t: &str, lines: usize) -> bool {
    let words: Vec<&str> = t.split(' ').collect();
    let lower = words
        .iter()
        .filter(|w| w.chars().next().is_some_and(char::is_lowercase))
        .count();
    lines == 1 && words.len() >= 5 && (lower * 2 > words.len() || CITATION_MARK.is_match(t))
}

/// Finds the title of a page: the first run of lines in the largest type that is clearly bigger
/// than the body text. Runs that are numbers, series labels ("Rapport 1a – 2025"), institution
/// names or running text don't count, and a run must be within a quarter of the largest size.
fn headline(ls: &[Line], page: u32) -> Option<Headline> {
    let idx: Vec<usize> = (0..ls.len()).filter(|&i| ls[i].page == page).collect();
    let mut sizes: Vec<f32> = idx
        .iter()
        .map(|&i| ls[i].size)
        .filter(|s| *s > 0.0)
        .collect();
    if idx.len() < 3 || sizes.is_empty() {
        return None;
    }
    sizes.sort_by(f32::total_cmp);
    // A sparse cover page has no body text to compare with, so the smallest type is the baseline.
    let median = if idx.len() < 10 {
        sizes[0]
    } else {
        sizes[sizes.len() / 2]
    };

    // Text recognition reports box heights, not type sizes: many different values on one page,
    // and the lines of one title differ by several percent. Allow that, and ask for more.
    let noisy = idx
        .iter()
        .map(|&i| (ls[i].size * 10.0).round() as i32)
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 12;
    let (tolerance, jump) = if noisy { (0.10, 1.5) } else { (0.03, 1.25) };

    // runs of neighbouring lines with the same size
    let mut runs: Vec<(usize, usize, f32)> = vec![];
    for &i in &idx {
        match runs.last_mut() {
            Some((_, end, size))
                if *end == i && (ls[i].size - *size).abs() <= *size * tolerance =>
            {
                *end = i + 1
            }
            _ => runs.push((i, i + 1, ls[i].size)),
        }
    }
    let text_of = |s: usize, e: usize| {
        ls[s..e.min(s + 5)]
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let cands: Vec<&(usize, usize, f32)> = runs
        .iter()
        .filter(|(s, e, size)| {
            let t = text_of(*s, *e);
            *size >= median * jump
                && (!noisy || *e - *s >= 2)
                && t.chars().count() >= 8
                && letters_ratio(&t) >= 0.6
                && !INSTITUTION.is_match(&t)
                && !prose(&t, (*e - *s).min(5))
        })
        .collect();
    let max = cands.iter().map(|r| r.2).fold(0.0, f32::max);
    let &&(start, end, size) = cands.iter().find(|r| r.2 >= max * 0.75)?;
    let end = end.min(start + 5);
    let mut title = text_of(start, end);
    let mut last = end;

    // "Fiskehelserapporten" with the year set on its own line below
    if let Some(next) = ls
        .get(last)
        .filter(|l| l.page == page && l.size >= size * 0.5)
        && re(r"^(?:19|20)\d{2}$").is_match(&next.text)
    {
        title = format!("{title} {}", next.text);
        last += 1;
    }
    // a title in capitals with a subtitle in ordinary type under it
    if all_caps(&title)
        && let Some(&(s, e, sub_size)) = runs.iter().find(|r| r.0 == last)
        && sub_size >= size * 0.5
        && sub_size < size * 0.9
        && e - s <= 3
    {
        let sub = text_of(s, e);
        if sub.chars().any(char::is_lowercase)
            && sub.split(' ').count() >= 3
            && parse_names(&sub).is_none()
        {
            title = format!("{title}: {sub}");
            last = e;
        }
    }
    Some(Headline {
        title,
        start,
        end: last,
    })
}

/// A title in capitals gets the ordinary-case copy printed elsewhere in the first pages
/// ("For citation purposes: SoundToxins manual (2016)"), if there is one.
fn restore_case(title: &str, head: &str) -> String {
    if !all_caps(title) {
        return title.to_string();
    }
    let pattern = title
        .split_whitespace()
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"\s+");
    let Ok(rx) = Regex::new(&format!("(?i){pattern}")) else {
        return title.to_string();
    };
    rx.find_iter(head)
        .map(|m| m.as_str())
        .find(|m| m.chars().any(char::is_lowercase))
        .unwrap_or(title)
        .to_string()
}

fn title_case(s: &str) -> String {
    const SMALL: &[&str] = &["of", "and", "the", "in", "on", "for", "de", "a"];
    s.to_lowercase()
        .split(' ')
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && SMALL.contains(&w) {
                return w.to_string();
            }
            let mut c = w.chars();
            c.next()
                .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Names on a run of lines. A line that doesn't parse on its own is kept, because a name can
/// wrap ("... Liv Bente Skancke, Birger" / "Skjelbred, Jan-Erik Thrane"), but two in a row end
/// the list.
fn names_in_run<'a>(
    run: impl Iterator<Item = &'a Line>,
    page: u32,
    max_size: f32,
    reverse: bool,
) -> Option<Vec<String>> {
    let mut acc = String::new();
    let (mut best, mut misses) = (None, 0);
    for l in run {
        if l.page != page || l.size > max_size {
            break;
        }
        acc = match (acc.is_empty(), reverse) {
            (true, _) => l.text.clone(),
            (false, false) => format!("{acc} {}", l.text),
            (false, true) => format!("{} {acc}", l.text),
        };
        match parse_names(&acc) {
            Some(n) => {
                best = Some(n);
                misses = 0;
            }
            None => {
                misses += 1;
                if misses == 2 {
                    break;
                }
            }
        }
    }
    best
}

/// Names on the lines after a headline, as long as the lines keep being names.
fn names_after(ls: &[Line], from: usize, page: u32, max_size: f32) -> Option<Vec<String>> {
    names_in_run(ls[from..].iter().take(10), page, max_size, false).filter(|n| believable(n))
}

/// Names on the lines right before the headline (SIFO puts the authors above the title).
/// Needs several names, because one or two capitalised words could be a journal name.
fn names_before(ls: &[Line], to: usize, page: u32) -> Option<Vec<String>> {
    names_in_run(ls[..to].iter().rev().take(4), page, f32::MAX, true).filter(|n| n.len() >= 2)
}

// ---------------------------------------------------------------- labelled fields (reports)

/// Labels on report title pages. A value ends at the next one.
static LABEL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(tittel|title|forfatter\(e\)|forfattere|forfatter|authors?\(s\)|authors?|redaktør\(er\)|redaktører|redaksjon|medarbeidere|dato|date|sider|fagområde|distribusjon|oppdragsgiver(?:\(e\))?|kontaktperson[^:]*|utgitt av|rapport l\.?nr\.?|løpenummer|isbn|issn|prosjektnr\.?|prosjektnummer|sammendrag|emneord|keywords|key words|rapportserie|år - nr\.|godkjent av|programleder\(e\)|program|forskningsgruppe\(r\)|kvalitetssikring|forord|layout og trykk|oppdraget er utført av|publisert|abstract)(?:\s*:\s*|\s+|$)(.*)$",
    )
});

/// The text after a label on pages 1 to 4: what follows it on the label's line and the lines
/// below, up to the next label (at most 12 lines).
fn field(ls: &[Line], labels: &[&str]) -> Option<Vec<String>> {
    let want: Vec<String> = labels.iter().map(|l| l.to_lowercase()).collect();
    for (i, l) in ls.iter().enumerate() {
        let Some(c) = LABEL.captures(&l.text) else {
            continue;
        };
        if !want.contains(&c[1].to_lowercase()) {
            continue;
        }
        let mut out = vec![];
        if !c[2].trim().is_empty() {
            out.push(c[2].trim().to_string());
        }
        for next in ls[i + 1..].iter().take(12) {
            if next.page != l.page || LABEL.is_match(&next.text) || next.text.ends_with(':') {
                break;
            }
            out.push(next.text.clone());
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}

/// A value that wraps over lines. Names listed one per line are joined with commas.
fn names_in(lines: &[String]) -> Option<Vec<String>> {
    let listy = lines
        .iter()
        .any(|l| l.contains(',') || l.contains(" og ") || l.contains(" and ") || l.contains('&'));
    parse_names(&lines.join(if listy { " " } else { ", " }))
}

#[derive(Default)]
struct Report {
    authors: Vec<String>,
    corporate: Option<String>,
    year: Option<i32>,
    series: Option<String>,
    publisher: Option<String>,
    isbn: Option<String>,
    issn: Option<String>,
}

fn year_in(s: &str, max_year: i32) -> Option<i32> {
    re(r"\b((?:19|20)\d{2})\b")
        .captures(s)
        .and_then(|c| c[1].parse().ok())
        .filter(|y| (1900..=max_year).contains(y))
}

fn report_fields(ls: &[Line], head: &str, max_year: i32) -> Report {
    let mut r = Report::default();
    // "Forfattere er kreditert i hvert kapittel": the report has no author list, its institute is the author
    if re(r"(?i)forfattere er kreditert").is_match(head) {
        let by = re(r"(?i)alle forfattere er tilsatt ved ([\p{L}-]+)")
            .captures(head)
            .map(|c| c[1].to_string());
        let own = || {
            re(r"©\s*([\p{L}-]+)\s+(?:19|20)\d{2}")
                .captures(head)
                .map(|c| c[1].to_string())
        };
        // an empty name still means "no personal authors", so the editors are not taken for them
        r.corporate = Some(by.or_else(own).unwrap_or_default());
    } else if let Some(lines) = field(
        ls,
        &[
            "forfatter(e)",
            "forfattere",
            "forfatter",
            "author(s)",
            "authors",
            "author",
        ],
    ) {
        r.authors = names_in(&lines).unwrap_or_default();
    }
    if r.authors.is_empty() && r.corporate.is_none() {
        r.authors = field(ls, &["redaktør(er)", "redaktører"])
            .and_then(|l| names_in(&l))
            .unwrap_or_default();
    }

    // dates, most specific first
    r.year = field(ls, &["dato"])
        .and_then(|l| re(r"^\d{1,2}\.\d{1,2}\.((?:19|20)\d{2})").captures(&l[0]).and_then(|c| c[1].parse().ok()))
        .or_else(|| re(r"(?i)\bpublisert\s+\d{1,2}\.\d{1,2}\.((?:19|20)\d{2})").captures(head).and_then(|c| c[1].parse().ok()))
        .or_else(|| re(r"(?i)\butgitt i [a-zæøå]+ ((?:19|20)\d{2})").captures(head).and_then(|c| c[1].parse().ok()))
        .or_else(|| field(ls, &["år - nr."]).and_then(|l| year_in(&l[0], max_year)))
        .or_else(|| {
            ls.iter().find_map(|l| {
                re(r"^\p{Lu}[\p{L} ]+, \d{1,2}\. (?:januar|februar|mars|april|mai|juni|juli|august|september|oktober|november|desember) ((?:19|20)\d{2})$")
                    .captures(&l.text)
                    .and_then(|c| c[1].parse().ok())
            })
        })
        .filter(|y| (1900..=max_year).contains(y));

    // report series and number
    let nva = head.contains("NIVA") || head.contains("Norsk institutt for vannforskning");
    let number = re(r"(?i)(?:rapport l\.?nr\.?|løpenummer:?)\s*(\d{3,5}-(?:19|20)\d{2})");
    r.series = number
        .captures(head)
        .map(|c| format!("{} {}", if nva { "NIVA-rapport" } else { "Rapport" }, &c[1]))
        .or_else(|| {
            let (serie, nr) = (field(ls, &["rapportserie"])?, field(ls, &["år - nr."])?);
            let serie = serie[0]
                .split("ISSN")
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            (!serie.is_empty()).then(|| format!("{serie} {}", nr[0].trim()))
        })
        .or_else(|| {
            re(r"(Veterinærinstituttets rapportserie nr\.? \d+[a-z]?/\d{4})")
                .captures(head)
                .map(|c| c[1].to_string())
        })
        .or_else(|| {
            re(r"\b(NINA Rapport \d+)\b")
                .captures(head)
                .map(|c| c[1].to_string())
        })
        .or_else(|| {
            re(r"(Prosjektnotat nr\.? \d+) ?[-–] ?((?:19|20)\d{2})")
                .captures(head)
                .map(|c| format!("{} - {}", &c[1], &c[2]))
        });
    if r.year.is_none() {
        r.year = re(r"Prosjektnotat nr\.? \d+ ?[-–] ?((?:19|20)\d{2})")
            .captures(head)
            .and_then(|c| c[1].parse().ok());
    }

    let copyright = re(
        r"©\s*(?:(?:19|20)\d{2}\s+)?(\p{Lu}[\p{L}&-]*(?: \p{Lu}[\p{L}&-]*){0,3})(?:\s+(?:19|20)\d{2}|\.|,|\s*$|\s+All rights|\s+Printed)",
    );
    let published_by = re(r"Published by (\p{Lu}[\p{L}&. -]{2,40}?)(?:\.\s|$)");
    r.publisher = field(ls, &["utgitt av"])
        .map(|l| l[0].clone())
        .or_else(|| {
            ls.iter()
                .find_map(|l| published_by.captures(&l.text).map(|c| c[1].to_string()))
        })
        .or_else(|| {
            ls.iter()
                .filter(|l| l.page <= 2)
                .find_map(|l| copyright.captures(&l.text).map(|c| c[1].to_string()))
        })
        .filter(|p| {
            !p.split(' ')
                .any(|w| re(r"\p{Ll}\p{Lu}.*\p{Ll}\p{Lu}").is_match(w))
        })
        .map(|p| {
            if p.ends_with("B.V") {
                format!("{p}.")
            } else {
                p
            }
        });
    r.isbn = re(r"ISBN[:\s]*((?:97[89][- ]?)?\d[\d-]{8,15}[\dX])")
        .captures(head)
        .map(|c| c[1].to_string());
    r.issn = re(r"ISSN[:\s]*(\d{4}-\d{3}[\dX])")
        .captures(head)
        .map(|c| c[1].to_string());
    r
}

// ---------------------------------------------------------------- journal headers and cover sheets

#[derive(Default, Debug)]
struct Journal {
    journal: Option<String>,
    volume: Option<String>,
    pages: Option<String>,
    year: Option<i32>,
}

fn pages_of(s: &str) -> String {
    s.replace('–', "-").replace(' ', "")
}

/// "Harmful Algae 118 (2022) 102310", "Journal of Applied Phycology 5: 259-265, 1993." and
/// "Vol. 14: 105-114. 1992 DISEASES OF AQUATIC ORGANISMS" in the first lines of page 1.
fn journal_header(ls: &[Line], max_year: i32) -> Option<Journal> {
    let a = re(
        r"^(\p{Lu}[\p{L}.&' -]{2,60}?),?\s+(\d{1,4})\s*\(((?:19|20)\d{2})\)\s*(\d{1,6}(?:\s*[–-]\s*\d{1,6})?)\b",
    );
    let b = re(
        r"^(\p{Lu}[\p{L}.&' -]{2,60}?)\s+(\d{1,4}):\s*(\d+\s*[–-]\s*\d+),?\s*((?:19|20)\d{2})\.?$",
    );
    let c =
        re(r"^Vol\.?\s*(\d+):\s*(\d+\s*[–-]\s*\d+)\.\s*((?:19|20)\d{2})\s*(\p{Lu}[\p{Lu} ]{5,})?$");
    let ok = |y: &str| {
        y.parse::<i32>()
            .ok()
            .filter(|y| (1900..=max_year).contains(y))
    };
    for l in ls.iter().filter(|l| l.page == 1).take(14) {
        let t = &l.text;
        if let Some(m) = a.captures(t) {
            return Some(Journal {
                journal: Some(m[1].trim().to_string()),
                volume: Some(m[2].into()),
                year: ok(&m[3]),
                pages: Some(pages_of(&m[4])),
            });
        }
        if let Some(m) = b.captures(t) {
            return Some(Journal {
                journal: Some(m[1].trim().to_string()),
                volume: Some(m[2].into()),
                pages: Some(pages_of(&m[3])),
                year: ok(&m[4]),
            });
        }
        if let Some(m) = c.captures(t) {
            return Some(Journal {
                journal: m.get(4).map(|j| title_case(j.as_str().trim())),
                volume: Some(m[1].into()),
                pages: Some(pages_of(&m[2])),
                year: ok(&m[3]),
            });
        }
    }
    None
}

/// The publisher's cover sheet on a reprint: "DOI: ...", "Science 309, 1034 (2005);", the authors.
struct Cover {
    doi: Option<String>,
    journal: Journal,
    authors: Vec<String>,
}

fn cover_sheet(ls: &[Line], max_year: i32) -> Option<Cover> {
    let p1: Vec<&Line> = ls.iter().filter(|l| l.page == 1).collect();
    if !p1
        .iter()
        .any(|l| l.text.starts_with("This copy is for your personal"))
    {
        return None;
    }
    let doi = p1.iter().find_map(|l| {
        re(r"^DOI:?\s*(10\.\S+)")
            .captures(&l.text)
            .map(|c| c[1].trim_end_matches(['.', ';', ',']).to_string())
    });
    let cite = re(r"^(\p{Lu}[\p{L} .&-]{1,60}?)\s+(\d+),\s*(\d+)\s*\(((?:19|20)\d{2})\);?$");
    let (i, m) = p1
        .iter()
        .enumerate()
        .find_map(|(i, l)| cite.captures(&l.text).map(|m| (i, m)))?;
    let journal = Journal {
        journal: Some(m[1].trim().to_string()),
        volume: Some(m[2].into()),
        pages: Some(m[3].into()),
        year: m[4].parse().ok().filter(|y| (1900..=max_year).contains(y)),
    };
    let authors = p1
        .get(i + 1)
        .and_then(|l| parse_names(&l.text))
        .unwrap_or_default();
    Some(Cover {
        doi,
        journal,
        authors,
    })
}

// ---------------------------------------------------------------- the whole thing

fn first_doi(text: &str) -> Option<String> {
    meta::dois(text).into_iter().next()
}

/// A reprint's year: when the first six pages name an edition ("Second revised edition") and a
/// copyright line gives a year ("© UNESCO 2004"), that year, because "First published in 2003"
/// is the first edition's. Later pages are read for this only.
fn edition_year(blocks: &[Block], max_year: i32) -> Option<i32> {
    let text = blocks
        .iter()
        .filter(|b| (1..=6).contains(&b.page))
        .map(|b| normalize(&b.text))
        .collect::<Vec<_>>()
        .join(" ");
    re(r"(?i)\b(?:(?:second|third|fourth|fifth|sixth|seventh|\d{1,2}(?:st|nd|rd|th))\s+(?:(?:revised|updated|expanded|new)\s+)*edition|(?:revised|updated|new)\s+edition)\b").find(&text)?;
    re(r"(?i)(?:©|\(c\)|copyright)\s*(?:[\p{L}&.-]+\s+){0,4}?((?:19|20)\d{2})")
        .captures_iter(&text)
        .filter_map(|c| c[1].parse::<i32>().ok())
        .filter(|y| *y <= max_year)
        .max()
}

/// Title, authors, year, DOI and more from file properties and the first pages.
pub fn detect(info: &Info, blocks: &[Block], file_name: &str, max_year: i32) -> Extracted {
    let ls = lines_of(blocks);
    let text_of = |pages: u32| {
        ls.iter()
            .filter(|l| l.page <= pages)
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Pages 1 to 3 for DOIs and years in running text (later pages hold references). All four for
    // the title page's own fields: NINA's citation and ISBN are on page 4.
    let (head, all) = (text_of(3), text_of(PAGES));
    let mut m = Extracted::default();

    let cover = cover_sheet(&ls, max_year);
    let journal = cover
        .as_ref()
        .map(|c| &c.journal)
        .filter(|j| j.journal.is_some());
    let header = journal_header(&ls, max_year);
    let journal = journal.or(header.as_ref());
    let report = report_fields(&ls, &all, max_year);

    // title
    let info_title = info.title.as_deref().and_then(meta::clean_title);
    let page = if cover.is_some() { 2 } else { 1 };
    let headline = headline(&ls, page);
    let title = info_title
        .clone()
        .or_else(|| headline.as_ref().and_then(|h| meta::clean_title(&h.title)));
    m.title = title.map(|t| restore_case(&t, &head));

    // authors: labelled, cover sheet, around the headline, file properties
    let positional = headline.as_ref().and_then(|h| {
        names_after(&ls, h.end, page, f32::MAX).or_else(|| names_before(&ls, h.start, page))
    });
    let from_info = info
        .author
        .as_deref()
        .map(meta::split_authors)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !a.starts_with('[') && !re(r"^\p{Lu}{2,6}$").is_match(a))
        .collect::<Vec<_>>();
    // file properties of a template are as wrong as its title ("Jonas Bratseth" with "18100-NO Mal - ...")
    let info_trusted = info_title.is_some() || from_info.iter().any(|a| head.contains(a.as_str()));
    m.authors = if !report.authors.is_empty() {
        report.authors.clone()
    } else if let Some(c) = cover.as_ref().filter(|c| !c.authors.is_empty()) {
        c.authors.clone()
    } else if let Some(p) = positional {
        p
    } else if report.corporate.is_some() {
        vec![]
    } else if info_trusted {
        from_info
    } else {
        vec![]
    };

    // year
    m.year = report
        .year
        .or(journal.and_then(|j| j.year))
        .or_else(|| {
            let t = m.title.as_deref()?;
            let prefix: String = t.chars().take(25).collect();
            re(&format!(
                r"((?:19|20)\d{{2}})\.\s+{}",
                regex::escape(&prefix)
            ))
            .captures(&all)
            .and_then(|c| c[1].parse().ok())
        })
        .or_else(|| {
            re(r"(?i)for citation purposes:[^(]{0,80}\(((?:19|20)\d{2})\)")
                .captures(&head)
                .and_then(|c| c[1].parse().ok())
        })
        .or_else(|| meta::find_year(&head, max_year))
        .or_else(|| {
            re(r"(?i)\b(?:approved|accepted) (?:\d{1,2} )?[a-z]+ (?:\d{1,2}, )?((?:19|20)\d{2})")
                .captures(&head)
                .and_then(|c| c[1].parse().ok())
        })
        .or_else(|| {
            info.title
                .as_deref()
                .and_then(|t| meta::series_year(t, max_year))
        })
        .filter(|y| (1900..=max_year).contains(y));
    if let Some(y) = edition_year(blocks, max_year)
        && m.year.is_none_or(|cur| y > cur)
    {
        m.year = Some(y);
    }

    // DOI: file properties, the cover sheet, then the text. A page that starts in the middle of a
    // sentence is a page from the middle of an article, and the DOI on it is the neighbour's.
    let mid_article = ls
        .iter()
        .find(|l| l.page == 1)
        .is_some_and(|l| l.text.chars().next().is_some_and(char::is_lowercase));
    let from_text = || {
        let all = meta::dois(&head);
        all.iter()
            .find(|d| meta::doi_matches_file(d, file_name))
            .cloned()
            .or_else(|| {
                if mid_article {
                    None
                } else {
                    all.first().cloned()
                }
            })
    };
    m.doi = info
        .subject
        .as_deref()
        .and_then(first_doi)
        .or_else(|| info.title.as_deref().and_then(first_doi))
        .or_else(|| cover.as_ref().and_then(|c| c.doi.clone()))
        .or_else(from_text);

    if let Some(j) = journal {
        m.journal = j.journal.clone();
        m.volume = j.volume.clone();
        m.pages = j.pages.clone();
    }
    m.series = report.series;
    m.publisher = report
        .corporate
        .filter(|c| !c.is_empty())
        .or(report.publisher);
    m.isbn = report.isbn;
    m.issn = report.issn;
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    type Row = (u32, f32, &'static str);

    fn blocks(rows: &[Row]) -> Vec<Block> {
        rows.iter()
            .map(|&(page, size, text)| Block {
                page,
                text: text.into(),
                rect: None,
                font_size: size,
                join: Join::Space,
            })
            .collect()
    }

    fn run(rows: &[Row], file: &str) -> Extracted {
        detect(&Info::default(), &blocks(rows), file, 2026)
    }

    fn with_info(rows: &[Row], file: &str, title: &str, author: &str) -> Extracted {
        let info = Info {
            title: Some(title.into()),
            author: Some(author.into()),
            subject: None,
        };
        detect(&info, &blocks(rows), file, 2026)
    }

    #[test]
    fn a_later_copyright_year_beats_first_published_when_the_text_names_an_edition() {
        let rows: &[Row] = &[
            (1, 30.0, "Manual on Harmful Marine Microalgae"),
            (5, 8.0, "Second revised edition"),
            (5, 8.0, "First published in 2003"),
            (
                5,
                8.0,
                "by the United Nations Educational, Scientific and Cultural Organization",
            ),
            (5, 8.0, "ISBN 92-3-103948-2"),
            (5, 8.0, "© UNESCO 2004"),
        ];
        assert_eq!(run(rows, "m.pdf").year, Some(2004));
        // no edition statement: a copyright line on a later page changes nothing
        let plain: &[Row] = &[
            (1, 30.0, "Manual on Harmful Marine Microalgae"),
            (1, 8.0, "First published in 2003"),
            (5, 8.0, "© UNESCO 2004"),
        ];
        assert_eq!(run(plain, "m.pdf").year, Some(2003));
        // an edition statement with an earlier copyright year doesn't move the year back
        let earlier: &[Row] = &[
            (1, 30.0, "Manual on Harmful Marine Microalgae"),
            (1, 8.0, "Published 2005"),
            (5, 8.0, "Second edition © UNESCO 2004"),
        ];
        assert_eq!(run(earlier, "m.pdf").year, Some(2005));
    }

    #[test]
    fn names_lose_affiliation_marks_and_reject_everything_else() {
        let n = |s| parse_names(s);
        assert_eq!(
            n("Charles G. Tricka,1, Brian D. Billb,c, William P. Cochlanb , Mark L. Wellsd , and Lisa D. Pickelld").unwrap(),
            ["Charles G. Trick", "Brian D. Bill", "William P. Cochlan", "Mark L. Wells", "Lisa D. Pickell"]
        );
        assert_eq!(
            n("Svetlana Esenkulova a,* , Chrys Neville b , Emiliano DiCicco a,b").unwrap(),
            ["Svetlana Esenkulova", "Chrys Neville", "Emiliano DiCicco"]
        );
        assert_eq!(
            n("P. J. Harrison , P. A. Thompson 2 ' 3, M. Guo 2 & F. J. R. TaylorI").unwrap(),
            [
                "P. J. Harrison",
                "P. A. Thompson",
                "M. Guo",
                "F. J. R. Taylor"
            ]
        );
        assert_eq!(
            n("Renee Mina van Dorst, Gaute Velle").unwrap(),
            ["Renee Mina van Dorst", "Gaute Velle"]
        );
        assert_eq!(
            n("Sussie Dalvin (HI), Ola Diserud NINA og Kjell Rong Utne (HI)").unwrap(),
            ["Sussie Dalvin", "Ola Diserud", "Kjell Rong Utne"]
        );
        assert_eq!(n("P. W. Boyd et al.").unwrap(), ["P. W. Boyd"]);
        assert!(n("Departments of Biology, and Microbiology and Immunology, Schulich School of Medicine").is_none());
        assert!(
            n("Institute for Aquaculture Research, Simon Fraser University, Burnaby, BC, Canada")
                .is_none()
        );
        assert!(n("Pacific Salmon Foundation, Vancouver, BC, V6J 4S6, Canada").is_none());
        assert!(n("Edited by Penny W. Chisholm").is_none());
        assert!(!believable(&n("Washington Sea Grant").unwrap()));
        // surnames that happen to end in a to e are only stripped when all of them do
        assert_eq!(
            n("Philip Boyd, Tim Jickells, Cliff Law").unwrap(),
            ["Philip Boyd", "Tim Jickells", "Cliff Law"]
        );
    }

    #[test]
    fn a_niva_report_with_one_author_per_line() {
        let rows: &[Row] = &[
            (1, 8.0, "1"),
            (1, 12.1, "RAPPORT L.NR. 7132-2017"),
            (1, 22.0, "Miljøtilstanden i Mjøsa"),
            (1, 22.0, "med tilløpselver 2016"),
            (
                1,
                0.0,
                "Mjøsa sett fra Gillundstranda i Stange mot Skreifjella i sør. Foto: J.E. Løvik/NIVA",
            ),
            (2, 8.0, "2"),
            (2, 8.0, "Tittel:"),
            (2, 8.0, "Miljøtilstanden i Mjøsa med"),
            (2, 8.0, "tilløpselver 2016"),
            (2, 8.0, "Rapport lnr. 7132-2017"),
            (2, 8.0, "ISBN 978-82-577-6867-6"),
            (2, 8.0, "Prosjektnr.: O-16228"),
            (2, 8.0, "Oppdraget er utført av Norsk institutt"),
            (2, 8.0, "for vannforskning, NIVA"),
            (2, 8.0, "Forfattere:"),
            (2, 8.0, "Jarl Eivind Løvik"),
            (2, 8.0, "Anne Lyche Solheim"),
            (2, 8.0, "Odd Henning Stuen"),
            (2, 8.0, "Maia Røst Kile"),
            (2, 8.0, "Medarbeidere:"),
            (2, 8.0, "Stein Roger Andersen"),
            (2, 8.0, "Jon Brevik"),
            (2, 8.0, "Utgitt i mars 2017"),
        ];
        let m = run(rows, "7132-2017_web.pdf");
        assert_eq!(
            m.title.as_deref(),
            Some("Miljøtilstanden i Mjøsa med tilløpselver 2016")
        );
        assert_eq!(
            m.authors,
            [
                "Jarl Eivind Løvik",
                "Anne Lyche Solheim",
                "Odd Henning Stuen",
                "Maia Røst Kile"
            ]
        );
        assert_eq!(
            (m.year, m.series.as_deref(), m.isbn.as_deref()),
            (
                Some(2017),
                Some("NIVA-rapport 7132-2017"),
                Some("978-82-577-6867-6")
            )
        );
    }

    #[test]
    fn a_niva_report_with_a_template_in_its_file_properties() {
        let rows: &[Row] = &[
            (1, 16.0, "7936-2024"),
            (1, 26.0, "Overvåking av begroingsalger og"),
            (1, 26.0, "heterotrof begroing på 24 stasjoner i"),
            (1, 26.0, "vannområde Glomma sør for Øyeren"),
            (1, 26.0, "2011-2023"),
            (2, 10.0, "Tittel norsk/engelsk"),
            (2, 10.0, "Overvåking av begroingsalger og"),
            (2, 10.0, "Sider"),
            (2, 10.0, "23 + vedlegg"),
            (2, 10.0, "Dato"),
            (2, 10.0, "09.02.2024"),
            (2, 10.0, "Forfatter(e)"),
            (2, 10.0, "Maia Røst Kile"),
            (2, 10.0, "Fagområde"),
            (2, 10.0, "Utgitt av NIVA"),
            (2, 10.0, "200054"),
            (2, 10.0, "Løpenummer: 7936-2024 ISBN 978-82-577-7672-5"),
            (2, 10.0, "NIVA-rapport ISSN 1894-7948"),
        ];
        let m = with_info(
            rows,
            "7936-2024.pdf",
            "18100-NO Mal - Rapport Glomma sør2023",
            "Jonas Bratseth",
        );
        assert_eq!(
            m.title.as_deref(),
            Some(
                "Overvåking av begroingsalger og heterotrof begroing på 24 stasjoner i vannområde Glomma sør for Øyeren 2011-2023"
            )
        );
        assert_eq!(
            m.authors,
            ["Maia Røst Kile"],
            "the label wins over the template author"
        );
        assert_eq!(
            (m.year, m.publisher.as_deref(), m.issn.as_deref()),
            (Some(2024), Some("NIVA"), Some("1894-7948"))
        );
        // with no label to fall back on, the template's author is not trusted
        let bare = with_info(
            &rows[..5],
            "7936-2024.pdf",
            "18100-NO Mal - Rapport Glomma sør2023",
            "Jonas Bratseth",
        );
        assert!(bare.authors.is_empty());
    }

    #[test]
    fn a_report_with_a_capitals_title_and_a_wrapped_author_list() {
        let rows: &[Row] = &[
            (1, 10.4, "RAPPORT FRA"),
            (1, 10.4, "HAVFORSKNINGEN"),
            (1, 10.4, "NR. 2026-10"),
            (1, 25.5, "RISIKORAPPORT NORSK"),
            (1, 25.5, "FISKEOPPDRETT 2026"),
            (1, 16.1, "Produksjonsdødelighet hos oppdrettsfisk og"),
            (1, 16.1, "miljøeffekter av norsk fiskeoppdrett"),
            (
                1,
                7.9,
                "Redaktør(er): Ellen Sofie Grefsrud, Lasse Berg Andersen, Katherine Mary",
            ),
            (1, 7.9, "Dunlop og Lars Helge Stien (HI)"),
            (2, 9.9, "Rapportserie:"),
            (2, 9.3, "Rapport fra havforskningen"),
            (2, 9.3, "ISSN:1893-4536"),
            (2, 9.9, "År - Nr.:"),
            (2, 9.3, "2026-10"),
            (2, 9.9, "Dato:"),
            (2, 9.3, "11.02.2026"),
            (2, 9.9, "Forfatter(e):"),
            (
                2,
                8.6,
                "Ellen Sofie Grefsrud, Ann-Lisbeth Agnalt, Lasse Berg Andersen,",
            ),
            (
                2,
                8.6,
                "Samantha Bui, Sussie Dalvin (HI), Ola Diserud NINA, Ole",
            ),
            (2, 8.6, "Folkedal, Morten D"),
            (2, 8.6, "Skogen og Kjell Rong Utne (HI)"),
            (
                2,
                8.6,
                "Redaktør(er): Ellen Sofie Grefsrud, Lasse Berg Andersen",
            ),
        ];
        let m = run(rows, "document.pdf");
        assert_eq!(
            m.title.as_deref(),
            Some(
                "RISIKORAPPORT NORSK FISKEOPPDRETT 2026: Produksjonsdødelighet hos oppdrettsfisk og miljøeffekter av norsk fiskeoppdrett"
            )
        );
        assert_eq!(
            &m.authors[..3],
            [
                "Ellen Sofie Grefsrud",
                "Ann-Lisbeth Agnalt",
                "Lasse Berg Andersen"
            ]
        );
        assert!(
            m.authors.contains(&"Morten D Skogen".to_string())
                && m.authors.contains(&"Ola Diserud".to_string())
        );
        assert_eq!(
            (m.year, m.series.as_deref()),
            (Some(2026), Some("Rapport fra havforskningen 2026-10"))
        );
    }

    #[test]
    fn a_corporate_report_has_no_authors_and_a_year_on_its_own_line() {
        let rows: &[Row] = &[
            (
                1,
                9.0,
                "Fremtidens fiskefôr? Soldatfluelarve forstørret 80 ganger. Bildet er tatt med elektronmikroskop",
            ),
            (1, 16.0, "Rapport 1a – 2025"),
            (1, 40.0, "Fiskehelserapporten"),
            (1, 50.0, "2024"),
            (
                1,
                9.0,
                "Fiskehelserapporten 2024 Veterinærinstituttets rapportserie nr 1a/2025",
            ),
            (1, 9.0, "Forfattere"),
            (
                1,
                9.0,
                "Forfattere er kreditert i hvert kapittel. Alle forfattere er tilsatt ved Veterinærinstituttet med",
            ),
            (1, 9.0, "Redaksjon"),
            (
                1,
                9.0,
                "Torfinn Moldal, Jannicke Wiik-Nielsen og Ingunn Sommerset",
            ),
            (
                2,
                9.0,
                "Publisert 16.05.2025 på www.vetinst.no (2. utg.) ISSN 1890-3290",
            ),
        ];
        let m = run(rows, "Fiskehelserapporten 2024_Utgave 2.pdf");
        assert_eq!(m.title.as_deref(), Some("Fiskehelserapporten 2024"));
        assert!(
            m.authors.is_empty(),
            "the editors are not the authors: {:?}",
            m.authors
        );
        assert_eq!(
            (m.year, m.publisher.as_deref()),
            (Some(2025), Some("Veterinærinstituttet"))
        );
        assert_eq!(
            m.series.as_deref(),
            Some("Veterinærinstituttets rapportserie nr 1a/2025")
        );
    }

    #[test]
    fn a_title_page_with_the_authors_above_the_title() {
        let rows: &[Row] = &[
            (
                1,
                20.0,
                "Harald Throne-Holst, Gunnar Vittersø, Line Barkved, Trine",
            ),
            (1, 20.0, "Dale og Tore Brembu"),
            (1, 28.0, "Etiske, lovmessige, samfunns"),
            (1, 28.0, "messige og miljømessige aspekter"),
            (1, 28.0, "ved bruk av mikroalger for CCS"),
            (1, 17.0, "Prosjektnotat nr. 3 - 2016"),
        ];
        let m = run(rows, "x.pdf");
        assert_eq!(
            m.title.as_deref(),
            Some(
                "Etiske, lovmessige, samfunns messige og miljømessige aspekter ved bruk av mikroalger for CCS"
            )
        );
        assert_eq!(m.authors.len(), 5);
        assert_eq!(
            (m.year, m.series.as_deref()),
            (Some(2016), Some("Prosjektnotat nr. 3 - 2016"))
        );
    }

    #[test]
    fn a_nina_report_takes_the_authors_below_the_title_and_the_year_from_the_citation() {
        let rows: &[Row] = &[
            (1, 16.0, "Norsk institutt for naturforskning"),
            (1, 26.0, "Effekter av klimaendringer i ferskvann:"),
            (1, 26.0, "analyser av nasjonale overvåkingsdata"),
            (
                1,
                14.0,
                "Ann Kristin Schartau, Rolf David Vogt, Francesca Pilotto, Knut Andreas Eikland,",
            ),
            (
                1,
                14.0,
                "Susanne Claudia Schneider, Marianne Stave Sekkenes, Liv Bente Skancke, Birger",
            ),
            (
                1,
                14.0,
                "Skjelbred, Jan-Erik Thrane, Renee Mina van Dorst, Gaute Velle",
            ),
            (1, 30.0, "2669"),
            (
                4,
                10.0,
                "Schartau, A.K., Vogt, R.D., Pilotto, F. & Velle, G.",
            ),
            (
                4,
                10.0,
                "2026. Effekter av klimaendringer i ferskvann: analyser av nasjonale overvåkingsdata. NINA Rapport",
            ),
            (
                4,
                10.0,
                "2669. Norsk institutt for naturforskning. ISSN: 1504-3312 ISBN: 978-82-426-5494-6",
            ),
        ];
        let m = run(rows, "ninarapport2669.pdf");
        assert_eq!(
            m.title.as_deref(),
            Some("Effekter av klimaendringer i ferskvann: analyser av nasjonale overvåkingsdata")
        );
        assert_eq!(m.authors.len(), 4 + 3 + 4);
        assert!(m.authors.contains(&"Birger Skjelbred".to_string()));
        assert_eq!(
            (m.year, m.series.as_deref(), m.isbn.as_deref()),
            (
                Some(2026),
                Some("NINA Rapport 2669"),
                Some("978-82-426-5494-6")
            )
        );
    }

    #[test]
    fn pnas_takes_the_title_not_the_abstract_lead() {
        let rows: &[Row] = &[
            (
                1,
                21.9,
                "Iron enrichment stimulates toxic diatom production in",
            ),
            (1, 21.9, "high-nitrate, low-chlorophyll areas"),
            (
                1,
                9.0,
                "Charles G. Tricka,1, Brian D. Billb,c, William P. Cochlanb",
            ),
            (1, 9.0, ", Mark L. Wellsd"),
            (1, 9.0, ", Vera L. Trainerc"),
            (1, 9.0, ", and Lisa D. Pickelld"),
            (1, 4.6, "a"),
            (
                1,
                7.0,
                "Departments of Biology, and Microbiology and Immunology, Schulich School of Medicine and Dentistry, University of Western Ontario",
            ),
            (
                1,
                7.0,
                "Edited by Penny W. Chisholm, Massachusetts Institute of Technology, Cambridge, MA, and approved February 1, 2010 (received for review S",
            ),
            (
                1,
                12.0,
                "carbon sequestering | domoic acid | ocean iron fertilization |",
            ),
            (
                1,
                23.9,
                "Mesoscale iron fertilization experiments have been performedin",
            ),
            (
                1,
                9.0,
                "order to examine the effects of iron on phytoplankton, bottle incubations were carried out with surface water",
            ),
            (
                1,
                9.0,
                "from the Gulf of Alaska and the subarctic Pacific. The results show that iron addition stimulated growth",
            ),
        ];
        let m = with_info(rows, "pnas.pdf", "pnas200910579 5887..5892", "");
        assert_eq!(
            m.title.as_deref(),
            Some(
                "Iron enrichment stimulates toxic diatom production in high-nitrate, low-chlorophyll areas"
            )
        );
        assert_eq!(
            m.authors,
            [
                "Charles G. Trick",
                "Brian D. Bill",
                "William P. Cochlan",
                "Mark L. Wells",
                "Vera L. Trainer",
                "Lisa D. Pickell"
            ]
        );
        assert_eq!(m.year, Some(2010));
    }

    #[test]
    fn a_science_cover_sheet_gives_the_details_and_the_title_comes_from_page_two() {
        let rows: &[Row] = &[
            (1, 12.0, "DOI: 10.1126/science.1131669"),
            (1, 12.0, "Science 315, 612 (2007);"),
            (1, 12.0, "P. W. Boyd et al."),
            (1, 12.0, "Future Directions"),
            (
                1,
                12.0,
                "Mesoscale Iron Enrichment Experiments 1993-2005: Synthesis and",
            ),
            (
                1,
                10.0,
                "This copy is for your personal, non-commercial use only.",
            ),
            (
                1,
                10.0,
                "www.sciencemag.org (this information is current as of June 2, 2014 ):",
            ),
            (2, 21.9, "Mesoscale Iron Enrichment"),
            (2, 21.9, "Experiments 1993–2005: Synthesis"),
            (2, 21.9, "and Future Directions"),
            (2, 9.0, "P. W. Boyd,1"),
            (2, 9.0, "* T. Jickells,2 C. S. Law,3 S. Blain,4"),
            (2, 38.9, "The work of John Martin (1, 2) sharply"),
            (
                2,
                9.0,
                "Since the mid-1980s, our understanding of nutrient limitation of oceanic primary production has",
            ),
            (
                2,
                9.0,
                "radically changed. Mesoscale iron addition experiments (FeAXs) have unequivocally shown that",
            ),
        ];
        let m = run(rows, "science.1131669.pdf");
        assert_eq!(
            m.title.as_deref(),
            Some(
                "Mesoscale Iron Enrichment Experiments 1993–2005: Synthesis and Future Directions"
            )
        );
        assert_eq!(m.authors, ["P. W. Boyd"]);
        assert_eq!(
            (m.year, m.doi.as_deref(), m.journal.as_deref()),
            (Some(2007), Some("10.1126/science.1131669"), Some("Science"))
        );
        assert_eq!(
            (m.volume.as_deref(), m.pages.as_deref()),
            (Some("315"), Some("612"))
        );
    }

    #[test]
    fn journal_headers_give_journal_volume_pages_and_year() {
        let at = |header: &'static str| {
            let rows = [
                (1, 9.0, header),
                (1, 15.0, "A title that is long enough"),
                (1, 15.0, "to fill two lines on the page"),
                (1, 10.0, "Body text goes here and continues."),
                (1, 10.0, "More body text on the page."),
            ];
            let b = blocks(&rows);
            journal_header(&lines_of(&b), 2026)
        };
        let j = at("Journal of Applied Phycology 5: 259-265, 1993.").unwrap();
        assert_eq!(
            (
                j.journal.as_deref(),
                j.volume.as_deref(),
                j.pages.as_deref(),
                j.year
            ),
            (
                Some("Journal of Applied Phycology"),
                Some("5"),
                Some("259-265"),
                Some(1993)
            )
        );
        let j = at("Deep-Sea Research II 53 (2006) 2095–2113").unwrap();
        assert_eq!(
            (j.journal.as_deref(), j.pages.as_deref(), j.year),
            (Some("Deep-Sea Research II"), Some("2095-2113"), Some(2006))
        );
        let j = at("Harmful Algae 118 (2022) 102310").unwrap();
        assert_eq!(
            (j.volume.as_deref(), j.pages.as_deref()),
            (Some("118"), Some("102310"))
        );
        let j = at("Vol. 14: 105-114. 1992 DISEASES OF AQUATIC ORGANISMS").unwrap();
        assert_eq!(
            (j.journal.as_deref(), j.year),
            (Some("Diseases of Aquatic Organisms"), Some(1992))
        );
        assert!(at("Yang & Albright (1994) Dis. Aquat. Org. 14: 105").is_none());
    }

    #[test]
    fn garbled_titles_are_left_blank_and_the_authors_still_found() {
        let rows: &[Row] = &[
            (1, 7.9, "Aquaculfure, 117 (1993) 215-225"),
            (1, 7.9, "Elsevier Science Publishers B.V., Amsterdam"),
            (1, 15.8, "Sub-lethal concentrations of the harmful diatoms,"),
            (1, 15.8, "Chaetoceros concavicornis and C. c~nv~Z~t~~,"),
            (1, 15.8, "increase mortality rates of penned Pacific salmon"),
            (1, 10.6, "L.J. Albright, C.Z. Yang and S. Johnson"),
            (
                1,
                7.9,
                ".Institutefor Aquaculture Research, Simon Fraser University, Burnaby, BC, Canada",
            ),
            (1, 7.9, "(Accepted 26 June 1993)"),
        ];
        let m = run(rows, "0044-8486_2893_2990321-o.pdf");
        assert_eq!(m.title, None);
        assert_eq!(m.authors, ["L.J. Albright", "C.Z. Yang", "S. Johnson"]);
        assert_eq!(
            (m.year, m.journal.as_deref()),
            (Some(1993), Some("Aquaculfure"))
        );
    }

    #[test]
    fn a_title_in_capitals_gets_its_ordinary_case_from_the_citation_line() {
        let rows: &[Row] = &[
            (1, 9.0, "SoundToxins I"),
            (1, 34.0, "Puget Sound Harmful Algal Bloom"),
            (1, 34.0, "Monitoring Program"),
            (1, 65.0, "SOUNDTOXINS"),
            (1, 65.0, "MANUAL"),
            (1, 9.0, "Washington Sea Grant"),
            (
                1,
                9.0,
                "For citation purposes: SoundToxins manual (2016) Washington Sea Grant, 36 p.",
            ),
            (1, 9.0, "Vera Trainer Program Director T: 206.860.6788"),
        ];
        let m = run(rows, "noaa_42096_DS1.pdf");
        assert_eq!(m.title.as_deref(), Some("SoundToxins manual"));
        assert_eq!(m.year, Some(2016));
        assert!(
            m.authors.is_empty(),
            "a program name is not an author: {:?}",
            m.authors
        );
    }

    #[test]
    fn a_page_from_the_middle_of_an_article_keeps_only_a_doi_that_matches_the_file() {
        let rows: &[Row] = &[
            (
                1,
                7.0,
                "films were evaporated on the disk surfaces, yielding well-defined internal markers of",
            ),
            (
                1,
                6.5,
                "Received 17 November 2003; accepted 17 February 2004; doi:10.1038/nature02412.",
            ),
            (
                1,
                6.0,
                "1. Raleigh, C. B. & Paterson, M. S. Experimental deformation of serpentinite. J. Geophys. Res. 70 (1965).",
            ),
        ];
        assert_eq!(run(rows, "nature02437.pdf").doi, None);
        assert_eq!(
            run(rows, "nature02412.pdf").doi.as_deref(),
            Some("10.1038/nature02412")
        );
        let article: &[Row] = &[
            (1, 9.0, "Nature 428, 1-5 (2004)"),
            (1, 9.0, "doi:10.1038/nature02437 is the article"),
        ];
        assert_eq!(
            run(article, "x.pdf").doi.as_deref(),
            Some("10.1038/nature02437")
        );
    }

    #[test]
    fn file_properties_are_cleaned_and_used_when_the_page_says_nothing() {
        let rows: &[Row] = &[
            (1, 10.0, "-ONOGRAPHSON/CEANOGRAPHIC-ETHODOLOGY"),
            (1, 10.0, "0DQXDORQ"),
            (1, 10.0, "11. Manual on Harmful Marine Microalgae"),
        ];
        let m = with_info(
            rows,
            "manual.pdf",
            "Manual on harmful marine microalgae; Monographs on oceanographic methodology; Vol.:11; 2003",
            "Hallegraeff, Gustaaf M.; Anderson, Don M.; IOC",
        );
        assert_eq!(
            m.title.as_deref(),
            Some("Manual on harmful marine microalgae")
        );
        assert_eq!(m.authors, ["Hallegraeff, Gustaaf M.", "Anderson, Don M."]);
        assert_eq!(m.year, Some(2003));
        let bf = with_info(
            rows,
            "x.pdf",
            "Effects of light on <Emphasis Type=\"Italic\">Chaetoceros</Emphasis> growth",
            "[Personer]",
        );
        assert_eq!(
            bf.title.as_deref(),
            Some("Effects of light on Chaetoceros growth")
        );
        assert!(bf.authors.is_empty());
    }

    #[test]
    fn the_subject_line_of_the_file_properties_can_carry_the_doi() {
        let info = Info {
            subject: Some("Harmful Algae, 118 (2022) 102310. doi:10.1016/j.hal.2022.102310".into()),
            ..Default::default()
        };
        let m = detect(
            &info,
            &blocks(&[(1, 9.0, "nothing useful here")]),
            "x.pdf",
            2026,
        );
        assert_eq!(m.doi.as_deref(), Some("10.1016/j.hal.2022.102310"));
    }
}
