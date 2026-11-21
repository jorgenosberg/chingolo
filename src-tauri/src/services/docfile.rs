//! Serves stored document files to the reader through the `chingolo-doc` URI scheme.
//! A file is addressed by document id only. The webview never supplies a path, and the path
//! comes from the database row.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tauri::http::{Method, Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager};

use crate::domain::library::DocKind;
use crate::persistence::library as repo;
use crate::services::ingestion::queue::library_path;
use crate::state::AppState;

pub const SCHEME: &str = "chingolo-doc";

#[derive(Debug, PartialEq, Eq)]
pub enum ByteRange {
    /// No usable range: send the whole file.
    Full,
    /// First and last byte, both inclusive.
    Partial(u64, u64),
    Unsatisfiable,
}

/// Reads a single `bytes=a-b`, `bytes=a-` or `bytes=-n` range. Anything else (several ranges,
/// other units, junk) is ignored, which the HTTP spec allows.
pub fn parse_range(header: Option<&str>, len: u64) -> ByteRange {
    fn inner(h: &str, len: u64) -> Option<ByteRange> {
        let spec = h.trim().strip_prefix("bytes=")?;
        if spec.contains(',') || len == 0 {
            return None;
        }
        let (a, b) = spec.split_once('-')?;
        let (a, b) = (a.trim(), b.trim());
        let num = |s: &str| -> Option<Option<u64>> {
            if s.is_empty() {
                Some(None)
            } else {
                s.parse().ok().map(Some)
            }
        };
        match (num(a)?, num(b)?) {
            (Some(start), end) => {
                let end = end.map_or(len - 1, |e| e.min(len - 1));
                if start >= len {
                    Some(ByteRange::Unsatisfiable)
                } else if end < start {
                    None
                } else {
                    Some(ByteRange::Partial(start, end))
                }
            }
            (None, Some(n)) if n > 0 => Some(ByteRange::Partial(len.saturating_sub(n), len - 1)),
            _ => None,
        }
    }
    header
        .and_then(|h| inner(h, len))
        .unwrap_or(ByteRange::Full)
}

fn mime(kind: DocKind) -> &'static str {
    match kind {
        DocKind::Pdf => "application/pdf",
        DocKind::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    }
}

fn base(status: StatusCode) -> tauri::http::response::Builder {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "range")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "content-range, accept-ranges, content-length",
        )
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    base(status).body(Vec::new()).unwrap_or_default()
}

/// Reads the requested bytes of a stored file and builds the reply.
pub fn read_response(path: &Path, kind: DocKind, range: Option<&str>) -> Response<Vec<u8>> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return empty(StatusCode::NOT_FOUND);
    };

    let (status, start, end) = match parse_range(range, len) {
        ByteRange::Full => (StatusCode::OK, 0, len.saturating_sub(1)),
        ByteRange::Partial(s, e) => (StatusCode::PARTIAL_CONTENT, s, e),
        ByteRange::Unsatisfiable => {
            return base(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Vec::new())
                .unwrap_or_default();
        }
    };
    let size = if len == 0 { 0 } else { end - start + 1 };
    let mut body = vec![0u8; size as usize];
    if file
        .seek(SeekFrom::Start(start))
        .and_then(|_| file.read_exact(&mut body))
        .is_err()
    {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut reply = base(status)
        .header(header::CONTENT_TYPE, mime(kind))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, size);
    if status == StatusCode::PARTIAL_CONTENT {
        reply = reply.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    reply.body(body).unwrap_or_default()
}

/// Handles one request for `chingolo-doc://localhost/<document id>`.
pub async fn handle(app: &AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() == Method::OPTIONS {
        return empty(StatusCode::NO_CONTENT);
    }
    if request.method() != Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let id = request.uri().path().trim_start_matches('/').to_string();
    let state = app.state::<AppState>();
    let Ok(doc) = state.db.call(move |c| repo::get_document(c, &id)).await else {
        return empty(StatusCode::NOT_FOUND);
    };
    let path = library_path(state.ingest.library_dir(), &doc.content_hash, doc.kind);
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    tauri::async_runtime::spawn_blocking(move || read_response(&path, doc.kind, range.as_deref()))
        .await
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        use ByteRange::*;
        assert_eq!(parse_range(None, 100), Full);
        assert_eq!(parse_range(Some("bytes=0-9"), 100), Partial(0, 9));
        assert_eq!(parse_range(Some("bytes=90-"), 100), Partial(90, 99));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Partial(90, 99));
        assert_eq!(parse_range(Some("bytes=50-5000"), 100), Partial(50, 99));
        assert_eq!(parse_range(Some("bytes=-500"), 100), Partial(0, 99));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Full);
        assert_eq!(parse_range(Some("bytes=9-3"), 100), Full);
        assert_eq!(parse_range(Some("items=0-9"), 100), Full);
        assert_eq!(parse_range(Some("bytes=abc-5"), 100), Full);
        assert_eq!(parse_range(Some("bytes=0-9"), 0), Full);
    }

    #[test]
    fn serves_whole_file_and_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.pdf");
        std::fs::write(&path, b"0123456789").unwrap();

        let all = read_response(&path, DocKind::Pdf, None);
        assert_eq!(
            (all.status(), all.body().as_slice()),
            (StatusCode::OK, &b"0123456789"[..])
        );

        let part = read_response(&path, DocKind::Pdf, Some("bytes=2-4"));
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body().as_slice(), b"234");
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 2-4/10");

        let bad = read_response(&path, DocKind::Pdf, Some("bytes=10-"));
        assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let missing = read_response(&dir.path().join("nope.pdf"), DocKind::Pdf, None);
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
