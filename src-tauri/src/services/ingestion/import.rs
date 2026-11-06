//! Getting files into the library: find them, hash them, skip duplicates, copy them into the
//! managed folder and queue them. Nothing here judges a file by topic.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;

use super::queue::{Ingestor, library_path};
use crate::domain::library::{DocKind, Document};
use crate::error::Result;
use crate::persistence::{Db, library as repo};

/// Folders are searched this deep and files are counted up to this many, so dropping a drive's
/// root can't run away.
const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 5000;
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// New documents, now queued for reading.
    pub added: Vec<Document>,
    /// Files that were already in the library (same content).
    pub duplicates: Vec<Document>,
    pub skipped: Vec<SkippedFile>,
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn kind_of(p: &Path) -> Option<DocKind> {
    p.extension().and_then(|e| e.to_str()).and_then(DocKind::from_extension)
}

/// Files to import from a mix of files and folders. Symlinks are not followed, hidden files are
/// skipped, and Word's `~$` lock files are ignored.
pub fn collect_files(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<SkippedFile>) {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = paths.into_iter().rev().map(|p| (p, 0)).collect();
    while let Some((p, depth)) = stack.pop() {
        let name = name_of(&p);
        let Ok(meta) = fs::symlink_metadata(&p) else {
            skipped.push(SkippedFile { name, reason: "Couldn't be read.".into() });
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if depth >= MAX_DEPTH {
                skipped.push(SkippedFile { name, reason: "Folder is nested too deeply.".into() });
                continue;
            }
            let mut children: Vec<PathBuf> = fs::read_dir(&p)
                .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
                .unwrap_or_default();
            children.sort();
            stack.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
        } else if name.starts_with('.') || name.starts_with("~$") {
            continue;
        } else if kind_of(&p).is_none() {
            // Only report unsupported files that were chosen directly, not every file in a folder.
            if depth == 0 {
                skipped.push(SkippedFile { name, reason: "Only PDF and Word (.docx) files can be imported.".into() });
            }
        } else if meta.len() > MAX_FILE_BYTES {
            skipped.push(SkippedFile { name, reason: "Larger than 1 GB.".into() });
        } else if files.len() >= MAX_FILES {
            skipped.push(SkippedFile { name, reason: format!("Only the first {MAX_FILES} files are imported at once.") });
        } else {
            files.push(p);
        }
    }
    (files, skipped)
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Copies to the managed folder under a temporary name, then renames, so a crash never leaves a
/// half-written file at the final path.
fn copy_into_library(src: &Path, dest: &Path) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("part");
    fs::copy(src, &tmp)?;
    fs::rename(&tmp, dest).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Imports files and folders. Duplicates are recognised by content, so the same paper under
/// another name is not stored twice; it is only attached to the project if one is given.
pub async fn import_paths(
    db: &Db,
    ingest: &Ingestor,
    paths: Vec<PathBuf>,
    project_id: Option<String>,
) -> Result<ImportSummary> {
    let (files, skipped) = tokio::task::spawn_blocking(move || collect_files(paths))
        .await
        .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
    let mut summary = ImportSummary { skipped, ..Default::default() };

    for file in files {
        let name = name_of(&file);
        let Some(kind) = kind_of(&file) else { continue };

        let src = file.clone();
        let hash = match tokio::task::spawn_blocking(move || hash_file(&src)).await {
            Ok(Ok(h)) => h,
            _ => {
                summary.skipped.push(SkippedFile { name, reason: "Couldn't be read.".into() });
                continue;
            }
        };

        let (h, pid) = (hash.clone(), project_id.clone());
        let existing = db
            .call(move |c| {
                let found = repo::find_by_hash(c, &h)?;
                if let (Some(d), Some(p)) = (&found, &pid) {
                    repo::attach(c, p, &d.id)?;
                }
                Ok(found)
            })
            .await?;
        if let Some(doc) = existing {
            summary.duplicates.push(doc);
            continue;
        }

        let dest = library_path(ingest.library_dir(), &hash, kind);
        let (src, d) = (file.clone(), dest.clone());
        if tokio::task::spawn_blocking(move || copy_into_library(&src, &d)).await.map_or(true, |r| r.is_err()) {
            summary.skipped.push(SkippedFile { name, reason: "Couldn't be copied into the library.".into() });
            continue;
        }

        let (h, n, pid) = (hash, name.clone(), project_id.clone());
        let inserted = db
            .call(move |c| {
                let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let doc = repo::insert_document(&tx, &h, &n, kind)?;
                if let Some(p) = &pid {
                    repo::attach(&tx, p, &doc.id)?;
                }
                tx.commit()?;
                Ok(doc)
            })
            .await;
        match inserted {
            Ok(doc) => {
                ingest.enqueue(&doc);
                ingest.emit(doc.clone(), None);
                summary.added.push(doc);
            }
            Err(e) => {
                let _ = fs::remove_file(&dest);
                summary.skipped.push(SkippedFile { name, reason: format!("Couldn't be added: {e}") });
            }
        }
    }
    Ok(summary)
}

/// Removes a document and its managed file. A running read of it is cancelled first.
pub async fn delete_document(db: &Db, ingest: &Ingestor, id: &str) -> Result<()> {
    ingest.cancel(id);
    let id = id.to_string();
    let (hash, kind) = db.call(move |c| repo::delete_document(c, &id)).await?;
    let _ = fs::remove_file(library_path(ingest.library_dir(), &hash, kind));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    #[test]
    fn folders_are_walked_and_noise_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(&r.join("a.pdf"), b"1");
        touch(&r.join("sub/b.DOCX"), b"2");
        touch(&r.join("sub/deeper/c.pdf"), b"3");
        touch(&r.join("notes.txt"), b"x");
        touch(&r.join(".hidden.pdf"), b"x");
        touch(&r.join("~$lock.docx"), b"x");
        let (files, skipped) = collect_files(vec![r.to_path_buf()]);
        let names: Vec<_> = files.iter().map(|f| name_of(f)).collect();
        assert_eq!(names, ["a.pdf", "b.DOCX", "c.pdf"]);
        assert!(skipped.is_empty(), "files inside folders are not reported: {skipped:?}");

        let (files, skipped) = collect_files(vec![r.join("notes.txt"), r.join("missing.pdf")]);
        assert!(files.is_empty());
        assert_eq!(skipped.len(), 2);
        assert!(skipped[0].reason.contains("PDF and Word"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("real/a.pdf"), b"1");
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("link")).unwrap();
        let (files, _) = collect_files(vec![d.path().join("link"), d.path().join("real")]);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn same_content_gives_the_same_hash() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a.pdf"), b"same bytes");
        touch(&d.path().join("b.pdf"), b"same bytes");
        touch(&d.path().join("c.pdf"), b"other bytes");
        let h = |n: &str| hash_file(&d.path().join(n)).unwrap();
        assert_eq!(h("a.pdf"), h("b.pdf"));
        assert_ne!(h("a.pdf"), h("c.pdf"));
        assert_eq!(h("a.pdf").len(), 64);
        // sha256 of "abc"
        touch(&d.path().join("abc"), b"abc");
        assert_eq!(h("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn copy_leaves_no_partial_file() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a.pdf"), b"data");
        let dest = d.path().join("lib/ab/abcd.pdf");
        copy_into_library(&d.path().join("a.pdf"), &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"data");
        assert!(!dest.with_extension("part").exists());
        assert!(copy_into_library(&d.path().join("nope.pdf"), &d.path().join("lib/ab/x.pdf")).is_err());
        assert!(!d.path().join("lib/ab/x.part").exists());
    }

    // ---- whole pipeline: real worker thread, real database file ----

    use std::io::Write;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::domain::library::DocStatus;
    use crate::persistence::library::SearchOptions;
    use crate::persistence::projects;

    fn write_docx(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        write!(zip, r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#).unwrap();
        zip.finish().unwrap();
    }

    async fn settled(db: &Db) -> Vec<Document> {
        let t = Instant::now();
        loop {
            let docs = db.call(|c| repo::list_documents(c, None)).await.unwrap();
            if docs.iter().all(|d| !matches!(d.status, DocStatus::Queued | DocStatus::Processing)) {
                return docs;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "documents never settled: {docs:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn import_to_search_to_delete_and_resume() {
        let dir = tempfile::tempdir().unwrap();
        let (db_path, lib) = (dir.path().join("t.sqlite"), dir.path().join("library"));
        let db = Db::open(&db_path).unwrap();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let ingest = Ingestor::start(&db_path, lib.clone(), Arc::new(move |e| sink.lock().unwrap().push(e.document.status))).unwrap();

        let src = dir.path().join("papers");
        write_docx(&src.join("alpha.docx"), "Domoic acid was measured in Pseudo-nitzschia cells near Ærø.");
        fs::copy(src.join("alpha.docx"), src.join("alpha copy.docx")).unwrap();
        write_docx(&src.join("sub/beta.docx"), "Iron enrichment doubled the abundance of diatoms.");
        touch(&src.join("broken.pdf"), b"%PDF-1.4 not really");
        touch(&src.join("notes.txt"), b"x");
        let project = db.call(|c| projects::create_project(c, "P")).await.unwrap();

        let summary = import_paths(&db, &ingest, vec![src.join("alpha.docx"), src.join("alpha copy.docx"), src.join("sub"), src.join("broken.pdf"), src.join("notes.txt")], Some(project.id.clone())).await.unwrap();
        assert_eq!((summary.added.len(), summary.duplicates.len(), summary.skipped.len()), (3, 1, 1), "{summary:?}");
        assert_eq!(summary.duplicates[0].original_name, "alpha.docx");

        let docs = settled(&db).await;
        let status = |n: &str| docs.iter().find(|d| d.original_name == n).unwrap().status;
        assert_eq!((status("alpha.docx"), status("beta.docx"), status("broken.pdf")), (DocStatus::Ready, DocStatus::Ready, DocStatus::Error), "{:?}", docs.iter().map(|d| (&d.original_name, &d.error)).collect::<Vec<_>>());
        assert!(events.lock().unwrap().contains(&DocStatus::Processing));
        for d in &docs {
            assert!(library_path(&lib, &d.content_hash, d.kind).exists(), "{} is stored once", d.original_name);
        }
        assert_eq!(db.call(|c| repo::list_documents(c, None)).await.unwrap().len(), 3, "the copy was not added again");

        let pf = crate::domain::library::DocFilter::Project(project.id.clone());
        let hits = db.call({ let pf = pf.clone(); move |c| repo::search_passages(c, "aero", &SearchOptions { filter: &pf, all_terms: true, prefix_last: false, limit: 5 }) }).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].document_title, "alpha.docx");

        // delete removes the row, the index entries and the managed file
        let alpha = docs.iter().find(|d| d.original_name == "alpha.docx").unwrap().clone();
        delete_document(&db, &ingest, &alpha.id).await.unwrap();
        assert!(!library_path(&lib, &alpha.content_hash, alpha.kind).exists());
        let none = db.call(|c| repo::search_passages(c, "domoic", &SearchOptions { filter: &crate::domain::library::DocFilter::All, all_terms: true, prefix_last: false, limit: 5 })).await.unwrap();
        assert!(none.is_empty());

        // a document left "processing" by a crash is read again on the next start
        let gamma = dir.path().join("gamma.docx");
        write_docx(&gamma, "Gamma was interrupted halfway through reading.");
        let hash = hash_file(&gamma).unwrap();
        let dest = library_path(&lib, &hash, DocKind::Docx);
        copy_into_library(&gamma, &dest).unwrap();
        let h = hash.clone();
        db.call(move |c| {
            let d = repo::insert_document(c, &h, "gamma.docx", DocKind::Docx)?;
            repo::set_status(c, &d.id, DocStatus::Processing, None)?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(ingest.resume(&db).await.unwrap(), 1);
        let docs = settled(&db).await;
        assert_eq!(docs.iter().find(|d| d.original_name == "gamma.docx").unwrap().status, DocStatus::Ready);
    }

    /// Imports every paper in $CHINGUI_TEST_PAPERS through the real queue and prints the result.
    /// Run: CHINGUI_TEST_PAPERS=~/papers cargo test corpus_import -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn corpus_import() {
        let papers = PathBuf::from(std::env::var("CHINGUI_TEST_PAPERS").expect("set CHINGUI_TEST_PAPERS"));
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.sqlite");
        let db = Db::open(&db_path).unwrap();
        let ingest = Ingestor::start(&db_path, dir.path().join("library"), Arc::new(|_| {})).unwrap();
        let t = Instant::now();
        let summary = import_paths(&db, &ingest, vec![papers], None).await.unwrap();
        println!("queued {} files in {:.1}s, {} duplicates, skipped {:?}", summary.added.len(), t.elapsed().as_secs_f32(), summary.duplicates.len(), summary.skipped);
        let docs = settled(&db).await;
        println!("all read after {:.1}s", t.elapsed().as_secs_f32());
        for d in &docs {
            println!("{:>9?} {:>4}p {:>5} passages | {:?} | {}", d.status, d.page_count.unwrap_or(0), d.passage_count, d.error.as_deref().unwrap_or(""), d.original_name.chars().take(34).collect::<String>());
        }
        for q in ["domoic acid", "palmer maloney", "alcian blue gill", "Pseudo-nitzschia razor clams", "resilient"] {
            let hits = db.call({ let q = q.to_string(); move |c| repo::search_passages(c, &q, &SearchOptions { filter: &crate::domain::library::DocFilter::All, all_terms: true, prefix_last: true, limit: 3 }) }).await.unwrap();
            let top: Vec<_> = hits.iter().map(|h| format!("{} p{}", h.document_title.chars().take(22).collect::<String>(), h.page_start)).collect();
            println!("search {q:?}: {top:?}");
        }
        assert_eq!(docs.iter().filter(|d| d.status == DocStatus::NoText).count(), 2, "the two scans");
    }
}
