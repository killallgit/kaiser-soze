use crate::graph::Edge;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use serde_json::Value;

/// What one tap line says. `Untyped` holds every kind that has no type yet: its event name and its payload as the producer built them.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Untyped { kind: String, payload: Value },
    Edge(Edge),
}

impl Entry {
    pub fn untyped(kind: &str, payload: Value) -> Self {
        Self::Untyped {
            kind: kind.to_string(),
            payload,
        }
    }

    /// The name the line carries under its `event` key.
    pub fn event(&self) -> &str {
        match self {
            Self::Untyped { kind, .. } => kind,
            Self::Edge(_) => "edge",
        }
    }
}

/// Serialises as the line's payload alone; the event name is the line's own key.
impl Serialize for Entry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Untyped { payload, .. } => payload.serialize(serializer),
            Self::Edge(edge) => edge.serialize(serializer),
        }
    }
}

/// One line of the tap file: where it came from, when, which bytes of the exchange it covers, and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub exchange: u64,
    pub adapter: &'static str,
    pub mono_us: u64,
    pub wall_ms: u64,
    pub start: u64,
    pub end: u64,
    pub payload: Entry,
}

/// The tap file has its keys sorted, so the fields are written in key order.
impl Serialize for Line {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut line = serializer.serialize_struct("Line", 8)?;
        line.serialize_field("adapter", self.adapter)?;
        line.serialize_field("end", &self.end)?;
        line.serialize_field("event", self.payload.event())?;
        line.serialize_field("exchange", &self.exchange)?;
        line.serialize_field("mono_us", &self.mono_us)?;
        line.serialize_field("payload", &self.payload)?;
        line.serialize_field("start", &self.start)?;
        line.serialize_field("wall_ms", &self.wall_ms)?;
        line.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeKind;
    use serde_json::json;

    fn line(payload: Entry) -> Line {
        Line {
            exchange: 7,
            adapter: "claude",
            mono_us: 1500,
            wall_ms: 1_700_000_000_000,
            start: 4,
            end: 9,
            payload,
        }
    }

    fn written_before_typing(line: &Line) -> String {
        json!({
            "exchange": line.exchange,
            "adapter": line.adapter,
            "event": line.payload.event(),
            "mono_us": line.mono_us,
            "wall_ms": line.wall_ms,
            "start": line.start,
            "end": line.end,
            "payload": serde_json::to_value(&line.payload).unwrap(),
        })
        .to_string()
    }

    #[test]
    fn an_untyped_line_serialises_its_keys_in_sorted_order() {
        // It writes the event name from the entry and the payload as the producer built it
        let line = line(Entry::untyped("usage", json!({"output": 3, "input": null})));

        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"usage","exchange":7,"mono_us":1500,"payload":{"input":null,"output":3},"start":4,"wall_ms":1700000000000}"#
        );
    }

    #[test]
    fn an_edge_line_is_an_edge_event_with_the_edge_as_its_payload() {
        // It names the event edge and nests the sorted edge
        let line = line(Entry::Edge(Edge::new(EdgeKind::Turn, "step:7.1", "turn:u1")));

        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"edge","exchange":7,"mono_us":1500,"payload":{"from":"step:7.1","kind":"turn","to":"turn:u1"},"start":4,"wall_ms":1700000000000}"#
        );
    }

    #[test]
    fn a_line_serialises_to_the_bytes_the_untyped_json_line_had() {
        // It matches the object the recorder used to build, for an untyped entry and an edge
        let untyped = line(Entry::untyped(
            "record",
            json!({"b": [1, "é"], "a": {"d": true, "c": null}}),
        ));
        let edge = line(Entry::Edge(Edge {
            branch: Some("main".into()),
            ..Edge::new(EdgeKind::InProject, "session:s1", "project:p")
        }));

        for line in [untyped, edge] {
            assert_eq!(serde_json::to_string(&line).unwrap(), written_before_typing(&line));
        }
    }
}
