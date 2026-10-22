use crate::support::*;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use kaiser_soze::adapter::Adapter;
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::live::Live;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn shows_one_request_line_and_one_reply_line_without_message_text() {
    // The live view prints a line per request and per reply, and the tap file is written as usual
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (printed, mut screen) = tokio::io::duplex(8192);
    let live = Live::new(printed, "/home/me".into(), 0);
    let adapters: Vec<Arc<dyn Adapter>> = vec![Arc::new(ClaudeAdapter::new(vec![CLAIMED.to_string()]))];
    let (proxy, tap_roots) = start_proxy_live(&upstream, dir.path(), adapters, Some(live)).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    let body = r#"{"model":"m1","stream":true,"max_tokens":8,"system":"SECRETWORD","messages":[{"role":"user","content":"SECRETWORD"}]}"#;
    let request = Request::post("/v1/messages")
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    response.into_body().collect().await.unwrap();

    let mut seen = String::new();
    let mut chunk = [0u8; 512];
    tokio::time::timeout(Duration::from_secs(5), async {
        while seen.matches('\n').count() < 2 {
            let n = screen.read(&mut chunk).await.unwrap();
            seen.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    })
    .await
    .unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("-> 1 msgs"));
    assert!(lines[1].contains("<-"));
    assert!(!seen.contains("SECRETWORD"));
    let tap = dir.path().join("tap.jsonl");
    wait_until(|| std::fs::read_to_string(&tap).unwrap_or_default().contains("\"record\"")).await;
}
