use super::*;

fn written(note: &Note) -> String {
    serde_json::to_string(note).unwrap()
}

fn label() -> Label {
    Label {
        command: "claude".into(),
        dir: "/repo".into(),
    }
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn start(label: Option<Label>) -> Note {
    Note::ExchangeStart(ExchangeStart {
        headers: pairs(&[("a", "1"), ("b", "2")]),
        host: "h.test".into(),
        http_version: "HTTP/1.1".into(),
        label,
        method: "POST".into(),
        path: "/v1/messages".into(),
        upgrade: false,
    })
}

#[test]
fn a_label_serialises_its_command_then_its_directory() {
    let found = label();

    assert_eq!(
        serde_json::to_string(&found).unwrap(),
        r#"{"command":"claude","dir":"/repo"}"#
    );
}

#[test]
fn an_exchange_start_writes_its_keys_sorted_and_its_headers_as_pairs() {
    // It nests the label between http_version and method
    assert_eq!(
        written(&start(Some(label()))),
        r#"{"headers":[["a","1"],["b","2"]],"host":"h.test","http_version":"HTTP/1.1","label":{"command":"claude","dir":"/repo"},"method":"POST","path":"/v1/messages","upgrade":false}"#
    );
}

#[test]
fn an_exchange_start_without_a_label_leaves_the_key_out() {
    assert_eq!(
        written(&start(None)),
        r#"{"headers":[["a","1"],["b","2"]],"host":"h.test","http_version":"HTTP/1.1","method":"POST","path":"/v1/messages","upgrade":false}"#
    );
}

#[test]
fn a_response_head_names_each_transport_in_lower_case() {
    // It writes http, sse and websocket in lower case
    let head = |transport| {
        Note::ResponseHead(ResponseHead {
            headers: pairs(&[("content-type", "text/plain")]),
            http_version: "HTTP/1.1".into(),
            status: 200,
            transport,
        })
    };

    assert_eq!(
        [Transport::Http, Transport::Sse, Transport::Websocket].map(|transport| written(&head(transport))),
        [
            r#"{"headers":[["content-type","text/plain"]],"http_version":"HTTP/1.1","status":200,"transport":"http"}"#,
            r#"{"headers":[["content-type","text/plain"]],"http_version":"HTTP/1.1","status":200,"transport":"sse"}"#,
            r#"{"headers":[["content-type","text/plain"]],"http_version":"HTTP/1.1","status":200,"transport":"websocket"}"#,
        ]
    );
}

#[test]
fn an_exchange_end_writes_all_eight_stats_and_a_moment_that_never_came_is_null() {
    // It writes every stat, with null for a moment that never came
    let end = Note::ExchangeEnd(ExchangeEnd {
        bytes: 5,
        chunks: 2,
        first_byte_us: None,
        forward_max_us: 4,
        forward_sum_us: 6,
        response_head_us: Some(9),
        sink_dropped: 1,
        tap_dropped: 3,
    });

    assert_eq!(
        written(&end),
        r#"{"bytes":5,"chunks":2,"first_byte_us":null,"forward_max_us":4,"forward_sum_us":6,"response_head_us":9,"sink_dropped":1,"tap_dropped":3}"#
    );
}

#[test]
fn a_tunnel_writes_its_label_between_the_host_and_the_port_when_it_has_one() {
    // It sorts the label between the host and the port, and leaves it out when there is none
    let tunnel = |label| {
        Note::Tunnel(Tunnel {
            host: "t.test".into(),
            label,
            port: 443,
        })
    };

    assert_eq!(
        written(&tunnel(Some(label()))),
        r#"{"host":"t.test","label":{"command":"claude","dir":"/repo"},"port":443}"#
    );
    assert_eq!(written(&tunnel(None)), r#"{"host":"t.test","port":443}"#);
}

#[test]
fn an_unsupported_writes_its_label_between_the_host_and_the_path_when_it_has_one() {
    // It sorts the label between the host and the path, and leaves it out when there is none
    let unsupported = |label| {
        Note::Unsupported(Unsupported {
            host: "u.test".into(),
            label,
            path: "/v1/other".into(),
        })
    };

    assert_eq!(
        written(&unsupported(Some(label()))),
        r#"{"host":"u.test","label":{"command":"claude","dir":"/repo"},"path":"/v1/other"}"#
    );
    assert_eq!(written(&unsupported(None)), r#"{"host":"u.test","path":"/v1/other"}"#);
}

#[test]
fn each_note_names_its_event() {
    // It names the event each note is written under
    let notes = [
        start(None),
        Note::ResponseHead(ResponseHead {
            headers: Vec::new(),
            http_version: "HTTP/1.1".into(),
            status: 200,
            transport: Transport::Http,
        }),
        Note::ExchangeEnd(ExchangeEnd {
            bytes: 0,
            chunks: 0,
            first_byte_us: None,
            forward_max_us: 0,
            forward_sum_us: 0,
            response_head_us: None,
            sink_dropped: 0,
            tap_dropped: 0,
        }),
        Note::Tunnel(Tunnel {
            host: "t.test".into(),
            label: None,
            port: 443,
        }),
        Note::Unsupported(Unsupported {
            host: "u.test".into(),
            label: None,
            path: "/".into(),
        }),
    ];

    assert_eq!(
        notes.map(|note| note.event()),
        [
            "exchange_start",
            "response_head",
            "exchange_end",
            "tunnel",
            "unsupported"
        ]
    );
}
