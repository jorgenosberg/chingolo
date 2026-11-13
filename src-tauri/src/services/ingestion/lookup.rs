//! Looks a paper up by an identifier it carries: a DOI (Crossref, then doi.org for DataCite DOIs),
//! an arXiv id (a DataCite DOI), a PubMed id (NCBI gives the DOI) or an ISBN (the National Library
//! of Norway, Open Library). Only the identifier is sent. The caller still checks that the record's
//! title is in the document, so a wrong identifier never puts another work's details on it.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::crossref::{self, Record, fix_caps, send_json, text};
use crate::domain::library::fold;
use crate::error::Result;

/// Where each service lives. Tests point all of them at one local server.
#[derive(Debug, Clone)]
pub struct Bases {
    /// Ends with `/works/`.
    pub crossref: String,
    /// DataCite's `/dois` endpoint (title search).
    pub datacite: String,
    /// Ends with `/`. The DOI is appended.
    pub doi: String,
    pub ncbi: String,
    pub nb: String,
    pub openlibrary: String,
}

impl Bases {
    pub fn live() -> Self {
        Bases {
            crossref: crossref::BASE.into(),
            datacite: crossref::DATACITE_BASE.into(),
            doi: "https://doi.org/".into(),
            ncbi: "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi".into(),
            nb: "https://api.nb.no/catalog/v1/items".into(),
            openlibrary: "https://openlibrary.org/api/books".into(),
        }
    }

    #[cfg(test)]
    pub fn at(base: &str) -> Self {
        Bases {
            crossref: format!("{base}/works/"),
            datacite: format!("{base}/datacite"),
            doi: format!("{base}/doi/"),
            ncbi: format!("{base}/ncbi"),
            nb: format!("{base}/nb"),
            openlibrary: format!("{base}/ol"),
        }
    }
}

/// One thing to try, in the order they are tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Doi(String),
    /// A DataCite DOI built from an arXiv id (`10.48550/arXiv.<id>`).
    Arxiv(String),
    Pmid(String),
    /// An ISBN-13.
    Nb(String),
    OpenLibrary(String),
}

impl Step {
    /// For logs. Never text from the document beyond the identifier itself.
    pub fn label(&self) -> String {
        match self {
            Step::Doi(d) => format!("DOI {d}"),
            Step::Arxiv(a) => format!("arXiv {a}"),
            Step::Pmid(p) => format!("PubMed {p}"),
            Step::Nb(i) => format!("ISBN {i} at the National Library of Norway"),
            Step::OpenLibrary(i) => format!("ISBN {i} at Open Library"),
        }
    }
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("lookup regex")
}

static ARXIV_TEXT: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)arxiv:\s*(\d{4}\.\d{4,5}|[a-z]+(?:-[a-z]+)?(?:\.[A-Za-z]{2})?/\d{7})(?:v\d+)?"));
static ARXIV_FILE: LazyLock<Regex> = LazyLock::new(|| re(r"^(\d{4}\.\d{4,5})(?:v\d+)?$"));
static PMID: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bPMID:?\s*(\d{6,8})\b"));
static ISBN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)ISBN(?:-1[03])?[:\s]*((?:97[89][- ]?)?\d[\d\- ]{8,16}[\dX])"));

/// True if the ISBN-10 or ISBN-13 (digits and a final X only) has a valid check digit.
fn isbn_valid(d: &str) -> bool {
    let v: Vec<u32> = d.chars().filter_map(|c| if c == 'X' { Some(10) } else { c.to_digit(10) }).collect();
    match v.len() {
        13 if !d.contains('X') => v.iter().enumerate().map(|(i, x)| x * if i % 2 == 0 { 1 } else { 3 }).sum::<u32>() % 10 == 0,
        10 => v.iter().enumerate().map(|(i, x)| x * (10 - i as u32)).sum::<u32>() % 11 == 0 && !d[..9].contains('X'),
        _ => false,
    }
}

/// The ISBN-13 for a valid ISBN-10 or ISBN-13, from text such as "92-3-103948-2".
pub fn isbn13(raw: &str) -> Option<String> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').map(|c| c.to_ascii_uppercase()).collect();
    // a year or page number glued on behind the number
    let d = [d.len(), 13, 10].into_iter().find_map(|n| d.get(..n).filter(|s| isbn_valid(s)).map(String::from))?;
    if d.len() == 13 {
        return Some(d);
    }
    let body = format!("978{}", &d[..9]);
    let sum: u32 = body.chars().enumerate().map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 }).sum();
    Some(format!("{body}{}", (10 - sum % 10) % 10))
}

/// Up to three valid ISBN-13s that the text names, in order, without repeats.
pub fn isbns_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in ISBN.captures_iter(text) {
        if let Some(i) = isbn13(&c[1])
            && !out.contains(&i)
        {
            out.push(i);
        }
    }
    out.truncate(3);
    out
}

/// What to try for a document, from its DOI, ISBN, file name and the text of its first pages.
pub fn steps(doi: Option<&str>, isbn: Option<&str>, file_name: &str, head: &str) -> Vec<Step> {
    let mut out: Vec<Step> = vec![];
    let mut add = |s: Step| {
        let seen = out.iter().any(|x| x == &s || matches!((x, &s), (Step::Doi(a), Step::Doi(b)) if a.eq_ignore_ascii_case(b)));
        if !seen {
            out.push(s);
        }
    };
    for d in doi.map(String::from).into_iter().chain(crossref::candidates(file_name)) {
        add(Step::Doi(d));
    }
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).trim();
    let from_file = ARXIV_FILE.captures(stem).map(|c| c[1].to_string());
    for id in from_file.into_iter().chain(ARXIV_TEXT.captures_iter(head).map(|c| c[1].to_string())) {
        add(Step::Arxiv(id));
    }
    for c in PMID.captures_iter(head).take(2) {
        add(Step::Pmid(c[1].to_string()));
    }
    let mut isbns: Vec<String> = isbn.and_then(isbn13).into_iter().collect();
    for i in isbns_in(head) {
        if !isbns.contains(&i) {
            isbns.push(i);
        }
    }
    isbns.truncate(3);
    for i in isbns {
        // Norwegian publishers first for 978-82 numbers, the other order for the rest
        if i.starts_with("97882") {
            add(Step::Nb(i.clone()));
            add(Step::OpenLibrary(i));
        } else {
            add(Step::OpenLibrary(i.clone()));
            add(Step::Nb(i));
        }
    }
    out
}

/// Runs one step. `Ok(None)` when the service doesn't know the identifier.
pub async fn run(client: &reqwest::Client, bases: &Bases, step: &Step) -> Result<Option<Record>> {
    match step {
        Step::Doi(d) => by_doi(client, bases, d, true).await,
        Step::Arxiv(id) => by_doi(client, bases, &format!("10.48550/arXiv.{id}"), false).await,
        Step::Pmid(p) => match pubmed_doi(client, &bases.ncbi, p).await? {
            Some(doi) => by_doi(client, bases, &doi, true).await,
            None => Ok(None),
        },
        Step::Nb(i) => nb(client, &bases.nb, i).await,
        Step::OpenLibrary(i) => open_library(client, &bases.openlibrary, i).await,
    }
}

/// Crossref first (unless the DOI is known to be DataCite's), then doi.org.
async fn by_doi(client: &reqwest::Client, bases: &Bases, doi: &str, crossref_first: bool) -> Result<Option<Record>> {
    if crossref_first
        && let Some(r) = crossref::fetch(client, &bases.crossref, doi).await?
    {
        return Ok(Some(r));
    }
    let req = client.get(format!("{}{doi}", bases.doi)).header("Accept", "application/vnd.citationstyles.csl+json");
    Ok(send_json(req, "doi.org").await?.and_then(|v| crossref::parse_csl(&v, doi)))
}

async fn pubmed_doi(client: &reqwest::Client, base: &str, pmid: &str) -> Result<Option<String>> {
    let req = client.get(base).query(&[("db", "pubmed"), ("id", pmid), ("retmode", "json")]);
    let Some(v) = send_json(req, "PubMed").await? else { return Ok(None) };
    Ok(v["result"][pmid]["articleids"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["idtype"] == "doi"))
        .and_then(|x| x["value"].as_str())
        .map(str::to_string))
}

/// "Schartau, Ann Kristin" becomes "Ann Kristin Schartau". Names without a comma are left alone.
fn flip(name: &str) -> String {
    match name.split_once(',') {
        Some((family, given)) if !given.trim().is_empty() => format!("{} {}", given.trim(), fix_caps(family.trim())),
        _ => name.trim().to_string(),
    }
}

fn year_in(s: &str) -> Option<i32> {
    re(r"\b(1[5-9]\d{2}|20\d{2})\b").captures(s).and_then(|c| c[1].parse().ok())
}

fn issn_dashed(s: &str) -> Option<String> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit() || *c == 'X').collect();
    (d.len() == 8).then(|| format!("{}-{}", &d[..4], &d[4..]))
}

/// The National Library of Norway's catalogue. The first hit for an exact ISBN.
async fn nb(client: &reqwest::Client, base: &str, isbn: &str) -> Result<Option<Record>> {
    let req = client.get(base).query(&[("q", format!("isbn:{isbn}"))]);
    let Some(v) = send_json(req, "National Library of Norway").await? else { return Ok(None) };
    let Some(m) = v["_embedded"]["items"].get(0).map(|i| &i["metadata"]) else { return Ok(None) };
    // a hit for another number is not ours
    if let Some(ids) = m["identifiers"]["isbn13"].as_array()
        && !ids.iter().any(|x| x.as_str() == Some(isbn))
    {
        return Ok(None);
    }
    let Some(title) = text(&m["title"]).map(|t| t.replace(" : ", ": ")) else { return Ok(None) };
    Ok(Some(Record {
        source: "nb",
        doi: None,
        title,
        authors: m["creators"].as_array().map(|a| a.iter().filter_map(|n| n.as_str()).map(flip).collect()).unwrap_or_default(),
        year: m["originInfo"]["issued"].as_str().and_then(year_in),
        journal: None,
        volume: None,
        issue: None,
        pages: None,
        publisher: text(&m["originInfo"]["publisher"]),
        issn: m["identifiers"]["issn"].get(0).and_then(Value::as_str).and_then(issn_dashed),
        isbn: Some(isbn.to_string()),
    }))
}

async fn open_library(client: &reqwest::Client, base: &str, isbn: &str) -> Result<Option<Record>> {
    let key = format!("ISBN:{isbn}");
    let req = client.get(base).query(&[("bibkeys", key.as_str()), ("format", "json"), ("jscmd", "data")]);
    let Some(v) = send_json(req, "Open Library").await? else { return Ok(None) };
    let m = &v[&key];
    let Some(mut title) = text(&m["title"]) else { return Ok(None) };
    if let Some(sub) = text(&m["subtitle"]) {
        title = format!("{title}: {sub}");
    }
    let names = |k: &str| -> Vec<String> { m[k].as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str()).map(|n| n.trim().to_string()).collect()).unwrap_or_default() };
    Ok(Some(Record {
        source: "openlibrary",
        doi: None,
        title,
        authors: names("authors"),
        year: m["publish_date"].as_str().and_then(year_in),
        journal: None,
        volume: None,
        issue: None,
        pages: None,
        publisher: names("publishers").into_iter().next(),
        issn: None,
        isbn: Some(isbn.to_string()),
    }))
}

/// First author's surname, for a title search.
pub fn first_surname(authors: &[String]) -> Option<String> {
    let last = crossref::surname_of(authors.first()?).to_string();
    (fold(&last).chars().any(char::is_alphabetic)).then_some(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::testserver::serve;

    #[test]
    fn isbns_are_checked_and_made_13_digits() {
        assert_eq!(isbn13("978-82-577-6867-6").as_deref(), Some("9788257768676"));
        assert_eq!(isbn13("92-3-103948-2").as_deref(), Some("9789231039485"));
        assert_eq!(isbn13("951-649-727-6").as_deref(), Some("9789516497276"));
        assert_eq!(isbn13("0-8044-2957-X").as_deref(), Some("9780804429573"));
        assert_eq!(isbn13("978-82-577-6867-5"), None, "bad check digit");
        assert_eq!(isbn13("12345"), None);
        // a year glued on behind the number is dropped
        assert_eq!(isbn13("978-82-426-5494-6 2026").as_deref(), Some("9788242654946"));
    }

    #[test]
    fn isbns_come_out_of_running_text() {
        let t = "ISSN: 1504-3312 ISBN: 978-82-426-5494-6 and later (ISBN 951- 649-727-6.) but ISBN 978-82-426-0000-0 is wrong";
        assert_eq!(isbns_in(t), ["9788242654946", "9789516497276"]);
    }

    #[test]
    fn identifiers_in_text_and_file_names_become_steps_in_order() {
        let head = "Attention. arXiv:1706.03762v5 [cs.CL] 2 Aug 2023. PMID: 20231478. Hep: arXiv:hep-th/9901001v2. ISBN 978-82-577-6867-6";
        let s = steps(Some("10.1038/nature02437"), None, "nature02437.pdf", head);
        assert_eq!(
            s,
            [
                Step::Doi("10.1038/nature02437".into()),
                Step::Arxiv("1706.03762".into()),
                Step::Arxiv("hep-th/9901001".into()),
                Step::Pmid("20231478".into()),
                Step::Nb("9788257768676".into()),
                Step::OpenLibrary("9788257768676".into()),
            ]
        );
        let file = steps(None, None, "2101.12345v2.pdf", "");
        assert_eq!(file, [Step::Arxiv("2101.12345".into())]);
        let foreign = steps(None, Some("92-3-103948-2"), "x.pdf", "");
        assert_eq!(foreign, [Step::OpenLibrary("9789231039485".into()), Step::Nb("9789231039485".into())]);
        assert!(steps(None, None, "x.pdf", "Version 2.3, 1706 pages").is_empty());
    }

    const CSL: &str = r#"{"title":"Attention Is All You Need","author":[{"given":"Ashish","family":"Vaswani"},{"given":"Noam","family":"Shazeer"}],"issued":{"date-parts":[[2017]]},"publisher":"arXiv"}"#;
    const NB: &str = r#"{"_embedded":{"items":[{"metadata":{"title":"Effekter av klimaendringer i ferskvann : analyser av nasjonale overvåkingsdata",
        "creators":["Schartau, Ann Kristin","Pilotto, Francesca"],"originInfo":{"publisher":"Niva","issued":"2026"},
        "identifiers":{"isbn13":["9788242654946"],"issn":["15043312"]}}}]}}"#;
    const OL: &str = r#"{"ISBN:9780000000002":{"title":"Manual on harmful marine microalgae","subtitle":"second edition","authors":[{"name":"Gustaaf M. Hallegraeff"}],"publish_date":"January 1, 2003","publishers":[{"name":"UNESCO"}]}}"#;
    const NCBI: &str = r#"{"result":{"20231478":{"articleids":[{"idtype":"pubmed","value":"20231478"},{"idtype":"doi","value":"10.1073/pnas.0914859107"}]}}}"#;

    #[tokio::test]
    async fn arxiv_goes_straight_to_doi_org_and_crossref_404_falls_back_to_it() {
        let (base, seen) = serve(vec![(200, CSL.into()), (404, "Resource not found.".into()), (200, CSL.into())]);
        let (client, bases) = (crossref::http_client(), Bases::at(&base));
        let r = run(&client, &bases, &Step::Arxiv("1706.03762".into())).await.unwrap().unwrap();
        assert_eq!((r.source, r.doi.as_deref(), r.year), ("doi", Some("10.48550/arXiv.1706.03762"), Some(2017)));
        assert_eq!(r.authors, ["Ashish Vaswani", "Noam Shazeer"]);
        let r = run(&client, &bases, &Step::Doi("10.48550/arXiv.1706.03762".into())).await.unwrap().unwrap();
        assert_eq!(r.source, "doi");
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.to_lowercase().starts_with("get /doi/10.48550/arxiv.1706.03762 "), "{}", seen[0].0);
        assert!(seen[0].0.to_lowercase().contains("accept: application/vnd.citationstyles.csl+json"));
        assert!(seen[1].0.to_lowercase().starts_with("get /works/10.48550/"), "crossref asked first for a plain DOI");
    }

    #[tokio::test]
    async fn a_pubmed_id_gives_a_doi_that_is_then_looked_up() {
        let crossref_reply = r#"{"message":{"DOI":"10.1073/pnas.0914859107","title":["Lack of linguistic support"],"issued":{"date-parts":[[2010]]}}}"#;
        let (base, seen) = serve(vec![(200, NCBI.into()), (200, crossref_reply.into()), (200, r#"{"result":{"1":{"articleids":[]}}}"#.into())]);
        let (client, bases) = (crossref::http_client(), Bases::at(&base));
        let r = run(&client, &bases, &Step::Pmid("20231478".into())).await.unwrap().unwrap();
        assert_eq!((r.source, r.doi.as_deref()), ("crossref", Some("10.1073/pnas.0914859107")));
        assert!(run(&client, &bases, &Step::Pmid("1".into())).await.unwrap().is_none(), "no DOI, nothing to look up");
        let head = seen.lock().unwrap()[0].0.to_lowercase();
        assert!(head.starts_with("get /ncbi?") && head.contains("id=20231478") && head.contains("db=pubmed"), "{head}");
    }

    #[tokio::test]
    async fn the_national_library_gives_a_report_its_details() {
        let (base, seen) = serve(vec![(200, NB.into()), (200, r#"{"_embedded":{"items":[]}}"#.into())]);
        let (client, bases) = (crossref::http_client(), Bases::at(&base));
        let r = run(&client, &bases, &Step::Nb("9788242654946".into())).await.unwrap().unwrap();
        assert_eq!(r.source, "nb");
        assert_eq!(r.title, "Effekter av klimaendringer i ferskvann: analyser av nasjonale overvåkingsdata");
        assert_eq!(r.authors, ["Ann Kristin Schartau", "Francesca Pilotto"]);
        assert_eq!((r.year, r.publisher.as_deref(), r.issn.as_deref(), r.isbn.as_deref()), (Some(2026), Some("Niva"), Some("1504-3312"), Some("9788242654946")));
        assert!(run(&client, &bases, &Step::Nb("9788257768676".into())).await.unwrap().is_none());
        assert!(seen.lock().unwrap()[0].0.contains("q=isbn%3A9788242654946"), "{}", seen.lock().unwrap()[0].0);
    }

    #[tokio::test]
    async fn a_hit_for_another_isbn_is_not_used() {
        let (base, _) = serve(vec![(200, NB.into())]);
        let r = run(&crossref::http_client(), &Bases::at(&base), &Step::Nb("9788257768676".into())).await.unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn open_library_gives_title_year_and_publisher() {
        let (base, _) = serve(vec![(200, OL.into()), (200, "{}".into())]);
        let (client, bases) = (crossref::http_client(), Bases::at(&base));
        let r = run(&client, &bases, &Step::OpenLibrary("9780000000002".into())).await.unwrap().unwrap();
        assert_eq!(r.title, "Manual on harmful marine microalgae: second edition");
        assert_eq!((r.source, r.year, r.publisher.as_deref(), r.authors.len()), ("openlibrary", Some(2003), Some("UNESCO"), 1));
        assert!(run(&client, &bases, &Step::OpenLibrary("9780000000002".into())).await.unwrap().is_none());
    }

    #[test]
    fn a_search_hit_must_fit_title_year_and_author() {
        let hit = |title: &str, year: i32, authors: &[&str]| Record {
            source: "crossref",
            doi: Some("10.1/x".into()),
            title: title.into(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            year: Some(year),
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            publisher: None,
            issn: None,
            isbn: None,
        };
        let title = "Sub-lethal concentrations of the harmful diatoms, Chaetoceros concavicornis and C. convolutus, increase mortality rates of penned Pacific salmon";
        let good = hit(title, 1993, &["Albright L. J."]);
        let wanted = ["L. J. Albright".to_string()];
        assert!(crossref::search_hit_fits(&good, title, &wanted, Some(1993)));
        assert!(crossref::search_hit_fits(&good, title, &[], None));
        assert!(!crossref::search_hit_fits(&good, title, &wanted, Some(2005)), "year too far off");
        assert!(!crossref::search_hit_fits(&good, title, &["A. Someone".to_string()], Some(1993)), "other author");
        // the real near misses from the owner's library
        assert!(!crossref::search_hit_fits(&hit("Part 2: Protozoa, Myxozoa, Mesozoa", 2024, &[]), "Protozoa and Myxozoa", &[], None));
        assert!(!crossref::search_hit_fits(&hit("SoundToxins: A Research and Monitoring Partnership for Harmful Phytoplankton", 2023, &[]), "SoundToxins manual", &[], None));
        assert_eq!(first_surname(&wanted).as_deref(), Some("Albright"));
        assert_eq!(first_surname(&["Hallegraeff, Gustaaf M.".to_string()]).as_deref(), Some("Hallegraeff"));
        // an edition statement on one side is fine, other extra words are not
        let manual = ["Hallegraeff, Gustaaf M.".to_string()];
        assert!(crossref::search_hit_fits(&hit("Manual on Harmful Marine Microalgae. 2nd revised edition.", 2004, &["Hallegraeff, G.M."]), "Manual on Harmful Marine Microalgae", &manual, Some(2003)));
        assert!(!crossref::search_hit_fits(&hit("Manual on Harmful Marine Microalgae. Part 2: Toxins", 2004, &[]), "Manual on Harmful Marine Microalgae", &[], None));
        assert_eq!(crossref::without_edition("Manual on Harmful Marine Microalgae. 2nd revised edition."), "Manual on Harmful Marine Microalgae.");
        let by_family_first = hit("Manual on harmful marine microalgae", 2003, &["G. M. Hallegraeff"]);
        assert!(crossref::search_hit_fits(&by_family_first, "Manual on harmful marine microalgae", &["Hallegraeff, Gustaaf M.".to_string()], Some(2003)));
    }
    /// `cargo test live_title_search -- --ignored --nocapture`: the real Crossref on three papers from the
    /// test library. The first must be accepted, the other two must not.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn live_title_search() {
        let cases = [
            ("Sub-lethal concentrations of the harmful diatoms, Chaetoceros concavicornis and C. convolutus, increase mortality rates of penned Pacific salmon", Some("Albright"), Some(1993), true),
            ("Protozoa and Myxozoa", None, None, false),
            ("SoundToxins manual", None, None, false),
        ];
        // the Manual is not in Crossref: DataCite has it as a report with an edition suffix
        let manual = crossref::search_datacite(&crossref::http_client(), crossref::DATACITE_BASE, "Manual on Harmful Marine Microalgae").await.unwrap();
        let m = manual.iter().find(|r| r.doi.as_deref() == Some("10.25607/obp-1370")).expect("Ocean Best Practices record");
        assert!(crossref::search_hit_fits(m, "Manual on Harmful Marine Microalgae", &["Gustaaf M. Hallegraeff".to_string()], Some(2003)));
        let client = crossref::http_client();
        for (title, surname, year, accept) in cases {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let authors: Vec<String> = surname.map(|s| vec![format!("L. J. {s}")]).unwrap_or_default();
            let hits = crossref::search(&client, crossref::BASE, title, surname).await.unwrap();
            let ok = hits.first().is_some_and(|h| crossref::search_hit_fits(h, title, &authors, year) && crossref::title_in_text(&h.title, title));
            println!("{title:.50}: top hit {:?}, accepted {ok}", hits.first().map(|h| (&h.doi, h.year)));
            assert_eq!(ok, accept, "{title}");
        }
    }
    /// `cargo test live_identifiers -- --ignored --nocapture`: arXiv, PubMed and the National Library of Norway for real.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn live_identifiers() {
        let (client, bases) = (crossref::http_client(), Bases::live());
        let cases = [
            (Step::Arxiv("1706.03762".into()), "Attention Is All You Need"),
            (Step::Pmid("20231478".into()), "Lack of linguistic support for Proto-Uto-Aztecan at 8900 BP"),
            (Step::Nb("9788242654946".into()), "Effekter av klimaendringer i ferskvann"),
        ];
        for (step, title) in cases {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let r = run(&client, &bases, &step).await.unwrap().unwrap_or_else(|| panic!("{} unknown", step.label()));
            println!("{}: {:?} ({}, {:?}, {} authors)", step.label(), r.title, r.source, r.year, r.authors.len());
            assert!(crossref::title_in_text(&r.title, title) || crossref::title_in_text(title, &r.title), "{}", step.label());
        }
    }
}
