pub struct SseEvent {
    pub data: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Default)]
pub struct SseSplitter {
    line: Vec<u8>,
    data: Vec<String>,
    event_start: Option<u64>,
    consumed: u64,
}

impl SseSplitter {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for &byte in chunk {
            self.consumed += 1;
            if byte == b'\n' {
                events.extend(self.finish_line(1));
            } else {
                self.line.push(byte);
            }
        }
        events
    }

    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events: Vec<SseEvent> = self.finish_line(0).into_iter().collect();
        events.extend(self.emit());
        events
    }

    fn finish_line(&mut self, newline_bytes: u64) -> Option<SseEvent> {
        let mut line = std::mem::take(&mut self.line);
        let line_start = self.consumed - line.len() as u64 - newline_bytes;
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            return self.emit();
        }
        if let Some(rest) = line.strip_prefix(b"data:") {
            self.event_start.get_or_insert(line_start);
            let rest = rest.strip_prefix(b" ").unwrap_or(rest);
            self.data.push(String::from_utf8_lossy(rest).into_owned());
        }
        None
    }

    fn emit(&mut self) -> Option<SseEvent> {
        let start = self.event_start.take()?;
        let data = std::mem::take(&mut self.data).join("\n");
        Some(SseEvent {
            data,
            start,
            end: self.consumed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datas(events: &[SseEvent]) -> Vec<&str> {
        events.iter().map(|e| e.data.as_str()).collect()
    }

    #[test]
    fn splits_events_on_blank_lines() {
        // It yields one event per blank-line terminated block, with byte offsets
        let mut splitter = SseSplitter::default();

        let events = splitter.push(b"data: one\n\ndata: two\n\n");

        assert_eq!(datas(&events), ["one", "two"]);
        assert_eq!((events[0].start, events[0].end), (0, 11));
        assert_eq!((events[1].start, events[1].end), (11, 22));
    }

    #[test]
    fn joins_an_event_split_across_chunks() {
        // It holds a partial line until the rest arrives, even mid-character
        let mut splitter = SseSplitter::default();
        let whole = "event: x\r\ndata: h\u{e9}llo\r\n\r\n".as_bytes();
        let (head, tail) = whole.split_at(17);

        let first = splitter.push(head);
        let second = splitter.push(tail);

        assert!(first.is_empty());
        assert_eq!(datas(&second), ["h\u{e9}llo"]);
    }

    #[test]
    fn joins_multiple_data_lines_with_newlines() {
        // It joins consecutive data lines of one event with a newline
        let mut splitter = SseSplitter::default();

        let events = splitter.push(b"data: a\ndata: b\n\n");

        assert_eq!(datas(&events), ["a\nb"]);
    }

    #[test]
    fn finish_flushes_a_partial_final_line() {
        // It yields the unterminated last event when the stream ends
        let mut splitter = SseSplitter::default();
        assert!(splitter.push(b"data: tail").is_empty());

        let events = splitter.finish();

        assert_eq!(datas(&events), ["tail"]);
        assert_eq!((events[0].start, events[0].end), (0, 10));
    }

    #[test]
    fn ignores_comments_and_other_fields() {
        // It skips comment lines and non-data fields
        let mut splitter = SseSplitter::default();

        let events = splitter.push(b": keepalive\n\nevent: ping\nid: 7\n\n");

        assert!(events.is_empty());
    }
}
