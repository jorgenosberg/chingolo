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
const MAX_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub doi: String,
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

fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.as_str(),
        Value::Array(a) => a.first()?.as_str()?,
        _ => return None,
    };
    let s = MARKUP.replace_all(s, "").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// Older records spell surnames in capitals ("WATSON"). Ordinary case, including after hyphens.
fn fix_caps(s: &str) -> String {
    if s.chars().filter(|c| c.is_alphabetic()).count() < 2 || s.chars().any(char::is_lowercase) {
        return s.to_string();
    }
    s.split(' ')
        .map(|w| {
            w.split('-')
                .map(|p| {
                    let mut c = p.chars();
                    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect())
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn person(a: &Value) -> Option<String> {
    let given = a["given"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let family = a["family"].as_str().map(str::trim).filter(|s| !s.is_empty());
    match (given, family) {
        (Some(g), Some(f)) => Some(format!("{g} {}", fix_caps(f))),
        (None, Some(f)) => Some(fix_caps(f)),
        _ => a["name"].as_str().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
    }
}

/// Reads the `message` of a Crossref reply.
pub fn parse(reply: &Value) -> Option<Record> {
    let m = &reply["message"];
    Some(Record {
        doi: m["DOI"].as_str()?.to_string(),
        title: text(&m["title"])?,
        authors: m["author"].as_array().map(|a| a.iter().filter_map(person).collect()).unwrap_or_default(),
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
    fold(s).split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(String::from).collect()
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
    if want.len() < 5 { found == want.len() } else { found * 10 >= want.len() * 9 }
}

/// What the record adds to a document. The title, authors and year of a publisher's record are
/// better than what was read off the page, so they replace it. The publisher's legal name
/// ("Springer Science and Business Media LLC") is only used when nothing else is known.
pub fn to_extracted(r: &Record, has_publisher: bool) -> Extracted {
    Extracted {
        title: Some(r.title.clone()),
        authors: r.authors.clone(),
        year: r.year,
        doi: Some(r.doi.clone()),
        journal: r.journal.clone(),
        publisher: if has_publisher { None } else { r.publisher.clone() },
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
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).trim();
    let rules: [(&str, &dyn Fn(&str) -> String); 5] = [
        (r"^(?i)bf\d{8}$", &|s| format!("10.1007/{}", s.to_uppercase())),
        (r"^(?i)dao\d{6}$", &|s| format!("10.3354/{}", s.to_lowercase())),
        (r"^(?i)nature\d{5}$", &|s| format!("10.1038/{}", s.to_lowercase())),
        (r"^\d{6}a\d$", &|s| format!("10.1038/{s}")),
        (r"^(?i)science\.\d{7}$", &|s| format!("10.1126/{}", s.to_lowercase())),
    ];
    rules.iter().filter(|(p, _)| Regex::new(p).is_ok_and(|r| r.is_match(stem))).map(|(_, f)| f(stem)).collect()
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // A generic name and no contact details: only the DOI leaves the machine.
        .user_agent(format!("Chingui/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

/// One lookup. `None` when Crossref doesn't know the DOI.
pub async fn fetch(client: &reqwest::Client, base: &str, doi: &str) -> Result<Option<Record>> {
    let net = |e: reqwest::Error| AppError::Internal(format!("Crossref lookup failed: {e}"));
    let resp = client.get(format!("{base}{doi}")).timeout(Duration::from_secs(15)).send().await.map_err(net)?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!("Crossref lookup failed: HTTP {}", resp.status().as_u16())));
    }
    let bytes = resp.bytes().await.map_err(net)?;
    if bytes.len() > MAX_BYTES {
        return Err(AppError::Internal("Crossref sent far more than expected.".into()));
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| AppError::Internal(format!("Crossref sent something unreadable: {e}")))?;
    Ok(parse(&v))
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
        assert_eq!((r.year, r.journal.as_deref(), r.volume.as_deref(), r.pages.as_deref()), (Some(1953), Some("Nature"), Some("171"), Some("737-738")));
        assert_eq!((r.issn.as_deref(), r.isbn), (Some("0028-0836"), None));
        assert_eq!(fix_caps("BENITEZ-NELSON"), "Benitez-Nelson");
        assert_eq!(fix_caps("DiCicco"), "DiCicco");
        let marked = serde_json::json!({"message": {"DOI": "10.1/x", "title": ["Effects on <i>Chaetoceros</i> &amp; trout"]}});
        assert_eq!(parse(&marked).unwrap().title, "Effects on Chaetoceros & trout");
        assert!(parse(&serde_json::json!({"message": {"DOI": "10.1/x"}})).is_none(), "no title, no record");
    }

    #[test]
    fn the_title_must_be_in_the_document() {
        let head = "Mesoscale Iron Enrichment Experiments 1993–2005: Synthesis and Future Directions P. W. Boyd";
        assert!(title_in_text("Mesoscale iron enrichment experiments 1993-2005: synthesis and future directions", head));
        assert!(!title_in_text("Seismic structure of the Cascadia subduction zone and its tectonic implications", head));
        // one word lost to a garbled text layer out of many is fine, out of a short title is not
        let garbled = "increase mortality rates of penned Pacific salmon and C. c~nv~Z~t~~ Sub-lethal concentrations of the harmful diatoms Chaetoceros concavicornis";
        assert!(title_in_text("Sub-lethal concentrations of the harmful diatoms, Chaetoceros concavicornis and C. convolutus, increase mortality rates of penned Pacific salmon", garbled));
        assert!(!title_in_text("Reply", "A long text without that word"));
        assert!(!title_in_text("", head));
    }

    #[test]
    fn file_names_that_spell_a_doi_give_candidates() {
        assert_eq!(candidates("bf00004028.pdf"), ["10.1007/BF00004028"]);
        assert_eq!(candidates("dao014105.pdf"), ["10.3354/dao014105"]);
        assert_eq!(candidates("nature02437.pdf"), ["10.1038/nature02437"]);
        assert_eq!(candidates("383495a0.pdf"), ["10.1038/383495a0"]);
        assert_eq!(candidates("science.1131669.pdf"), ["10.1126/science.1131669"]);
        assert!(candidates("ninarapport2669.pdf").is_empty());
    }

    #[test]
    fn a_publishers_publisher_name_does_not_replace_a_known_one() {
        let r = parse(&serde_json::from_str(WATSON).unwrap()).unwrap();
        assert_eq!(to_extracted(&r, true).publisher, None);
        assert_eq!(to_extracted(&r, false).publisher.as_deref(), Some("Springer Science and Business Media LLC"));
    }

    #[tokio::test]
    async fn lookups_handle_a_record_an_unknown_doi_and_garbage() {
        let (base, seen) = serve(vec![(200, WATSON.into()), (404, "Resource not found.".into()), (200, "not json".into()), (500, "oops".into())]);
        let client = http_client();
        let base = format!("{base}/works/");
        let r = fetch(&client, &base, "10.1038/171737a0").await.unwrap().unwrap();
        assert_eq!(r.year, Some(1953));
        assert!(fetch(&client, &base, "10.1000/none").await.unwrap().is_none());
        assert!(fetch(&client, &base, "10.1000/bad").await.is_err());
        assert!(fetch(&client, &base, "10.1000/down").await.is_err());
        let head = seen.lock().unwrap()[0].0.to_lowercase();
        assert!(head.starts_with("get /works/10.1038/171737a0 "), "{head}");
        assert!(head.contains("user-agent: chingui/") && !head.contains('@'), "{head}");
    }
}
