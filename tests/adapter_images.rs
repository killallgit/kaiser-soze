use bytes::Bytes;
use kaiser_soze::adapter::claude::ClaudeAdapter;
use kaiser_soze::adapter::codex::CodexAdapter;
use kaiser_soze::adapter::{Adapter, RequestHead, TapInput};
use kaiser_soze::common::event::Event;
use kaiser_soze::common::message::Block;
use serde_json::json;

#[test]
fn both_adapters_keep_image_sizes_and_request_offsets() {
    let claude = ClaudeAdapter::default();
    let codex = CodexAdapter::default();
    let adapters: [(&dyn Adapter, &str, &str); 2] = [
        (&claude, "api.anthropic.com", "/v1/messages"),
        (&codex, "chatgpt.com", "/backend-api/codex/responses"),
    ];
    for (adapter, host, path) in adapters {
        for (data, size) in [
            ("", 0),
            ("AA==", 1),
            ("AAA=", 2),
            ("AAAA", 3),
            ("QUJDREVGRw==", 7),
            ("=", 0),
            ("AAA", 0),
        ] {
            let image = if adapter.name() == "claude" {
                json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}})
            } else {
                json!({"type": "input_image", "image_url": format!("data:image/png;base64,{data}")})
            };
            let message = json!({"role": "user", "content": [image]});
            let body = if adapter.name() == "claude" {
                json!({"messages": [message]})
            } else {
                json!({"input": [message]})
            }
            .to_string();
            let head = RequestHead {
                method: "POST".into(),
                host: host.into(),
                path: path.into(),
                headers: Vec::new(),
            };
            let mut parser = adapter.claim(&head).unwrap();
            let end = body.len() as u64;
            for chunk in body.as_bytes().chunks(7) {
                assert!(parser
                    .feed(TapInput::RequestBody(Bytes::copy_from_slice(chunk)))
                    .is_empty());
            }
            let events = parser.feed(TapInput::RequestEnd);
            assert_eq!(events.len(), 1);
            assert_eq!((events[0].start, events[0].end), (0, end));
            let Event::Conversation(conversation) = &events[0].event else {
                panic!();
            };
            assert_eq!(
                conversation.messages[0].blocks,
                vec![Block::Image {
                    content_type: "image/png".into(),
                    reference: "0:0".into(),
                    size,
                }]
            );
        }
    }
}
