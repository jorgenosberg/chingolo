use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Row, ToSql, params, params_from_iter};

use super::sources;

use crate::domain::library::Scope;
use crate::domain::{
    Conversation, ConversationFilter, DEFAULT_CONVERSATION_TITLE, Id, Message, MessageHit,
    MessageStatus, Role, clean_name, new_id, now_ms,
};
use crate::error::{AppError, Result};

const CONV_COLS: &str =
    "id, project_id, chapter_id, title, provider, model_id, archived_at, created_at, updated_at,
    (SELECT CASE WHEN m.status <> 'error' THEN NULLIF(m.input_tokens, 0) END
       FROM messages m WHERE m.conversation_id = conversations.id AND m.role = 'assistant'
       ORDER BY m.id DESC LIMIT 1),
    forked_from_title, fork_boundary_message_id, source_scope, draft,
    EXISTS (SELECT 1 FROM messages m2 WHERE m2.conversation_id = conversations.id)";
const MSG_COLS: &str = "id, conversation_id, role, content, status, provider, model_id, input_tokens, output_tokens, error, created_at, updated_at, cached_input_tokens, citations";

fn conversation_from_row(r: &Row) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: r.get(0)?,
        project_id: r.get(1)?,
        chapter_id: r.get(2)?,
        title: r.get(3)?,
        provider: r.get(4)?,
        model_id: r.get(5)?,
        archived_at: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        switch_cost_tokens: r.get(9)?,
        forked_from_title: r.get(10)?,
        fork_boundary_message_id: r.get(11)?,
        source_scope: r.get::<_, Option<String>>(12)?.and_then(|json| {
            serde_json::from_str(&json)
                .map_err(|e| log::warn!("unreadable source_scope, using the default: {e}"))
                .ok()
        }),
        draft: r.get(13)?,
        has_messages: r.get(14)?,
    })
}

fn message_from_row(r: &Row) -> rusqlite::Result<Message> {
    let role: String = r.get(2)?;
    let status: String = r.get(4)?;
    Ok(Message {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        role: Role::parse(&role).ok_or(rusqlite::Error::InvalidQuery)?,
        content: r.get(3)?,
        status: MessageStatus::parse(&status).ok_or(rusqlite::Error::InvalidQuery)?,
        provider: r.get(5)?,
        model_id: r.get(6)?,
        input_tokens: r.get(7)?,
        output_tokens: r.get(8)?,
        error: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        cached_input_tokens: r.get(12)?,
        citations: r
            .get::<_, Option<String>>(13)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
    })
}

pub fn create_conversation(
    conn: &Connection,
    project_id: Option<&str>,
    chapter_id: Option<&str>,
    provider: &str,
    model_id: Option<&str>,
) -> Result<Conversation> {
    let (id, now) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO conversations
           (id, project_id, chapter_id, title, provider, model_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            id,
            project_id,
            chapter_id,
            DEFAULT_CONVERSATION_TITLE,
            provider,
            model_id,
            now
        ],
    )?;
    get_conversation(conn, &id)
}

pub fn get_conversation(conn: &Connection, id: &str) -> Result<Conversation> {
    conn.query_row(
        &format!("SELECT {CONV_COLS} FROM conversations WHERE id = ?1"),
        [id],
        conversation_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

/// Turns user input into a safe FTS5 prefix query: every word is quoted.
fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| w.replace('"', ""))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn like_pattern(q: &str) -> String {
    let escaped = q
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub fn list_conversations(
    conn: &Connection,
    filter: &ConversationFilter,
) -> Result<Vec<Conversation>> {
    let mut sql = format!("SELECT {CONV_COLS} FROM conversations WHERE 1=1");
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(p) = &filter.project_id {
        sql.push_str(" AND project_id = ?");
        args.push(Box::new(p.clone()));
    } else if filter.unassigned_only {
        sql.push_str(" AND project_id IS NULL");
    }
    if !filter.include_archived {
        sql.push_str(" AND archived_at IS NULL");
    }
    if let Some(q) = filter.query.as_deref().filter(|q| !q.trim().is_empty()) {
        sql.push_str(" AND (title LIKE ? ESCAPE '\\'");
        args.push(Box::new(like_pattern(q)));
        if let Some(fts) = fts_query(q) {
            sql.push_str(
                " OR id IN (SELECT conversation_id FROM messages WHERE rowid IN
                   (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?))",
            );
            args.push(Box::new(fts));
        }
        sql.push(')');
    }
    sql.push_str(" ORDER BY updated_at DESC, id DESC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), conversation_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Messages that match `query`, best first, at most `per_chat` per conversation and `limit` in
/// all. Archived conversations are included.
pub fn search_messages(
    conn: &Connection,
    query: &str,
    per_chat: u32,
    limit: u32,
) -> Result<Vec<MessageHit>> {
    let Some(fts) = fts_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        // FTS5 auxiliary functions only work on a plain scan of the FTS table, so the scan is its
        // own CTE and the joins and the per-chat window sit above it.
        "WITH hits AS MATERIALIZED (
           SELECT rowid AS rid, snippet(messages_fts, 0, char(2), char(3), '…', 12) AS snippet, rank
           FROM messages_fts WHERE messages_fts MATCH ?1
         )
         SELECT conversation_id, title, project_id, archived_at, id, role, created_at, snippet
         FROM (
           SELECT m.conversation_id, c.title, c.project_id, c.archived_at, m.id, m.role,
                  m.created_at, h.snippet, h.rank,
                  ROW_NUMBER() OVER (PARTITION BY m.conversation_id ORDER BY h.rank) AS n
           FROM hits h
           JOIN messages m ON m.rowid = h.rid
           JOIN conversations c ON c.id = m.conversation_id
         )
         WHERE n <= ?2
         ORDER BY rank
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![fts, per_chat, limit], |r| {
        let role: String = r.get(5)?;
        Ok(MessageHit {
            conversation_id: r.get(0)?,
            conversation_title: r.get(1)?,
            project_id: r.get(2)?,
            archived: r.get::<_, Option<i64>>(3)?.is_some(),
            message_id: r.get(4)?,
            role: Role::parse(&role).ok_or(rusqlite::Error::InvalidQuery)?,
            created_at: r.get(6)?,
            snippet: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn rename_conversation(conn: &Connection, id: &str, title: &str) -> Result<Conversation> {
    let title = clean_name(title, "Title")?;
    let n = conn.execute(
        "UPDATE conversations SET title = ?1 WHERE id = ?2",
        params![title, id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_conversation(conn, id)
}

pub fn set_archived(conn: &Connection, id: &str, archived: bool) -> Result<Conversation> {
    let at = archived.then(now_ms);
    let n = conn.execute(
        "UPDATE conversations SET archived_at = ?1 WHERE id = ?2",
        params![at, id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_conversation(conn, id)
}

pub fn set_model(
    conn: &Connection,
    id: &str,
    provider: &str,
    model_id: &str,
) -> Result<Conversation> {
    let n = conn.execute(
        "UPDATE conversations SET provider = ?1, model_id = ?2 WHERE id = ?3",
        params![provider, model_id, id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_conversation(conn, id)
}

/// Sets or clears (`None`) what a conversation may read. The caller validates the ids.
pub fn set_scope(conn: &Connection, id: &str, scope: Option<&Scope>) -> Result<Conversation> {
    let json = scope
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let n = conn.execute(
        "UPDATE conversations SET source_scope = ?1 WHERE id = ?2",
        params![json, id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_conversation(conn, id)
}

/// Most documents a chosen scope may name.
const MAX_SCOPE_DOCUMENTS: usize = 200;

/// What the user asked for, checked: a project scope needs a project, and a document list needs
/// at least one id, all of which exist. Ids are deduplicated, order kept.
pub fn change_scope(conn: &Connection, id: &str, scope: Option<Scope>) -> Result<Conversation> {
    let conv = get_conversation(conn, id)?;
    let scope = match scope {
        Some(Scope::Project) if conv.project_id.is_none() => {
            return Err(AppError::Invalid(
                "This conversation isn't in a project.".into(),
            ));
        }
        Some(Scope::Documents { ids }) => {
            let mut unique: Vec<Id> = Vec::with_capacity(ids.len());
            for i in ids {
                if !unique.contains(&i) {
                    unique.push(i);
                }
            }
            if unique.is_empty() {
                return Err(AppError::Invalid("Choose at least one document.".into()));
            }
            if unique.len() > MAX_SCOPE_DOCUMENTS {
                return Err(AppError::Invalid(format!(
                    "Choose at most {MAX_SCOPE_DOCUMENTS} documents."
                )));
            }
            if super::library::existing_ids(conn, &unique)?.len() != unique.len() {
                return Err(AppError::Invalid(
                    "One of those documents no longer exists.".into(),
                ));
            }
            Some(Scope::Documents { ids: unique })
        }
        other => other,
    };
    set_scope(conn, id, scope.as_ref())
}

/// Copies a conversation, up to and including `up_to_message_id` if given, into a new one in the
/// same project and chapter. Only finished messages are copied. Token counts are not, because
/// the copy starts with no cached input.
pub fn fork_conversation(
    conn: &mut Connection,
    source_id: &str,
    up_to_message_id: Option<&str>,
) -> Result<Conversation> {
    let source = get_conversation(conn, source_id)?;
    let mut messages = list_messages(conn, source_id)?;
    if let Some(up_to) = up_to_message_id {
        let pos = messages
            .iter()
            .position(|m| m.id == up_to)
            .ok_or(AppError::NotFound)?;
        messages.truncate(pos + 1);
    }
    messages.retain(|m| m.status == MessageStatus::Complete);

    let tx = conn.transaction()?;
    let (id, now) = (new_id(), now_ms());
    tx.execute(
        "INSERT INTO conversations
           (id, project_id, chapter_id, title, provider, model_id, source_scope, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                 (SELECT source_scope FROM conversations WHERE id = ?8), ?7, ?7)",
        params![
            id,
            source.project_id,
            source.chapter_id,
            format!("{} (fork)", source.title),
            source.provider,
            source.model_id,
            now,
            source_id
        ],
    )?;
    let mut boundary: Option<Id> = None;
    for m in &messages {
        let copy = insert_message(
            &tx,
            &id,
            m.role,
            &m.content,
            MessageStatus::Complete,
            m.provider.as_deref(),
            m.model_id.as_deref(),
        )?;
        // Tool turns and citation checks come along, so the copy reads like the original.
        tx.execute(
            "UPDATE messages SET
               turns = (SELECT turns FROM messages WHERE id = ?1),
               citations = (SELECT citations FROM messages WHERE id = ?1)
             WHERE id = ?2",
            params![m.id, copy.id],
        )?;
        boundary = Some(copy.id);
    }
    // All handles, not only those up to the fork point: harmless, and numbering continues cleanly.
    sources::copy_all(&tx, source_id, &id)?;
    if boundary.is_some() {
        tx.execute(
            "UPDATE conversations SET forked_from_title = ?1, fork_boundary_message_id = ?2 WHERE id = ?3",
            params![source.title, boundary, id],
        )?;
    }
    tx.commit()?;
    get_conversation(conn, &id)
}

pub fn delete_conversation(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Stores the unsent text of a conversation. A conversation that no longer exists is not an error:
/// a delayed save can arrive after it was deleted. Does not change `updated_at`.
pub fn set_draft(conn: &Connection, id: &str, text: &str) -> Result<()> {
    conn.execute(
        "UPDATE conversations SET draft = ?1 WHERE id = ?2",
        params![text, id],
    )?;
    Ok(())
}

/// Deletes a conversation that never got a message or a draft. Returns whether it was deleted.
pub fn delete_if_empty(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM conversations WHERE id = ?1 AND draft = ''
           AND NOT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?1)",
        [id],
    )?;
    Ok(n > 0)
}

pub fn list_messages(conn: &Connection, conversation_id: &str) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MSG_COLS} FROM messages WHERE conversation_id = ?1 ORDER BY id"
    ))?;
    let rows = stmt.query_map([conversation_id], message_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get_message(conn: &Connection, id: &str) -> Result<Message> {
    conn.query_row(
        &format!("SELECT {MSG_COLS} FROM messages WHERE id = ?1"),
        [id],
        message_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

pub fn insert_message(
    conn: &Connection,
    conversation_id: &str,
    role: Role,
    content: &str,
    status: MessageStatus,
    provider: Option<&str>,
    model_id: Option<&str>,
) -> Result<Message> {
    let (id, now): (Id, i64) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO messages
           (id, conversation_id, role, content, status, provider, model_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            id,
            conversation_id,
            role.as_str(),
            content,
            status.as_str(),
            provider,
            model_id,
            now
        ],
    )?;
    touch_conversation(conn, conversation_id)?;
    get_message(conn, &id)
}

fn touch_conversation(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        params![now_ms(), id],
    )?;
    Ok(())
}

/// Names a fresh conversation after its first message.
pub fn title_from_first_message(conn: &Connection, id: &str, text: &str) -> Result<()> {
    let title: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = title.chars().take(60).collect();
    if title.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE conversations SET title = ?1 WHERE id = ?2 AND title = ?3",
        params![title, id, DEFAULT_CONVERSATION_TITLE],
    )?;
    Ok(())
}

/// Periodic save while a reply streams in.
pub fn flush_content(conn: &Connection, id: &str, content: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET content = ?1, updated_at = ?2 WHERE id = ?3",
        params![content, now_ms(), id],
    )?;
    Ok(())
}

pub struct Finish<'a> {
    pub content: &'a str,
    pub status: MessageStatus,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cached_input_tokens: Option<u32>,
    pub error: Option<&'a str>,
}

pub fn finish_message(conn: &Connection, id: &str, f: Finish) -> Result<Message> {
    conn.execute(
        "UPDATE messages
            SET content = ?1, status = ?2, input_tokens = ?3, output_tokens = ?4,
                cached_input_tokens = ?5, error = ?6, updated_at = ?7
          WHERE id = ?8",
        params![
            f.content,
            f.status.as_str(),
            f.input_tokens,
            f.output_tokens,
            f.cached_input_tokens,
            f.error,
            now_ms(),
            id
        ],
    )?;
    let msg = get_message(conn, id)?;
    touch_conversation(conn, &msg.conversation_id)?;
    Ok(msg)
}

/// Stores the turns of a reply as a JSON array (serialized by the caller, which owns the type).
pub fn set_turns(conn: &Connection, id: &str, turns_json: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET turns = ?1 WHERE id = ?2",
        params![turns_json, id],
    )?;
    Ok(())
}

pub fn set_citations(conn: &Connection, id: &str, citations_json: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET citations = ?1 WHERE id = ?2",
        params![citations_json, id],
    )?;
    Ok(())
}

/// Stored turns (raw JSON) of the conversation's messages that have them, by message id.
pub fn turns_by_message(conn: &Connection, conversation_id: &str) -> Result<HashMap<Id, String>> {
    let mut stmt = conn.prepare(
        "SELECT id, turns FROM messages WHERE conversation_id = ?1 AND turns IS NOT NULL",
    )?;
    let rows = stmt.query_map([conversation_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Called at startup: replies that were still streaming when the app stopped.
pub fn recover_interrupted(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE messages SET status = 'interrupted', error = 'The app closed before this reply finished.'
          WHERE status = 'streaming'",
        [],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{projects, test_conn};

    fn conv(conn: &Connection) -> Conversation {
        create_conversation(conn, None, None, "gemini", Some("m")).unwrap()
    }

    #[test]
    fn conversation_lifecycle() {
        let conn = test_conn();
        let c = conv(&conn);
        assert_eq!(c.title, DEFAULT_CONVERSATION_TITLE);
        let c = rename_conversation(&conn, &c.id, "Sleep and memory").unwrap();
        let c = set_archived(&conn, &c.id, true).unwrap();
        assert!(c.archived_at.is_some());

        let f = ConversationFilter::default();
        assert!(list_conversations(&conn, &f).unwrap().is_empty());
        let f = ConversationFilter {
            include_archived: true,
            ..Default::default()
        };
        assert_eq!(list_conversations(&conn, &f).unwrap().len(), 1);

        let c = set_archived(&conn, &c.id, false).unwrap();
        let c = set_model(&conn, &c.id, "openai", "other-model").unwrap();
        assert_eq!(c.model_id.as_deref(), Some("other-model"));
        assert_eq!(c.provider, "openai");
        delete_conversation(&conn, &c.id).unwrap();
        assert!(matches!(
            get_conversation(&conn, &c.id),
            Err(AppError::NotFound)
        ));
    }

    fn reply(
        conn: &Connection,
        conv: &str,
        text: &str,
        status: MessageStatus,
        input: Option<u32>,
    ) -> Message {
        let m = insert_message(
            conn,
            conv,
            Role::Assistant,
            "",
            MessageStatus::Streaming,
            Some("gemini"),
            Some("m"),
        )
        .unwrap();
        finish_message(
            conn,
            &m.id,
            Finish {
                content: text,
                status,
                input_tokens: input,
                output_tokens: Some(5),
                cached_input_tokens: None,
                error: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn switch_cost_follows_the_latest_reply() {
        let conn = test_conn();
        let c = conv(&conn);
        assert_eq!(c.switch_cost_tokens, None, "no replies yet");
        insert_message(
            &conn,
            &c.id,
            Role::User,
            "hi",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        reply(&conn, &c.id, "hello", MessageStatus::Complete, Some(1200));
        assert_eq!(
            get_conversation(&conn, &c.id).unwrap().switch_cost_tokens,
            Some(1200)
        );
        reply(&conn, &c.id, "", MessageStatus::Error, None);
        assert_eq!(
            get_conversation(&conn, &c.id).unwrap().switch_cost_tokens,
            None,
            "when the current model is failing, switching is free of warnings"
        );
        reply(&conn, &c.id, "again", MessageStatus::Complete, Some(2500));
        assert_eq!(
            get_conversation(&conn, &c.id).unwrap().switch_cost_tokens,
            Some(2500)
        );
    }

    #[test]
    fn fork_copies_finished_messages_up_to_a_point() {
        let mut conn = test_conn();
        let p = projects::create_project(&conn, "P").unwrap();
        let c = create_conversation(&conn, Some(&p.id), None, "gemini", Some("m")).unwrap();
        rename_conversation(&conn, &c.id, "Topic").unwrap();
        let mut ids = Vec::new();
        for i in 0..30 {
            let u = insert_message(
                &conn,
                &c.id,
                Role::User,
                &format!("q{i}"),
                MessageStatus::Complete,
                None,
                None,
            )
            .unwrap();
            let a = reply(
                &conn,
                &c.id,
                &format!("a{i}"),
                MessageStatus::Complete,
                Some(900),
            );
            ids.push((u.id, a.id));
        }
        reply(&conn, &c.id, "", MessageStatus::Error, None);

        let whole = fork_conversation(&mut conn, &c.id, None).unwrap();
        assert_eq!(whole.title, "Topic (fork)");
        assert_eq!(whole.project_id.as_deref(), Some(p.id.as_str()));
        assert_eq!(whole.switch_cost_tokens, None, "a fork starts uncached");
        let texts: Vec<_> = list_messages(&conn, &whole.id)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect();
        let expected: Vec<_> = (0..30)
            .flat_map(|i| [format!("q{i}"), format!("a{i}")])
            .collect();
        assert_eq!(
            texts, expected,
            "failed replies are skipped and order is kept"
        );

        let partial = fork_conversation(&mut conn, &c.id, Some(&ids[1].1)).unwrap();
        let texts: Vec<_> = list_messages(&conn, &partial.id)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect();
        assert_eq!(texts, ["q0", "a0", "q1", "a1"]);
        let copied = list_messages(&conn, &partial.id).unwrap();
        assert_eq!(partial.forked_from_title.as_deref(), Some("Topic"));
        assert_eq!(
            partial.fork_boundary_message_id.as_deref(),
            Some(copied.last().unwrap().id.as_str()),
            "the marker points at the last copied message"
        );
        assert_eq!(c.fork_boundary_message_id, None);
        assert_eq!(
            list_messages(&conn, &c.id).unwrap().len(),
            61,
            "the original is untouched"
        );
        assert!(matches!(
            fork_conversation(&mut conn, &c.id, Some("nope")),
            Err(AppError::NotFound)
        ));
    }

    #[test]
    fn citations_saved_before_labels_existed_still_load() {
        let conn = test_conn();
        let conv = create_conversation(&conn, None, None, "anthropic", None).unwrap();
        let m = insert_message(
            &conn,
            &conv.id,
            Role::Assistant,
            "See [S1].",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        let old = r#"[{"handle":1,"status":"ok","quoteVerified":null,"passageId":"p","documentId":"d","title":"T","page":3,"snippet":"s"}]"#;
        set_citations(&conn, &m.id, old).unwrap();
        let got = list_messages(&conn, &conv.id).unwrap();
        assert_eq!(got[0].citations.len(), 1);
        assert_eq!(got[0].citations[0].label, "");
    }

    #[test]
    fn fork_keeps_tool_turns_citations_scope_and_handles() {
        let mut conn = test_conn();
        let c = conv(&conn);
        conn.execute(
            "UPDATE conversations SET source_scope = '{\"kind\":\"library\"}' WHERE id = ?1",
            [&c.id],
        )
        .unwrap();
        insert_message(
            &conn,
            &c.id,
            Role::User,
            "q",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        let a = reply(
            &conn,
            &c.id,
            "answer [S1]",
            MessageStatus::Complete,
            Some(10),
        );
        set_turns(&conn, &a.id, "[{\"role\":\"model\",\"parts\":[]}]").unwrap();
        set_citations(&conn, &a.id, "[{\"handle\":1,\"status\":\"ok\"}]").unwrap();
        let s = sources::NewSource {
            passage_id: "p1",
            document_id: "d1",
            title: "T",
            label: "",
            page: 2,
            text: "x",
        };
        sources::handle_for(&conn, &c.id, &s).unwrap();

        let f = fork_conversation(&mut conn, &c.id, None).unwrap();
        let copied = list_messages(&conn, &f.id).unwrap();
        let turns = turns_by_message(&conn, &f.id).unwrap();
        assert_eq!(turns.len(), 1, "only the reply had turns");
        assert_eq!(turns[&copied[1].id], "[{\"role\":\"model\",\"parts\":[]}]");
        let citations: Option<String> = conn
            .query_row(
                "SELECT citations FROM messages WHERE id = ?1",
                [&copied[1].id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            citations.as_deref(),
            Some("[{\"handle\":1,\"status\":\"ok\"}]")
        );
        let scope: Option<String> = conn
            .query_row(
                "SELECT source_scope FROM conversations WHERE id = ?1",
                [&f.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scope.as_deref(), Some("{\"kind\":\"library\"}"));
        assert_eq!(f.source_scope, Some(Scope::Library));
        assert_eq!(sources::list(&conn, &f.id).unwrap().len(), 1);
    }

    #[test]
    fn a_chosen_scope_is_checked_stored_and_cleared() {
        use crate::domain::library::DocKind;
        let conn = test_conn();
        let loose = conv(&conn);
        assert_eq!(loose.source_scope, None);
        let d = super::super::library::insert_document(&conn, "h1", "a.pdf", DocKind::Pdf).unwrap();

        assert!(
            change_scope(&conn, &loose.id, Some(Scope::Project)).is_err(),
            "no project"
        );
        assert!(
            change_scope(&conn, &loose.id, Some(Scope::Documents { ids: vec![] })).is_err(),
            "empty list"
        );
        assert!(
            change_scope(
                &conn,
                &loose.id,
                Some(Scope::Documents {
                    ids: vec!["nope".into()]
                })
            )
            .is_err(),
            "unknown id"
        );
        assert!(matches!(
            change_scope(&conn, "nope", Some(Scope::Library)),
            Err(AppError::NotFound)
        ));
        assert_eq!(
            get_conversation(&conn, &loose.id).unwrap().source_scope,
            None,
            "a rejected scope changes nothing"
        );

        let picked = change_scope(
            &conn,
            &loose.id,
            Some(Scope::Documents {
                ids: vec![d.id.clone(), d.id.clone()],
            }),
        )
        .unwrap();
        assert_eq!(
            picked.source_scope,
            Some(Scope::Documents {
                ids: vec![d.id.clone()]
            }),
            "duplicates are dropped"
        );
        assert_eq!(
            get_conversation(&conn, &loose.id).unwrap().source_scope,
            picked.source_scope,
            "it is stored"
        );

        let p = crate::persistence::projects::create_project(&conn, "P").unwrap();
        let in_p = create_conversation(&conn, Some(&p.id), None, "gemini", Some("m")).unwrap();
        assert_eq!(
            change_scope(&conn, &in_p.id, Some(Scope::Project))
                .unwrap()
                .source_scope,
            Some(Scope::Project)
        );
        assert_eq!(
            change_scope(&conn, &in_p.id, None).unwrap().source_scope,
            None,
            "back to the default"
        );
    }

    #[test]
    fn filters_by_project() {
        let conn = test_conn();
        let p = projects::create_project(&conn, "P").unwrap();
        create_conversation(&conn, Some(&p.id), None, "gemini", None).unwrap();
        conv(&conn);
        let in_p = ConversationFilter {
            project_id: Some(p.id.clone()),
            ..Default::default()
        };
        assert_eq!(list_conversations(&conn, &in_p).unwrap().len(), 1);
        let loose = ConversationFilter {
            unassigned_only: true,
            ..Default::default()
        };
        assert_eq!(list_conversations(&conn, &loose).unwrap().len(), 1);
        projects::delete_project(&conn, &p.id).unwrap();
        assert_eq!(
            list_conversations(&conn, &Default::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn search_matches_titles_and_message_text_including_norwegian() {
        let conn = test_conn();
        let a = conv(&conn);
        rename_conversation(&conn, &a.id, "Regression notes").unwrap();
        let b = conv(&conn);
        insert_message(
            &conn,
            &b.id,
            Role::User,
            "Hva er forskjellen på årsak og sammenheng?",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        let c = conv(&conn);
        insert_message(
            &conn,
            &c.id,
            Role::User,
            "Unrelated",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();

        let q = |s: &str| {
            let f = ConversationFilter {
                query: Some(s.into()),
                ..Default::default()
            };
            list_conversations(&conn, &f)
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(q("regress"), [a.id.clone()]);
        assert_eq!(q("arsak"), [b.id.clone()], "diacritics are ignored");
        assert_eq!(q("sammenheng"), [b.id.clone()]);
        assert!(
            q("\"quoted\" OR (weird").is_empty(),
            "odd input must not break the query"
        );
        assert!(q("100%").is_empty());
    }

    #[test]
    fn message_search_returns_hits_with_snippets() {
        let conn = test_conn();
        let a = conv(&conn);
        rename_conversation(&conn, &a.id, "Notes").unwrap();
        let m = insert_message(
            &conn,
            &a.id,
            Role::User,
            "Hva er forskjellen på årsak og sammenheng?",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        insert_message(
            &conn,
            &a.id,
            Role::User,
            "Unrelated",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        let archived = conv(&conn);
        let am = insert_message(
            &conn,
            &archived.id,
            Role::Assistant,
            "Årsak is cause.",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        set_archived(&conn, &archived.id, true).unwrap();

        let hits = search_messages(&conn, "arsak", 3, 40).unwrap();
        assert_eq!(hits.len(), 2, "diacritics are ignored");
        let hit = hits.iter().find(|h| h.message_id == m.id).unwrap();
        assert_eq!(hit.conversation_id, a.id);
        assert_eq!(hit.conversation_title, "Notes");
        assert!(!hit.archived);
        assert!(hit.snippet.contains('\u{2}') && hit.snippet.contains('\u{3}'));
        let ah = hits.iter().find(|h| h.message_id == am.id).unwrap();
        assert!(ah.archived, "archived chats are searched");
        assert_eq!(ah.role, Role::Assistant);

        assert!(search_messages(&conn, "   ", 3, 40).unwrap().is_empty());
        assert!(search_messages(&conn, "\"", 3, 40).unwrap().is_empty());
        assert!(
            search_messages(&conn, "\"quoted\" OR (weird", 3, 40)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn message_search_caps_hits_per_chat() {
        let conn = test_conn();
        let a = conv(&conn);
        for i in 0..5 {
            insert_message(
                &conn,
                &a.id,
                Role::User,
                &format!("plankton bloom {i}"),
                MessageStatus::Complete,
                None,
                None,
            )
            .unwrap();
        }
        let b = conv(&conn);
        insert_message(
            &conn,
            &b.id,
            Role::User,
            "plankton too",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();

        let hits = search_messages(&conn, "plankton", 3, 40).unwrap();
        assert_eq!(hits.iter().filter(|h| h.conversation_id == a.id).count(), 3);
        assert_eq!(hits.iter().filter(|h| h.conversation_id == b.id).count(), 1);
        assert_eq!(search_messages(&conn, "plankton", 3, 2).unwrap().len(), 2);
    }

    #[test]
    fn fts_stays_in_sync_when_content_changes() {
        let conn = test_conn();
        let c = conv(&conn);
        let m = insert_message(
            &conn,
            &c.id,
            Role::Assistant,
            "",
            MessageStatus::Streaming,
            Some("gemini"),
            Some("m"),
        )
        .unwrap();
        flush_content(&conn, &m.id, "partial answer about entropy").unwrap();
        let f = ConversationFilter {
            query: Some("entropy".into()),
            ..Default::default()
        };
        assert_eq!(list_conversations(&conn, &f).unwrap().len(), 1);
        finish_message(
            &conn,
            &m.id,
            Finish {
                content: "final answer about enthalpy",
                status: MessageStatus::Complete,
                input_tokens: Some(3),
                output_tokens: Some(5),
                cached_input_tokens: None,
                error: None,
            },
        )
        .unwrap();
        assert!(list_conversations(&conn, &f).unwrap().is_empty());
        let f = ConversationFilter {
            query: Some("enthalpy".into()),
            ..Default::default()
        };
        assert_eq!(list_conversations(&conn, &f).unwrap().len(), 1);
    }

    #[test]
    fn interrupted_replies_are_marked_on_startup() {
        let conn = test_conn();
        let c = conv(&conn);
        let m = insert_message(
            &conn,
            &c.id,
            Role::Assistant,
            "",
            MessageStatus::Streaming,
            None,
            None,
        )
        .unwrap();
        flush_content(&conn, &m.id, "half a sentence").unwrap();
        assert_eq!(recover_interrupted(&conn).unwrap(), 1);
        let m = get_message(&conn, &m.id).unwrap();
        assert_eq!(m.status, MessageStatus::Interrupted);
        assert_eq!(m.content, "half a sentence", "partial text is kept");
        assert_eq!(recover_interrupted(&conn).unwrap(), 0);
    }

    #[test]
    fn messages_keep_order_and_title_comes_from_first_message() {
        let conn = test_conn();
        let c = conv(&conn);
        title_from_first_message(&conn, &c.id, "  What is\n a p-value? ").unwrap();
        assert_eq!(
            get_conversation(&conn, &c.id).unwrap().title,
            "What is a p-value?"
        );
        title_from_first_message(&conn, &c.id, "Something else").unwrap();
        assert_eq!(
            get_conversation(&conn, &c.id).unwrap().title,
            "What is a p-value?"
        );

        for text in ["one", "two", "three"] {
            insert_message(
                &conn,
                &c.id,
                Role::User,
                text,
                MessageStatus::Complete,
                None,
                None,
            )
            .unwrap();
        }
        let texts: Vec<_> = list_messages(&conn, &c.id)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect();
        assert_eq!(texts, ["one", "two", "three"]);
    }

    #[test]
    fn only_a_conversation_without_messages_is_discarded() {
        let conn = test_conn();
        let empty = create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        let used = create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        insert_message(
            &conn,
            &used.id,
            Role::User,
            "hello",
            MessageStatus::Complete,
            None,
            None,
        )
        .unwrap();
        assert!(delete_if_empty(&conn, &empty.id).unwrap());
        assert!(!delete_if_empty(&conn, &used.id).unwrap());
        assert!(get_conversation(&conn, &empty.id).is_err());
        assert!(get_conversation(&conn, &used.id).is_ok());
    }

    #[test]
    fn a_conversation_with_a_draft_is_kept_and_reports_it() {
        let conn = test_conn();
        let c = create_conversation(&conn, None, None, "gemini", Some("m")).unwrap();
        assert!(!c.has_messages && c.draft.is_empty());
        set_draft(&conn, &c.id, "half a thought").unwrap();
        assert!(!delete_if_empty(&conn, &c.id).unwrap());
        let got = get_conversation(&conn, &c.id).unwrap();
        assert_eq!(
            (got.draft.as_str(), got.has_messages),
            ("half a thought", false)
        );
        assert_eq!(got.updated_at, c.updated_at);
        set_draft(&conn, &c.id, "").unwrap();
        assert!(delete_if_empty(&conn, &c.id).unwrap());
        set_draft(&conn, &c.id, "late save").unwrap();
    }
}
