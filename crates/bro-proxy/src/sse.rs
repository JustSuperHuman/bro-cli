//! Incremental Server-Sent Events parsing and encoding.

use bytes::Bytes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` field, if any
    pub event: Option<String>,
    /// joined `data:` lines
    pub data: String,
}

/// Byte-level SSE parser: feed arbitrary chunks, get complete events.
/// Handles `\n`, `\r\n`, comments, multi-line data and UTF-8 split across chunks.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    has_data: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(pos) = self.buf[start..].iter().position(|b| *b == b'\n') {
            let end = start + pos;
            let mut line = &self.buf[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let line = String::from_utf8_lossy(line).into_owned();
            start = end + 1;
            if let Some(ev) = self.line(&line) {
                out.push(ev);
            }
        }
        self.buf.drain(..start);
        out
    }

    /// Flush a trailing event that wasn't terminated by a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let line = String::from_utf8_lossy(&rest)
                .trim_end_matches('\r')
                .to_string();
            if let Some(ev) = self.line(&line) {
                out.push(ev);
            }
        }
        if let Some(ev) = self.dispatch() {
            out.push(ev);
        }
        out
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.find(':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(' ').unwrap_or(v))
            }
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => {
                self.data.push(value.to_string());
                self.has_data = true;
            }
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if !self.has_data && self.event.is_none() {
            return None;
        }
        let ev = SseEvent {
            event: self.event.take(),
            data: self.data.join("\n"),
        };
        self.data.clear();
        self.has_data = false;
        Some(ev)
    }
}

/// `event: {name}\ndata: {data}\n\n`
pub fn encode(event: Option<&str>, data: &str) -> Bytes {
    let mut s = String::with_capacity(data.len() + 32);
    if let Some(e) = event {
        s.push_str("event: ");
        s.push_str(e);
        s.push('\n');
    }
    for line in data.split('\n') {
        s.push_str("data: ");
        s.push_str(line);
        s.push('\n');
    }
    s.push('\n');
    Bytes::from(s)
}

pub fn comment(text: &str) -> Bytes {
    Bytes::from(format!(": {text}\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_chunks_and_crlf() {
        let mut p = SseParser::new();
        let mut evs = p.push(b"event: a\r\ndata: {\"x\"");
        assert!(evs.is_empty());
        evs.extend(p.push(b":1}\r\n\r\n: comment\n\ndata: l1\ndata: l2\n\n"));
        assert_eq!(evs.len(), 2);
        assert_eq!(
            evs[0],
            SseEvent {
                event: Some("a".into()),
                data: "{\"x\":1}".into()
            }
        );
        assert_eq!(evs[1].data, "l1\nl2");
        evs = p.push(b"data: [DONE]");
        assert!(evs.is_empty());
        assert_eq!(p.finish()[0].data, "[DONE]");
    }

    #[test]
    fn utf8_split() {
        let mut p = SseParser::new();
        let s = "data: é\n\n".as_bytes();
        let mut evs = p.push(&s[..7]);
        evs.extend(p.push(&s[7..]));
        assert_eq!(evs[0].data, "é");
    }

    #[test]
    fn encode_roundtrip() {
        let b = encode(Some("x"), "a\nb");
        let mut p = SseParser::new();
        assert_eq!(
            p.push(&b)[0],
            SseEvent {
                event: Some("x".into()),
                data: "a\nb".into()
            }
        );
    }
}
