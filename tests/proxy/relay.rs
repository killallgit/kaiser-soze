use crate::support::*;
use http_body_util::BodyExt;
use hyper::StatusCode;
use rustls::RootCertStore;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

#[tokio::test]
async fn relays_a_claimed_sse_stream_live_and_taps_it() {
    // It forwards each event as it arrives, byte-identical, and records parsed events
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (status, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;

    let mut response = client.send_request(post_request(CLAIMED)).await.unwrap();
    let mut received = Vec::new();
    let mut first_chunk_at = None;
    let mut checked_live = false;
    let tap_file = dir.path().join("tap.jsonl");
    while let Some(frame) = response.body_mut().frame().await {
        if let Some(data) = frame.unwrap().data_ref() {
            first_chunk_at.get_or_insert_with(Instant::now);
            received.extend_from_slice(data);
            if !checked_live && String::from_utf8_lossy(&received).contains("\"delta\":\"a\"") {
                checked_live = true;
                wait_for_line(&tap_file, "block_delta").await;
                assert_eq!(count(&tap_lines(&tap_file), "record"), 0);
            }
        }
    }

    assert_eq!(status, 200);
    assert_eq!(received, sse_events().concat().into_bytes());
    let finished_at = upstream.finished_at.lock().unwrap().expect("upstream finished");
    assert!(first_chunk_at.unwrap() < finished_at);
    let events = read_events(&dir.path().join("tap.jsonl")).await;
    assert!(checked_live);
    assert_eq!(count(&events, "block_delta"), 4);
    assert_eq!(count(&events, "usage"), 1);
    assert_eq!(count(&events, "reply_end"), 1);
    let records = records_of(&events, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["incomplete"], false);
    assert_eq!(records[0]["conversation"]["system"], "sys");
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "abcd");
    assert_eq!(fold(&events, 1), records);
    let raw = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(!raw.contains(SECRET));
    let end = events.iter().find(|e| e["event"] == "exchange_end").unwrap();
    assert_eq!(end["payload"]["sink_dropped"], 0);
    let heads = upstream.heads.lock().unwrap();
    assert!(heads.iter().all(|head| !head.contains("accept-encoding")));
}

#[tokio::test]
async fn redials_upstream_when_it_closed_between_requests() {
    // It answers a second request on the same tunnel after the upstream hung up
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    fetch(&mut client, CLAIMED).await;
    wait_until(|| upstream.finished_at.lock().unwrap().is_some()).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (status, body) = fetch(&mut client, CLAIMED).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, sse_events().concat().as_bytes());
}

#[tokio::test]
async fn tunnels_an_unclaimed_host_without_decrypting() {
    // It lets the client see the upstream's own certificate, and records only that a tunnel opened
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, _tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (_, tunnel) = connect_tunnel(proxy, UNCLAIMED).await;
    let mut client = http_client(tunnel, UNCLAIMED, upstream.roots.clone()).await;

    let response = client.send_request(post_request(UNCLAIMED)).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(body, sse_events().concat().as_bytes());
    let lines = tap_lines(&dir.path().join("tap.jsonl"));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "tunnel");
    assert_eq!(lines[0]["payload"], serde_json::json!({"host": UNCLAIMED, "port": 443}));
}

#[tokio::test]
async fn refuses_the_tunnel_when_upstream_is_unreachable() {
    // It answers CONNECT with a failure status instead of 200
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    let upstream = FakeUpstream {
        addr: dead_addr,
        roots: RootCertStore::empty(),
        finished_at: Arc::default(),
        heads: Arc::default(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (proxy, _) = start_proxy(&upstream, dir.path(), codex()).await;

    let (status, _) = connect_tunnel(proxy, CLAIMED).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY.as_u16());
}
