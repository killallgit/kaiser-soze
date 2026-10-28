use crate::support::*;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[tokio::test]
async fn relays_a_claimed_websocket_in_both_directions_and_finishes_capture() {
    let completed = tokio::time::timeout(Duration::from_secs(5), roundtrip("/backend-api/codex/responses", true)).await;
    assert!(completed.is_ok());
}

#[tokio::test]
async fn relays_an_unclaimed_websocket_path_without_capturing_frames() {
    let completed = tokio::time::timeout(Duration::from_secs(5), roundtrip("/other", false)).await;
    assert!(completed.is_ok());
}

async fn roundtrip(path: &str, captured: bool) {
    let upstream = start_upstream(Vec::new()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (status, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    assert_eq!(status, 200);
    let mut client = tls_client(tunnel, CLAIMED, roots).await;
    client
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {CLAIMED}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nAccept-Encoding: gzip\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(client.read_u8().await.unwrap());
    }
    assert_eq!(&head[9..12], b"101");

    // The upstream sends this frame together with its upgrade response.
    let mut first = [0; 3];
    client.read_exact(&mut first).await.unwrap();
    assert_eq!(first, [0x89, 1, b'?']);
    client.write_all(&[0x89, 0x81, 1, 2, 3, 4, 0x20]).await.unwrap();
    client.flush().await.unwrap();
    let mut echo = [0; 9];
    client.read_exact(&mut echo).await.unwrap();
    assert_eq!(echo, [0x82, 7, 0x89, 0x81, 1, 2, 3, 4, 0x20]);

    // Ending client writes must still allow the final upstream frame through.
    client.shutdown().await.unwrap();
    let mut tail = Vec::new();
    client.read_to_end(&mut tail).await.unwrap();
    assert_eq!(tail, [0x88, 0]);
    wait_until(|| upstream.finished_at.lock().unwrap().is_some()).await;
    let tap_file = dir.path().join("tap.jsonl");
    if captured {
        let lines = read_events(&tap_file).await;
        let head = lines.iter().find(|line| line["event"] == "response_head").unwrap();
        assert_eq!(head["payload"]["status"], 101);
        assert_eq!(head["payload"]["transport"], "websocket");
        assert_eq!(count(&lines, "exchange_end"), 1);
        let end = lines.iter().find(|line| line["event"] == "exchange_end").unwrap();
        assert_eq!(end["payload"]["bytes"], 14);
        assert!(end["payload"]["chunks"].as_u64().unwrap() >= 3);
        assert!(end["payload"]["first_byte_us"].is_u64());
        assert!(end["payload"]["response_head_us"].is_u64());
        assert_eq!(end["payload"]["tap_dropped"], 0);
        assert_eq!(end["payload"]["sink_dropped"], 0);
    } else {
        wait_for_line(&tap_file, "unsupported").await;
        let lines = tap_lines(&tap_file);
        assert_eq!(count(&lines, "unsupported"), 1);
        assert_eq!(count(&lines, "exchange_start"), 0);
        assert_eq!(count(&lines, "response_head"), 0);
    }
    let heads = upstream.heads.lock().unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].contains("accept-encoding"), !captured);
}

pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(tls: &mut S) {
    tls.write_all(b"HTTP/1.1 101 Switching Protocols\r\nconnection: Upgrade\r\nupgrade: websocket\r\n\r\n\x89\x01?")
        .await
        .unwrap();
    tls.flush().await.unwrap();
    let mut frame = [0; 7];
    tls.read_exact(&mut frame).await.unwrap();
    tls.write_all(&[0x82, 7]).await.unwrap();
    tls.write_all(&frame).await.unwrap();
    tls.flush().await.unwrap();
    let mut remaining = Vec::new();
    tls.read_to_end(&mut remaining).await.unwrap();
    assert!(remaining.is_empty());
    tls.write_all(&[0x88, 0]).await.unwrap();
    tls.shutdown().await.unwrap();
}
