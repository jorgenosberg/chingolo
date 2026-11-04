use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_bool(conn: &Connection, key: &str) -> Result<bool> {
    Ok(get(conn, key)?.as_deref() == Some("true"))
}

pub fn set_bool(conn: &Connection, key: &str, value: bool) -> Result<()> {
    set(conn, key, if value { "true" } else { "false" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_conn;

    #[test]
    fn set_get_overwrite() {
        let conn = test_conn();
        assert_eq!(get(&conn, "k").unwrap(), None);
        set(&conn, "k", "a").unwrap();
        set(&conn, "k", "b").unwrap();
        assert_eq!(get(&conn, "k").unwrap().as_deref(), Some("b"));
        assert!(!get_bool(&conn, "flag").unwrap());
        set_bool(&conn, "flag", true).unwrap();
        assert!(get_bool(&conn, "flag").unwrap());
    }
}
