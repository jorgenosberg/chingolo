//! Retrieval benchmark: embedding models and BM25 over passages built from the pdf_spike output.
//!
//! Env: CHINGUI_BENCH_DIR  folder holding bench.json (the question set, next to the test papers)
//!      CHINGUI_BENCH_WORK scratch folder with spike-out/ (from `cargo run --example pdf_spike`),
//!                         models/ (model cache, models/granite97 for Granite) and bench-out/
//! Usage: retrieval-bench <bm25|e5s|e5b|gemma|gemma4|granite|hybrid:<model>|app> [out_dir]
//! Dense runs save rankings, so run `bm25` and the model before `hybrid:<model>`.
//!
//! `app` measures what the app built: passages and stored vectors come from its own database
//! (CHINGUI_BENCH_DB=<path to chingui.sqlite>, read only), so the real segmenter is used.
//! Queries are embedded with the same model. It reports BM25, vectors and hybrid. The papers
//! must have been imported and fully indexed in the app, and the file names must match the
//! ones in bench.json. CHINGUI_BENCH_WORK still names the model cache and output folder.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use fastembed::{
    AutoDevicePolicy, EmbeddingModel, InitOptionsUserDefined, Pooling, TextEmbedding, TextInitOptions,
    TokenizerFiles, UserDefinedEmbeddingModel,
};
use serde_json::{json, Value};

fn base() -> String {
    std::env::var("CHINGUI_BENCH_WORK").expect("set CHINGUI_BENCH_WORK")
}
const DISTRACTOR: &str = "science.1082000.pdf";
const PASSAGE_CHARS: usize = 1200;

struct Passage {
    file: String,
    page: u64,
    /// Last page of the passage (the same as `page` for the spike output).
    page_end: u64,
    text: String,
    /// Stored vector, only for passages read from the app database.
    vec: Option<Vec<f32>>,
}

fn load_passages() -> Vec<Passage> {
    let base = base();
    let mut paths: Vec<_> = fs::read_dir(format!("{base}/spike-out"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let d: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        let file = d["file"].as_str().unwrap().to_string();
        for pg in d["pages"].as_array().unwrap() {
            if pg["quality"] == "none" {
                continue;
            }
            let page = pg["page"].as_u64().unwrap();
            let mut cur = String::new();
            for l in pg["lines"].as_array().unwrap() {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(l["text"].as_str().unwrap());
                if cur.len() >= PASSAGE_CHARS {
                    out.push(Passage { file: file.clone(), page, page_end: page, text: std::mem::take(&mut cur), vec: None });
                }
            }
            if cur.trim().len() >= 40 {
                out.push(Passage { file: file.clone(), page, page_end: page, text: cur, vec: None });
            }
        }
    }
    out
}

/// Passages of ready documents from the app database, in reading order, with their vectors.
fn load_passages_db() -> Vec<Passage> {
    let path = std::env::var("CHINGUI_BENCH_DB").expect("set CHINGUI_BENCH_DB");
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut st = conn
        .prepare(
            "SELECT d.original_name, p.page_start, p.page_end, p.text, v.vec
               FROM passages p
               JOIN documents d ON d.id = p.document_id AND d.status = 'ready'
               LEFT JOIN passage_vectors v ON v.passage_id = p.id
              ORDER BY d.created_at, d.id, p.ordinal",
        )
        .unwrap();
    st.query_map([], |r| {
        let blob: Option<Vec<u8>> = r.get(4)?;
        Ok(Passage {
            file: r.get(0)?,
            page: r.get::<_, i64>(1)? as u64,
            page_end: r.get::<_, i64>(2)? as u64,
            text: r.get(3)?,
            vec: blob.map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()),
        })
    })
    .unwrap()
    .map(|x| x.unwrap())
    .collect()
}

type Ranking = Vec<(usize, f32)>;

fn bm25(passages: &[Passage], queries: &[String]) -> Vec<Ranking> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE VIRTUAL TABLE fts USING fts5(text, tokenize='unicode61 remove_diacritics 2')").unwrap();
    {
        let mut ins = conn.prepare("INSERT INTO fts(rowid, text) VALUES (?1, ?2)").unwrap();
        for (i, p) in passages.iter().enumerate() {
            ins.execute((i as i64, &p.text)).unwrap();
        }
    }
    let stop: HashSet<&str> = "the and for are was were what how when why which who does did has have had that this with from into than then them they their there these those can could would should about over under after before between during each other some such only also not but out any all its our your you his her she him why where whose been being more most very much many may might own same too via per a an of in on to is it as at by be or if do so no we he my me up us".split(' ').collect();
    queries
        .iter()
        .map(|q| {
            let terms: Vec<String> = q
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| t.len() > 2 && !stop.contains(t))
                .map(|t| format!("\"{t}\""))
                .collect();
            if terms.is_empty() {
                return vec![];
            }
            let mut st = conn.prepare("SELECT rowid, bm25(fts) FROM fts WHERE fts MATCH ?1 ORDER BY bm25(fts) LIMIT 50").unwrap();
            st.query_map([terms.join(" OR ")], |r| Ok((r.get::<_, i64>(0)? as usize, -(r.get::<_, f64>(1)? as f32))))
                .unwrap()
                .map(|x| x.unwrap())
                .collect()
        })
        .collect()
}

fn embedder(name: &str) -> (TextEmbedding, &'static str, &'static str) {
    let base = base();
    let cache = PathBuf::from(format!("{base}/models/cache"));
    let opts = |m| TextInitOptions::new(m).with_cache_dir(cache.clone()).with_show_download_progress(true);
    match name {
        "e5s" => (TextEmbedding::try_new(opts(EmbeddingModel::MultilingualE5Small)).unwrap(), "query: ", "passage: "),
        "e5b" => (TextEmbedding::try_new(opts(EmbeddingModel::MultilingualE5Base)).unwrap(), "query: ", "passage: "),
        "gemma" => (TextEmbedding::try_new(opts(EmbeddingModel::EmbeddingGemma300MQ)).unwrap(), "task: search result | query: ", "title: none | text: "),
        "gemma4" => (TextEmbedding::try_new(opts(EmbeddingModel::EmbeddingGemma300MQ4)).unwrap(), "task: search result | query: ", "title: none | text: "),
        "granite" => {
            let dir = PathBuf::from(format!("{base}/models/granite97"));
            let rd = |n: &str| fs::read(dir.join(n)).unwrap_or_else(|_| panic!("missing {n}"));
            let tok = TokenizerFiles {
                tokenizer_file: rd("tokenizer.json"),
                config_file: rd("config.json"),
                special_tokens_map_file: rd("special_tokens_map.json"),
                tokenizer_config_file: rd("tokenizer_config.json"),
            };
            let m = UserDefinedEmbeddingModel::new(rd("model.onnx"), tok).with_pooling(Pooling::Cls);
            (TextEmbedding::try_new_from_user_defined(m, InitOptionsUserDefined::default().with_max_length(512).with_auto_device(AutoDevicePolicy::PreferCPU)).unwrap(), "", "")
        }
        _ => panic!("unknown model {name}"),
    }
}

/// Reciprocal rank fusion of the saved BM25 and dense rankings.
fn hybrid(out_dir: &str, dense_name: &str) -> Vec<Ranking> {
    let load = |n: &str| -> Vec<Ranking> { serde_json::from_slice(&fs::read(format!("{out_dir}/{n}.rank.json")).unwrap()).unwrap() };
    fuse(&load("bm25"), &load(dense_name))
}

fn fuse(a: &[Ranking], b: &[Ranking]) -> Vec<Ranking> {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let mut s: std::collections::HashMap<usize, f32> = Default::default();
            for r in [x, y] {
                for (rank, (i, _)) in r.iter().enumerate() {
                    *s.entry(*i).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
                }
            }
            let mut v: Vec<(usize, f32)> = s.into_iter().collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            v.truncate(50);
            v
        })
        .collect()
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

fn dense(name: &str, passages: &[Passage], queries: &[String]) -> (Vec<Ranking>, usize, f32) {
    let (mut model, qp, dp) = embedder(name);
    let docs: Vec<String> = passages.iter().map(|p| format!("{dp}{}", p.text)).collect();
    let started = Instant::now();
    let mut dvecs: Vec<Vec<f32>> = Vec::with_capacity(docs.len());
    for (i, chunk) in docs.chunks(128).enumerate() {
        dvecs.extend(model.embed(chunk, Some(32)).unwrap().into_iter().map(normalize));
        eprintln!("  embedded {}/{} ({:.0}s)", ((i + 1) * 128).min(docs.len()), docs.len(), started.elapsed().as_secs_f32());
    }
    let secs = started.elapsed().as_secs_f32();
    let qs: Vec<String> = queries.iter().map(|q| format!("{qp}{q}")).collect();
    let qvecs: Vec<Vec<f32>> = model.embed(&qs, Some(32)).unwrap().into_iter().map(normalize).collect();
    let dim = dvecs[0].len();
    let rankings = qvecs
        .iter()
        .map(|q| {
            let mut s: Vec<(usize, f32)> = dvecs.iter().enumerate().map(|(i, d)| (i, d.iter().zip(q).map(|(a, b)| a * b).sum())).collect();
            s.sort_by(|a, b| b.1.total_cmp(&a.1));
            s.truncate(50);
            s
        })
        .collect();
    (rankings, dim, secs)
}

fn main() {
    let base = base();
    let bench_path = format!("{}/bench.json", std::env::var("CHINGUI_BENCH_DIR").expect("set CHINGUI_BENCH_DIR"));
    let mut args = std::env::args().skip(1);
    let name = args.next().expect("model");
    let out_dir = args.next().unwrap_or_else(|| format!("{base}/bench-out"));
    fs::create_dir_all(&out_dir).unwrap();

    let bench: Value = serde_json::from_slice(&fs::read(&bench_path).unwrap()).unwrap();
    let qs = bench["questions"].as_array().unwrap();
    let queries: Vec<String> = qs.iter().map(|q| q["question"].as_str().unwrap().to_string()).collect();

    if name == "app" {
        return run_app(qs, &queries, &out_dir);
    }

    let passages = load_passages();
    println!("{name}: {} passages, {} questions", passages.len(), qs.len());

    let (rankings, dim, secs) = if name == "bm25" {
        (bm25(&passages, &queries), 0, 0.0)
    } else if let Some(m) = name.strip_prefix("hybrid:") {
        (hybrid(&out_dir, m), 0, 0.0)
    } else {
        dense(&name, &passages, &queries)
    };
    if !name.starts_with("hybrid:") {
        fs::write(format!("{out_dir}/{name}.rank.json"), serde_json::to_vec(&rankings).unwrap()).unwrap();
    }
    let timing = (name != "bm25" && !name.starts_with("hybrid:")).then_some((dim, secs));
    report(&name, &passages, qs, rankings, &out_dir, timing);
}

/// BM25, stored vectors and their fusion over the passages the app built.
fn run_app(qs: &[Value], queries: &[String], out_dir: &str) {
    let passages = load_passages_db();
    let embedded = passages.iter().filter(|p| p.vec.is_some()).count();
    println!("app: {} passages ({embedded} with a vector), {} questions", passages.len(), qs.len());
    assert!(embedded > 0, "no vectors in the database: let the app finish indexing first");
    if embedded < passages.len() {
        println!("  warning: {} passages have no vector yet, the vector ranking ignores them", passages.len() - embedded);
    }

    let (mut model, qp, _) = embedder("gemma4");
    let qs_text: Vec<String> = queries.iter().map(|q| format!("{qp}{q}")).collect();
    let qvecs: Vec<Vec<f32>> = model.embed(&qs_text, Some(32)).unwrap().into_iter().map(normalize).collect();
    let vector: Vec<Ranking> = qvecs
        .iter()
        .map(|q| {
            let mut s: Vec<(usize, f32)> = passages
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.vec.as_ref().map(|d| (i, d.iter().zip(q).map(|(a, b)| a * b).sum())))
                .collect();
            s.sort_by(|a, b| b.1.total_cmp(&a.1));
            s.truncate(50);
            s
        })
        .collect();
    let keyword = bm25(&passages, queries);
    let both = fuse(&keyword, &vector);

    for (label, rankings) in [("app-bm25", keyword), ("app-vector", vector), ("app-hybrid", both)] {
        println!("{label}");
        report(label, &passages, qs, rankings, out_dir, None);
    }
}

/// Prints the metrics for one ranking per question and saves each question's top 10.
fn report(
    name: &str,
    passages: &[Passage],
    qs: &[Value],
    rankings: Vec<Ranking>,
    out_dir: &str,
    timing: Option<(usize, f32)>,
) {
    let mut rankings = rankings;
    rankings.iter_mut().for_each(|r| r.truncate(20));

    // type -> (n, hit1, hit5, hit10, rr_sum)
    let mut stats: std::collections::BTreeMap<String, (u32, u32, u32, u32, f32)> = Default::default();
    let (mut multi_cov, mut multi_n) = (0.0f32, 0u32);
    let (mut intrusions, mut marine_n) = (0u32, 0u32);
    let mut answerable_top1: Vec<f32> = vec![];
    let mut per_q = vec![];
    for (q, r) in qs.iter().zip(&rankings) {
        let ty = q["type"].as_str().unwrap();
        let exp = q["expected"].as_array().unwrap();
        let also: Vec<&str> = q["alsoAcceptable"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        if ty == "multi" {
            let docs: HashSet<&str> = r.iter().map(|(i, _)| passages[*i].file.as_str()).collect();
            let hit = exp.iter().filter(|e| docs.contains(e["file"].as_str().unwrap())).count();
            multi_cov += hit as f32 / exp.len() as f32;
            multi_n += 1;
        } else if ty != "none" {
            let ok = |i: usize| {
                let p = &passages[i];
                exp.iter().any(|e| {
                    e["file"] == p.file.as_str() && e["pages"].as_array().unwrap().iter().any(|n| n.as_u64().is_some_and(|n| (p.page..=p.page_end).contains(&n)))
                }) || also.contains(&p.file.as_str())
            };
            let rank = r.iter().position(|(i, _)| ok(*i));
            let s = stats.entry(ty.to_string()).or_default();
            s.0 += 1;
            if let Some(k) = rank {
                if k < 1 { s.1 += 1 }
                if k < 5 { s.2 += 1 }
                if k < 10 { s.3 += 1 }
                if k < 10 { s.4 += 1.0 / (k as f32 + 1.0) }
                answerable_top1.push(r[0].1);
            }
            if !exp.iter().any(|e| e["file"] == DISTRACTOR) {
                marine_n += 1;
                if r.iter().take(10).any(|(i, _)| passages[*i].file == DISTRACTOR) { intrusions += 1 }
            }
        }
        per_q.push(json!({
            "id": q["id"], "type": ty,
            "top": r.iter().take(10).map(|(i, s)| json!([passages[*i].file, passages[*i].page, s])).collect::<Vec<_>>()
        }));
    }
    let mut all = (0, 0, 0, 0, 0.0f32);
    for (ty, s) in &stats {
        println!("  {ty:10} n={:2}  hit@1 {:.2}  hit@5 {:.2}  hit@10 {:.2}  MRR {:.2}", s.0, s.1 as f32 / s.0 as f32, s.2 as f32 / s.0 as f32, s.3 as f32 / s.0 as f32, s.4 / s.0 as f32);
        all = (all.0 + s.0, all.1 + s.1, all.2 + s.2, all.3 + s.3, all.4 + s.4);
    }
    println!("  {:10} n={:2}  hit@1 {:.2}  hit@5 {:.2}  hit@10 {:.2}  MRR {:.2}", "ALL", all.0, all.1 as f32 / all.0 as f32, all.2 as f32 / all.0 as f32, all.3 as f32 / all.0 as f32, all.4 / all.0 as f32);
    println!("  multi      docs covered in top-20 passages: {:.2} (n={multi_n})", multi_cov / multi_n as f32);
    println!("  distractor intrusion: {intrusions}/{marine_n} marine questions had {DISTRACTOR} in top 10");
    answerable_top1.sort_by(|a, b| a.total_cmp(b));
    if !answerable_top1.is_empty() {
        println!("  top-1 score when answered: min {:.3}  median {:.3}", answerable_top1[0], answerable_top1[answerable_top1.len() / 2]);
    }
    for (q, r) in qs.iter().zip(&rankings) {
        if q["type"] == "none" {
            let (i, s) = r.first().copied().unwrap_or((0, 0.0));
            println!("  none {}: top-1 {:.3} -> {} p{}", q["id"].as_str().unwrap(), s, &passages[i].file[..passages[i].file.len().min(30)], passages[i].page);
        }
    }
    if let Some((dim, secs)) = timing {
        println!("  dim {dim}, embedded {} passages in {secs:.0}s ({:.1}/s)", passages.len(), passages.len() as f32 / secs);
    }
    fs::write(format!("{out_dir}/{name}.json"), serde_json::to_vec_pretty(&per_q).unwrap()).unwrap();
}
