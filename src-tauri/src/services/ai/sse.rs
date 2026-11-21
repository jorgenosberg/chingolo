//! Minimal Server-Sent Events reader. Feed it raw bytes, get back the `data:` payload of each
//! completed event.

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

fn find_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((at, len)) = find_boundary(&self.buf) {
            let event: Vec<u8> = self.buf.drain(..at + len).collect();
            let text = String::from_utf8_lossy(&event[..at]).replace("\r\n", "\n");
            let data: Vec<&str> = text
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                out.push(data.join("\n"));
            }
        }
        out
    }

    /// Whatever is left when the stream ends without a trailing blank line.
    pub fn finish(&mut self) -> Vec<String> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        self.buf.extend_from_slice(b"\n\n");
        self.push(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: {\"a\":").is_empty());
        assert_eq!(
            p.push(b"1}\n\ndata: {\"b\":2}\n\n"),
            ["{\"a\":1}", "{\"b\":2}"]
        );
    }

    #[test]
    fn handles_crlf_comments_and_multibyte_split() {
        let mut p = SseParser::default();
        let bytes = "data: blåbær\r\n\r\n".as_bytes();
        // split inside the multi-byte "å"
        assert!(p.push(&bytes[..9]).is_empty());
        assert_eq!(p.push(&bytes[9..]), ["blåbær"]);
        assert!(p.push(b": keep-alive\n\n").is_empty());
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: tail").is_empty());
        assert_eq!(p.finish(), ["tail"]);
        assert!(p.finish().is_empty());
    }
}
