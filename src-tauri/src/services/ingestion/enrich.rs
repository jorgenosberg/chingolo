//! Adds details to a document after it is read, when the user allowed it: a lookup by identifier
//! (DOI, arXiv, PubMed, ISBN), then a model's reading of the first pages for what is still
//! missing, then, with its own opt-in, a Crossref search by title. One task works
//! through the documents in turn. Its own events skip the hook that queues documents, so it
//! never queues itself.

use std::collections::HashMap;

use tokio::sync::mpsc;

use super::activity::Activity;
use super::details_ai::{self, Missing, Target};
use super::lookup::{self, Bases};
use super::{crossref, pdf::current_year};
use super::queue::{DocumentChanged, Emit};
use crate::domain::Id;
use crate::domain::library::Document;
use crate::error::Result;
use crate::persistence::usage::{self, NewUsage};
use crate::persistence::{Db, library as repo, settings};
use crate::security;
use crate::services::ai::Providers;

/// Looks papers up online by identifier. The key keeps its old name: it began as the Crossref switch.
pub const CROSSREF_KEY: &str = "crossref_enabled";
/// Searches Crossref by title for papers with no identifier. Sends the title and first author's surname.
pub const TITLE_KEY: &str = "title_search_enabled";

#[derive(Clone)]
pub struct Enricher(mpsc::UnboundedSender<Id>, Activity);

/// A lookup that failed is tried once more after this long. After that it waits for the next start.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(45);

impl Enricher {
    pub fn start(db: Db, emit: Emit, providers: Providers, activity: Activity) -> Self {
        let busy = activity.clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<Id>();
        let again = Enricher(tx.clone(), activity.clone());
        tauri::async_runtime::spawn(async move {
            let client = crossref::http_client();
            let bases = Bases::live();
            let mut tries: HashMap<Id, u8> = HashMap::new();
            while let Some(id) = rx.recv().await {
                let target = match ai_target(&db, &providers).await {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!("details model unavailable: {e}");
                        None
                    }
                };
                match enrich(&db, &client, &bases, target.as_ref(), &id).await {
                    Ok(out) => {
                        if let Some(document) = out.document {
                            emit(DocumentChanged { document, progress: None });
                        }
                        if out.failed {
                            let n = tries.entry(id.clone()).or_default();
                            *n += 1;
                            if *n < 2 {
                                log::info!("details for document {id} will be tried again in {} s", RETRY_AFTER.as_secs());
                                let again = again.clone();
                                let id = id.clone();
                                tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(RETRY_AFTER).await;
                                    again.queue(&id);
                                });
                            }
                        } else {
                            tries.remove(&id);
                        }
                    }
                    Err(e) => log::debug!("details lookup for document {id} ended early: {e}"),
                }
                busy.finish(&id);
            }
        });
        Enricher(tx, activity)
    }

    pub fn queue(&self, id: &str) {
        self.1.start(id);
        let _ = self.0.send(id.to_string());
    }

    /// Queues the ready documents whose lookups never finished, or with `all` every ready one
    /// (a lookup was just turned on). Does nothing while both lookups are off. Returns how many.
    pub async fn resume(&self, db: &Db, all: bool) -> Result<usize> {
        let ids = db
            .call(move |c| {
                let on = settings::get_bool(c, CROSSREF_KEY)? || settings::get_bool(c, TITLE_KEY)? || settings::get_bool(c, details_ai::ENABLED_KEY)?;
                if on { repo::unenriched(c, all) } else { Ok(vec![]) }
            })
            .await?;
        for id in &ids {
            self.queue(id);
        }
        if !ids.is_empty() {
            log::info!("details: {} documents are still waiting for a lookup", ids.len());
        }
        Ok(ids.len())
    }
}

/// The model for the details pass: the one chosen in Settings, else the default chat model.
/// `None` when the pass is off or no key is saved for it.
async fn ai_target(db: &Db, providers: &Providers) -> Result<Option<Target>> {
    let (on, model, provider, default_model, default_provider) = db
        .call(|c| {
            let get = |k: &str| settings::get(c, k).map(|v| v.filter(|s| !s.is_empty()));
            Ok((settings::get_bool(c, details_ai::ENABLED_KEY)?, get(details_ai::MODEL_KEY)?, get(details_ai::PROVIDER_KEY)?, get("default_model")?, get("default_provider")?))
        })
        .await?;
    if !on {
        return Ok(None);
    }
    let (provider_id, model) = match (provider, model, default_provider, default_model) {
        (Some(p), Some(m), _, _) | (_, _, Some(p), Some(m)) => (p, m),
        _ => return Ok(None),
    };
    let provider = providers.get(&provider_id)?;
    let Some(api_key) = security::get_key(provider.id()).await? else { return Ok(None) };
    Ok(Some(Target { provider, api_key, model }))
}

/// What one pass over a document did.
pub struct Outcome {
    /// The document as it is now, when a lookup ran.
    pub document: Option<Document>,
    /// A lookup failed, so the document is marked and tried again later.
    pub failed: bool,
}

/// Everything that can be added to a document: identifier lookups first, then the model for what
/// is still missing, then a title search if nothing matched by identifier. A failed step is
/// recorded on the document and doesn't stop the next one. With every lookup off nothing is
/// recorded, so turning one on later picks the document up.
pub async fn enrich(db: &Db, client: &reqwest::Client, bases: &Bases, ai: Option<&Target>, id: &str) -> Result<Outcome> {
    log::debug!("details: document {id} queued (model pass {})", if ai.is_some() { "on" } else { "off" });
    let (ident_on, title_on) = db.call(|c| Ok((settings::get_bool(c, CROSSREF_KEY)?, settings::get_bool(c, TITLE_KEY)?))).await?;
    if !ident_on && !title_on && ai.is_none() {
        return Ok(Outcome { document: None, failed: false });
    }
    let mut problems: Vec<&str> = vec![];
    let matched = match lookup_step(db, client, bases, id).await {
        Ok(found) => found.is_some(),
        Err(e) => {
            log::warn!("Paper lookup failed for document {id}: {e}");
            problems.push("An online lookup couldn't be reached.");
            false
        }
    };
    if let Some(t) = ai
        && ai_step(db, t, id).await.is_err()
    {
        problems.push("The details model didn't answer.");
    }
    if !matched
        && title_on
        && let Err(e) = title_step(db, client, bases, id).await
    {
        log::warn!("Title search failed for document {id}: {e}");
        problems.push("Crossref couldn't be reached.");
    }
    let note = (!problems.is_empty()).then(|| problems.join(" "));
    let id = id.to_string();
    let document = db
        .call(move |c| {
            repo::mark_enriched(c, &id, note.as_deref())?;
            repo::get_document(c, &id)
        })
        .await?;
    let failed = document.enrich_error.is_some();
    Ok(Outcome { document: Some(document), failed })
}

/// Asks the model about a document whose title, authors or year is still missing, or a report with
/// no ISBN or publisher (then excerpts from later pages go along), and stores what the text backs up. Every call is recorded as usage of kind `metadata`.
async fn ai_step(db: &Db, target: &Target, id: &str) -> Result<Option<Document>> {
    let id = id.to_string();
    let (doc, head, later) = db
        .call({
            let id = id.clone();
            move |c| {
                let doc = repo::get_document(c, &id)?;
                let later = repo::imprint_passages(c, &id, doc.page_count)?;
                Ok((doc, repo::head_text(c, &id, 4)?, later))
            }
        })
        .await?;
    let want = Missing::of(&doc);
    if !want.worth_asking(&doc) || head.trim().is_empty() {
        log::debug!("details model: nothing to ask for {}", doc.original_name);
        return Ok(None);
    }
    log::info!("details model {}: asking about {} (title {}, authors {}, year {} missing)", target.model, doc.original_name, want.title, want.authors, want.year);
    let imprint = if want.imprint_wanted(&doc) { details_ai::imprint_snippets(&later) } else { String::new() };
    let asked = details_ai::ask(target, &doc, &head, &imprint, &want, current_year()).await;
    let (tokens, latency_ms, ok) = match &asked {
        Ok(a) => (a.tokens, a.latency_ms, true),
        Err(_) => ((0, 0, 0), 0, false),
    };
    let (provider, model) = (target.provider.id(), target.model.clone());
    let recorded = db
        .call(move |c| {
            usage::record(
                c,
                &NewUsage {
                    provider,
                    model: &model,
                    kind: "metadata",
                    input_tokens: tokens.0,
                    output_tokens: tokens.1,
                    cached_tokens: tokens.2,
                    estimated: false,
                    latency_ms,
                    ok,
                    conversation_id: None,
                },
            )
        })
        .await;
    if let Err(e) = recorded {
        log::warn!("couldn't record details usage: {e}");
    }
    let asked = asked.inspect_err(|e| log::warn!("details model {} failed for {}: {e}", target.model, doc.original_name))?;
    log::info!(
        "details model {}: {} tokens in, {} out, {} ms, kept {}",
        target.model,
        tokens.0,
        tokens.1,
        latency_ms,
        if asked.found.fields().is_empty() { "nothing".to_string() } else { asked.found.fields().join(", ") }
    );
    if !asked.dropped.is_empty() {
        log::info!("details model {}: {} for {} not backed by the text, dropped", target.model, asked.dropped.join(", "), doc.original_name);
    }
    let found = asked.found;
    if found.title.is_none() && found.authors.is_empty() && found.year.is_none() && found.series.is_none() && found.publisher.is_none() && found.isbn.is_none() && found.issn.is_none() {
        return Ok(None);
    }
    let updated = db
        .call(move |c| {
            repo::apply_metadata(c, &id, &found, "ai", false)?;
            repo::get_document(c, &id)
        })
        .await?;
    Ok(Some(updated))
}

/// Tries every identifier the document carries and stores the first record whose title is in the
/// document's text. `Ok(None)` when nothing matched. A service that can't be reached doesn't stop
/// the others, but if nothing matched the failure is returned so the document is tried again.
async fn lookup_step(db: &Db, client: &reqwest::Client, bases: &Bases, id: &str) -> Result<Option<Document>> {
    let id = id.to_string();
    let job = db
        .call({
            let id = id.clone();
            move |c| {
                if !settings::get_bool(c, CROSSREF_KEY)? {
                    log::debug!("Online lookup by identifier is off");
                    return Ok(None);
                }
                let doc = repo::get_document(c, &id)?;
                let head = repo::head_text(c, &id, 4)?;
                Ok(Some((doc, head)))
            }
        })
        .await?;
    let Some((doc, head)) = job else { return Ok(None) };

    let mut failure = None;
    for step in lookup::steps(doc.doi.as_deref(), doc.isbn.as_deref(), &doc.original_name, &head) {
        let record = match lookup::run(client, bases, &step).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                log::info!("lookup: {} is unknown ({})", step.label(), doc.original_name);
                continue;
            }
            Err(e) => {
                log::warn!("lookup: {} failed ({}): {e}", step.label(), doc.original_name);
                failure.get_or_insert(e);
                continue;
            }
        };
        if !crossref::title_in_text(&record.title, &head) {
            log::info!("lookup: record for {} is a different work than {}, ignored", step.label(), doc.original_name);
            continue;
        }
        let mut found = crossref::to_extracted(&record, doc.publisher.is_some());
        // another edition's date must not replace the year the document itself gives
        if record.source == "openlibrary" && doc.year.is_some() {
            found.year = None;
        }
        log::info!("lookup: {} matches {}, adding {} from {}", step.label(), doc.original_name, found.fields().join(", "), record.source);
        let source = record.source;
        let updated = db
            .call(move |c| {
                repo::apply_metadata(c, &id, &found, source, false)?;
                repo::get_document(c, &id)
            })
            .await?;
        return Ok(Some(updated));
    }
    match failure {
        Some(e) => Err(e),
        None => {
            log::debug!("lookup: no usable record for {}", doc.original_name);
            Ok(None)
        }
    }
}

/// For a document with a title but no identifier that matched: a Crossref search by title and
/// first author, then a DataCite search by title. The top hit is used only if it fits the document
/// closely (`search_hit_fits`) and its title is in the text.
async fn title_step(db: &Db, client: &reqwest::Client, bases: &Bases, id: &str) -> Result<Option<Document>> {
    let id = id.to_string();
    let job = db
        .call({
            let id = id.clone();
            move |c| {
                if !settings::get_bool(c, TITLE_KEY)? {
                    return Ok(None);
                }
                Ok(Some((repo::get_document(c, &id)?, repo::head_text(c, &id, 4)?)))
            }
        })
        .await?;
    let Some((doc, head)) = job else { return Ok(None) };
    // a title of one or two words finds too many wrong works to be worth sending
    let Some(title) = doc.title.clone().filter(|t| t.split_whitespace().count() >= 3) else {
        log::debug!("title search: {} has no usable title", doc.original_name);
        return Ok(None);
    };
    let surname = lookup::first_surname(&doc.authors);
    let fits = |r: &crossref::Record| crossref::search_hit_fits(r, &title, &doc.authors, doc.year) && crossref::title_in_text(&crossref::without_edition(&r.title), &head);
    let hits = crossref::search(client, &bases.crossref, &title, surname.as_deref()).await?;
    let mut top = hits.into_iter().next().filter(|r| fits(r));
    if top.is_none() {
        // Reports and other repository records are in DataCite, not Crossref. A failure here is only
        // logged: Crossref answered, and a flaky second service must not flag the document.
        match crossref::search_datacite(client, &bases.datacite, &title).await {
            // a record with no creators can't be checked against the document's authors
            Ok(hits) => top = hits.into_iter().next().filter(|r| !r.authors.is_empty() && fits(r)),
            Err(e) => log::warn!("title search: DataCite failed for {}: {e}", doc.original_name),
        }
    }
    let Some(top) = top else {
        log::info!("title search: no fitting record for {}", doc.original_name);
        return Ok(None);
    };
    let mut found = crossref::to_extracted(&top, doc.publisher.is_some());
    if top.source == "datacite" {
        // Repository titles carry edition text ("... 2nd revised edition.") and creator lists are cut
        // short ("et al"), so they only fill a title or authors the document doesn't have.
        if doc.title.is_some() {
            found.title = None;
        }
        if !doc.authors.is_empty() {
            found.authors.clear();
        }
    }
    log::info!("title search: {} matched, adding {}", doc.original_name, found.fields().join(", "));
    let updated = db
        .call(move |c| {
            repo::apply_metadata(c, &id, &found, top.source, false)?;
            repo::get_document(c, &id)
        })
        .await?;
    Ok(Some(updated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::library::{DocKind, DocStatus};
    use crate::persistence::library::{Extracted, NewPage, NewPassage};
    use crate::services::testserver::serve;

    const RECORD: &str = r#"{"message":{"DOI":"10.1038/nature02437","title":["The decline and fate of an iron-induced subarctic phytoplankton bloom"],
        "author":[{"given":"P. W.","family":"BOYD"},{"given":"C. S.","family":"Law"}],"issued":{"date-parts":[[2004]]},"container-title":["Nature"],"volume":"428","page":"549-553"}}"#;

    async fn setup(enabled: bool, file: &str, text: &str, doi: Option<&str>) -> (Db, String, tempfile::TempDir) {
        let meta = Extracted { doi: doi.map(String::from), title: Some("Read off the page".into()), ..Default::default() };
        setup_with(enabled, false, file, text, meta).await
    }

    async fn setup_with(enabled: bool, title_search: bool, file: &str, text: &str, meta: Extracted) -> (Db, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        let (file, text) = (file.to_string(), text.to_string());
        let id = db
            .call(move |c| {
                settings::set_bool(c, CROSSREF_KEY, enabled)?;
                settings::set_bool(c, TITLE_KEY, title_search)?;
                let d = repo::insert_document(c, "h1", &file, DocKind::Pdf)?;
                let pages = [NewPage { page_no: 1, width: 1.0, height: 1.0, quality: "ok", ocr: false }];
                let passages = [NewPassage { page_start: 1, page_end: 1, text, boxes: None }];
                let mut conn = rusqlite::Connection::open(c.path().unwrap())?;
                repo::store_content(&mut conn, &d.id, &pages, &passages, &meta, DocStatus::Ready, None)?;
                Ok(d.id)
            })
            .await
            .unwrap();
        (db, id, dir)
    }

    const PAGE: &str = "The decline and fate of an iron-induced subarctic phytoplankton bloom Philip W. Boyd et al.";

    #[tokio::test]
    async fn a_matching_record_fills_the_details_through_the_file_name() {
        let (db, id, _dir) = setup(true, "nature02437.pdf", PAGE, None).await;
        let (base, seen) = serve(vec![(200, RECORD.into())]);
        let doc = lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().unwrap();
        assert_eq!(doc.doi.as_deref(), Some("10.1038/nature02437"));
        assert_eq!(doc.authors, ["P. W. Boyd", "C. S. Law"]);
        assert_eq!((doc.year, doc.journal.as_deref(), doc.volume.as_deref()), (Some(2004), Some("Nature"), Some("428")));
        assert_eq!(doc.meta_source.get("year").map(String::as_str), Some("crossref"));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_record_for_another_article_is_ignored() {
        let (db, id, _dir) = setup(true, "x.pdf", "Seismic structure of the Cascadia subduction zone", Some("10.1038/nature02437")).await;
        let (base, _) = serve(vec![(200, RECORD.into())]);
        assert!(lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().is_none());
        let doc = db.call(move |c| repo::get_document(c, &id)).await.unwrap();
        assert_eq!(doc.title.as_deref(), Some("Read off the page"));
    }

    #[tokio::test]
    async fn nothing_is_sent_unless_the_user_allowed_it() {
        let (db, id, _dir) = setup(false, "nature02437.pdf", PAGE, Some("10.1038/nature02437")).await;
        let (base, seen) = serve(vec![(200, RECORD.into())]);
        assert!(lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().is_none());
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn edited_fields_stay_and_a_failed_lookup_changes_nothing() {
        let (db, id, _dir) = setup(true, "nature02437.pdf", PAGE, None).await;
        let edit = id.clone();
        db.call(move |c| {
            repo::update_metadata(c, &edit, &crate::domain::library::MetadataPatch { title: Some("My own title".into()), ..Default::default() })
        })
        .await
        .unwrap();
        let (base, _) = serve(vec![(200, RECORD.into())]);
        let doc = lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().unwrap();
        assert_eq!((doc.title.as_deref(), doc.year), (Some("My own title"), Some(2004)));
        let (down, _) = serve(vec![(503, "busy".into())]);
        assert!(lookup_step(&db, &crossref::http_client(), &Bases::at(&down), &id).await.is_err());
    }

    #[tokio::test]
    async fn a_finished_lookup_is_recorded_and_a_failed_one_is_marked_for_another_try() {
        let (db, id, _dir) = setup(true, "nature02437.pdf", PAGE, None).await;
        let (down, _) = serve(vec![(503, "busy".into())]);
        let out = enrich(&db, &crossref::http_client(), &Bases::at(&down), None, &id).await.unwrap();
        assert!(out.failed);
        assert_eq!(out.document.unwrap().enrich_error.as_deref(), Some("An online lookup couldn't be reached."));
        let waiting = db.call(|c| repo::unenriched(c, false)).await.unwrap();
        assert_eq!(waiting, [id.clone()]);

        let (base, _) = serve(vec![(200, RECORD.into())]);
        let out = enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap();
        assert!(!out.failed);
        assert_eq!(out.document.unwrap().enrich_error, None);
        assert!(db.call(|c| repo::unenriched(c, false)).await.unwrap().is_empty());
        assert_eq!(db.call(|c| repo::unenriched(c, true)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn with_both_lookups_off_nothing_is_recorded() {
        let (db, id, _dir) = setup(false, "nature02437.pdf", PAGE, None).await;
        let out = enrich(&db, &crossref::http_client(), &Bases::at("http://unused"), None, &id).await.unwrap();
        assert!(out.document.is_none() && !out.failed);
        assert_eq!(db.call(|c| repo::unenriched(c, false)).await.unwrap(), [id]);
    }

    const NB_RECORD: &str = r#"{"_embedded":{"items":[{"metadata":{"title":"Effekter av klimaendringer i ferskvann : analyser av nasjonale overvåkingsdata",
        "creators":["Schartau, Ann Kristin"],"originInfo":{"publisher":"Niva","issued":"2026"},"identifiers":{"isbn13":["9788242654946"],"issn":["15043312"]}}}]}}"#;
    const ISBN_REPORT: &str = "Effekter av klimaendringer i ferskvann: analyser av nasjonale overvåkingsdata NINA Rapport 2669. ISBN: 978-82-426-5494-6";

    #[tokio::test]
    async fn a_report_is_found_by_its_isbn_and_the_source_is_kept_on_a_re_read() {
        let meta = Extracted { title: Some("Effekter av klimaendringer".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(true, false, "ninarapport2669.pdf", ISBN_REPORT, meta).await;
        let (base, seen) = serve(vec![(200, NB_RECORD.into())]);
        let doc = lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().unwrap();
        assert_eq!(doc.title.as_deref(), Some("Effekter av klimaendringer i ferskvann: analyser av nasjonale overvåkingsdata"));
        assert_eq!((doc.authors.clone(), doc.year, doc.publisher.as_deref()), (vec!["Ann Kristin Schartau".to_string()], Some(2026), Some("Niva")));
        assert_eq!((doc.isbn.as_deref(), doc.issn.as_deref()), (Some("9788242654946"), Some("1504-3312")));
        assert_eq!(doc.meta_source.get("title").map(String::as_str), Some("nb"));
        assert_eq!(seen.lock().unwrap().len(), 1);
        // reading the file again must not undo what the library catalogue gave
        let again = id.clone();
        let doc = db
            .call(move |c| {
                repo::apply_metadata(c, &again, &Extracted { title: Some("Page title".into()), ..Default::default() }, "file", true)?;
                repo::get_document(c, &again)
            })
            .await
            .unwrap();
        assert_eq!(doc.title.as_deref(), Some("Effekter av klimaendringer i ferskvann: analyser av nasjonale overvåkingsdata"));
    }

    #[tokio::test]
    async fn an_arxiv_paper_gets_its_details_and_a_doi() {
        let csl = r#"{"title":"Attention Is All You Need","author":[{"given":"Ashish","family":"Vaswani"}],"issued":{"date-parts":[[2017]]},"publisher":"arXiv"}"#;
        let meta = Extracted { title: Some("Attention Is All You Need".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(true, false, "1706.03762v5.pdf", "Attention Is All You Need Ashish Vaswani arXiv:1706.03762v5 [cs.CL]", meta).await;
        let (base, seen) = serve(vec![(200, csl.into())]);
        let doc = lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().unwrap();
        assert_eq!((doc.doi.as_deref(), doc.year), (Some("10.48550/arXiv.1706.03762"), Some(2017)));
        assert_eq!(doc.meta_source.get("doi").map(String::as_str), Some("doi"));
        assert!(seen.lock().unwrap()[0].0.starts_with("GET /doi/10.48550/arXiv.1706.03762 "), "straight to doi.org, no Crossref call");
    }

    #[tokio::test]
    async fn another_editions_year_does_not_replace_the_documents_own() {
        let ol = r#"{"ISBN:9789231039485":{"title":"Manual on harmful marine microalgae","publish_date":"2003","publishers":[{"name":"UNESCO"}]}}"#;
        let meta = Extracted { title: Some("Manual".into()), year: Some(2004), isbn: Some("92-3-103948-2".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(true, false, "manual.pdf", "Manual on harmful marine microalgae. ISBN 92-3-103948-2 © UNESCO 2004", meta).await;
        let (base, _) = serve(vec![(200, ol.into())]);
        let doc = lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.unwrap().unwrap();
        assert_eq!(doc.year, Some(2004));
        assert_eq!(doc.meta_source.get("title").map(String::as_str), Some("openlibrary"));
    }

    #[tokio::test]
    async fn one_service_being_down_does_not_stop_the_next() {
        let meta = Extracted { title: Some("x".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(true, false, "ninarapport2669.pdf", ISBN_REPORT, meta).await;
        // the National Library answers 503, Open Library knows nothing: the failure is reported so it is tried again
        let (base, seen) = serve(vec![(503, "busy".into()), (200, "{}".into())]);
        assert!(lookup_step(&db, &crossref::http_client(), &Bases::at(&base), &id).await.is_err());
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    const HIT: &str = r#"{"message":{"items":[{"DOI":"10.1016/0044-8486(93)90321-o","title":["Sub-lethal concentrations of the harmful diatoms, Chaetoceros concavicornis and C. convolutus, increase mortality rates of penned Pacific salmon"],
        "author":[{"given":"L. J.","family":"Albright"}],"issued":{"date-parts":[[1993]]},"container-title":["Aquaculture"],"volume":"117"}]}}"#;
    const TITLE: &str = "Sub-lethal concentrations of the harmful diatoms, Chaetoceros concavicornis and C. convolutus, increase mortality rates of penned Pacific salmon";

    #[tokio::test]
    async fn a_title_search_fills_a_paper_with_no_identifier_when_the_hit_fits() {
        let meta = Extracted { title: Some(TITLE.into()), authors: vec!["L. J. Albright".into()], year: Some(1993), ..Default::default() };
        let (db, id, _dir) = setup_with(false, true, "0044.pdf", &format!("{TITLE} L. J. Albright"), meta).await;
        let (base, seen) = serve(vec![(200, HIT.into())]);
        let out = enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap();
        let doc = out.document.unwrap();
        assert!(!out.failed);
        assert_eq!((doc.doi.as_deref(), doc.journal.as_deref()), (Some("10.1016/0044-8486(93)90321-o"), Some("Aquaculture")));
        let head = seen.lock().unwrap()[0].0.clone();
        assert!(head.starts_with("GET /works?") && head.contains("query.bibliographic=") && head.contains("query.author=Albright"), "{head}");
    }

    #[tokio::test]
    async fn a_title_search_falls_back_to_datacite_for_a_report_with_an_edition_suffix() {
        let datacite = r#"{"data":[{"attributes":{"doi":"10.25607/obp-1370","titles":[{"title":"Manual on Harmful Marine Microalgae. 2nd revised edition. "}],
            "creators":[{"name":"Hallegraeff, G.M.","nameType":"Personal"},{"name":"et al"}],"publicationYear":2004,"publisher":"Unesco"}}]}"#;
        let meta = Extracted { title: Some("Manual on Harmful Marine Microalgae".into()), authors: vec!["Gustaaf M. Hallegraeff".into()], year: Some(2003), ..Default::default() };
        let (db, id, _dir) = setup_with(false, true, "manual.pdf", "Manual on Harmful Marine Microalgae Second revised edition Hallegraeff", meta).await;
        let (base, seen) = serve(vec![(200, r#"{"message":{"items":[]}}"#.into()), (200, datacite.into())]);
        let doc = enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap().document.unwrap();
        assert_eq!((doc.doi.as_deref(), doc.year, doc.publisher.as_deref()), (Some("10.25607/obp-1370"), Some(2004), Some("Unesco")));
        assert_eq!(doc.meta_source.get("doi").map(String::as_str), Some("datacite"));
        assert_eq!((doc.title.as_deref(), doc.authors.as_slice()), (Some("Manual on Harmful Marine Microalgae"), ["Gustaaf M. Hallegraeff".to_string()].as_slice()), "the file's title and authors stay");
        assert_eq!(doc.meta_source.get("title").map(String::as_str), Some("file"));
        assert!(seen.lock().unwrap()[1].0.starts_with("GET /datacite?"), "{:?}", seen.lock().unwrap()[1].0);
    }

    #[tokio::test]
    async fn a_datacite_failure_does_not_flag_the_document() {
        let meta = Extracted { title: Some("Manual on Harmful Marine Microalgae".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(false, true, "manual.pdf", "Manual on Harmful Marine Microalgae", meta).await;
        let (base, _) = serve(vec![(200, r#"{"message":{"items":[]}}"#.into()), (500, "{}".into())]);
        let out = enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap();
        assert!(!out.failed);
    }

    #[tokio::test]
    async fn a_title_search_hit_for_another_work_is_ignored() {
        let wrong = r#"{"message":{"items":[{"DOI":"10.1/x","title":["Part 2: Protozoa, Myxozoa, Mesozoa"],"issued":{"date-parts":[[2024]]}}]}}"#;
        let meta = Extracted { title: Some("Protozoa and Myxozoa Part".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(false, true, "x.pdf", "Protozoa and Myxozoa Part 2: Protozoa, Myxozoa, Mesozoa", meta).await;
        let (base, _) = serve(vec![(200, wrong.into())]);
        let doc = enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap().document.unwrap();
        assert_eq!(doc.doi, None);
    }

    #[tokio::test]
    async fn the_title_search_is_skipped_when_an_identifier_matched_or_it_is_off() {
        let meta = Extracted { title: Some(TITLE.into()), doi: Some("10.1038/nature02437".into()), ..Default::default() };
        let (db, id, _dir) = setup_with(true, true, "nature02437.pdf", PAGE, meta.clone()).await;
        let (base, seen) = serve(vec![(200, RECORD.into()), (200, HIT.into())]);
        enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1, "the DOI matched, so no search");

        let (db, id, _dir) = setup_with(false, false, "0044.pdf", TITLE, Extracted { doi: None, ..meta }).await;
        let (base, seen) = serve(vec![(200, HIT.into())]);
        let out = enrich(&db, &crossref::http_client(), &Bases::at(&base), None, &id).await.unwrap();
        assert!(out.document.is_none());
        assert!(seen.lock().unwrap().is_empty(), "nothing is sent while the switches are off");
    }

    struct Canned(&'static str);

    impl crate::services::ai::Provider for Canned {
        fn id(&self) -> &'static str {
            "canned"
        }
        fn capabilities(&self) -> crate::services::ai::Capabilities {
            crate::services::ai::Capabilities { tools: false, implicit_caching: false, vision: false }
        }
        fn list_models<'a>(&'a self, _: &'a str) -> futures_util::future::BoxFuture<'a, Result<Vec<crate::domain::ModelInfo>>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn generate(&self, _key: String, _request: crate::services::ai::GenerationRequest) -> crate::services::ai::EventStream {
            let events = vec![
                crate::services::ai::GenerationEvent::Text(self.0.to_string()),
                crate::services::ai::GenerationEvent::Usage { input_tokens: 2000, output_tokens: 60, cached_tokens: 0 },
            ];
            Box::pin(futures_util::stream::iter(events.into_iter().map(Ok)))
        }
    }

    fn target(reply: &'static str) -> Target {
        Target { provider: std::sync::Arc::new(Canned(reply)), api_key: "k".into(), model: "small".into() }
    }

    const REPORT: &str = "Miljøtilstanden i Mjøsa med tilløpselver 2016 Forfattere Jarl Eivind Løvik Anne Lyche Solheim Utgitt i mars 2017";

    async fn usage_kinds(db: &Db) -> Vec<(String, i64)> {
        db.call(|c| {
            let mut stmt = c.prepare("SELECT kind, input_tokens FROM api_usage")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_model_fills_what_the_text_backs_up_and_is_recorded() {
        let (db, id, _dir) = setup(false, "r.pdf", REPORT, None).await;
        db.call({
            let id = id.clone();
            move |c| repo::update_metadata(c, &id, &crate::domain::library::MetadataPatch { title: Some(String::new()), ..Default::default() })
        })
        .await
        .unwrap();
        let reply = r#"{"title": "Miljøtilstanden i Mjøsa med tilløpselver 2016", "authors": ["Jarl Eivind Løvik", "Kari Nordmann"], "year": 2017}"#;
        let doc = ai_step(&db, &target(reply), &id).await.unwrap().unwrap();
        // the title was cleared by the user, so it counts as edited and stays empty
        assert_eq!(doc.title, None);
        assert_eq!((doc.authors.as_slice(), doc.year), (&["Jarl Eivind Løvik".to_string()][..], Some(2017)));
        assert_eq!(doc.meta_source.get("year").map(String::as_str), Some("ai"));
        assert_eq!(usage_kinds(&db).await, [("metadata".to_string(), 2000)]);
    }

    #[tokio::test]
    async fn a_made_up_reply_changes_nothing_but_is_still_recorded() {
        let (db, id, _dir) = setup(false, "r.pdf", REPORT, None).await;
        let before = db.call({ let id = id.clone(); move |c| repo::get_document(c, &id) }).await.unwrap();
        let reply = r#"{"title": "Something else entirely about whales", "authors": ["Nobody Here"], "year": 1850}"#;
        assert!(ai_step(&db, &target(reply), &id).await.unwrap().is_none());
        let after = db.call({ let id = id.clone(); move |c| repo::get_document(c, &id) }).await.unwrap();
        assert_eq!((before.title, before.authors, before.year), (after.title, after.authors, after.year));
        assert_eq!(usage_kinds(&db).await.len(), 1);
    }

    #[tokio::test]
    async fn a_document_with_title_authors_and_year_is_not_sent() {
        let (db, id, _dir) = setup(false, "r.pdf", REPORT, None).await;
        db.call({
            let id = id.clone();
            move |c| {
                repo::apply_metadata(
                    c,
                    &id,
                    &Extracted { title: Some("T".into()), authors: vec!["A B".into()], year: Some(2000), isbn: Some("978-82-577-6867-6".into()), publisher: Some("NIVA".into()), ..Default::default() },
                    "file",
                    true,
                )
            }
        })
        .await
        .unwrap();
        assert!(ai_step(&db, &target("{}"), &id).await.unwrap().is_none());
        assert!(usage_kinds(&db).await.is_empty());
    }

    /// A report with title, authors and year but no ISBN or publisher: the copyright page is on page 6.
    async fn report_with_a_later_imprint(journal: Option<&str>) -> (Db, String, tempfile::TempDir) {
        let (db, id, dir) = setup(false, "m.pdf", REPORT, None).await;
        db.call({
            let id = id.clone();
            let journal = journal.map(String::from);
            move |c| {
                repo::apply_metadata(c, &id, &Extracted { title: Some("T".into()), authors: vec!["A B".into()], year: Some(2000), journal, ..Default::default() }, "file", true)?;
                c.execute(
                    "INSERT INTO passages (id, document_id, ordinal, page_start, page_end, text, text_folded) VALUES ('p6', ?1, 9, 6, 6, ?2, ?2)",
                    rusqlite::params![id, "Second revised edition. First published in 2003. ISBN 92-3-103948-2 © UNESCO Publishing 2004"],
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();
        (db, id, dir)
    }

    #[tokio::test]
    async fn a_report_without_an_imprint_gets_it_from_later_pages() {
        let (db, id, _dir) = report_with_a_later_imprint(None).await;
        let doc = ai_step(&db, &target(r#"{"isbn": "92-3-103948-2", "publisher": "UNESCO Publishing"}"#), &id).await.unwrap().unwrap();
        assert_eq!((doc.isbn.as_deref(), doc.publisher.as_deref()), (Some("92-3-103948-2"), Some("UNESCO Publishing")));
        assert_eq!(doc.meta_source.get("isbn").map(String::as_str), Some("ai"));
    }

    #[tokio::test]
    async fn an_article_is_not_sent_for_its_imprint() {
        let (db, id, _dir) = report_with_a_later_imprint(Some("Nature")).await;
        assert!(ai_step(&db, &target("{}"), &id).await.unwrap().is_none());
        assert!(usage_kinds(&db).await.is_empty());
    }
}
