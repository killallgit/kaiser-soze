use super::*;
use bytes::Bytes;

struct Silent;

impl ExchangeParser for Silent {
    fn feed(&mut self, _input: TapInput) -> Vec<TapEvent> {
        Vec::new()
    }
}

fn head() -> RequestHead {
    RequestHead {
        method: "POST".into(),
        host: "api.example.com".into(),
        path: "/v1/messages?beta=true".into(),
        headers: vec![("X-Version".into(), "secret-value".into())],
    }
}

fn parser() -> (ShapedParser, tokio::sync::mpsc::Receiver<Value>) {
    let (sink, rx) = Sink::detached(64);
    (ShapedParser::new(ShapeLog::new(sink), Box::new(Silent), 7, &head()), rx)
}

fn lines(mut rx: tokio::sync::mpsc::Receiver<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(line) = rx.try_recv() {
        out.push(line);
    }
    out
}

#[test]
fn keeps_keys_and_types_and_drops_values() {
    let body = json!({"model": "m", "n": 3, "ok": true, "none": null, "items": [{"a": "x"}, {"a": "y"}]});

    let got = shape(&body);

    assert_eq!(
        got,
        json!({"model": "string", "n": "number", "ok": "boolean", "none": "null", "items": [{"a": "string"}]})
    );
}

#[test]
fn keeps_schema_tags_and_distinct_variants_of_an_array() {
    let body =
        json!({"content": [{"type": "text", "text": "hi"}, {"type": "tool_use", "input": {"k": 1}}], "role": "user"});

    let got = shape(&body);

    assert_eq!(
        got,
        json!({
            "role": "string=user",
            "content": [{"type": "string=text", "text": "string"}, {"type": "string=tool_use", "input": "opaque"}]
        })
    );
}

#[test]
fn collapses_user_defined_schemas_but_keeps_an_input_array() {
    let body = json!({"tools": [{"input_schema": {"properties": {"p": {}}}}], "input": [{"role": "user"}]});

    let got = shape(&body);

    assert_eq!(
        got,
        json!({"tools": [{"input_schema": "opaque"}], "input": [{"role": "string=user"}]})
    );
}

#[test]
fn stops_at_a_depth_limit() {
    let mut body = json!("leaf");
    for _ in 0..30 {
        body = json!({"a": body});
    }

    assert!(shape(&body).to_string().contains("deep"));
}

#[test]
fn logs_header_names_without_values_and_the_query_free_path() {
    let (_parser, rx) = parser();

    let got = lines(rx);

    assert_eq!(got[0]["headers"], json!(["x-version"]));
    assert_eq!(got[0]["path"], json!("/v1/messages"));
    assert!(!got[0].to_string().contains("secret-value"));
}

#[test]
fn logs_a_request_body_shape_once_however_often_it_repeats() {
    let (mut parser, rx) = parser();

    for _ in 0..3 {
        parser.feed(TapInput::RequestBody(Bytes::from_static(
            br#"{"model":"m","stream":true}"#,
        )));
        parser.feed(TapInput::RequestEnd);
        parser.request = RequestBuffer::new(false);
    }

    let requests: Vec<Value> = lines(rx).into_iter().filter(|l| l["direction"] == "request").collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["shape"], json!({"model": "string", "stream": "boolean"}));
    assert_eq!(requests[0]["exchange"], json!(7));
}

#[test]
fn logs_each_server_sent_event_with_its_type() {
    let (mut parser, rx) = parser();
    parser.feed(TapInput::ResponseHead(ResponseHead {
        status: 200,
        headers: vec![("content-type".into(), "text/event-stream".into())],
    }));

    parser.feed(TapInput::ResponseBody(Bytes::from_static(
        b"event: x\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"i\"}}\n\ndata: [DONE]\n\n",
    )));
    parser.feed(TapInput::ResponseEnd);

    let frames: Vec<Value> = lines(rx)
        .into_iter()
        .filter(|l| l["transport"] == "sse" && l["direction"] == "response")
        .collect();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["kind"], json!("message_start"));
    assert_eq!(frames[0]["shape"]["message"], json!({"id": "string"}));
}

#[test]
fn logs_websocket_messages_by_direction() {
    let (mut parser, rx) = parser();
    let message = |from_client, text: &str| TapInput::WsMessage {
        from_client,
        text: text.into(),
        start: 0,
        end: 0,
    };

    parser.feed(message(true, r#"{"type":"response.create","input":[]}"#));
    parser.feed(message(false, r#"{"type":"response.completed"}"#));
    parser.feed(message(false, "not json"));

    let got: Vec<(String, String)> = lines(rx)
        .into_iter()
        .filter(|l| l["transport"] == "websocket")
        .map(|l| {
            (
                l["direction"].as_str().unwrap().into(),
                l["kind"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("request".to_string(), "response.create".to_string()),
            ("response".to_string(), "response.completed".to_string())
        ]
    );
}

#[test]
fn hands_every_input_on_to_the_real_parser() {
    struct Counts(Arc<Mutex<usize>>);
    impl ExchangeParser for Counts {
        fn feed(&mut self, _input: TapInput) -> Vec<TapEvent> {
            *self.0.lock().unwrap() += 1;
            Vec::new()
        }
    }
    let count = Arc::new(Mutex::new(0));
    let (sink, _rx) = Sink::detached(64);
    let mut parser = ShapedParser::new(ShapeLog::new(sink), Box::new(Counts(count.clone())), 1, &head());

    parser.feed(TapInput::RequestBody(Bytes::from_static(b"not json")));
    parser.feed(TapInput::RequestEnd);
    parser.feed(TapInput::ResponseEnd);

    assert_eq!(*count.lock().unwrap(), 3);
}
