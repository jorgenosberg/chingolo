//! Tools the model can call to read the user's library. Every passage a tool returns gets a
//! handle (`S12`) saved with the conversation before the model sees it. The model never sees a
//! passage id, so it can only cite handles the app handed out.

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::domain::library::SearchHit;
use crate::error::{AppError, Result};
use crate::persistence::library::{self as repo, SearchOptions};
use crate::persistence::sources::{self, NewSource};
use crate::persistence::Db;
use crate::services::ai::ToolSpec;
use crate::services::embedding::engine::Embedder;
use crate::services::retrieval;

const SEARCH_LIMIT_DEFAULT: u32 = 6;
const SEARCH_LIMIT_MAX: u32 = 8;
const NEIGHBOURS_MAX: u32 = 3;
const PAGE_LIMIT: u32 = 8;

/// What a tool call needs: where to look and which conversation the handles belong to.
#[derive(Clone)]
pub struct ToolCtx {
    pub db: Db,
    pub embedder: Embedder,
    pub conversation_id: String,
    /// Documents of this project only, or the whole library.
    pub project_id: Option<String>,
}

/// Which documents a conversation can draw on, or `None` when there are none to read. A project
/// without ready documents falls back to the whole library.
pub fn scope_for(conn: &Connection, project_id: Option<&str>) -> Result<Option<Option<String>>> {
    if let Some(p) = project_id
        && repo::ready_document_count(conn, Some(p))? > 0
    {
        return Ok(Some(Some(p.to_string())));
    }
    Ok((repo::ready_document_count(conn, None)? > 0).then_some(None))
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
    let (conversation_id, project_id) = (ctx.conversation_id.clone(), ctx.project_id.clone());
    ctx.db
        .call(move |c| {
            let hits = match vector {
                Some(v) => retrieval::hybrid_search(c, &query, &v, identity, project_id.as_deref(), limit)?,
                None => {
                    let opts = SearchOptions { project_id: project_id.as_deref(), all_terms: false, prefix_last: false, limit };
                    repo::search_passages(c, &query, &opts)?
                }
            };
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

/// Accepts `S12`, `[S12]`, `s12` or 12.
fn parse_handle(v: &Value) -> Result<u32> {
    let parsed = match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().trim_matches(['[', ']']).trim_start_matches(['S', 's']).parse().ok(),
        _ => None,
    };
    parsed.ok_or_else(|| AppError::Invalid("handle must look like S3.".into()))
}

fn source_label(h: &SearchHit) -> String {
    match (h.page_start, h.page_end) {
        (0, _) => h.document_title.clone(),
        (a, b) if a == b => format!("{}, p. {a}", h.document_title),
        (a, b) => format!("{}, pp. {a}-{b}", h.document_title),
    }
}

/// Gives each hit its handle (saved before the model can see it) and builds the tool result.
fn results_json(conn: &mut Connection, conversation_id: &str, hits: &[SearchHit]) -> Result<Value> {
    let tx = conn.transaction()?;
    let mut results = Vec::with_capacity(hits.len());
    for h in hits {
        let handle = sources::handle_for(
            &tx,
            conversation_id,
            &NewSource {
                passage_id: &h.passage_id,
                document_id: &h.document_id,
                title: &h.document_title,
                page: h.page_start,
                text: &h.text,
            },
        )?;
        results.push(json!({ "handle": format!("S{handle}"), "source": source_label(h), "text": h.text }));
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
        let pages = vec![NewPage { page_no: 1, width: 600.0, height: 800.0, quality: "ok" }];
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

    #[test]
    fn scope_prefers_a_project_with_documents_then_the_library() {
        let mut conn = test_conn();
        assert_eq!(scope_for(&conn, None).unwrap(), None, "nothing to read yet");
        let d = doc(&mut conn, "h1", "Alpha", &[1]);
        let p = crate::persistence::projects::create_project(&conn, "P").unwrap();
        assert_eq!(scope_for(&conn, Some(&p.id)).unwrap(), Some(None), "an empty project falls back to the library");
        assert_eq!(scope_for(&conn, None).unwrap(), Some(None));
        conn.execute("INSERT INTO project_documents (project_id, document_id, added_at) VALUES (?1, ?2, 0)", [&p.id, &d]).unwrap();
        assert_eq!(scope_for(&conn, Some(&p.id)).unwrap(), Some(Some(p.id.clone())));
    }
}
