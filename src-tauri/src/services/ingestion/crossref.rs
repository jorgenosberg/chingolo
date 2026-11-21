//! Looks a DOI up in Crossref (https://api.crossref.org/works/<doi>) for the exact title, authors,
//! year and journal. Only the DOI is sent. A record is used only if its title is found in the
//! document's own first pages, so a wrong DOI never puts another paper's details on a document.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::domain::library::fold;
use crate::error::{AppError, Result};
use crate::persistence::library::Extracted;

pub const BASE: &str = "https://api.crossref.org/works/";
pub const DATACITE_BASE: &str = "https://api.datacite.org/dois";
const MAX_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Which service gave this record: the value stored in `meta_source` ("crossref", "doi", "datacite", "nb", "openlibrary").
    pub source: &'static str,
    pub doi: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub journal: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub publisher: Option<String>,
    pub issn: Option<String>,
    pub isbn: Option<String>,
}

static MARKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());

pub(super) fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.as_str(),
        Value::Array(a) => a.first()?.as_str()?,
        _ => return None,
    };
    let s = MARKUP
        .replace_all(s, "")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// Older records spell surnames in capitals ("WATSON"). Ordinary case, including after hyphens.
pub(super) fn fix_caps(s: &str) -> String {
    if s.chars().filter(|c| c.is_alphabetic()).count() < 2 || s.chars().any(char::is_lowercase) {
        return s.to_string();
    }
    s.split(' ')
        .map(|w| {
            w.split('-')
                .map(|p| {
                    let mut c = p.chars();
                    c.next().map_or(String::new(), |f| {
                        f.to_uppercase()
                            .chain(c.flat_map(char::to_lowercase))
                            .collect()
                    })
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn person(a: &Value) -> Option<String> {
    let given = a["given"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let family = a["family"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (given, family) {
        (Some(g), Some(f)) => Some(format!("{g} {}", fix_caps(f))),
        (None, Some(f)) => Some(fix_caps(f)),
        _ => a["name"]
            .as_str()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty()),
    }
}

/// Reads the `message` of a Crossref reply.
pub fn parse(reply: &Value) -> Option<Record> {
    let m = &reply["message"];
    Some(Record {
        source: "crossref",
        doi: Some(m["DOI"].as_str()?.to_string()),
        title: text(&m["title"])?,
        authors: m["author"]
            .as_array()
            .map(|a| a.iter().filter_map(person).collect())
            .unwrap_or_default(),
        year: ["issued", "published", "published-print", "published-online"]
            .iter()
            .find_map(|k| m[k]["date-parts"][0][0].as_i64())
            .and_then(|y| i32::try_from(y).ok())
            .filter(|y| (1000..=2200).contains(y)),
        journal: text(&m["container-title"]),
        volume: text(&m["volume"]),
        issue: text(&m["issue"]),
        pages: text(&m["page"]).map(|p| p.replace('–', "-")),
        publisher: text(&m["publisher"]),
        issn: text(&m["ISSN"]),
        isbn: text(&m["ISBN"]),
    })
}

fn tokens(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// True if the record's title is in the document's text: all of it for a short title, at least 90
/// percent of its words for a longer one (a garbled text layer can lose a word).
pub fn title_in_text(title: &str, head: &str) -> bool {
    let want = tokens(title);
    if want.is_empty() {
        return false;
    }
    let have: HashSet<String> = tokens(head).into_iter().collect();
    let found = want.iter().filter(|t| have.contains(*t)).count();
    if want.len() < 5 {
        found == want.len()
    } else {
        found * 10 >= want.len() * 9
    }
}

/// What the record adds to a document. The title, authors and year of a publisher's record are
/// better than what was read off the page, so they replace it. The publisher's legal name
/// ("Springer Science and Business Media LLC") is only used when nothing else is known.
pub fn to_extracted(r: &Record, has_publisher: bool) -> Extracted {
    Extracted {
        title: Some(r.title.clone()),
        authors: r.authors.clone(),
        year: r.year,
        doi: r.doi.clone(),
        journal: r.journal.clone(),
        publisher: if has_publisher {
            None
        } else {
            r.publisher.clone()
        },
        volume: r.volume.clone(),
        issue: r.issue.clone(),
        pages: r.pages.clone(),
        series: None,
        isbn: r.isbn.clone(),
        issn: r.issn.clone(),
    }
}

/// DOIs that file names of well-known sources spell out ("nature02437.pdf", "dao014105.pdf").
/// Only tried through a lookup whose title must then be found in the document.
pub fn candidates(file_name: &str) -> Vec<String> {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(s, _)| s)
        .trim();
    let rules: [(&str, &dyn Fn(&str) -> String); 5] = [
        (r"^(?i)bf\d{8}$", &|s| {
            format!("10.1007/{}", s.to_uppercase())
        }),
        (r"^(?i)dao\d{6}$", &|s| {
            format!("10.3354/{}", s.to_lowercase())
        }),
        (r"^(?i)nature\d{5}$", &|s| {
            format!("10.1038/{}", s.to_lowercase())
        }),
        (r"^\d{6}a\d$", &|s| format!("10.1038/{s}")),
        (r"^(?i)science\.\d{7}$", &|s| {
            format!("10.1126/{}", s.to_lowercase())
        }),
    ];
    rules
        .iter()
        .filter(|(p, _)| Regex::new(p).is_ok_and(|r| r.is_match(stem)))
        .map(|(_, f)| f(stem))
        .collect()
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // A generic name and no contact details: only the DOI leaves the machine.
        .user_agent(format!("Chingolo/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

/// One lookup. `None` when Crossref doesn't know the DOI.
pub async fn fetch(client: &reqwest::Client, base: &str, doi: &str) -> Result<Option<Record>> {
    let started = std::time::Instant::now();
    let v = send_json(client.get(format!("{base}{doi}")), "Crossref").await?;
    log::debug!(
        "Crossref: {doi} answered in {} ms",
        started.elapsed().as_millis()
    );
    Ok(v.and_then(|v| parse(&v)))
}

/// Sends a lookup request and reads a small JSON reply. `None` for 404. `who` names the service in
/// error messages.
pub(super) async fn send_json(req: reqwest::RequestBuilder, who: &str) -> Result<Option<Value>> {
    send_json_with(req, who, false).await
}

/// With `big_is_none`, a reply over the size limit counts as "nothing found" instead of an error.
async fn send_json_with(
    req: reqwest::RequestBuilder,
    who: &str,
    big_is_none: bool,
) -> Result<Option<Value>> {
    let net = |e: reqwest::Error| AppError::Internal(format!("{who} lookup failed: {e}"));
    let resp = req
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(net)?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "{who} lookup failed: HTTP {}",
            resp.status().as_u16()
        )));
    }
    let bytes = resp.bytes().await.map_err(net)?;
    if bytes.len() > MAX_BYTES {
        if big_is_none {
            log::debug!("{who} reply of {} bytes ignored", bytes.len());
            return Ok(None);
        }
        return Err(AppError::Internal(format!(
            "{who} sent far more than expected."
        )));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| AppError::Internal(format!("{who} sent something unreadable: {e}")))
}

/// Reads a CSL-JSON record, which is what doi.org returns for DOIs Crossref doesn't hold (DataCite:
/// arXiv, Zenodo, many repositories).
pub fn parse_csl(v: &Value, doi: &str) -> Option<Record> {
    let year = ["issued", "published", "published-print", "published-online"]
        .iter()
        .find_map(|k| {
            v[k]["date-parts"][0][0].as_i64().or_else(|| {
                v[k]["date-parts"][0][0]
                    .as_str()
                    .and_then(|s| s.parse().ok())
            })
        })
        .and_then(|y| i32::try_from(y).ok())
        .filter(|y| (1000..=2200).contains(y));
    Some(Record {
        source: "doi",
        doi: Some(doi.to_string()),
        title: text(&v["title"])?,
        authors: v["author"]
            .as_array()
            .map(|a| a.iter().filter_map(person).collect())
            .unwrap_or_default(),
        year,
        journal: text(&v["container-title"]),
        volume: text(&v["volume"]),
        issue: text(&v["issue"]),
        pages: text(&v["page"]).map(|p| p.replace('–', "-")),
        publisher: text(&v["publisher"]),
        issn: text(&v["ISSN"]),
        isbn: text(&v["ISBN"]),
    })
}

/// Crossref's search by title and first author. Returns up to three records, best first.
pub async fn search(
    client: &reqwest::Client,
    base: &str,
    title: &str,
    surname: Option<&str>,
) -> Result<Vec<Record>> {
    let mut req = client.get(base.trim_end_matches('/')).query(&[
        ("query.bibliographic", title),
        ("rows", "3"),
        ("select", "DOI,title,author,issued,published,published-print,published-online,container-title,volume,issue,page,publisher,ISSN,ISBN"),
    ]);
    if let Some(s) = surname {
        req = req.query(&[("query.author", s)]);
    }
    // A search can rank a paper with thousands of authors first (a CERN paper's reply is over 1 MB).
    // That is never our document, so an oversized reply is "no match", not a failure.
    let v = send_json_with(req, "Crossref", true).await?;
    Ok(v.and_then(|v| v["message"]["items"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| parse(&serde_json::json!({ "message": item })))
        .collect())
}

/// The surname in "Family, Given" (how file properties often list names) or "Given Family".
pub fn surname_of(name: &str) -> &str {
    match name.split_once(',') {
        Some((family, _)) if !family.trim().is_empty() => family.trim(),
        _ => name.trim().rsplit(' ').next().unwrap_or(name),
    }
}

fn covered(want: &[String], have: &HashSet<String>) -> bool {
    if want.is_empty() {
        return false;
    }
    let found = want.iter().filter(|t| have.contains(*t)).count();
    if want.len() < 5 {
        found == want.len()
    } else {
        found * 10 >= want.len() * 9
    }
}

const EDITION_WORDS: [&str; 12] = [
    "edition", "ed", "revised", "rev", "updated", "expanded", "first", "second", "third", "fourth",
    "fifth", "new",
];

/// "2nd", "3rd", "11th" and the words that name an edition.
fn is_edition_word(t: &str) -> bool {
    EDITION_WORDS.contains(&t)
        || t.strip_suffix("st")
            .or_else(|| t.strip_suffix("nd"))
            .or_else(|| t.strip_suffix("rd"))
            .or_else(|| t.strip_suffix("th"))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The title without a trailing edition statement ("... Microalgae. 2nd revised edition.").
pub fn without_edition(title: &str) -> String {
    let mut words: Vec<&str> = title.split_whitespace().collect();
    while words
        .last()
        .is_some_and(|w| tokens(w).iter().all(|t| is_edition_word(t)))
    {
        words.pop();
    }
    words.join(" ")
}

/// Both titles cover each other, or one is the other plus an edition statement (repositories file
/// "Manual on X. 2nd revised edition." while the paper's own title page says "Manual on X").
fn titles_match(a: &[String], b: &[String]) -> bool {
    let (sa, sb): (HashSet<_>, HashSet<_>) =
        (a.iter().cloned().collect(), b.iter().cloned().collect());
    if covered(a, &sb) && covered(b, &sa) {
        return true;
    }
    let extends = |short: &[String], long: &[String]| {
        !short.is_empty()
            && long.len() > short.len()
            && long[..short.len()] == *short
            && long[short.len()..].iter().all(|t| is_edition_word(t))
    };
    extends(a, b) || extends(b, a)
}

/// Strict check for a search hit: the two titles match, the years are within one, and the first
/// author's surname is among the record's authors (each only when both are known).
pub fn search_hit_fits(
    record: &Record,
    title: &str,
    authors: &[String],
    year: Option<i32>,
) -> bool {
    if !titles_match(&tokens(title), &tokens(&record.title)) {
        return false;
    }
    if let (Some(x), Some(y)) = (year, record.year)
        && (x - y).abs() > 1
    {
        return false;
    }
    if let (Some(first), false) = (authors.first(), record.authors.is_empty()) {
        let surname = tokens(surname_of(first));
        let known = |r: &String| {
            let t = tokens(r);
            surname.iter().all(|s| t.contains(s))
        };
        if !surname.is_empty() && !record.authors.iter().any(known) {
            return false;
        }
    }
    true
}

/// One creator of a DataCite record as "Given Family". `None` for "et al".
fn creator(c: &Value) -> Option<String> {
    let name = c["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())?;
    if name.trim_end_matches('.').eq_ignore_ascii_case("et al") {
        return None;
    }
    if c["nameType"].as_str() == Some("Organizational") {
        return Some(name.to_string());
    }
    match name.split_once(',') {
        Some((family, given)) if !family.trim().is_empty() && !given.trim().is_empty() => {
            Some(format!("{} {}", given.trim(), fix_caps(family.trim())))
        }
        _ => Some(fix_caps(name)),
    }
}

/// Reads one item of a DataCite `/dois` reply.
pub fn parse_datacite(item: &Value) -> Option<Record> {
    let a = &item["attributes"];
    let year = a["publicationYear"]
        .as_i64()
        .or_else(|| a["publicationYear"].as_str().and_then(|s| s.parse().ok()))
        .and_then(|y| i32::try_from(y).ok())
        .filter(|y| (1000..=2200).contains(y));
    Some(Record {
        source: "datacite",
        doi: Some(a["doi"].as_str()?.to_string()),
        title: text(&a["titles"][0]["title"])?,
        authors: a["creators"]
            .as_array()
            .map(|c| c.iter().filter_map(creator).collect())
            .unwrap_or_default(),
        year,
        journal: None,
        volume: None,
        issue: None,
        pages: None,
        publisher: text(&a["publisher"]).or_else(|| text(&a["publisher"]["name"])),
        issn: None,
        isbn: None,
    })
}

/// DataCite's search by title, for reports and other repository records Crossref doesn't hold.
/// Returns up to three records, best first.
pub async fn search_datacite(
    client: &reqwest::Client,
    base: &str,
    title: &str,
) -> Result<Vec<Record>> {
    let phrase = format!("titles.title:\"{}\"", title.replace('"', " "));
    let req = client
        .get(base)
        .query(&[("query", phrase.as_str()), ("page[size]", "3")]);
    let v = send_json_with(req, "DataCite", true).await?;
    Ok(v.and_then(|v| v["data"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(parse_datacite)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::testserver::serve;

    const WATSON: &str = r#"{"status":"ok","message":{"DOI":"10.1038/171737a0","title":["Molecular Structure of Nucleic Acids: A Structure for Deoxyribose Nucleic Acid"],
        "author":[{"given":"J. D.","family":"WATSON","sequence":"first"},{"given":"F. H. C.","family":"CRICK","sequence":"additional"}],
        "issued":{"date-parts":[[1953,4,25]]},"container-title":["Nature"],"volume":"171","issue":"4356","page":"737–738",
        "publisher":"Springer Science and Business Media LLC","ISSN":["0028-0836","1476-4687"],"ISBN":null}}"#;

    #[test]
    fn a_record_is_read_with_names_in_ordinary_case() {
        let r = parse(&serde_json::from_str(WATSON).unwrap()).unwrap();
        assert_eq!(r.authors, ["J. D. Watson", "F. H. C. Crick"]);
        assert_eq!(
            (
                r.year,
                r.journal.as_deref(),
                r.volume.as_deref(),
                r.pages.as_deref()
            ),
            (Some(1953), Some("Nature"), Some("171"), Some("737-738"))
        );
        assert_eq!((r.issn.as_deref(), r.isbn), (Some("0028-0836"), None));
        assert_eq!(fix_caps("BENITEZ-NELSON"), "Benitez-Nelson");
        assert_eq!(fix_caps("DiCicco"), "DiCicco");
        let marked = serde_json::json!({"message": {"DOI": "10.1/x", "title": ["Effects on <i>Chaetoceros</i> &amp; trout"]}});
        assert_eq!(
            parse(&marked).unwrap().title,
            "Effects on Chaetoceros & trout"
        );
        assert!(
            parse(&serde_json::json!({"message": {"DOI": "10.1/x"}})).is_none(),
            "no title, no record"
        );
    }

    #[test]
    fn the_title_must_be_in_the_document() {
        let head = "Mesoscale Iron Enrichment Experiments 1993–2005: Synthesis and Future Directions P. W. Boyd";
        assert!(title_in_text(
            "Mesoscale iron enrichment experiments 1993-2005: synthesis and future directions",
            head
        ));
        assert!(!title_in_text(
            "Seismic structure of the Cascadia subduction zone and its tectonic implications",
            head
        ));
        // one word lost to a garbled text layer out of many is fine, out of a short title is not
        let garbled = "increase mortality rates of penned Pacific salmon and C. c~nv~Z~t~~ Sub-lethal concentrations of the harmful diatoms Chaetoceros concavicornis";
        assert!(title_in_text(
            "Sub-lethal concentrations of the harmful diatoms, Chaetoceros concavicornis and C. convolutus, increase mortality rates of penned Pacific salmon",
            garbled
        ));
        assert!(!title_in_text("Reply", "A long text without that word"));
        assert!(!title_in_text("", head));
    }

    #[test]
    fn file_names_that_spell_a_doi_give_candidates() {
        assert_eq!(candidates("bf00004028.pdf"), ["10.1007/BF00004028"]);
        assert_eq!(candidates("dao014105.pdf"), ["10.3354/dao014105"]);
        assert_eq!(candidates("nature02437.pdf"), ["10.1038/nature02437"]);
        assert_eq!(candidates("383495a0.pdf"), ["10.1038/383495a0"]);
        assert_eq!(
            candidates("science.1131669.pdf"),
            ["10.1126/science.1131669"]
        );
        assert!(candidates("ninarapport2669.pdf").is_empty());
    }

    #[test]
    fn a_publishers_publisher_name_does_not_replace_a_known_one() {
        let r = parse(&serde_json::from_str(WATSON).unwrap()).unwrap();
        assert_eq!(to_extracted(&r, true).publisher, None);
        assert_eq!(
            to_extracted(&r, false).publisher.as_deref(),
            Some("Springer Science and Business Media LLC")
        );
    }

    #[tokio::test]
    async fn lookups_handle_a_record_an_unknown_doi_and_garbage() {
        let (base, seen) = serve(vec![
            (200, WATSON.into()),
            (404, "Resource not found.".into()),
            (200, "not json".into()),
            (500, "oops".into()),
        ]);
        let client = http_client();
        let base = format!("{base}/works/");
        let r = fetch(&client, &base, "10.1038/171737a0")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.year, Some(1953));
        assert!(
            fetch(&client, &base, "10.1000/none")
                .await
                .unwrap()
                .is_none()
        );
        assert!(fetch(&client, &base, "10.1000/bad").await.is_err());
        assert!(fetch(&client, &base, "10.1000/down").await.is_err());
        let head = seen.lock().unwrap()[0].0.to_lowercase();
        assert!(head.starts_with("get /works/10.1038/171737a0 "), "{head}");
        assert!(
            head.contains("user-agent: chingolo/") && !head.contains('@'),
            "{head}"
        );
    }

    #[tokio::test]
    async fn a_search_reply_with_thousands_of_authors_is_no_match_not_an_error() {
        let big = format!(
            r#"{{"message":{{"items":[{{"DOI":"10.1/x","title":["T"],"author":[{}]}}]}}}}"#,
            vec![r#"{"family":"Name","given":"Some"}"#; 40_000].join(",")
        );
        assert!(big.len() > MAX_BYTES);
        let (base, _) = serve(vec![(200, big.clone()), (200, big)]);
        let client = http_client();
        assert!(
            search(
                &client,
                &format!("{base}/works/"),
                "A title of some words",
                None
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert!(
            fetch(&client, &format!("{base}/works/"), "10.1/x")
                .await
                .is_err(),
            "a DOI lookup keeps the strict limit"
        );
    }
}
