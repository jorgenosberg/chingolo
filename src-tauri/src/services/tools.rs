//! Tools the model can call to read the user's library. Every passage a tool returns gets a
//! handle (`S12`) saved with the conversation before the model sees it. The model never sees a
//! passage id, so it can only cite handles the app handed out.

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::domain::Conversation;
use crate::domain::library::{DocFilter, DocStatus, Scope, SearchHit};
use crate::error::{AppError, Result};
use crate::persistence::library::{self as repo, SearchOptions};
use crate::persistence::sources::{self, NewSource};
use crate::persistence::usage::{self, NewUsage};
use crate::persistence::Db;
use crate::services::ai::ToolSpec;
use crate::services::embedding::engine::Embedder;
use crate::services::ingestion::tables;
use crate::services::rerank::{self, RerankCtx};
use crate::services::retrieval;

const SEARCH_LIMIT_DEFAULT: u32 = 6;
const SEARCH_LIMIT_MAX: u32 = 8;
const NEIGHBOURS_MAX: u32 = 3;
const PAGE_LIMIT: u32 = 8;
const LIST_MAX: usize = 100;

/// What a tool call needs: where to look and which conversation the handles belong to.
#[derive(Clone)]
pub struct ToolCtx {
    pub db: Db,
    pub embedder: Embedder,
    pub conversation_id: String,
    /// Which documents every tool may read.
    pub filter: DocFilter,
    /// Set when the user turned on reranking in Settings.
    pub rerank: Option<RerankCtx>,
}

/// Which documents a conversation can draw on, or `None` when there are none to read.
/// Without a chosen scope, a project without ready documents falls back to the whole library.
/// A scope the user chose never falls back: if nothing in it is ready, there are no tools.
pub fn scope_for(conn: &Connection, conv: &Conversation) -> Result<Option<DocFilter>> {
    let project = conv.project_id.as_deref();
    let filter = match &conv.source_scope {
        None => {
            if let Some(p) = project {
                let f = DocFilter::Project(p.to_string());
                if repo::ready_document_count(conn, &f)? > 0 {
                    return Ok(Some(f));
                }
            }
            DocFilter::All
        }
        Some(Scope::Library) => DocFilter::All,
        Some(Scope::Project) => match project {
            Some(p) => DocFilter::Project(p.to_string()),
            None => return Ok(None),
        },
        Some(Scope::Documents { ids }) => DocFilter::Documents(ids.clone()),
    };
    Ok((repo::ready_document_count(conn, &filter)? > 0).then_some(filter))
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "search_library".into(),
            description: "Search the user's papers. Use it before answering anything about their documents \
                or the research in them. Returns the best passages, each with a handle like S3. Search again \
                with different words if the first results don't answer the question."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to look for, in the words a paper would use." },
                    "limit": { "type": "integer", "description": "How many passages, 1 to 8. Default 6." }
                },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "list_documents".into(),
            description: "List the documents you can search: title, year, pages and whether each could be read. \
                Use it first when the question is about each or all of the user's papers, or how many there are."
                .into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "read_section".into(),
            description: "Read the passages before and after one you already have, in reading order. \
                Use it when a passage cuts off or you need its surrounding context."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "A handle you were given, such as S3." },
                    "before": { "type": "integer", "description": "Passages before it, 0 to 3. Default 1." },
                    "after": { "type": "integer", "description": "Passages after it, 0 to 3. Default 1." }
                },
                "required": ["handle"]
            }),
        },
        ToolSpec {
            name: "get_page".into(),
            description: "Read the passages on a page of a document you already have a passage from. \
                Defaults to the page of that passage."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "A handle you were given, such as S3. Picks the document." },
                    "page": { "type": "integer", "description": "Page number, starting at 1." }
                },
                "required": ["handle"]
            }),
        },
    ]
}

/// A short line for the UI while a tool runs.
pub fn summary(name: &str, args: &Value) -> String {
    match name {
        "search_library" => match args["query"].as_str().map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => format!("Searching your library for \"{}\"", q.chars().take(80).collect::<String>()),
            None => "Searching your library".into(),
        },
        "list_documents" => "Listing your documents".into(),
        "read_section" => "Reading around a passage".into(),
        "get_page" => "Reading a page".into(),
        _ => "Working".into(),
    }
}

/// Runs one call. A failure goes back to the model as a result so it can recover, never as a
/// failed reply.
pub async fn run(ctx: &ToolCtx, name: &str, args: &Value) -> Value {
    let result = match name {
        "search_library" => search(ctx, args).await,
        "list_documents" => list_documents(ctx).await,
        "read_section" => read_section(ctx, args).await,
        "get_page" => get_page(ctx, args).await,
        other => Err(AppError::Invalid(format!("There is no tool called {other}."))),
    };
    result.unwrap_or_else(|e| {
        log::warn!("tool {name} failed: {e}");
        json!({ "error": e.to_string() })
    })
}

async fn search(ctx: &ToolCtx, args: &Value) -> Result<Value> {
    let query = args["query"]
        .as_str()
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
        .ok_or_else(|| AppError::Invalid("query is required.".into()))?;
    let limit = (args["limit"].as_u64().unwrap_or(SEARCH_LIMIT_DEFAULT as u64) as u32).clamp(1, SEARCH_LIMIT_MAX);

    let embedder = ctx.embedder.clone();
    let text = query.clone();
    let vector = match tauri::async_runtime::spawn_blocking(move || embedder.embed_query(&text)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            log::warn!("tool search: query embedding failed, using keywords only: {e}");
            None
        }
        Err(e) => {
            log::warn!("tool search: query embedding task failed, using keywords only: {e}");
            None
        }
    };
    let by_meaning = vector.is_some();
    let identity = ctx.embedder.backend().identity();
    let filter = ctx.filter.clone();
    // With reranking on, the model sees a wider pool and the best `limit` of it come back.
    let pool = if ctx.rerank.is_some() { rerank::CANDIDATES.max(limit) } else { limit };
    let text = query.clone();
    let mut hits = ctx
        .db
        .call(move |c| match vector {
            Some(v) => retrieval::hybrid_search(c, &text, &v, identity, &filter, pool),
            None => {
                let opts = SearchOptions { filter: &filter, all_terms: false, prefix_last: false, limit: pool };
                repo::search_passages(c, &text, &opts)
            }
        })
        .await?;

    if let (Some(r), true) = (&ctx.rerank, hits.len() > 1) {
        let result = rerank::rerank(r, &query, hits.clone()).await;
        let (tokens, latency_ms, ok) = match &result {
            Ok(x) => (x.tokens, x.latency_ms, true),
            Err(e) => {
                log::warn!("tool search: rerank failed, keeping the search order: {e}");
                ((0, 0, 0), 0, false)
            }
        };
        if let Ok(x) = result {
            hits = x.hits;
        }
        let (provider, model, conv) = (r.provider.id(), r.model.clone(), ctx.conversation_id.clone());
        let recorded = ctx
            .db
            .call(move |c| {
                usage::record(
                    c,
                    &NewUsage {
                        provider,
                        model: &model,
                        kind: "rerank",
                        input_tokens: tokens.0,
                        output_tokens: tokens.1,
                        cached_tokens: tokens.2,
                        estimated: false,
                        latency_ms,
                        ok,
                        conversation_id: Some(&conv),
                    },
                )
            })
            .await;
        if let Err(e) = recorded {
            log::warn!("couldn't record rerank usage: {e}");
        }
    }
    hits.truncate(limit as usize);

    let conversation_id = ctx.conversation_id.clone();
    ctx.db
        .call(move |c| {
            let mut out = results_json(c, &conversation_id, &hits)?;
            if !by_meaning {
                out["note"] = json!(
                    "Matched on words only, because search by meaning isn't available right now. \
                     Try again with words the papers would use."
                );
            }
            if hits.is_empty() {
                out["note"] = json!("No passages matched.");
            }
            Ok(out)
        })
        .await
}

async fn list_documents(ctx: &ToolCtx) -> Result<Value> {
    let filter = ctx.filter.clone();
    ctx.db.call(move |c| list_documents_sync(c, &filter)).await
}

/// Title, year, pages and a plain status for each scoped document. No ids and no handles.
fn list_documents_sync(conn: &Connection, filter: &DocFilter) -> Result<Value> {
    let docs = repo::list_documents_in(conn, filter)?;
    let ready = docs.iter().filter(|d| d.status == DocStatus::Ready).count();
    let items: Vec<Value> = docs
        .iter()
        .take(LIST_MAX)
        .map(|d| {
            let title = d.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&d.original_name);
            let status = match d.status {
                DocStatus::Ready => "ready",
                DocStatus::Queued | DocStatus::Processing => "still being read, not searchable yet",
                DocStatus::NoText => "no text could be read (probably a scan), not searchable",
                DocStatus::Error | DocStatus::Cancelled => "could not be read, not searchable",
            };
            let by: Vec<&String> = d.authors.iter().take(3).collect();
            json!({
                "cite_as": d.label(),
                "title": title,
                "authors": by,
                "year": d.year,
                "journal": d.journal,
                "series": d.series,
                "pages": d.page_count,
                "status": status
            })
        })
        .collect();
    let mut out = json!({ "total": docs.len(), "ready": ready, "documents": items });
    if docs.len() > LIST_MAX {
        out["note"] = json!(format!("Only the newest {LIST_MAX} are listed."));
    }
    Ok(out)
}

async fn read_section(ctx: &ToolCtx, args: &Value) -> Result<Value> {
    let handle = parse_handle(&args["handle"])?;
    let before = (args["before"].as_u64().unwrap_or(1) as u32).min(NEIGHBOURS_MAX);
    let after = (args["after"].as_u64().unwrap_or(1) as u32).min(NEIGHBOURS_MAX);
    let conversation_id = ctx.conversation_id.clone();
    ctx.db.call(move |c| read_section_sync(c, &conversation_id, handle, before, after)).await
}

async fn get_page(ctx: &ToolCtx, args: &Value) -> Result<Value> {
    let handle = parse_handle(&args["handle"])?;
    let page = args["page"].as_u64().map(|p| p as u32);
    let conversation_id = ctx.conversation_id.clone();
    ctx.db.call(move |c| get_page_sync(c, &conversation_id, handle, page)).await
}

fn read_section_sync(conn: &mut Connection, conversation_id: &str, handle: u32, before: u32, after: u32) -> Result<Value> {
    let src = known_source(conn, conversation_id, handle)?;
    let hits = repo::passages_around(conn, &src.passage_id, before, after)?;
    if hits.is_empty() {
        return Err(AppError::Invalid(format!(
            "S{handle} is no longer in the library (the file may have been read again). Search again."
        )));
    }
    results_json(conn, conversation_id, &hits)
}

fn get_page_sync(conn: &mut Connection, conversation_id: &str, handle: u32, page: Option<u32>) -> Result<Value> {
    let src = known_source(conn, conversation_id, handle)?;
    let page = page.unwrap_or(src.page);
    if page == 0 {
        return Err(AppError::Invalid("This document has no pages. Use read_section instead.".into()));
    }
    let hits = repo::passages_on_page(conn, &src.document_id, page, PAGE_LIMIT)?;
    if hits.is_empty() {
        return Err(AppError::Invalid(format!("There is no text on page {page} of that document.")));
    }
    results_json(conn, conversation_id, &hits)
}

fn known_source(conn: &Connection, conversation_id: &str, handle: u32) -> Result<sources::Source> {
    sources::get(conn, conversation_id, handle)?
        .ok_or_else(|| AppError::Invalid(format!("S{handle} is not a handle you were given.")))
}

/// Said to the model next to every table passage. Row and column association is a model's reading
/// of the layout, so it is never presented as certain.
const TABLE_NOTE: &str = "A table. The numbers were read from the PDF's text. Which row and column each belongs to was worked out by a model from the page layout, so say so if the answer depends on it, and don't total or compare cells yourself. Cells listed as not confirmed by a second reading are uncertain: don't quote their numbers or signs without saying so.";

/// Accepts `S12`, `[S12]`, `s12` or 12.
fn parse_handle(v: &Value) -> Result<u32> {
    let parsed = match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().trim_matches(['[', ']']).trim_start_matches(['S', 's']).parse().ok(),
        _ => None,
    };
    parsed.ok_or_else(|| AppError::Invalid("handle must look like S3.".into()))
}

/// What the model sees for a passage: who wrote it and when, the title and the pages, such as
/// `Boyd et al. 2004, "The decline and fate...", p. 3`.
fn source_label(h: &SearchHit, label: &str) -> String {
    let named = if label.is_empty() { h.document_title.clone() } else { format!("{label}, \"{}\"", h.document_title) };
    match (h.page_start, h.page_end) {
        (0, _) => named,
        (a, b) if a == b => format!("{named}, p. {a}"),
        (a, b) => format!("{named}, pp. {a}-{b}"),
    }
}

/// Gives each hit its handle (saved before the model can see it) and builds the tool result.
fn results_json(conn: &mut Connection, conversation_id: &str, hits: &[SearchHit]) -> Result<Value> {
    let tx = conn.transaction()?;
    let mut results = Vec::with_capacity(hits.len());
    let mut labels: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for h in hits {
        if !labels.contains_key(h.document_id.as_str()) {
            let label = repo::get_document(&tx, &h.document_id).map(|d| d.label()).unwrap_or_default();
            labels.insert(&h.document_id, label);
        }
        let label = labels[h.document_id.as_str()].as_str();
        let handle = sources::handle_for(
            &tx,
            conversation_id,
            &NewSource {
                passage_id: &h.passage_id,
                document_id: &h.document_id,
                title: &h.document_title,
                label,
                page: h.page_start,
                text: &h.text,
            },
        )?;
        let mut result = json!({ "handle": format!("S{handle}"), "source": source_label(h, label), "text": h.text });
        if tables::is_grid(&h.text) {
            result["kind"] = json!("table");
            result["note"] = json!(TABLE_NOTE);
        }
        results.push(result);
    }
    tx.commit()?;
    Ok(json!({ "results": results }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::chat;
    use crate::persistence::library::{NewPage, NewPassage};
    use crate::persistence::test_conn;

    fn doc(conn: &mut Connection, hash: &str, name: &str, pages: &[u32]) -> String {
        let d = repo::insert_document(conn, hash, name, crate::domain::library::DocKind::Pdf).unwrap();
        let passages: Vec<_> = pages
            .iter()
            .enumerate()
            .map(|(i, p)| NewPassage { page_start: *p, page_end: *p, text: format!("{name} passage {i}"), boxes: None })
            .collect();
        let pages = vec![NewPage { page_no: 1, width: 600.0, height: 800.0, quality: "ok", ocr: false }];
        repo::store_content(
            conn,
            &d.id,
            &pages,
            &passages,
            &Default::default(),
            crate::domain::library::DocStatus::Ready,
            None,
        )
        .unwrap();
        d.id
    }

    fn hits(conn: &Connection, doc_id: &str) -> Vec<SearchHit> {
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM passages WHERE document_id = ?1 ORDER BY ordinal")
            .unwrap()
            .query_map([doc_id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        repo::hits_for_ids(conn, &ids, crate::domain::library::MatchKind::Keyword).unwrap()
    }

    #[test]
    fn results_get_stable_handles_and_hide_ids() {
        let mut conn = test_conn();
        let conv = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        let d = doc(&mut conn, "h1", "Alpha", &[1, 2, 3]);
        let all = hits(&conn, &d);

        let first = results_json(&mut conn, &conv.id, &all[..2]).unwrap();
        assert_eq!(first["results"][0]["handle"], "S1");
        assert_eq!(first["results"][1]["handle"], "S2");
        assert_eq!(first["results"][1]["source"], "Alpha, p. 2");
        assert!(!first.to_string().contains(&all[0].passage_id), "the model never sees a passage id");

        let again = results_json(&mut conn, &conv.id, &all[1..]).unwrap();
        assert_eq!(again["results"][0]["handle"], "S2", "a passage keeps its handle");
        assert_eq!(again["results"][1]["handle"], "S3");
    }

    #[test]
    fn a_table_passage_is_marked_as_a_table_read_by_a_model() {
        let mut conn = test_conn();
        let conv = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        let d = repo::insert_document(&conn, "ht", "Beta", crate::domain::library::DocKind::Pdf).unwrap();
        let grid = "| Site | Chl a |\n| --- | --- |\n| A | 12.5 |";
        let passages = [grid, "Plain text about the sites in the study."].map(|t| NewPassage { page_start: 1, page_end: 1, text: t.into(), boxes: None });
        let pages = vec![NewPage { page_no: 1, width: 600.0, height: 800.0, quality: "ok", ocr: false }];
        repo::store_content(&mut conn, &d.id, &pages, &passages, &Default::default(), crate::domain::library::DocStatus::Ready, None).unwrap();
        let all = hits(&conn, &d.id);
        let out = results_json(&mut conn, &conv.id, &all).unwrap();
        assert_eq!(out["results"][0]["kind"], "table");
        assert!(out["results"][0]["note"].as_str().unwrap().contains("worked out by a model"));
        assert!(out["results"][1].get("kind").is_none());
    }

    #[test]
    fn sources_carry_author_and_year_to_the_model_and_the_snapshot() {
        use crate::persistence::library::{Extracted, apply_metadata};
        let mut conn = test_conn();
        let conv = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        let d = doc(&mut conn, "h1", "Alpha", &[1, 2]);
        let m = Extracted { authors: vec!["Philip W. Boyd".into(), "Tim Jickells".into(), "Cliff Law".into()], year: Some(2004), journal: Some("Nature".into()), ..Default::default() };
        apply_metadata(&conn, &d, &m, "file", true).unwrap();
        let all = hits(&conn, &d);
        let out = results_json(&mut conn, &conv.id, &all[1..2]).unwrap();
        assert_eq!(out["results"][0]["source"], "Boyd et al. 2004, \"Alpha\", p. 2");
        let saved = sources::get(&conn, &conv.id, 1).unwrap().unwrap();
        assert_eq!(saved.label, "Boyd et al. 2004");
        let listed = list_documents_sync(&conn, &DocFilter::All).unwrap();
        let first = &listed["documents"][0];
        assert_eq!((first["cite_as"].as_str(), first["journal"].as_str()), (Some("Boyd et al. 2004"), Some("Nature")));
        assert_eq!(first["authors"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn read_section_and_get_page_work_from_handles_only() {
        let mut conn = test_conn();
        let conv = chat::create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        let d = doc(&mut conn, "h1", "Alpha", &[1, 1, 2, 3]);
        let all = hits(&conn, &d);
        results_json(&mut conn, &conv.id, &all[2..3]).unwrap(); // S1 is the page 2 passage

        let around = read_section_sync(&mut conn, &conv.id, 1, 1, 1).unwrap();
        let texts: Vec<_> = around["results"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(texts, ["Alpha passage 1", "Alpha passage 2", "Alpha passage 3"], "reading order, with the neighbours");

        let page = get_page_sync(&mut conn, &conv.id, 1, Some(1)).unwrap();
        assert_eq!(page["results"].as_array().unwrap().len(), 2);
        let default_page = get_page_sync(&mut conn, &conv.id, 1, None).unwrap();
        assert_eq!(default_page["results"].as_array().unwrap().len(), 1, "defaults to the handle's page");

        let unknown = read_section_sync(&mut conn, &conv.id, 9, 1, 1).unwrap_err().to_string();
        assert!(unknown.contains("S9"), "{unknown}");
        assert!(get_page_sync(&mut conn, &conv.id, 1, Some(40)).is_err());
        assert!(get_page_sync(&mut conn, &conv.id, 1, Some(0)).is_err());
    }

    #[test]
    fn handles_are_read_in_any_common_spelling() {
        for v in [json!("S12"), json!("[S12]"), json!("s12"), json!(12), json!(" S12 ")] {
            assert_eq!(parse_handle(&v).unwrap(), 12, "{v}");
        }
        for v in [json!("abc"), json!(null), json!(-1), json!("S")] {
            assert!(parse_handle(&v).is_err(), "{v}");
        }
    }

    fn conv_with(conn: &Connection, project: Option<&str>, scope: Option<Scope>) -> Conversation {
        let c = chat::create_conversation(conn, project, None, "gemini", Some("m")).unwrap();
        chat::set_scope(conn, &c.id, scope.as_ref()).unwrap()
    }

    #[test]
    fn default_scope_prefers_a_project_with_documents_then_the_library() {
        let mut conn = test_conn();
        let p = crate::persistence::projects::create_project(&conn, "P").unwrap();
        let loose = conv_with(&conn, None, None);
        let in_p = conv_with(&conn, Some(&p.id), None);
        assert_eq!(scope_for(&conn, &loose).unwrap(), None, "nothing to read yet");
        let d = doc(&mut conn, "h1", "Alpha", &[1]);
        assert_eq!(scope_for(&conn, &in_p).unwrap(), Some(DocFilter::All), "an empty project falls back to the library");
        assert_eq!(scope_for(&conn, &loose).unwrap(), Some(DocFilter::All));
        conn.execute("INSERT INTO project_documents (project_id, document_id, added_at) VALUES (?1, ?2, 0)", [&p.id, &d]).unwrap();
        assert_eq!(scope_for(&conn, &in_p).unwrap(), Some(DocFilter::Project(p.id.clone())));
    }

    #[test]
    fn a_chosen_scope_never_falls_back() {
        let mut conn = test_conn();
        let p = crate::persistence::projects::create_project(&conn, "P").unwrap();
        let a = doc(&mut conn, "h1", "Alpha", &[1]);
        let queued = repo::insert_document(&conn, "h2", "q.pdf", crate::domain::library::DocKind::Pdf).unwrap();

        let library = conv_with(&conn, Some(&p.id), Some(Scope::Library));
        assert_eq!(scope_for(&conn, &library).unwrap(), Some(DocFilter::All));
        let empty_project = conv_with(&conn, Some(&p.id), Some(Scope::Project));
        assert_eq!(scope_for(&conn, &empty_project).unwrap(), None, "the project is empty, the library is not used instead");
        let no_project = conv_with(&conn, None, Some(Scope::Project));
        assert_eq!(scope_for(&conn, &no_project).unwrap(), None);

        let chosen = conv_with(&conn, None, Some(Scope::Documents { ids: vec![a.clone()] }));
        assert_eq!(scope_for(&conn, &chosen).unwrap(), Some(DocFilter::Documents(vec![a.clone()])));
        let not_ready = conv_with(&conn, None, Some(Scope::Documents { ids: vec![queued.id] }));
        assert_eq!(scope_for(&conn, &not_ready).unwrap(), None, "a document still being read gives no tools");
        let gone = conv_with(&conn, None, Some(Scope::Documents { ids: vec!["deleted".into()] }));
        assert_eq!(scope_for(&conn, &gone).unwrap(), None);
    }

    #[test]
    fn list_documents_covers_only_the_scope_and_hides_ids() {
        let mut conn = test_conn();
        let a = doc(&mut conn, "h1", "Alpha", &[1]);
        let b = doc(&mut conn, "h2", "Beta", &[1]);
        repo::insert_document(&conn, "h3", "waiting.pdf", crate::domain::library::DocKind::Pdf).unwrap();

        let all = list_documents_sync(&conn, &DocFilter::All).unwrap();
        assert_eq!((all["total"].as_u64(), all["ready"].as_u64()), (Some(3), Some(2)));
        assert!(all.to_string().contains("not searchable yet"), "an unread document is reported as such");

        let one = list_documents_sync(&conn, &DocFilter::Documents(vec![b.clone()])).unwrap();
        assert_eq!(one["total"], 1);
        assert_eq!(one["documents"][0]["title"], "Beta");
        let text = one.to_string();
        assert!(!text.contains(&a) && !text.contains(&b), "no document ids reach the model");
    }
}
