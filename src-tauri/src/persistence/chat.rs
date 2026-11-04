use rusqlite::{Connection, OptionalExtension, Row, ToSql, params, params_from_iter};

use crate::domain::{
    Conversation, ConversationFilter, DEFAULT_CONVERSATION_TITLE, Id, Message, MessageStatus,
    Role, clean_name, new_id, now_ms,
};
use crate::error::{AppError, Result};

const CONV_COLS: &str = "id, project_id, chapter_id, title, provider, model_id, archived_at, created_at, updated_at,
    EXISTS(SELECT 1 FROM messages m WHERE m.conversation_id = conversations.id
           AND m.role = 'assistant' AND (m.status <> 'error' OR m.content <> ''))";
const MSG_COLS: &str = "id, conversation_id, role, content, status, provider, model_id, input_tokens, output_tokens, error, created_at, updated_at";

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
        model_locked: r.get(9)?,
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
        params![id, project_id, chapter_id, DEFAULT_CONVERSATION_TITLE, provider, model_id, now],
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

/// Switching models mid-conversation makes the new model re-read the whole history with no
/// prompt cache. Once a model has produced a reply, the conversation keeps it.
pub fn change_model(conn: &Connection, id: &str, model_id: &str) -> Result<Conversation> {
    if get_conversation(conn, id)?.model_locked {
        return Err(AppError::Invalid(
            "This conversation already has a reply, so its model is fixed. Start a new conversation to use another model."
                .into(),
        ));
    }
    set_model(conn, id, model_id)
}

pub fn set_model(conn: &Connection, id: &str, model_id: &str) -> Result<Conversation> {
    let n = conn.execute(
        "UPDATE conversations SET model_id = ?1 WHERE id = ?2",
        params![model_id, id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_conversation(conn, id)
}

pub fn delete_conversation(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
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
        params![id, conversation_id, role.as_str(), content, status.as_str(), provider, model_id, now],
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
    pub error: Option<&'a str>,
}

pub fn finish_message(conn: &Connection, id: &str, f: Finish) -> Result<Message> {
    conn.execute(
        "UPDATE messages
            SET content = ?1, status = ?2, input_tokens = ?3, output_tokens = ?4,
                error = ?5, updated_at = ?6
          WHERE id = ?7",
        params![f.content, f.status.as_str(), f.input_tokens, f.output_tokens, f.error, now_ms(), id],
    )?;
    let msg = get_message(conn, id)?;
    touch_conversation(conn, &msg.conversation_id)?;
    Ok(msg)
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
        let f = ConversationFilter { include_archived: true, ..Default::default() };
        assert_eq!(list_conversations(&conn, &f).unwrap().len(), 1);

        let c = set_archived(&conn, &c.id, false).unwrap();
        let c = set_model(&conn, &c.id, "other-model").unwrap();
        assert_eq!(c.model_id.as_deref(), Some("other-model"));
        delete_conversation(&conn, &c.id).unwrap();
        assert!(matches!(get_conversation(&conn, &c.id), Err(AppError::NotFound)));
    }

    #[test]
    fn model_is_locked_after_a_reply_but_not_after_a_failed_one() {
        let conn = test_conn();
        let c = conv(&conn);
        assert!(!c.model_locked);
        insert_message(&conn, &c.id, Role::User, "hi", MessageStatus::Complete, None, None).unwrap();
        let failed = insert_message(&conn, &c.id, Role::Assistant, "", MessageStatus::Streaming, None, None).unwrap();
        finish_message(&conn, &failed.id, Finish { content: "", status: MessageStatus::Error, input_tokens: None, output_tokens: None, error: Some("503") }).unwrap();
        assert!(change_model(&conn, &c.id, "other").is_ok(), "a failed first attempt must not lock the model");

        let ok = insert_message(&conn, &c.id, Role::Assistant, "", MessageStatus::Streaming, None, None).unwrap();
        finish_message(&conn, &ok.id, Finish { content: "hello", status: MessageStatus::Complete, input_tokens: None, output_tokens: None, error: None }).unwrap();
        assert!(get_conversation(&conn, &c.id).unwrap().model_locked);
        assert!(change_model(&conn, &c.id, "third").is_err());
        assert_eq!(get_conversation(&conn, &c.id).unwrap().model_id.as_deref(), Some("other"));
    }

    #[test]
    fn filters_by_project() {
        let conn = test_conn();
        let p = projects::create_project(&conn, "P").unwrap();
        create_conversation(&conn, Some(&p.id), None, "gemini", None).unwrap();
        conv(&conn);
        let in_p = ConversationFilter { project_id: Some(p.id.clone()), ..Default::default() };
        assert_eq!(list_conversations(&conn, &in_p).unwrap().len(), 1);
        let loose = ConversationFilter { unassigned_only: true, ..Default::default() };
        assert_eq!(list_conversations(&conn, &loose).unwrap().len(), 1);
        projects::delete_project(&conn, &p.id).unwrap();
        assert_eq!(list_conversations(&conn, &Default::default()).unwrap().len(), 1);
    }

    #[test]
    fn search_matches_titles_and_message_text_including_norwegian() {
        let conn = test_conn();
        let a = conv(&conn);
        rename_conversation(&conn, &a.id, "Regression notes").unwrap();
        let b = conv(&conn);
        insert_message(&conn, &b.id, Role::User, "Hva er forskjellen på årsak og sammenheng?", MessageStatus::Complete, None, None).unwrap();
        let c = conv(&conn);
        insert_message(&conn, &c.id, Role::User, "Unrelated", MessageStatus::Complete, None, None).unwrap();

        let q = |s: &str| {
            let f = ConversationFilter { query: Some(s.into()), ..Default::default() };
            list_conversations(&conn, &f).unwrap().into_iter().map(|c| c.id).collect::<Vec<_>>()
        };
        assert_eq!(q("regress"), [a.id.clone()]);
        assert_eq!(q("arsak"), [b.id.clone()], "diacritics are ignored");
        assert_eq!(q("sammenheng"), [b.id.clone()]);
        assert!(q("\"quoted\" OR (weird").is_empty(), "odd input must not break the query");
        assert!(q("100%").is_empty());
    }

    #[test]
    fn fts_stays_in_sync_when_content_changes() {
        let conn = test_conn();
        let c = conv(&conn);
        let m = insert_message(&conn, &c.id, Role::Assistant, "", MessageStatus::Streaming, Some("gemini"), Some("m")).unwrap();
        flush_content(&conn, &m.id, "partial answer about entropy").unwrap();
        let f = ConversationFilter { query: Some("entropy".into()), ..Default::default() };
        assert_eq!(list_conversations(&conn, &f).unwrap().len(), 1);
        finish_message(&conn, &m.id, Finish { content: "final answer about enthalpy", status: MessageStatus::Complete, input_tokens: Some(3), output_tokens: Some(5), error: None }).unwrap();
        assert!(list_conversations(&conn, &f).unwrap().is_empty());
        let f = ConversationFilter { query: Some("enthalpy".into()), ..Default::default() };
        assert_eq!(list_conversations(&conn, &f).unwrap().len(), 1);
    }

    #[test]
    fn interrupted_replies_are_marked_on_startup() {
        let conn = test_conn();
        let c = conv(&conn);
        let m = insert_message(&conn, &c.id, Role::Assistant, "", MessageStatus::Streaming, None, None).unwrap();
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
        assert_eq!(get_conversation(&conn, &c.id).unwrap().title, "What is a p-value?");
        title_from_first_message(&conn, &c.id, "Something else").unwrap();
        assert_eq!(get_conversation(&conn, &c.id).unwrap().title, "What is a p-value?");

        for text in ["one", "two", "three"] {
            insert_message(&conn, &c.id, Role::User, text, MessageStatus::Complete, None, None).unwrap();
        }
        let texts: Vec<_> = list_messages(&conn, &c.id).unwrap().into_iter().map(|m| m.content).collect();
        assert_eq!(texts, ["one", "two", "three"]);
    }
}
