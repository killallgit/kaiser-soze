use crate::support::*;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use kaiser_soze::adapter::event::*;
use kaiser_soze::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::common::*;
use kaiser_soze::message::{Block, Message};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn taps_an_anthropic_stream_through_the_claude_adapter() {
    // It relays the stream unchanged and records the request summary and every delta
    let events = anthropic_events();
    let upstream = start_upstream(events.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let adapter: Arc<dyn Adapter> = Arc::new(ClaudeAdapter::new(vec![CLAIMED.to_string()]));
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), adapter).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    let body = r#"{"model":"m1","stream":true,"max_tokens":8,"system":"s","messages":[{"role":"user","content":"hi"}],"tools":[{"name":"Bash"}]}"#;
    let request = Request::post("/v1/messages")
        .header("anthropic-beta", "beta-a")
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    let received = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(received, events.concat().as_bytes());
    let tapped = read_events(&dir.path().join("tap.jsonl")).await;
    assert_eq!(count(&tapped, "block_delta"), 3);
    assert_eq!(count(&tapped, "reply_end"), 1);
    let conversation = tapped.iter().find(|e| e["event"] == "conversation").unwrap();
    assert_eq!(conversation["adapter"], "claude");
    assert_eq!(conversation["payload"]["tools"][0]["name"], "Bash");
    let records = records_of(&tapped, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "abc");
    assert_eq!(records[0]["reply"]["usage"]["input"], 5);
    assert_eq!(records[0]["reply"]["usage"]["output"], 3);
    assert_eq!(fold(&tapped, 1), records);
    let raw = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(!raw.contains(SECRET));
}

#[tokio::test]
async fn writes_the_shape_of_each_request_and_event_without_their_values() {
    // It logs keys and types of the request and every distinct event, and no header or body value
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let adapter: Arc<dyn Adapter> = Arc::new(ClaudeAdapter::new(vec![CLAIMED.to_string()]));
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), adapter).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    let body =
        r#"{"model":"m1","stream":true,"max_tokens":8,"system":"s","messages":[{"role":"user","content":"hi"}]}"#;
    let request = Request::post("/v1/messages")
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    response.into_body().collect().await.unwrap();

    let shapes_file = dir.path().join("shapes.jsonl");
    wait_until(|| {
        std::fs::read_to_string(&shapes_file)
            .unwrap_or_default()
            .contains("message_stop")
    })
    .await;
    let lines = tap_lines(&shapes_file);
    let kinds: Vec<&str> = lines.iter().filter_map(|l| l["kind"].as_str()).collect();
    assert!(kinds.contains(&"message_start"));
    assert_eq!(kinds.iter().filter(|k| **k == "content_block_delta").count(), 1);
    let request_shape = lines.iter().find(|l| l["direction"] == "request").unwrap();
    assert_eq!(request_shape["shape"]["messages"][0]["role"], "string=user");
    assert_eq!(request_shape["shape"]["model"], "string");
    let raw = std::fs::read_to_string(&shapes_file).unwrap();
    assert!(!raw.contains(SECRET) && !raw.contains("\"hi\""));
}

#[tokio::test]
async fn concurrent_exchanges_of_both_agents_stay_separate_and_share_one_shape() {
    // It records one conversation and one record per request, never mixing exchanges
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy_with(&upstream, dir.path(), vec![codex(), claude()]).await;
    let mut calls = Vec::new();
    for _ in 0..3 {
        calls.push(tokio::spawn(send_through(
            proxy,
            roots.clone(),
            CLAIMED,
            post_request(CLAIMED),
        )));
        calls.push(tokio::spawn(send_through(
            proxy,
            roots.clone(),
            CLAUDE_HOST,
            claude_request("/v1/messages", CLAUDE_BODY),
        )));
    }

    let mut bodies = Vec::new();
    for call in calls {
        bodies.push(call.await.unwrap());
    }
    let tap_file = dir.path().join("tap.jsonl");
    for _ in 0..100 {
        if count(&tap_lines(&tap_file), "exchange_end") == 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let lines = tap_lines(&tap_file);
    let codex_bytes = sse_events().concat().into_bytes();
    let claude_bytes = anthropic_events().concat().into_bytes();
    assert_eq!(bodies.iter().filter(|body| **body == codex_bytes).count(), 3);
    assert_eq!(bodies.iter().filter(|body| **body == claude_bytes).count(), 3);
    assert_eq!(count(&lines, "conversation"), 6);
    assert_eq!(count(&lines, "record"), 6);
    let mut texts = Vec::new();
    for exchange in 1..=6u64 {
        let own: Vec<&Value> = lines.iter().filter(|line| line["exchange"] == exchange).collect();
        assert!(own.iter().all(|line| line["adapter"] == own[0]["adapter"]));
        let records = records_of(&lines, exchange);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["incomplete"], false);
        assert_eq!(fold(&lines, exchange), records);
        let mut fields: Vec<&String> = records[0].as_object().unwrap().keys().collect();
        fields.sort();
        assert_eq!(fields, ["conversation", "dropped", "incomplete", "reply", "step"]);
        texts.push(
            records[0]["reply"]["message"]["blocks"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    texts.sort();
    assert_eq!(texts, ["abc", "abc", "abc", "abcd", "abcd", "abcd"]);
}

#[tokio::test]
async fn a_claimed_host_whose_adapter_declines_is_relayed_untouched_and_reported() {
    // It writes one unsupported line with host and path only, without the query string
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;

    let body = send_through(
        proxy,
        roots,
        CLAUDE_HOST,
        claude_request("/v1/messages/count_tokens?beta=true", CLAUDE_BODY),
    )
    .await;

    assert_eq!(body, anthropic_events().concat().as_bytes());
    let tap_file = dir.path().join("tap.jsonl");
    wait_for_line(&tap_file, "unsupported").await;
    let lines = tap_lines(&tap_file);
    let unsupported = lines.iter().find(|line| line["event"] == "unsupported").unwrap();
    assert_eq!(
        unsupported["payload"],
        serde_json::json!({"host": CLAUDE_HOST, "path": "/v1/messages/count_tokens"})
    );
    assert_eq!(count(&lines, "record"), 0);
}

struct Echo {
    hosts: Vec<String>,
}

struct EchoParser;

impl Adapter for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn hosts(&self) -> &[String] {
        &self.hosts
    }

    fn claim(&self, _head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        Some(Box::new(EchoParser))
    }
}

impl ExchangeParser for EchoParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        if !matches!(input, TapInput::ResponseEnd) {
            return Vec::new();
        }
        vec![
            conversation(Value::Null, Value::Null, vec![Message::user(vec![Block::text("hi")])]),
            block_start(BlockStart {
                index: 0,
                kind: BlockKind::Text,
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Text("echo".into())),
            reply_end(Value::Null),
        ]
    }
}

#[tokio::test]
async fn a_new_adapter_that_emits_common_events_is_captured_without_a_core_change() {
    // It gets live lines and one record from the unchanged proxy
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let echo: Arc<dyn Adapter> = Arc::new(Echo {
        hosts: vec![CLAIMED.to_string()],
    });
    let (proxy, roots) = start_proxy(&upstream, dir.path(), echo).await;

    send_through(proxy, roots, CLAIMED, post_request(CLAIMED)).await;

    let lines = read_events(&dir.path().join("tap.jsonl")).await;
    let records = records_of(&lines, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "echo");
    assert_eq!(
        lines.iter().find(|line| line["event"] == "record").unwrap()["adapter"],
        "echo"
    );
}

#[tokio::test]
async fn credentials_and_image_bytes_never_reach_the_tap_file() {
    // It keeps both out of every line and leaves only a reference for the image
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;
    let body = r#"{"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"SU1HQllURVNNQVJLRVI="}}]}]}"#;

    send_through(proxy, roots, CLAUDE_HOST, claude_request("/v1/messages", body)).await;

    let tap_file = dir.path().join("tap.jsonl");
    let lines = read_events(&tap_file).await;
    let raw = std::fs::read_to_string(&tap_file).unwrap();
    assert_eq!(raw.matches(SECRET).count(), 0);
    assert_eq!(raw.matches("SU1HQllURVNNQVJLRVI").count(), 0);
    let conversation = lines.iter().find(|line| line["event"] == "conversation").unwrap();
    assert_eq!(
        conversation["payload"]["messages"][0]["blocks"][0],
        json!(Block::Image {
            content_type: "image/png".into(),
            reference: "0:0".into(),
            size: 14
        })
    );
}
