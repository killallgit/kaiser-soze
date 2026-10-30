use crate::support::*;
use bytes::Bytes;
use http_body_util::Full;
use hyper::Request;
use serde_json::Value;
use std::path::Path;

fn identified_request() -> Request<Full<Bytes>> {
    let mut request = post_request(CLAIMED);
    request
        .headers_mut()
        .insert("session-id", "session-synthetic".parse().unwrap());
    request
        .headers_mut()
        .insert("thread-id", "thread-synthetic".parse().unwrap());
    request
}

fn complete_lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.ends_with('\n'));
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn run_id(lines: &[Value]) -> &str {
    let id = lines[0]["run_id"].as_str().unwrap();
    assert!(!id.is_empty());
    assert!(lines.iter().all(|line| line["run_id"] == id));
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["sequence"].as_u64(), Some(index as u64 + 1));
    }
    id
}

#[tokio::test]
async fn every_tap_envelope_shares_one_run_id_including_anonymous_notes() {
    // It shares an epoch across concurrent agents, their graph edges, and unread traffic.
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy_with(&upstream, dir.path(), vec![codex(), claude()]).await;

    let (codex_body, claude_body, unsupported_body) = tokio::join!(
        send_through(proxy, roots.clone(), CLAIMED, identified_request()),
        send_through(
            proxy,
            roots.clone(),
            CLAUDE_HOST,
            claude_request("/v1/messages", CLAUDE_BODY),
        ),
        send_through(
            proxy,
            roots,
            CLAUDE_HOST,
            claude_request("/v1/messages/count_tokens", CLAUDE_BODY),
        ),
    );
    assert_eq!(codex_body, sse_events().concat().as_bytes());
    assert_eq!(claude_body, anthropic_events().concat().as_bytes());
    assert_eq!(unsupported_body, anthropic_events().concat().as_bytes());
    let (status, tunnel) = connect_tunnel(proxy, UNCLAIMED).await;
    assert_eq!(status, 200);
    drop(tunnel);

    let tap_file = dir.path().join("tap.jsonl");
    wait_until(|| {
        let lines = tap_lines(&tap_file);
        count(&lines, "exchange_end") == 2 && count(&lines, "tunnel") == 1
    })
    .await;
    let lines = complete_lines(&tap_file);
    let id = run_id(&lines);
    assert_ne!(id, "session-synthetic");
    assert_ne!(id, "thread-synthetic");
    for kind in [
        "exchange_start",
        "response_head",
        "conversation",
        "block_start",
        "block_delta",
        "block_stop",
        "usage",
        "reply_end",
        "record",
        "edge",
        "exchange_end",
        "unsupported",
        "tunnel",
    ] {
        assert!(count(&lines, kind) > 0);
    }
    let anonymous = lines
        .iter()
        .find(|line| line["event"] == "record" && line["adapter"] == "claude")
        .unwrap();
    assert!(anonymous["payload"].get("session").is_none());
    assert!(anonymous["payload"].get("thread").is_none());
    let shapes = complete_lines(&dir.path().join("shapes.jsonl"));
    assert!(!shapes.is_empty());
    assert!(shapes.iter().all(|line| line.get("run_id").is_none()));
    assert!(shapes.iter().all(|line| line.get("sequence").is_none()));
}

#[tokio::test]
async fn restarting_the_proxy_closes_the_first_run_into_its_own_segment_without_changing_native_identity() {
    // It keeps one run per file after a restart, even when exchange and native ids repeat.
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let tap_file = dir.path().join("tap.jsonl");
    let (first, roots) = start_proxy(&upstream, dir.path(), codex()).await;
    send_through(first, roots, CLAIMED, identified_request()).await;
    let first_lines = read_events(&tap_file).await;
    let first_id = run_id(&first_lines).to_string();

    let (second, roots) = start_proxy(&upstream, dir.path(), codex()).await;
    send_through(second, roots, CLAIMED, identified_request()).await;
    wait_until(|| count(&tap_lines(&tap_file), "exchange_end") == 1).await;
    let before = complete_lines(&dir.path().join("tap.000001.jsonl"));
    let after = complete_lines(&tap_file);
    assert_eq!(before.len(), first_lines.len());
    assert_eq!(run_id(&before), first_id);
    assert_ne!(run_id(&after), first_id);
    for lines in [&before[..], &after[..]] {
        assert!(lines.iter().all(|line| line["exchange"] == 1));
        assert_eq!(count(lines, "record"), 1);
        let record = &records_of(lines, 1)[0];
        assert_eq!(record["session"], "session-synthetic");
        assert_eq!(record["thread"], "thread-synthetic");
        let in_thread = lines
            .iter()
            .find(|line| line["event"] == "edge" && line["payload"]["kind"] == "in_thread")
            .unwrap();
        assert_eq!(in_thread["payload"]["from"], "exchange:1");
        assert_eq!(in_thread["payload"]["to"], "thread:thread-synthetic");
    }
}
