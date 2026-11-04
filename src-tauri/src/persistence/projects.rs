use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::domain::{Chapter, Id, Project, clean_name, new_id, now_ms};
use crate::error::{AppError, Result};

fn project_from_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
    })
}

fn chapter_from_row(r: &Row) -> rusqlite::Result<Chapter> {
    Ok(Chapter {
        id: r.get(0)?,
        project_id: r.get(1)?,
        title: r.get(2)?,
        position: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

const PROJECT_COLS: &str = "id, name, created_at, updated_at";
const CHAPTER_COLS: &str = "id, project_id, title, position, created_at, updated_at";

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PROJECT_COLS} FROM projects ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map([], project_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn create_project(conn: &Connection, name: &str) -> Result<Project> {
    let name = clean_name(name, "Project name")?;
    let (id, now) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![id, name, now],
    )?;
    get_project(conn, &id)
}

pub fn get_project(conn: &Connection, id: &str) -> Result<Project> {
    conn.query_row(
        &format!("SELECT {PROJECT_COLS} FROM projects WHERE id = ?1"),
        [id],
        project_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

pub fn rename_project(conn: &Connection, id: &str, name: &str) -> Result<Project> {
    let name = clean_name(name, "Project name")?;
    let n = conn.execute(
        "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![name, now_ms(), id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_project(conn, id)
}

/// Deletes the project, its chapters and its conversations.
pub fn delete_project(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub fn list_chapters(conn: &Connection, project_id: &str) -> Result<Vec<Chapter>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHAPTER_COLS} FROM chapters WHERE project_id = ?1 ORDER BY position, id"
    ))?;
    let rows = stmt.query_map([project_id], chapter_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn create_chapter(conn: &Connection, project_id: &str, title: &str) -> Result<Chapter> {
    let title = clean_name(title, "Chapter title")?;
    let (id, now): (Id, i64) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO chapters (id, project_id, title, position, created_at, updated_at)
         VALUES (?1, ?2, ?3,
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM chapters WHERE project_id = ?2),
                 ?4, ?4)",
        params![id, project_id, title, now],
    )?;
    get_chapter(conn, &id)
}

pub fn get_chapter(conn: &Connection, id: &str) -> Result<Chapter> {
    conn.query_row(
        &format!("SELECT {CHAPTER_COLS} FROM chapters WHERE id = ?1"),
        [id],
        chapter_from_row,
    )
    .optional()?
    .ok_or(AppError::NotFound)
}

pub fn rename_chapter(conn: &Connection, id: &str, title: &str) -> Result<Chapter> {
    let title = clean_name(title, "Chapter title")?;
    let n = conn.execute(
        "UPDATE chapters SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now_ms(), id],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    get_chapter(conn, id)
}

/// Conversations in the chapter stay in the project (their chapter link is cleared).
pub fn delete_chapter(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM chapters WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_conn;

    #[test]
    fn project_crud() {
        let conn = test_conn();
        let p = create_project(&conn, "  Thesis ").unwrap();
        assert_eq!(p.name, "Thesis");
        let p = rename_project(&conn, &p.id, "Master thesis").unwrap();
        assert_eq!(list_projects(&conn).unwrap()[0].name, p.name);
        delete_project(&conn, &p.id).unwrap();
        assert!(list_projects(&conn).unwrap().is_empty());
        assert!(matches!(
            delete_project(&conn, &p.id),
            Err(AppError::NotFound)
        ));
    }

    #[test]
    fn chapters_are_ordered_and_cascade() {
        let conn = test_conn();
        let p = create_project(&conn, "P").unwrap();
        let a = create_chapter(&conn, &p.id, "Intro").unwrap();
        let b = create_chapter(&conn, &p.id, "Methods").unwrap();
        assert_eq!((a.position, b.position), (0, 1));
        let titles: Vec<_> = list_chapters(&conn, &p.id)
            .unwrap()
            .into_iter()
            .map(|c| c.title)
            .collect();
        assert_eq!(titles, ["Intro", "Methods"]);
        delete_project(&conn, &p.id).unwrap();
        assert!(list_chapters(&conn, &p.id).unwrap().is_empty());
    }

    #[test]
    fn chapter_requires_existing_project() {
        let conn = test_conn();
        assert!(create_chapter(&conn, "missing", "X").is_err());
    }
}
