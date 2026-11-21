//! Passage vectors. They sit in a plain table and are scored in Rust: a few thousand
//! 768-dimension vectors scan in milliseconds, and a normal table follows the passage
//! cascade that deletes them with their document.

use rusqlite::{Connection, TransactionBehavior, params, params_from_iter};

use crate::domain::Id;
use crate::domain::library::DocFilter;
use crate::error::Result;
use crate::persistence::library::scope_clause;

/// A passage waiting for its vector.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub passage_id: Id,
    pub document_id: Id,
    pub text: String,
}

pub fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn decode(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Drops vectors made by another model or revision, so a model change re-embeds everything.
pub fn purge_stale(conn: &Connection, model: &str, revision: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM passage_vectors WHERE model <> ?1 OR revision <> ?2",
        params![model, revision],
    )?)
}

/// Passages of ready documents without a vector for this model. Oldest document first, then in reading order.
pub fn pending_batch(
    conn: &Connection,
    model: &str,
    revision: &str,
    n: u32,
) -> Result<Vec<Pending>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.document_id, p.text
           FROM passages p
           JOIN documents d ON d.id = p.document_id
          WHERE d.status = 'ready'
            AND NOT EXISTS (SELECT 1 FROM passage_vectors v
                             WHERE v.passage_id = p.id AND v.model = ?1 AND v.revision = ?2)
          ORDER BY d.created_at, d.id, p.ordinal
          LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![model, revision, n], |r| {
        Ok(Pending {
            passage_id: r.get(0)?,
            document_id: r.get(1)?,
            text: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Stores vectors in one transaction. Passages deleted since they were picked are skipped.
/// Returns how many rows were written.
pub fn store_vectors(
    conn: &mut Connection,
    items: &[(Id, Vec<f32>)],
    model: &str,
    revision: &str,
) -> Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut written = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO passage_vectors (passage_id, model, revision, vec)
             SELECT id, ?2, ?3, ?4 FROM passages WHERE id = ?1",
        )?;
        for (id, v) in items {
            written += stmt.execute(params![id, model, revision, encode(v)])?;
        }
    }
    tx.commit()?;
    Ok(written)
}

/// Passages of ready documents, and how many of them have a vector for this model.
pub fn totals(conn: &Connection, model: &str, revision: &str) -> Result<(u32, u32)> {
    Ok(conn.query_row(
        "SELECT count(*),
                count(v.passage_id)
           FROM passages p
           JOIN documents d ON d.id = p.document_id AND d.status = 'ready'
           LEFT JOIN passage_vectors v
             ON v.passage_id = p.id AND v.model = ?1 AND v.revision = ?2",
        params![model, revision],
        |r| Ok((r.get(1)?, r.get(0)?)),
    )?)
}

/// A ranked passage id with its similarity to the query.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub passage_id: Id,
    pub score: f32,
}

/// Cosine similarity (a dot product, vectors are normalized) against every vector of ready
/// documents in scope. Best first.
pub fn scan(
    conn: &Connection,
    query: &[f32],
    model: &str,
    revision: &str,
    filter: &DocFilter,
    limit: usize,
) -> Result<Vec<Scored>> {
    let (scope, scope_args) = scope_clause(filter, "d.id", 3);
    let sql = format!(
        "SELECT v.passage_id, v.vec
           FROM passage_vectors v
           JOIN passages p ON p.id = v.passage_id
           JOIN documents d ON d.id = p.document_id
          WHERE d.status = 'ready' AND v.model = ?1 AND v.revision = ?2 {scope}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&model, &revision];
    args.extend(scope_args.iter().map(|a| a as &dyn rusqlite::ToSql));
    let mut out: Vec<Scored> = Vec::new();
    let rows = stmt.query_map(params_from_iter(args), |r| {
        Ok((r.get::<_, Id>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    for row in rows {
        let (passage_id, blob) = row?;
        let v = decode(&blob);
        if v.len() != query.len() {
            continue;
        }
        let score: f32 = v.iter().zip(query).map(|(a, b)| a * b).sum();
        out.push(Scored { passage_id, score });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(limit);
    Ok(out)
}

/// The passages around one passage in the same document (`ordinal ± k`), in reading order,
/// not including the passage itself.
pub fn neighbours(conn: &Connection, passage_id: &str, k: u32) -> Result<Vec<Id>> {
    let mut stmt = conn.prepare(
        "SELECT n.id
           FROM passages p
           JOIN passages n ON n.document_id = p.document_id
                          AND n.ordinal <> p.ordinal
                          AND abs(n.ordinal - p.ordinal) <= ?2
          WHERE p.id = ?1
          ORDER BY n.ordinal",
    )?;
    let rows = stmt.query_map(params![passage_id, k], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::library::{DocKind, DocStatus};
    use crate::persistence::library::{self as lib, Extracted, NewPage, NewPassage};
    use crate::persistence::{projects, test_conn};

    const M: &str = "m";
    const R: &str = "r1";

    fn doc(conn: &mut Connection, hash: &str, texts: &[&str]) -> (Id, Vec<Id>) {
        let d = lib::insert_document(conn, hash, &format!("{hash}.pdf"), DocKind::Pdf).unwrap();
        let passages: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| NewPassage {
                page_start: i as u32 + 1,
                page_end: i as u32 + 1,
                text: (*t).into(),
                boxes: None,
            })
            .collect();
        let pages = vec![NewPage {
            page_no: 1,
            width: 1.0,
            height: 1.0,
            quality: "ok",
            ocr: false,
        }];
        lib::store_content(
            conn,
            &d.id,
            &pages,
            &passages,
            &Extracted::default(),
            DocStatus::Ready,
            None,
        )
        .unwrap();
        let ids = conn
            .prepare("SELECT id FROM passages WHERE document_id = ?1 ORDER BY ordinal")
            .unwrap()
            .query_map([&d.id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (d.id, ids)
    }

    fn unit(x: f32, y: f32) -> Vec<f32> {
        let n = (x * x + y * y).sqrt();
        vec![x / n, y / n]
    }

    #[test]
    fn vectors_round_trip_through_bytes() {
        let v = vec![0.5, -1.25, 3.0e-7];
        assert_eq!(decode(&encode(&v)), v);
    }

    #[test]
    fn pending_follows_document_then_reading_order_and_shrinks_as_vectors_arrive() {
        let mut conn = test_conn();
        let (_, a) = doc(&mut conn, "a", &["one", "two", "three"]);
        let (_, b) = doc(&mut conn, "b", &["four"]);
        let pending = pending_batch(&conn, M, R, 10).unwrap();
        let ids: Vec<_> = pending.iter().map(|p| p.passage_id.clone()).collect();
        assert_eq!(
            ids,
            [a[0].clone(), a[1].clone(), a[2].clone(), b[0].clone()]
        );
        assert_eq!(pending_batch(&conn, M, R, 2).unwrap().len(), 2);

        store_vectors(&mut conn, &[(a[0].clone(), vec![1.0])], M, R).unwrap();
        assert_eq!(pending_batch(&conn, M, R, 10).unwrap().len(), 3);
        assert_eq!(totals(&conn, M, R).unwrap(), (1, 4));
    }

    #[test]
    fn a_new_model_revision_makes_everything_pending_again() {
        let mut conn = test_conn();
        let (_, a) = doc(&mut conn, "a", &["one", "two"]);
        store_vectors(
            &mut conn,
            &a.iter()
                .map(|id| (id.clone(), vec![1.0]))
                .collect::<Vec<_>>(),
            M,
            R,
        )
        .unwrap();
        assert!(pending_batch(&conn, M, R, 10).unwrap().is_empty());
        assert_eq!(pending_batch(&conn, M, "r2", 10).unwrap().len(), 2);
        assert_eq!(purge_stale(&conn, M, "r2").unwrap(), 2);
        assert_eq!(totals(&conn, M, "r2").unwrap(), (0, 2));
    }

    #[test]
    fn documents_that_are_not_ready_are_not_pending() {
        let mut conn = test_conn();
        let (d, _) = doc(&mut conn, "a", &["one"]);
        lib::set_status(&conn, &d, DocStatus::Error, Some("x")).unwrap();
        assert!(pending_batch(&conn, M, R, 10).unwrap().is_empty());
    }

    #[test]
    fn storing_skips_passages_deleted_in_the_meantime() {
        let mut conn = test_conn();
        let (d, a) = doc(&mut conn, "a", &["one"]);
        lib::delete_document(&conn, &d).unwrap();
        assert_eq!(
            store_vectors(&mut conn, &[(a[0].clone(), vec![1.0])], M, R).unwrap(),
            0
        );
    }

    #[test]
    fn vectors_go_with_their_document_and_with_a_re_read() {
        let mut conn = test_conn();
        let (d, a) = doc(&mut conn, "a", &["one", "two"]);
        let all = |c: &Connection| {
            c.query_row("SELECT count(*) FROM passage_vectors", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
        };
        store_vectors(
            &mut conn,
            &a.iter()
                .map(|id| (id.clone(), vec![1.0]))
                .collect::<Vec<_>>(),
            M,
            R,
        )
        .unwrap();
        assert_eq!(all(&conn), 2);

        // re-reading the file replaces the passages, so their vectors must go
        let again = [NewPassage {
            page_start: 1,
            page_end: 1,
            text: "new".into(),
            boxes: None,
        }];
        lib::store_content(
            &mut conn,
            &d,
            &[],
            &again,
            &Extracted::default(),
            DocStatus::Ready,
            None,
        )
        .unwrap();
        assert_eq!(all(&conn), 0);
        assert_eq!(pending_batch(&conn, M, R, 10).unwrap().len(), 1);

        let (_, b) = doc(&mut conn, "b", &["x"]);
        store_vectors(&mut conn, &[(b[0].clone(), vec![1.0])], M, R).unwrap();
        lib::delete_document(&conn, &d).unwrap();
        assert_eq!(all(&conn), 1);
    }

    #[test]
    fn scan_ranks_by_similarity_and_respects_model_status_and_scope() {
        let mut conn = test_conn();
        let (da, a) = doc(&mut conn, "a", &["near", "far"]);
        let (db, b) = doc(&mut conn, "b", &["middle"]);
        let items = vec![
            (a[0].clone(), unit(1.0, 0.1)),
            (a[1].clone(), unit(0.0, 1.0)),
            (b[0].clone(), unit(1.0, 1.0)),
        ];
        store_vectors(&mut conn, &items, M, R).unwrap();
        let q = unit(1.0, 0.0);

        let hits = scan(&conn, &q, M, R, &DocFilter::All, 10).unwrap();
        let order: Vec<_> = hits.iter().map(|h| h.passage_id.clone()).collect();
        assert_eq!(order, [a[0].clone(), b[0].clone(), a[1].clone()]);
        assert!(hits[0].score > hits[1].score && hits[1].score > hits[2].score);
        assert_eq!(scan(&conn, &q, M, R, &DocFilter::All, 1).unwrap().len(), 1);
        assert!(
            scan(&conn, &q, M, "other", &DocFilter::All, 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            scan(&conn, &[1.0, 0.0, 0.0], M, R, &DocFilter::All, 10)
                .unwrap()
                .is_empty(),
            "wrong size is ignored"
        );

        let p = projects::create_project(&conn, "P").unwrap();
        lib::attach(&conn, &p.id, &db).unwrap();
        let scoped = scan(&conn, &q, M, R, &DocFilter::Project(p.id.clone()), 10).unwrap();
        assert_eq!(
            scoped
                .iter()
                .map(|h| h.passage_id.clone())
                .collect::<Vec<_>>(),
            [b[0].clone()]
        );

        lib::set_status(&conn, &da, DocStatus::Error, None).unwrap();
        assert_eq!(scan(&conn, &q, M, R, &DocFilter::All, 10).unwrap().len(), 1);
    }

    #[test]
    fn neighbours_stay_inside_the_document_and_stop_at_the_edges() {
        let mut conn = test_conn();
        let (_, a) = doc(&mut conn, "a", &["0", "1", "2", "3"]);
        let (_, b) = doc(&mut conn, "b", &["x", "y"]);
        assert_eq!(neighbours(&conn, &a[0], 1).unwrap(), [a[1].clone()]);
        assert_eq!(
            neighbours(&conn, &a[2], 1).unwrap(),
            [a[1].clone(), a[3].clone()]
        );
        assert_eq!(
            neighbours(&conn, &a[3], 2).unwrap(),
            [a[1].clone(), a[2].clone()]
        );
        assert_eq!(neighbours(&conn, &b[1], 5).unwrap(), [b[0].clone()]);
        assert!(neighbours(&conn, "missing", 1).unwrap().is_empty());
    }
}
