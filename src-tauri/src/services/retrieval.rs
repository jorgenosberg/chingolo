//! Hybrid search: keyword ranking (BM25) and meaning ranking (vectors) merged by reciprocal
//! rank fusion. Either list alone is fine when the other is empty.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::domain::Id;
use crate::domain::library::{DocFilter, MatchKind, SearchHit};
use crate::error::Result;
use crate::persistence::library::{self as repo, SearchOptions};
use crate::persistence::vectors;

/// The constant from the original paper, and the one the benchmark used.
const RRF_K: f32 = 60.0;
/// How many candidates each ranking contributes.
const POOL: u32 = 50;

/// Merges two ranked lists of passage ids. A passage scores `1 / (60 + rank)` in each list it
/// appears in (rank starts at 1). Ties keep the keyword order.
pub fn fuse(keyword: &[Id], meaning: &[Id]) -> Vec<(Id, MatchKind)> {
    let mut seen: HashMap<&str, (f32, bool, bool, usize)> = HashMap::new();
    for (list_is_meaning, list) in [(false, keyword), (true, meaning)] {
        for (i, id) in list.iter().enumerate() {
            let next = seen.len();
            let e = seen.entry(id).or_insert((0.0, false, false, next));
            e.0 += 1.0 / (RRF_K + (i + 1) as f32);
            if list_is_meaning {
                e.2 = true;
            } else {
                e.1 = true;
            }
        }
    }
    let mut all: Vec<_> = seen.into_iter().collect();
    all.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.1.3.cmp(&b.1.3)));
    all.into_iter()
        .map(|(id, (_, kw, vec, _))| {
            let kind = match (kw, vec) {
                (true, true) => MatchKind::Both,
                (true, false) => MatchKind::Keyword,
                _ => MatchKind::Meaning,
            };
            (id.to_string(), kind)
        })
        .collect()
}

/// Passages that match the question's words or its meaning, best first. `query_vec` is the
/// embedded query (already normalized). `identity` is the model and revision that made it.
pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    query_vec: &[f32],
    identity: (&str, &str),
    filter: &DocFilter,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    let opts = SearchOptions { filter, all_terms: false, prefix_last: false, limit: POOL };
    let keyword = repo::search_passages(conn, query, &opts)?;
    let meaning = vectors::scan(conn, query_vec, identity.0, identity.1, filter, POOL as usize)?;

    let keyword_ids: Vec<Id> = keyword.iter().map(|h| h.passage_id.clone()).collect();
    let meaning_ids: Vec<Id> = meaning.into_iter().map(|s| s.passage_id).collect();
    let mut known: HashMap<Id, SearchHit> = keyword.into_iter().map(|h| (h.passage_id.clone(), h)).collect();

    let ranked = fuse(&keyword_ids, &meaning_ids);
    let missing: Vec<Id> = ranked.iter().filter(|(id, _)| !known.contains_key(id)).map(|(id, _)| id.clone()).collect();
    for hit in repo::hits_for_ids(conn, &missing, MatchKind::Meaning)? {
        known.insert(hit.passage_id.clone(), hit);
    }
    Ok(ranked
        .into_iter()
        .filter_map(|(id, kind)| {
            known.remove(&id).map(|mut h| {
                h.matched_by = kind;
                h
            })
        })
        .take(limit as usize)
        .collect())
}

/// The passages around a hit, in reading order, so an answer isn't cut off mid-argument.
/// Slice 8's retrieval tools use this.
#[allow(dead_code)]
pub fn neighbour_passages(conn: &Connection, passage_id: &str, k: u32) -> Result<Vec<SearchHit>> {
    let ids = vectors::neighbours(conn, passage_id, k)?;
    repo::hits_for_ids(conn, &ids, MatchKind::Context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::library::{DocKind, DocStatus};
    use crate::persistence::library::{Extracted, NewPage, NewPassage};
    use crate::persistence::test_conn;
    use crate::services::embedding::{MODEL_NAME, model::REVISION};

    fn ids(v: &[&str]) -> Vec<Id> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fusion_matches_a_hand_computed_case() {
        // keyword: a, b, c   meaning: c, a, d
        // a: 1/61 + 1/62   b: 1/62   c: 1/63 + 1/61   d: 1/63
        let out = fuse(&ids(&["a", "b", "c"]), &ids(&["c", "a", "d"]));
        let got: Vec<_> = out.iter().map(|(i, k)| (i.as_str(), *k)).collect();
        assert_eq!(
            got,
            [
                ("a", MatchKind::Both), // 0.03252
                ("c", MatchKind::Both), // 0.03227
                ("b", MatchKind::Keyword),
                ("d", MatchKind::Meaning),
            ]
        );
    }

    #[test]
    fn a_passage_in_both_lists_beats_one_that_tops_a_single_list() {
        let out = fuse(&ids(&["x", "shared"]), &ids(&["y", "shared"]));
        assert_eq!(out[0].0, "shared");
        // x and y tie at 1/61: keyword order decides
        assert_eq!((out[1].0.as_str(), out[2].0.as_str()), ("x", "y"));
    }

    #[test]
    fn one_empty_list_keeps_the_other_order() {
        let out = fuse(&ids(&["a", "b"]), &[]);
        assert_eq!(out, [("a".to_string(), MatchKind::Keyword), ("b".to_string(), MatchKind::Keyword)]);
        let out = fuse(&[], &ids(&["m", "n"]));
        assert_eq!(out, [("m".to_string(), MatchKind::Meaning), ("n".to_string(), MatchKind::Meaning)]);
        assert!(fuse(&[], &[]).is_empty());
    }

    fn doc(conn: &mut Connection, hash: &str, texts: &[&str]) -> Vec<Id> {
        let d = repo::insert_document(conn, hash, &format!("{hash}.pdf"), DocKind::Pdf).unwrap();
        let passages: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| NewPassage { page_start: i as u32 + 1, page_end: i as u32 + 1, text: (*t).into(), boxes: None })
            .collect();
        let pages = vec![NewPage { page_no: 1, width: 1.0, height: 1.0, quality: "ok" }];
        repo::store_content(conn, &d.id, &pages, &passages, &Extracted::default(), DocStatus::Ready, None).unwrap();
        conn.prepare("SELECT id FROM passages WHERE document_id = ?1 ORDER BY ordinal")
            .unwrap()
            .query_map([&d.id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_meaning_match_with_no_shared_words_is_found_and_labelled() {
        let mut conn = test_conn();
        let a = doc(&mut conn, "a", &["Corals expel their algae under heat stress", "Gill tissue was stained blue"]);
        // axis 0 = "reef health", axis 1 = "lab method"
        let items = vec![(a[0].clone(), vec![1.0, 0.0]), (a[1].clone(), vec![0.0, 1.0])];
        vectors::store_vectors(&mut conn, &items, MODEL_NAME, REVISION).unwrap();

        // no word of the question appears in either passage
        let hits = hybrid_search(&conn, "why do reefs lose their colour", &[1.0, 0.0], (MODEL_NAME, REVISION), &DocFilter::All, 10).unwrap();
        assert_eq!(hits.first().map(|h| h.passage_id.as_str()), Some(a[0].as_str()));
        assert_eq!(hits[0].matched_by, MatchKind::Meaning);

        // a keyword hit that is also close in meaning is labelled Both and goes first
        let hits = hybrid_search(&conn, "heat stress corals", &[1.0, 0.0], (MODEL_NAME, REVISION), &DocFilter::All, 10).unwrap();
        assert_eq!((hits[0].passage_id.as_str(), hits[0].matched_by), (a[0].as_str(), MatchKind::Both));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].matched_by, MatchKind::Meaning);
    }

    #[test]
    fn question_filler_words_do_not_drive_the_keyword_side() {
        let mut conn = test_conn();
        let a = doc(&mut conn, "a", &["what is the answer to the question", "iron limits plankton growth"]);
        let opts = SearchOptions { filter: &DocFilter::All, all_terms: false, prefix_last: false, limit: 10 };
        let hits = repo::search_passages(&conn, "What limits the plankton?", &opts).unwrap();
        assert_eq!(hits.iter().map(|h| h.passage_id.as_str()).collect::<Vec<_>>(), [a[1].as_str()]);
        // a query of only common words still searches, instead of returning nothing
        assert_eq!(repo::search_passages(&conn, "what is the", &opts).unwrap().len(), 1);
    }

    #[test]
    fn limit_and_project_scope_apply_to_the_merged_list() {
        let mut conn = test_conn();
        let a = doc(&mut conn, "a", &["alpha text", "beta text", "gamma text"]);
        let b = doc(&mut conn, "b", &["delta text"]);
        let mut items: Vec<_> = a.iter().chain(&b).map(|id| (id.clone(), vec![1.0, 0.0])).collect();
        items[3].1 = vec![0.9, 0.1];
        vectors::store_vectors(&mut conn, &items, MODEL_NAME, REVISION).unwrap();
        assert_eq!(hybrid_search(&conn, "text", &[1.0, 0.0], (MODEL_NAME, REVISION), &DocFilter::All, 2).unwrap().len(), 2);

        let p = crate::persistence::projects::create_project(&conn, "P").unwrap();
        let doc_b: String = conn.query_row("SELECT document_id FROM passages WHERE id = ?1", [&b[0]], |r| r.get(0)).unwrap();
        repo::attach(&conn, &p.id, &doc_b).unwrap();
        let hits = hybrid_search(&conn, "text", &[1.0, 0.0], (MODEL_NAME, REVISION), &DocFilter::Project(p.id.clone()), 10).unwrap();
        assert_eq!(hits.iter().map(|h| h.passage_id.as_str()).collect::<Vec<_>>(), [b[0].as_str()]);
    }

    #[test]
    fn neighbours_come_back_as_context_in_reading_order() {
        let mut conn = test_conn();
        let a = doc(&mut conn, "a", &["one", "two", "three"]);
        let near = neighbour_passages(&conn, &a[1], 1).unwrap();
        assert_eq!(near.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(), ["one", "three"]);
        assert!(near.iter().all(|h| h.matched_by == MatchKind::Context));
    }
}
