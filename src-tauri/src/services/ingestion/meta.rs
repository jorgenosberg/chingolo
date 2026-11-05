//! Guesses at title, authors, year and DOI. Every field is only a starting point,
//! the user can correct it. A wrong guess is worse than a blank, so each rule is strict.

use std::sync::LazyLock;

use regex::Regex;

use super::segment::Block;

static DOI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b10\.\d{4,9}/[^\s"<>]+"#).unwrap());
static YEAR_LABELLED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:©|copyright|published|accepted|received)[^0-9]{0,30}((?:19|20)\d{2})").unwrap()
});

/// The DOI of this document. Pages often mention other DOIs (a neighbouring article on a scanned
/// journal page, "related articles" on a cover sheet), so a DOI that matches the file name wins
/// ("science.1131669.pdf" and 10.1126/science.1131669). Otherwise the first one in the text.
pub fn find_doi(text: &str, file_name: &str) -> Option<String> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).to_lowercase();
    let dois: Vec<String> = DOI
        .find_iter(text)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '"', '\'']).to_string())
        // "10.1234/" alone is not a DOI
        .filter(|d| d.len() > d.find('/').unwrap_or(0) + 1)
        .collect();
    let suffix = |d: &str| d.split_once('/').map_or(String::new(), |(_, s)| s.to_lowercase());
    dois.iter()
        .find(|d| stem.len() >= 6 && suffix(d).len() >= 6 && (stem.contains(&suffix(d)) || suffix(d).contains(&stem)))
        .or(dois.first())
        .cloned()
}

/// A year in a clearly labelled spot ("© 2004", "Published 1998"). Bare or parenthesised years are
/// ignored because they are usually citations, and so are dates in the file properties (for scans
/// they are the scan date).
pub fn find_year(text: &str, max_year: i32) -> Option<i32> {
    let ok = |y: &str| y.parse::<i32>().ok().filter(|y| (1900..=max_year).contains(y));
    YEAR_LABELLED.captures(text).and_then(|c| ok(&c[1]))
}

/// Rejects titles that are file names or editor leftovers.
pub fn clean_title(raw: &str) -> Option<String> {
    let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = t.to_lowercase();
    let junk = t.chars().count() < 4
        || t.chars().count() > 300
        || lower.starts_with("microsoft word")
        || lower.starts_with("untitled")
        || lower.starts_with("doi:")
        || lower.starts_with("pii:")
        || lower.starts_with("http")
        // one token with digits, such as "pnas200910579" or a catalogue number
        || (!t.contains(' ') && t.chars().any(|c| c.is_ascii_digit()))
        // a token with digits and then more codes: "pnas200910579 5887..5892"
        || t.split(' ').next().is_some_and(|w| w.len() > 6 && w.chars().filter(char::is_ascii_digit).count() >= 5)
        || [".pdf", ".doc", ".docx", ".indd", ".tex", ".qxd"].iter().any(|e| lower.ends_with(e))
        || t.chars().all(|c| !c.is_alphabetic());
    (!junk).then_some(t)
}

/// "A. Author; B. Other" or "A. Author and B. Other" into a list. Returns nothing for junk.
pub fn split_authors(raw: &str) -> Vec<String> {
    let lower = raw.to_lowercase();
    if lower.contains("unknown") || lower.contains("administrator") || lower.contains("user") {
        return vec![];
    }
    raw.split([';', '\n'])
        .flat_map(|s| s.split(" and "))
        .map(|s| s.trim().to_string())
        .filter(|s| s.chars().any(char::is_alphabetic) && s.chars().count() < 80)
        .take(30)
        .collect()
}

/// The first page's headline: the first run of lines in the largest type,
/// if that type is clearly bigger than the body text.
pub fn title_from_first_page(blocks: &[Block]) -> Option<String> {
    let page: Vec<&Block> = blocks.iter().filter(|b| b.page == 1 && b.font_size > 0.0).collect();
    if page.len() < 3 {
        return None;
    }
    let mut sizes: Vec<f32> = page.iter().map(|b| b.font_size).collect();
    sizes.sort_by(f32::total_cmp);
    let (median, max) = (sizes[sizes.len() / 2], sizes[sizes.len() - 1]);
    if max < median * 1.25 {
        return None;
    }
    let start = page.iter().position(|b| b.font_size >= max * 0.95)?;
    let run: Vec<&str> = page[start..]
        .iter()
        .take(4)
        .take_while(|b| b.font_size >= max * 0.95)
        .map(|b| b.text.trim())
        .collect();
    clean_title(&run.join(" ")).filter(|t| t.chars().count() >= 8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ingestion::segment::Join;

    fn b(text: &str, size: f32) -> Block {
        Block { page: 1, text: text.into(), rect: None, font_size: size, join: Join::Space }
    }

    #[test]
    fn finds_dois_and_trims_punctuation() {
        assert_eq!(find_doi("see doi: 10.1126/science.1131669.", "x.pdf"), Some("10.1126/science.1131669".into()));
        assert_eq!(find_doi("(https://doi.org/10.1016/j.hal.2022.102310)", "x.pdf"), Some("10.1016/j.hal.2022.102310".into()));
        assert_eq!(find_doi("10.1234/", "x.pdf"), None);
        assert_eq!(find_doi("version 10.5 of the tool", "x.pdf"), None);
    }

    #[test]
    fn a_doi_matching_the_file_name_beats_the_first_one() {
        let text = "Related: 10.1126/science.1111534 and this article 10.1126/science.1082000.";
        assert_eq!(find_doi(text, "science.1082000.pdf"), Some("10.1126/science.1082000".into()));
        assert_eq!(find_doi(text, "report.pdf"), Some("10.1126/science.1111534".into()));
    }

    #[test]
    fn year_needs_a_label_or_parentheses() {
        assert_eq!(find_year("© 2004 Nature Publishing Group", 2026), Some(2004));
        assert_eq!(find_year("Copyright 1998 Elsevier", 2026), Some(1998));
        assert_eq!(find_year("Yang & Albright (1994) Dis. Aquat. Org.", 2026), None, "citations don't count");
        assert_eq!(find_year("measured 2.5 mg at 1500 m depth in 1993", 2026), None);
        assert_eq!(find_year("© 2099", 2026), None);
    }

    #[test]
    fn rejects_file_name_titles() {
        assert_eq!(clean_title("Microsoft Word - draft.doc"), None);
        assert_eq!(clean_title("scan_0042.pdf"), None);
        assert_eq!(clean_title("  Iron   enrichment\nand diatoms "), Some("Iron enrichment and diatoms".into()));
        assert_eq!(clean_title("123"), None);
        assert_eq!(clean_title("PII: 0044-8486(93)90321-O"), None);
        assert_eq!(clean_title("pnas200910579 5887..5892"), None);
        assert_eq!(clean_title("doi:10.1016/j.dsr2.2006.05.038"), None);
        assert_eq!(clean_title("Protozoa and Myxozoa"), Some("Protozoa and Myxozoa".into()));
        assert_eq!(clean_title("Effects of iron on diatoms in 2004"), Some("Effects of iron on diatoms in 2004".into()));
    }

    #[test]
    fn authors_are_split_and_junk_is_ignored() {
        assert_eq!(split_authors("P. W. Boyd; A. J. Watson and C. S. Law"), ["P. W. Boyd", "A. J. Watson", "C. S. Law"]);
        assert!(split_authors("Unknown").is_empty());
        assert!(split_authors("Administrator").is_empty());
    }

    #[test]
    fn headline_is_the_first_run_in_the_largest_type() {
        let blocks = vec![
            b("Journal of Things", 9.0),
            b("Effects of a diatom on", 18.0),
            b("rainbow trout respiration", 18.0),
            b("A. Author", 11.0),
            b("Body text goes here and continues.", 10.0),
            b("More body text on the page.", 10.0),
        ];
        assert_eq!(title_from_first_page(&blocks), Some("Effects of a diatom on rainbow trout respiration".into()));
    }

    #[test]
    fn no_headline_when_type_sizes_are_flat() {
        let blocks: Vec<_> = (0..6).map(|i| b(&format!("Line number {i} of ordinary text"), 10.0)).collect();
        assert_eq!(title_from_first_page(&blocks), None);
    }
}
