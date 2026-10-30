use super::*;
use crate::graph::ThreadRole;
use crate::line::Entry;
use crate::note::{Note, Tunnel};
use serde_json::Value;

fn line(exchange: u64) -> Line {
    Line {
        run_id: "producer-synthetic".into(),
        sequence: 99,
        exchange,
        adapter: "test",
        mono_us: 0,
        wall_ms: 0,
        start: 0,
        end: 0,
        session: None,
        thread: None,
        thread_role: ThreadRole::Unknown,
        step: None,
        entry: Entry::Note(Note::Tunnel(Tunnel {
            host: "synthetic.test".into(),
            label: None,
            port: 443,
        })),
    }
}

async fn emitted(path: &Path, count: usize) -> Vec<Value> {
    for _ in 0..50 {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.ends_with('\n') {
            let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            if lines.len() == count {
                return lines;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("tap output did not emit the expected number of lines");
}

#[tokio::test]
async fn the_writer_owns_capture_identity_and_delivery_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tap.jsonl");
    let output = TapOutput::open(&path, Logger::default()).await.unwrap();

    assert!(output.sink.write(line(1)));
    assert!(output.sink.write(line(2)));

    let lines = emitted(&path, 2).await;
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["run_id"].as_str(), Some(output.run_id.as_ref()));
        assert_eq!(line["sequence"].as_u64(), Some(index as u64 + 1));
    }
}

#[tokio::test]
async fn tap_sequences_follow_file_order_with_a_reliable_producer_waiting_on_a_full_queue() {
    // It counts emissions after dequeue, sharing one sequence across producers and skipping refused inputs.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tap.jsonl");
    let output = TapOutput::open(&path, Logger::default()).await.unwrap();
    let mut accepted = 0;
    while output.sink.write(line(1)) {
        accepted += 1;
    }
    assert!(accepted > 0);
    assert_eq!(output.sink.dropped(), 1);

    let second = output.sink.clone();
    let mut waiting = std::pin::pin!(second.write_reliable(line(2)));
    let pending =
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(waiting.as_mut(), cx).is_pending()))
            .await;
    assert!(pending);

    let mut producers = Vec::new();
    for exchange in 3..11 {
        let sink = output.sink.clone();
        producers.push(tokio::spawn(async move {
            assert!(sink.write_reliable(line(exchange)).await);
        }));
    }
    assert!(waiting.await);
    for producer in producers {
        producer.await.unwrap();
    }
    assert!(output.sink.write_reliable(line(100)).await);

    let lines = emitted(&path, accepted + 10).await;
    assert!(lines.iter().position(|line| line["exchange"] == 2).unwrap() >= accepted);
    for exchange in 2..11 {
        assert_eq!(lines.iter().filter(|line| line["exchange"] == exchange).count(), 1);
    }
    assert_eq!(lines.last().unwrap()["exchange"], 100);
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["sequence"].as_u64(), Some(index as u64 + 1));
        assert_eq!(line["run_id"].as_str(), Some(output.run_id.as_ref()));
    }
    assert_eq!(output.sink.dropped(), 1);
}
