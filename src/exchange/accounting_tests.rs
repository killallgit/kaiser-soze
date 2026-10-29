use super::*;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use serde_json::Value;
use std::time::Duration;

async fn tap(dir: &tempfile::TempDir) -> ExchangeTap {
    let output = TapOutput::open(&dir.path().join("tap.jsonl"), Default::default())
        .await
        .unwrap();
    let exchanges = Exchanges::new(output, None, crate::project::ProjectLookups::new(Default::default()));
    super::tests::begin(&exchanges, &super::tests::nobody(), None)
}

async fn summary(dir: &tempfile::TempDir) -> Value {
    let path = dir.path().join("tap.jsonl");
    for _ in 0..100 {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(end) = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|line| line["event"] == "exchange_end")
        {
            return end["payload"].clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("exchange did not finish");
}

#[tokio::test]
async fn tap_accounts_response_inputs_before_queue_submission() {
    let dir = tempfile::tempdir().unwrap();
    let tap = tap(&dir).await;

    tap.send_body(Side::Request, Bytes::from_static(b"request"));
    tap.send_body(Side::Response, Bytes::from_static(b"abc"));
    tap.send_body(Side::Response, Bytes::from_static(b"de"));
    drop(tap);

    let end = summary(&dir).await;
    assert_eq!(end["bytes"], 5);
    assert_eq!(end["chunks"], 2);
    assert!(end["first_byte_us"].is_u64());
    assert!(end["response_head_us"].is_null());
    assert_eq!(end["tap_dropped"], 0);
}

#[tokio::test]
async fn tap_accounts_only_server_websocket_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let tap = tap(&dir).await;
    let frame = Bytes::from_static(&[0x81, 1, b'a']);

    tap.send_ws_bytes(true, frame.clone(), Duration::from_millis(2));
    tap.send_ws_bytes(false, frame, Duration::from_millis(5));
    drop(tap);

    let end = summary(&dir).await;
    assert_eq!(end["bytes"], 3);
    assert_eq!(end["chunks"], 1);
    assert_eq!(end["forward_sum_us"], 7000);
    assert_eq!(end["forward_max_us"], 5000);
    assert!(end["first_byte_us"].is_u64());
    assert_eq!(end["tap_dropped"], 0);
}

#[tokio::test]
async fn http_body_counts_response_bytes_once_and_preserves_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let tap = tap(&dir).await;
    tap.note_response_head(&hyper::Response::new(()));

    let body = TeeBody::new(Full::new(Bytes::from_static(b"abc")), Some(tap.clone()), Side::Response);
    let received = body.collect().await.unwrap().to_bytes();
    drop(tap);

    let end = summary(&dir).await;
    assert_eq!(received, Bytes::from_static(b"abc"));
    assert_eq!(end["bytes"], 3);
    assert_eq!(end["chunks"], 1);
    assert!(end["response_head_us"].is_u64());
    assert!(end["first_byte_us"].is_u64());
    assert_eq!(end["tap_dropped"], 0);
}
