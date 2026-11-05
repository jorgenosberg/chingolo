//! A tiny local HTTP server for provider tests. It answers each request with the next canned
//! reply and records what it was sent.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

pub type Seen = Arc<Mutex<Vec<(String, String)>>>;

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl Reply {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Reply { status, content_type: "application/json", headers: Vec::new(), body: body.into() }
    }

    pub fn sse(body: impl Into<String>) -> Self {
        Reply { status: 200, content_type: "text/event-stream", headers: Vec::new(), body: body.into() }
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_string()));
        self
    }
}

/// JSON replies only: `(status, body)`.
pub fn serve(replies: Vec<(u16, String)>) -> (String, Seen) {
    serve_with(replies.into_iter().map(|(s, b)| Reply::json(s, b)).collect())
}

/// Returns the base URL and the requests seen so far as `(head, body)`.
pub fn serve_with(replies: Vec<Reply>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for reply in replies {
            let Ok((mut s, _)) = listener.accept() else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head_end, len) = loop {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text[..i]
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (i + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            log.lock().unwrap().push((head, String::from_utf8_lossy(&buf[head_end..]).to_string()));
            let extra: String = reply.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
            let out = format!(
                "HTTP/1.1 {} X\r\ncontent-type: {}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{}",
                reply.status,
                reply.content_type,
                reply.body.len(),
                reply.body
            );
            let _ = s.write_all(out.as_bytes());
        }
    });
    (base, seen)
}
