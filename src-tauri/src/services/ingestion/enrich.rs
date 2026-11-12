//! Adds details to a document after it is read, when the user allowed it: a Crossref lookup by
//! DOI, then a model's reading of the first pages for what is still missing. One task works
//! through the documents in turn. Its own events skip the hook that queues documents, so it
//! never queues itself.

use tokio::sync::mpsc;

use super::details_ai::{self, Missing, Target};
use super::{crossref, pdf::current_year};
use super::queue::{DocumentChanged, Emit};
use crate::domain::Id;
use crate::domain::library::Document;
use crate::error::Result;
use crate::persistence::usage::{self, NewUsage};
use crate::persistence::{Db, library as repo, settings};
use crate::security;
use crate::services::ai::Providers;

pub const CROSSREF_KEY: &str = "crossref_enabled";

#[derive(Clone)]
pub struct Enricher(mpsc::UnboundedSender<Id>);

impl Enricher {
    pub fn start(db: Db, emit: Emit, providers: Providers) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Id>();
        tauri::async_runtime::spawn(async move {
            let client = crossref::http_client();
            while let Some(id) = rx.recv().await {
                let target = match ai_target(&db, &providers).await {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!("details model unavailable: {e}");
                        None
                    }
                };
                match enrich(&db, &client, crossref::BASE, target.as_ref(), &id).await {
                    Ok(Some(document)) => emit(DocumentChanged { document, progress: None }),
                    Ok(None) => {}
                    Err(e) => log::warn!("details lookup failed: {e}"),
                }
            }
        });
        Enricher(tx)
    }

    pub fn queue(&self, id: &str) {
        let _ = self.0.send(id.to_string());
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

/// Everything that can be added to a document: Crossref first, then the model for what is still
/// missing. `Ok(None)` when nothing changed. A failed lookup is logged, so the model still runs.
pub async fn enrich(db: &Db, client: &reqwest::Client, base: &str, ai: Option<&Target>, id: &str) -> Result<Option<Document>> {
    let from_crossref = crossref_step(db, client, base, id).await.unwrap_or_else(|e| {
        log::warn!("Crossref lookup failed: {e}");
        None
    });
    let from_model = match ai {
        Some(t) => ai_step(db, t, id).await?,
        None => None,
    };
    Ok(from_model.or(from_crossref))
}

/// Asks the model about a document whose title, authors or year is still missing and stores what
/// the text backs up. Every call is recorded as usage of kind `metadata`.
async fn ai_step(db: &Db, target: &Target, id: &str) -> Result<Option<Document>> {
    let id = id.to_string();
    let (doc, head) = db
        .call({
            let id = id.clone();
            move |c| Ok((repo::get_document(c, &id)?, repo::head_text(c, &id, 4)?))
        })
        .await?;
    let want = Missing::of(&doc);
    if !want.worth_asking() || head.trim().is_empty() {
        return Ok(None);
    }
    let asked = details_ai::ask(target, &doc, &head, &want, current_year()).await;
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
    let found = asked?.found;
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

/// Looks the document up and stores what was found. `Ok(None)` when nothing changed.
async fn crossref_step(db: &Db, client: &reqwest::Client, base: &str, id: &str) -> Result<Option<Document>> {
    let id = id.to_string();
    let job = db
        .call({
            let id = id.clone();
            move |c| {
                if !settings::get_bool(c, CROSSREF_KEY)? {
                    return Ok(None);
                }
                let doc = repo::get_document(c, &id)?;
                let head = repo::head_text(c, &id, 4)?;
                Ok(Some((doc, head)))
            }
        })
        .await?;
    let Some((doc, head)) = job else { return Ok(None) };

    let mut dois: Vec<String> = doc.doi.iter().cloned().collect();
    for c in crossref::candidates(&doc.original_name) {
        if !dois.iter().any(|d| d.eq_ignore_ascii_case(&c)) {
            dois.push(c);
        }
    }
    for doi in dois {
        // A lookup that fails (offline, busy) stops here and is tried again on the next refresh.
        let Some(record) = crossref::fetch(client, base, &doi).await? else { continue };
        if !crossref::title_in_text(&record.title, &head) {
            log::info!("Crossref record for {doi} is a different article, ignored");
            continue;
        }
        let found = crossref::to_extracted(&record, doc.publisher.is_some());
        let updated = db
            .call(move |c| {
                repo::apply_metadata(c, &id, &found, "crossref", false)?;
                repo::get_document(c, &id)
            })
            .await?;
        return Ok(Some(updated));
    }
    Ok(None)
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
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        let (file, text, doi) = (file.to_string(), text.to_string(), doi.map(String::from));
        let id = db
            .call(move |c| {
                settings::set_bool(c, CROSSREF_KEY, enabled)?;
                let d = repo::insert_document(c, "h1", &file, DocKind::Pdf)?;
                let pages = [NewPage { page_no: 1, width: 1.0, height: 1.0, quality: "ok", ocr: false }];
                let passages = [NewPassage { page_start: 1, page_end: 1, text, boxes: None }];
                let meta = Extracted { doi, title: Some("Read off the page".into()), ..Default::default() };
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
        let doc = crossref_step(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().unwrap();
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
        assert!(crossref_step(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().is_none());
        let doc = db.call(move |c| repo::get_document(c, &id)).await.unwrap();
        assert_eq!(doc.title.as_deref(), Some("Read off the page"));
    }

    #[tokio::test]
    async fn nothing_is_sent_unless_the_user_allowed_it() {
        let (db, id, _dir) = setup(false, "nature02437.pdf", PAGE, Some("10.1038/nature02437")).await;
        let (base, seen) = serve(vec![(200, RECORD.into())]);
        assert!(crossref_step(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().is_none());
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
        let doc = crossref_step(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().unwrap();
        assert_eq!((doc.title.as_deref(), doc.year), (Some("My own title"), Some(2004)));
        let (down, _) = serve(vec![(503, "busy".into())]);
        assert!(crossref_step(&db, &crossref::http_client(), &format!("{down}/works/"), &id).await.is_err());
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
                repo::apply_metadata(c, &id, &Extracted { title: Some("T".into()), authors: vec!["A B".into()], year: Some(2000), ..Default::default() }, "file", true)
            }
        })
        .await
        .unwrap();
        assert!(ai_step(&db, &target("{}"), &id).await.unwrap().is_none());
        assert!(usage_kinds(&db).await.is_empty());
    }
}
