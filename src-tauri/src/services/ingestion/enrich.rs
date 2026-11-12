//! Adds details to a document after it is read: a Crossref lookup by DOI when the user allowed it.
//! One task works through the documents in turn. Its own events skip the hook that queues
//! documents, so it never queues itself.

use tokio::sync::mpsc;

use super::crossref;
use super::queue::{DocumentChanged, Emit};
use crate::domain::Id;
use crate::persistence::{Db, library as repo, settings};
use crate::error::Result;

pub const CROSSREF_KEY: &str = "crossref_enabled";

#[derive(Clone)]
pub struct Enricher(mpsc::UnboundedSender<Id>);

impl Enricher {
    pub fn start(db: Db, emit: Emit) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Id>();
        tauri::async_runtime::spawn(async move {
            let client = crossref::http_client();
            while let Some(id) = rx.recv().await {
                match enrich(&db, &client, crossref::BASE, &id).await {
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

/// Looks the document up and stores what was found. `Ok(None)` when nothing changed.
pub async fn enrich(db: &Db, client: &reqwest::Client, base: &str, id: &str) -> Result<Option<crate::domain::library::Document>> {
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
        let doc = enrich(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().unwrap();
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
        assert!(enrich(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().is_none());
        let doc = db.call(move |c| repo::get_document(c, &id)).await.unwrap();
        assert_eq!(doc.title.as_deref(), Some("Read off the page"));
    }

    #[tokio::test]
    async fn nothing_is_sent_unless_the_user_allowed_it() {
        let (db, id, _dir) = setup(false, "nature02437.pdf", PAGE, Some("10.1038/nature02437")).await;
        let (base, seen) = serve(vec![(200, RECORD.into())]);
        assert!(enrich(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().is_none());
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
        let doc = enrich(&db, &crossref::http_client(), &format!("{base}/works/"), &id).await.unwrap().unwrap();
        assert_eq!((doc.title.as_deref(), doc.year), (Some("My own title"), Some(2004)));
        let (down, _) = serve(vec![(503, "busy".into())]);
        assert!(enrich(&db, &crossref::http_client(), &format!("{down}/works/"), &id).await.is_err());
    }
}
