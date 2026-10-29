//! Pins the bytes of the tap file. Synthetic exchanges run end to end through `Exchanges::begin`,
//! and the file they write is compared as raw text with the golden files in `tests/golden/`.
//! Clock digits and the random run id are normalised; every other byte, key order included, must match.
//! Set `SOZE_BLESS_GOLDEN` to write the golden files from the current code.

use bytes::Bytes;
use kaiser_soze::adapter::{Adapter, RequestHead, TapInput};
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::exchange::{ExchangeTap, Exchanges, Opening, TapOutput};
use kaiser_soze::logging::Logger;
use kaiser_soze::note::{self, Label, Note};
use kaiser_soze::project::{ProjectCache, ProjectLookups};
use serde_json::{json, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CLOCK_KEYS: [&str; 4] = [
    "\"mono_us\":",
    "\"wall_ms\":",
    "\"response_head_us\":",
    "\"first_byte_us\":",
];
const SSE_CHUNK: usize = 97;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

async fn wait_until(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the tap file never reached the expected state");
}

fn zero_digits_after(text: &str, key: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(key) {
        let (head, tail) = rest.split_at(at + key.len());
        out.push_str(head);
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push('0');
        }
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
}

struct Recording {
    dir: tempfile::TempDir,
    repo: PathBuf,
    exchanges: Exchanges,
    ended: Cell<usize>,
}

impl Recording {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://example.com/owner/repo.git\n",
        )
        .unwrap();
        let output = TapOutput::open(&dir.path().join("tap.jsonl"), Logger::default())
            .await
            .unwrap();
        let exchanges = Exchanges::new(output, None, ProjectLookups::new(ProjectCache::default()));
        Self {
            dir,
            repo,
            exchanges,
            ended: Cell::new(0),
        }
    }

    fn tap_file(&self) -> PathBuf {
        self.dir.path().join("tap.jsonl")
    }

    fn written(&self) -> String {
        std::fs::read_to_string(self.tap_file()).unwrap_or_default()
    }

    fn label(&self) -> Label {
        Label {
            command: "claude".into(),
            dir: self.repo.display().to_string(),
        }
    }

    async fn wait_for_lines(&self, count: usize) {
        wait_until(|| self.written().matches('\n').count() >= count).await;
    }

    /// Begins an exchange and waits for its start line and `start_edges` edges, which the edge task races the parser to write.
    async fn begin(
        &self,
        adapter: &dyn Adapter,
        head: &RequestHead,
        label: Option<&Label>,
        start_edges: usize,
    ) -> ExchangeTap {
        let before = self.written().matches('\n').count();
        let tap = self.exchanges.begin(Opening {
            adapter,
            head,
            parser: adapter.claim(head).expect("the adapter claims the request"),
            label,
            http_version: hyper::Version::HTTP_11,
            upgrade: head.headers.iter().any(|(name, _)| name == "upgrade"),
        });
        self.wait_for_lines(before + 1 + start_edges).await;
        tap
    }

    async fn end(&self, tap: ExchangeTap) {
        drop(tap);
        self.ended.set(self.ended.get() + 1);
        wait_until(|| self.written().matches("\"exchange_end\"").count() >= self.ended.get()).await;
    }

    /// Waits until the exchange that is running has written its one conversation line.
    async fn wait_for_conversation(&self) {
        wait_until(|| self.written().matches("\"event\":\"conversation\"").count() > self.ended.get()).await;
    }

    fn normalised(&self) -> String {
        let mut text = self.written().replace(&self.repo.display().to_string(), "/golden/repo");
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let run_id = first["run_id"].as_str().unwrap();
        text = text.replace(&format!(r#""run_id":"{run_id}""#), r#""run_id":"run-synthetic""#);
        for key in CLOCK_KEYS {
            text = zero_digits_after(&text, key);
        }
        text
    }
}

fn head(host: &str, path: &str, headers: &[(&str, &str)]) -> RequestHead {
    RequestHead {
        method: "POST".into(),
        host: host.into(),
        path: path.into(),
        headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
    }
}

fn response(status: u16, content_type: &str) -> hyper::Response<()> {
    hyper::Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("set-cookie", "synthetic=1")
        .body(())
        .unwrap()
}

fn sse(events: &[(&str, Value)]) -> Vec<u8> {
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect::<String>()
        .into_bytes()
}

fn send_request(tap: &ExchangeTap, body: &Value) {
    let body = body.to_string().into_bytes();
    let (first, second) = body.split_at(body.len() / 2);
    tap.send(TapInput::RequestBody(Bytes::copy_from_slice(first)));
    tap.send(TapInput::RequestBody(Bytes::copy_from_slice(second)));
    tap.send(TapInput::RequestEnd);
}

fn send_reply(tap: &ExchangeTap, stream: &[u8]) {
    for chunk in stream.chunks(SSE_CHUNK) {
        tap.send(TapInput::ResponseBody(Bytes::copy_from_slice(chunk)));
    }
    tap.send(TapInput::ResponseEnd);
}

fn claude_headers<'a>(prompt: &'a str, agent: Option<&'a str>) -> Vec<(&'a str, &'a str)> {
    let mut headers = vec![
        ("x-claude-code-session-id", "session-syn"),
        ("x-claude-code-prompt-id", prompt),
        ("anthropic-version", "2023-06-01"),
    ];
    match agent {
        Some(agent) => headers.extend([
            ("x-claude-code-agent-id", agent),
            ("x-claude-code-request-class", "subagent"),
        ]),
        None => headers.push(("x-claude-code-request-class", "main")),
    }
    headers
}

fn claude_first_request() -> Value {
    json!({
        "model": "synthetic-model",
        "max_tokens": 64,
        "stream": true,
        "system": "You are a synthetic assistant.",
        "tools": [{"name": "shell", "description": "Runs a command", "input_schema": {"type": "object"}}],
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "Describe caf\u{e9} \"quoted\"\nsecond line"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAAAAAA"}},
        ]}],
    })
}

fn claude_first_reply() -> Vec<u8> {
    sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg-syn-1", "usage": {
                "input_tokens": 12, "output_tokens": 1,
                "cache_read_input_tokens": 3, "cache_creation_input_tokens": 0}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Considering the picture."}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "signature-syn"}}),
        ),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Looking "}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "now."}}),
        ),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 2, "content_block": {
                "type": "tool_use", "id": "call-syn-1", "name": "shell", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"ls\"}"}}),
        ),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 2})),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 20}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ])
}

fn claude_second_request() -> Value {
    json!({
        "model": "synthetic-model",
        "system": "You are a synthetic assistant.",
        "diagnostics": {"previous_message_id": "msg-syn-1"},
        "messages": [
            {"role": "user", "content": "Describe the picture."},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Looking now."},
                {"type": "tool_use", "id": "call-syn-1", "name": "shell", "input": {"command": "ls"}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call-syn-1", "content": "two files", "is_error": false},
            ]},
        ],
    })
}

fn claude_failed_reply() -> Vec<u8> {
    sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg-syn-2", "usage": {"input_tokens": 40}}}),
        ),
        (
            "error",
            json!({"type": "error", "error": {"type": "overloaded_error", "message": "synthetic"}}),
        ),
    ])
}

fn claude_subagent_reply() -> Vec<u8> {
    sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg-syn-3", "usage": {"input_tokens": 5}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Done."}}),
        ),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ])
}

struct ClaudeExchange<'a> {
    headers: &'a [(&'a str, &'a str)],
    label: Option<&'a Label>,
    start_edges: usize,
    request: &'a Value,
    reply: &'a [u8],
}

async fn run_claude_exchange(recording: &Recording, exchange: ClaudeExchange<'_>) {
    let adapter = ClaudeAdapter::default();
    let head = head("api.anthropic.com", "/v1/messages", exchange.headers);
    let tap = recording
        .begin(&adapter, &head, exchange.label, exchange.start_edges)
        .await;
    send_request(&tap, exchange.request);
    recording.wait_for_conversation().await;
    tap.note_response_head(&response(200, "text/event-stream"));
    send_reply(&tap, exchange.reply);
    recording.end(tap).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_exchanges_write_these_exact_bytes() {
    // It writes a text, thinking and tool reply, a failed reply that continues it, and a subagent
    let recording = Recording::new().await;
    let label = recording.label();
    let subagent_request = json!({"model": "synthetic-model", "messages": [{"role": "user", "content": "Go."}]});

    let (first_request, first_reply) = (claude_first_request(), claude_first_reply());
    let (second_request, failed_reply) = (claude_second_request(), claude_failed_reply());
    let subagent_reply = claude_subagent_reply();
    let (first, second, subagent) = (
        claude_headers("prompt-syn-1", None),
        claude_headers("prompt-syn-2", None),
        claude_headers("prompt-syn-3", Some("agent-syn")),
    );

    let exchanges = [
        (&first, Some(&label), &first_request, &first_reply),
        (&second, Some(&label), &second_request, &failed_reply),
        (&subagent, None, &subagent_request, &subagent_reply),
    ];
    for (headers, label, request, reply) in exchanges {
        let exchange = ClaudeExchange {
            headers,
            label,
            start_edges: 3,
            request,
            reply,
        };
        run_claude_exchange(&recording, exchange).await;
    }

    assert_golden("claude", &recording.normalised());
}

fn codex_request(extra: Value, input: Value) -> String {
    let mut request = json!({
        "type": "response.create",
        "model": "synthetic-model",
        "instructions": "You are a synthetic coder.",
        "tools": [{"type": "function", "name": "shell"}],
        "input": input,
    });
    for (key, value) in extra.as_object().unwrap() {
        request[key] = value.clone();
    }
    request.to_string()
}

fn user_text(text: &str) -> Value {
    json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})
}

fn texts(events: Vec<Value>) -> Vec<String> {
    events.iter().map(Value::to_string).collect()
}

fn codex_warmup_reply() -> Vec<String> {
    texts(vec![
        json!({"type": "response.created", "response": {"id": "resp-syn-0"}}),
        json!({"type": "response.completed", "response": {
            "id": "resp-syn-0", "status": "completed", "output": [],
            "usage": {"input_tokens": 8, "output_tokens": 0}}}),
    ])
}

fn codex_tool_reply() -> Vec<String> {
    let reasoning = json!({"type": "reasoning", "summary": [], "encrypted_content": "encrypted-syn"});
    let call =
        json!({"type": "function_call", "call_id": "call-syn-9", "name": "shell", "arguments": "{\"command\":\"ls\"}"});
    let message =
        json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Two files."}]});
    texts(vec![
        json!({"type": "response.created", "response": {"id": "resp-syn-1"}}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "reasoning"}}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
        json!({"type": "response.output_item.added", "output_index": 1, "item": {
            "type": "function_call", "call_id": "call-syn-9", "name": "shell"}}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 1, "delta": "{\"command\":\"ls\"}"}),
        json!({"type": "response.output_item.done", "output_index": 1, "item": call}),
        json!({"type": "response.output_item.added", "output_index": 2, "item": {"type": "message"}}),
        json!({"type": "response.output_text.delta", "output_index": 2, "delta": "Two files."}),
        json!({"type": "response.output_item.done", "output_index": 2, "item": message}),
        json!({"type": "response.completed", "response": {
            "id": "resp-syn-1", "status": "completed", "output": [reasoning, call, message],
            "usage": {"input_tokens": 30, "output_tokens": 9,
                "input_tokens_details": {"cached_tokens": 4}, "output_tokens_details": {"reasoning_tokens": 5}}}}),
    ])
}

fn codex_final_reply() -> Vec<String> {
    texts(vec![
        json!({"type": "response.created", "response": {"id": "resp-syn-2"}}),
        json!({"type": "response.completed", "response": {
            "id": "resp-syn-2", "status": "completed", "output": [],
            "usage": {"input_tokens": 50, "output_tokens": 1}}}),
    ])
}

fn codex_failed_reply() -> Vec<String> {
    texts(vec![json!({"type": "error", "error": {"code": "rate_limit_exceeded"}})])
}

/// The messages of one Codex socket, each with the side that sent it.
fn codex_socket() -> Vec<(bool, String)> {
    let image_turn = json!([{"type": "message", "role": "user", "content": [
        {"type": "input_text", "text": "List the files."},
        {"type": "input_image", "image_url": "data:image/png;base64,AAAAAAAA"},
    ]}]);
    let tool_output = json!([{"type": "function_call_output", "call_id": "call-syn-9", "output": "two files"}]);
    let requests = [
        codex_request(
            json!({"generate": false, "client_metadata": {"turn_id": ""}}),
            json!([user_text("warm up")]),
        ),
        codex_request(
            json!({"previous_response_id": "resp-syn-0", "client_metadata": {"turn_id": "turn-syn-1"}}),
            image_turn,
        ),
        codex_request(
            json!({"previous_response_id": "resp-syn-1", "client_metadata": {"turn_id": "turn-syn-2"}}),
            tool_output,
        ),
        codex_request(
            json!({"previous_response_id": "resp-syn-gone", "client_metadata": {"turn_id": "turn-syn-2"}}),
            json!([user_text("start again")]),
        ),
    ];
    let replies = [
        codex_warmup_reply(),
        codex_tool_reply(),
        codex_final_reply(),
        codex_failed_reply(),
    ];
    requests
        .into_iter()
        .zip(replies)
        .flat_map(|(request, reply)| {
            std::iter::once((true, request)).chain(reply.into_iter().map(|event| (false, event)))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_socket_writes_these_exact_bytes() {
    // It writes a warm-up, a chained request, a tool result, a partial request and a failed reply
    let recording = Recording::new().await;
    let label = recording.label();
    let adapter = CodexAdapter::default();
    let head = head(
        "chatgpt.com",
        "/backend-api/codex/responses",
        &[
            ("upgrade", "websocket"),
            ("session-id", "session-syn"),
            ("thread-id", "thread-syn"),
            ("x-codex-turn-metadata", "{\"turn_id\":\"turn-syn-1\"}"),
        ],
    );

    let tap = recording.begin(&adapter, &head, Some(&label), 3).await;
    tap.note_response_head(&response(101, "text/plain"));
    let mut offset = 0;
    for (from_client, text) in codex_socket() {
        let end = offset + text.len() as u64;
        tap.send(TapInput::WsMessage {
            from_client,
            text,
            start: offset,
            end,
        });
        offset = end;
    }
    recording.end(tap).await;

    assert_golden("codex", &recording.normalised());
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_alone_write_these_exact_bytes() {
    // It writes an unsupported request and a tunnel, each with and without a label
    let recording = Recording::new().await;
    let label = recording.label();
    let unsupported = |label| {
        Note::Unsupported(note::Unsupported {
            host: "other.test".into(),
            label,
            path: "/v1/other".into(),
        })
    };
    let tunnel = |label| {
        Note::Tunnel(note::Tunnel {
            host: "tunnel.test".into(),
            label,
            port: 443,
        })
    };

    let exchanges = &recording.exchanges;
    exchanges.note_alone("claude", unsupported(Some(label.clone())));
    exchanges.note_alone("claude", unsupported(None));
    exchanges.note_alone("none", tunnel(Some(label)));
    exchanges.note_alone("none", tunnel(None));
    recording.wait_for_lines(4).await;

    assert_golden("notes", &recording.normalised());
}

fn assert_golden(name: &str, written: &str) {
    let path = golden_dir().join(format!("{name}.jsonl"));
    if std::env::var_os("SOZE_BLESS_GOLDEN").is_some() {
        std::fs::write(&path, written).unwrap();
        panic!("{name}.jsonl was rewritten; run again without SOZE_BLESS_GOLDEN");
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    let (got, want): (Vec<&str>, Vec<&str>) = (written.lines().collect(), expected.lines().collect());
    assert_eq!(got.len(), want.len());
    for (got, want) in got.iter().zip(&want) {
        assert_eq!(got, want);
    }
    assert_eq!(written, expected);
}

#[test]
fn every_golden_line_is_what_serde_json_writes_for_its_parsed_value() {
    // It proves the keys are sorted and the escapes are serde_json's, which a typed line must reproduce
    let mut checked = 0;
    for entry in std::fs::read_dir(golden_dir()).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in text.lines() {
            let parsed: Value = serde_json::from_str(line).unwrap();
            assert_eq!(parsed.to_string(), line);
            checked += 1;
        }
    }
    assert!(checked > 0);
}
