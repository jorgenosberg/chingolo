//! The search model's files: what to fetch, where they live, and a resumable, verified download.
//! Rust does all of this. The webview never touches the network or the files.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, Result};

pub const REPO_URL: &str = "https://huggingface.co/onnx-community/embeddinggemma-300m-ONNX/resolve";
/// Pinned commit of the model repository. Bumping it (and the hashes) rebuilds every vector.
pub const REVISION: &str = "5090578d9565bb06545b4552f76e6bc2c93e4a66";

pub struct ManifestFile {
    /// Name on disk. All files sit side by side.
    pub name: &'static str,
    /// Path inside the repository.
    pub remote: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const FILES: &[ManifestFile] = &[
    ManifestFile {
        name: "model_q4.onnx",
        remote: "onnx/model_q4.onnx",
        size: 519_322,
        sha256: "ad1dfee81a70f7944b9b9d1cc6e48075b832881cf33fab2f2b248be78f3f0043",
    },
    ManifestFile {
        name: "model_q4.onnx_data",
        remote: "onnx/model_q4.onnx_data",
        size: 196_725_760,
        sha256: "599962c3143b040de2dd05e5975be3e9091dd067cacc6a8f7186e3203bab9e02",
    },
    ManifestFile {
        name: "tokenizer.json",
        remote: "tokenizer.json",
        size: 20_323_312,
        sha256: "4dda02faaf32bc91031dc8c88457ac272b00c1016cc679757d1c441b248b9c47",
    },
    ManifestFile {
        name: "config.json",
        remote: "config.json",
        size: 1_765,
        sha256: "6e1f06404b7163e0325ed2ea3e6781cde50f4a50b31780a95ad0d30e8404d77b",
    },
    ManifestFile {
        name: "special_tokens_map.json",
        remote: "special_tokens_map.json",
        size: 662,
        sha256: "2f7b0adf4fb469770bb1490e3e35df87b1dc578246c5e7e6fc76ecf33213a397",
    },
    ManifestFile {
        name: "tokenizer_config.json",
        remote: "tokenizer_config.json",
        size: 1_156_830,
        sha256: "3ca953eea6c3c9fcda9cf3df22949ff18b216f7c74bd6459230f3f1013953f3a",
    },
];

pub fn total_bytes(files: &[ManifestFile]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

/// Where the files for the pinned revision go.
pub fn model_dir(app_data: &Path) -> PathBuf {
    app_data
        .join("models")
        .join(super::MODEL_NAME)
        .join(REVISION)
}

/// What the UI shows about the model files. Sizes are bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModelState {
    Missing,
    Downloading { done: f64, total: f64 },
    Ready,
    Failed { message: String },
}

/// True when every file is in place at its full size. Files only get their final name after the
/// checksum matched, so a size check is enough here.
pub fn is_complete(dir: &Path, files: &[ManifestFile]) -> bool {
    files
        .iter()
        .all(|f| std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size))
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

fn net(e: impl std::fmt::Display) -> AppError {
    AppError::Provider(format!("Download failed: {e}"))
}

/// Downloads whatever is missing into `dir`, then verifies each file. Each file goes to
/// `<name>.part` first, continues from there after an interruption, and only gets its real name
/// once the SHA-256 matches. `progress(done, total)` is called about once per megabyte.
pub async fn download(
    client: &reqwest::Client,
    base_url: &str,
    files: &[ManifestFile],
    dir: &Path,
    cancel: &CancellationToken,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(net)?;
    let total = total_bytes(files);
    let mut done: u64 = files
        .iter()
        .filter(|f| std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size))
        .map(|f| f.size)
        .sum();
    progress(done, total);

    for f in files {
        let target = dir.join(f.name);
        if std::fs::metadata(&target).is_ok_and(|m| m.len() == f.size) {
            continue;
        }
        let part = dir.join(format!("{}.part", f.name));
        download_one(
            client, base_url, f, &part, cancel, &mut done, total, progress,
        )
        .await?;
        // Hashing 200 MB blocks for a moment, so keep it off the async threads.
        let (p, want) = (part.clone(), f.sha256);
        let got = tokio::task::spawn_blocking(move || sha256_of(&p))
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .map_err(net)?;
        if got != want {
            let _ = std::fs::remove_file(&part);
            return Err(AppError::Provider(format!(
                "Download failed: {} did not match its checksum. Try again.",
                f.name
            )));
        }
        std::fs::rename(&part, &target).map_err(net)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_one(
    client: &reqwest::Client,
    base_url: &str,
    f: &ManifestFile,
    part: &Path,
    cancel: &CancellationToken,
    done: &mut u64,
    total: u64,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<()> {
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > f.size {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    if have == f.size {
        *done += have;
        return Ok(());
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }

    // A full URL in the manifest is used as is.
    let url = if f.remote.starts_with("http") {
        f.remote.to_string()
    } else {
        format!("{base_url}/{REVISION}/{}", f.remote)
    };
    let mut req = client.get(&url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(net)?;
    let status = resp.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The partial file doesn't fit what the server has. Start that file over.
        let _ = std::fs::remove_file(part);
        return Err(AppError::Provider(
            "Download failed: the partial file was discarded. Try again.".into(),
        ));
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "Download failed: the server answered {status}."
        )));
    }
    // 200 to a Range request means the server sent the whole file.
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut out = if resumed {
        std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(net)?
    } else {
        have = 0;
        std::fs::File::create(part).map_err(net)?
    };
    *done += have;

    let mut stream = Box::pin(resp.bytes_stream());
    let mut since_report = 0u64;
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            n = tokio::time::timeout(Duration::from_secs(30), stream.next()) => n,
        };
        let Ok(next) = next else {
            return Err(net("the connection stalled"));
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(net)?;
        out.write_all(&chunk).map_err(net)?;
        *done += chunk.len() as u64;
        since_report += chunk.len() as u64;
        if since_report >= 1 << 20 {
            since_report = 0;
            progress(*done, total);
        }
    }
    out.flush().map_err(net)?;
    progress(*done, total);
    let len = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if len != f.size {
        return Err(net("the file ended early"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Behaviour {
        /// Ignore Range headers and always send the whole file.
        ignore_range: bool,
        /// Close the first full-file response after this many body bytes.
        cut_first_at: Option<usize>,
    }

    struct Server {
        base: String,
        ranges: Arc<Mutex<Vec<Option<String>>>>,
    }

    fn serve(files: HashMap<String, Vec<u8>>, behaviour: Behaviour) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges.clone();
        let cuts = AtomicUsize::new(0);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut range = None;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = Some(v.trim().trim_end_matches('-').to_string());
                    }
                }
                seen.lock().unwrap().push(range.clone());
                // path is /<revision>/<remote>
                let name = path.splitn(3, '/').nth(2).unwrap_or("");
                let Some(body) = files.get(name) else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                };
                let start = if behaviour.ignore_range {
                    0
                } else {
                    range
                        .as_ref()
                        .and_then(|r| r.parse::<usize>().ok())
                        .unwrap_or(0)
                };
                let (head, slice) = if start > 0 {
                    (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len() - start,
                            body.len() - 1,
                            body.len()
                        ),
                        &body[start..],
                    )
                } else {
                    (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        ),
                        &body[..],
                    )
                };
                let _ = stream.write_all(head.as_bytes());
                let cut = behaviour
                    .cut_first_at
                    .filter(|_| start == 0 && cuts.fetch_add(1, Ordering::SeqCst) == 0);
                match cut {
                    Some(n) => {
                        let _ = stream.write_all(&slice[..n.min(slice.len())]);
                    }
                    None => {
                        let _ = stream.write_all(slice);
                    }
                }
                let _ = stream.flush();
            }
        });
        Server { base, ranges }
    }

    fn hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn data(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    /// Two small files. The first is bigger than the 1 MB progress step.
    struct Fixture {
        a: Vec<u8>,
        b: Vec<u8>,
        manifest: Vec<ManifestFile>,
    }

    fn fixture() -> Fixture {
        let (a, b) = (data(1, 2_500_000), data(2, 1_000));
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        let manifest = vec![
            ManifestFile {
                name: "a.bin",
                remote: "sub/a.bin",
                size: a.len() as u64,
                sha256: leak(hex(&a)),
            },
            ManifestFile {
                name: "b.bin",
                remote: "b.bin",
                size: b.len() as u64,
                sha256: leak(hex(&b)),
            },
        ];
        Fixture { a, b, manifest }
    }

    fn files_of(fx: &Fixture) -> HashMap<String, Vec<u8>> {
        HashMap::from([
            ("sub/a.bin".to_string(), fx.a.clone()),
            ("b.bin".to_string(), fx.b.clone()),
        ])
    }

    async fn run(
        server: &Server,
        fx: &Fixture,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> (Result<()>, Vec<(u64, u64)>) {
        let mut seen = Vec::new();
        let r = download(
            &http_client(),
            &server.base,
            &fx.manifest,
            dir,
            cancel,
            &mut |d, t| seen.push((d, t)),
        )
        .await;
        (r, seen)
    }

    #[tokio::test]
    async fn downloads_verifies_and_reports_progress() {
        let fx = fixture();
        let server = serve(files_of(&fx), Behaviour::default());
        let dir = tempfile::tempdir().unwrap();
        let (r, seen) = run(&server, &fx, dir.path(), &CancellationToken::new()).await;
        r.unwrap();
        assert!(is_complete(dir.path(), &fx.manifest));
        assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), fx.a);
        assert!(!dir.path().join("a.bin.part").exists());
        let total = (fx.a.len() + fx.b.len()) as u64;
        assert_eq!(seen.first(), Some(&(0, total)));
        assert_eq!(seen.last(), Some(&(total, total)));
        assert!(
            seen.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress never goes back"
        );

        // a second run finds everything in place and asks for nothing
        let before = server.ranges.lock().unwrap().len();
        run(&server, &fx, dir.path(), &CancellationToken::new())
            .await
            .0
            .unwrap();
        assert_eq!(server.ranges.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn continues_from_a_partial_file_with_a_range_request() {
        let fx = fixture();
        let server = serve(files_of(&fx), Behaviour::default());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin.part"), &fx.a[..1_000_000]).unwrap();
        run(&server, &fx, dir.path(), &CancellationToken::new())
            .await
            .0
            .unwrap();
        assert_eq!(server.ranges.lock().unwrap()[0].as_deref(), Some("1000000"));
        assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), fx.a);
    }

    #[tokio::test]
    async fn a_dropped_connection_can_be_resumed() {
        let fx = fixture();
        let server = serve(
            files_of(&fx),
            Behaviour {
                cut_first_at: Some(700_000),
                ..Default::default()
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let (r, _) = run(&server, &fx, dir.path(), &CancellationToken::new()).await;
        assert!(r.is_err());
        assert!(
            !dir.path().join("a.bin").exists(),
            "no final name before the file is whole"
        );
        let part = std::fs::metadata(dir.path().join("a.bin.part"))
            .unwrap()
            .len();
        assert!(part > 0 && part < fx.a.len() as u64);

        run(&server, &fx, dir.path(), &CancellationToken::new())
            .await
            .0
            .unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), fx.a);
        assert_eq!(
            server.ranges.lock().unwrap()[1].as_deref(),
            Some(part.to_string().as_str())
        );
    }

    #[tokio::test]
    async fn a_server_that_ignores_range_still_gives_a_correct_file() {
        let fx = fixture();
        let server = serve(
            files_of(&fx),
            Behaviour {
                ignore_range: true,
                ..Default::default()
            },
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin.part"), &fx.a[..1_000_000]).unwrap();
        let (r, seen) = run(&server, &fx, dir.path(), &CancellationToken::new()).await;
        r.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), fx.a);
        let total = (fx.a.len() + fx.b.len()) as u64;
        assert_eq!(seen.last(), Some(&(total, total)));
    }

    #[tokio::test]
    async fn a_wrong_checksum_deletes_the_file_and_fails() {
        let fx = fixture();
        let mut served = files_of(&fx);
        served.get_mut("sub/a.bin").unwrap()[10] ^= 0xff;
        let server = serve(served, Behaviour::default());
        let dir = tempfile::tempdir().unwrap();
        let (r, _) = run(&server, &fx, dir.path(), &CancellationToken::new()).await;
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("checksum"), "{msg}");
        assert!(!dir.path().join("a.bin").exists() && !dir.path().join("a.bin.part").exists());
        assert!(!is_complete(dir.path(), &fx.manifest));
    }

    #[tokio::test]
    async fn cancelling_keeps_the_partial_file_and_stops_before_any_request() {
        let fx = fixture();
        let server = serve(files_of(&fx), Behaviour::default());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin.part"), &fx.a[..1_000]).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (r, _) = run(&server, &fx, dir.path(), &cancel).await;
        assert!(matches!(r, Err(AppError::Cancelled)));
        assert!(server.ranges.lock().unwrap().is_empty());
        assert_eq!(
            std::fs::metadata(dir.path().join("a.bin.part"))
                .unwrap()
                .len(),
            1_000
        );
    }

    #[tokio::test]
    async fn a_full_url_in_the_manifest_is_used_as_is() {
        let fx = fixture();
        let server = serve(files_of(&fx), Behaviour::default());
        let url: &'static str =
            Box::leak(format!("{}/any-tag/b.bin", server.base).into_boxed_str());
        let manifest = [ManifestFile {
            name: "b.bin",
            remote: url,
            size: fx.b.len() as u64,
            sha256: fx.manifest[1].sha256,
        }];
        let dir = tempfile::tempdir().unwrap();
        // The base URL is ignored for this file, so a wrong one must not matter.
        download(
            &http_client(),
            "http://127.0.0.1:1",
            &manifest,
            dir.path(),
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(dir.path().join("b.bin")).unwrap(), fx.b);
    }

    #[tokio::test]
    async fn an_error_status_is_reported() {
        let fx = fixture();
        let server = serve(HashMap::new(), Behaviour::default());
        let dir = tempfile::tempdir().unwrap();
        let (r, _) = run(&server, &fx, dir.path(), &CancellationToken::new()).await;
        assert!(r.unwrap_err().to_string().contains("404"));
    }

    #[test]
    fn manifest_sizes_add_up_and_the_directory_is_pinned() {
        assert_eq!(
            total_bytes(FILES),
            519_322 + 196_725_760 + 20_323_312 + 1_765 + 662 + 1_156_830
        );
        let dir = model_dir(Path::new("/data"));
        assert!(dir.ends_with(format!(
            "models/{}/{REVISION}",
            crate::services::embedding::MODEL_NAME
        )));
    }
}
