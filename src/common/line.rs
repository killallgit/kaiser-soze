use super::event::Event;
use super::note::Note;
use super::record::Record;
use crate::graph::Edge;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use std::sync::Arc;

/// What one tap line says: a stream event, a note of the proxy, a graph edge or a finished record.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Event(Event),
    Note(Note),
    Edge(Edge),
    Record(Box<Record>),
}

impl Entry {
    /// The name the line carries under its `event` key.
    pub fn event(&self) -> &'static str {
        match self {
            Self::Event(event) => event.kind(),
            Self::Note(note) => note.event(),
            Self::Edge(_) => "edge",
            Self::Record(_) => "record",
        }
    }
}

/// Serialises as the line's payload alone, without its event name: that is the line's own key, so serialise a `Line`, not a bare `Entry`.
impl Serialize for Entry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Event(event) => event.serialize(serializer),
            Self::Note(note) => note.serialize(serializer),
            Self::Edge(edge) => edge.serialize(serializer),
            Self::Record(record) => record.serialize(serializer),
        }
    }
}

/// One line of the tap file: where it came from, when, which bytes of the exchange it covers, and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub run_id: Arc<str>,
    /// Assigned after dequeue by the tap writer; zero before emission.
    pub sequence: u64,
    pub exchange: u64,
    pub adapter: &'static str,
    pub mono_us: u64,
    pub wall_ms: u64,
    pub start: u64,
    pub end: u64,
    pub entry: Entry,
}

/// The tap file has its keys sorted, so the fields are written in key order.
impl Serialize for Line {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut line = serializer.serialize_struct("Line", 10)?;
        line.serialize_field("adapter", self.adapter)?;
        line.serialize_field("end", &self.end)?;
        line.serialize_field("event", self.entry.event())?;
        line.serialize_field("exchange", &self.exchange)?;
        line.serialize_field("mono_us", &self.mono_us)?;
        line.serialize_field("payload", &self.entry)?;
        line.serialize_field("run_id", self.run_id.as_ref())?;
        line.serialize_field("sequence", &self.sequence)?;
        line.serialize_field("start", &self.start)?;
        line.serialize_field("wall_ms", &self.wall_ms)?;
        line.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{BlockDelta, Delta, Usage};
    use crate::graph::EdgeKind;
    use crate::note::Tunnel;
    use serde_json::json;

    fn line(entry: Entry) -> Line {
        Line {
            run_id: "run-synthetic".into(),
            sequence: 5,
            exchange: 7,
            adapter: "claude",
            mono_us: 1500,
            wall_ms: 1_700_000_000_000,
            start: 4,
            end: 9,
            entry,
        }
    }

    #[test]
    fn an_event_line_names_its_kind_and_nests_the_event_in_key_order() {
        // It writes the event name from the event, and a delta whose key sorts before the index in that order
        let usage = Usage {
            input: Some(3),
            ..Usage::default()
        };
        let delta = BlockDelta {
            index: 1,
            delta: Delta::Encrypted("e".into()),
        };

        assert_eq!(
            serde_json::to_string(&line(Entry::Event(Event::Usage(usage)))).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"usage","exchange":7,"mono_us":1500,"payload":{"cache_read":null,"cache_write":null,"input":3,"output":null,"reasoning":null},"run_id":"run-synthetic","sequence":5,"start":4,"wall_ms":1700000000000}"#
        );
        assert_eq!(
            serde_json::to_string(&line(Entry::Event(Event::BlockDelta(delta)))).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"block_delta","exchange":7,"mono_us":1500,"payload":{"encrypted":"e","index":1},"run_id":"run-synthetic","sequence":5,"start":4,"wall_ms":1700000000000}"#
        );
    }

    #[test]
    fn a_note_line_names_its_event_and_nests_the_note() {
        // It writes the note's event name and, with no label, leaves the label key out
        let tunnel = Note::Tunnel(Tunnel {
            host: "t.test".into(),
            label: None,
            port: 443,
        });

        assert_eq!(
            serde_json::to_string(&line(Entry::Note(tunnel))).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"tunnel","exchange":7,"mono_us":1500,"payload":{"host":"t.test","port":443},"run_id":"run-synthetic","sequence":5,"start":4,"wall_ms":1700000000000}"#
        );
    }

    #[test]
    fn an_edge_line_is_an_edge_event_with_the_edge_as_its_payload() {
        // It names the event edge and nests the sorted edge
        let line = line(Entry::Edge(Edge::new(EdgeKind::Turn, "step:7.1", "turn:u1")));

        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"edge","exchange":7,"mono_us":1500,"payload":{"from":"step:7.1","kind":"turn","to":"turn:u1"},"run_id":"run-synthetic","sequence":5,"start":4,"wall_ms":1700000000000}"#
        );
    }

    #[test]
    fn a_record_line_is_a_record_event_with_the_sorted_record_as_its_payload() {
        // It names the event record and nests the record in key order
        let line = line(Entry::Record(Box::new(Record::empty())));

        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"adapter":"claude","end":9,"event":"record","exchange":7,"mono_us":1500,"payload":{"conversation":null,"dropped":0,"incomplete":false,"reply":{"message":{"blocks":[],"role":"assistant"},"stop_reason":null,"usage":{"cache_read":null,"cache_write":null,"input":null,"output":null,"reasoning":null}}},"run_id":"run-synthetic","sequence":5,"start":4,"wall_ms":1700000000000}"#
        );
    }

    #[test]
    fn a_value_writes_its_keys_sorted() {
        // The tap's key order for Value payloads rests on serde_json keeping its map sorted, so preserve_order must stay off
        let value = json!({"b": 1, "a": {"d": 2, "c": 3}});

        assert_eq!(value.to_string(), r#"{"a":{"c":3,"d":2},"b":1}"#);
    }
}
