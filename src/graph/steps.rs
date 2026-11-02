use super::place::Place;
use super::step::{claimed_step, Step};
use super::{Edge, Identity, Threads};
use crate::event::Event;
use crate::record::Record;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long a record waits for the earlier exchanges of its thread before it goes ahead regardless.
const TURN_WAIT: Duration = Duration::from_secs(2);

/// What the exchanges of one proxy share: the threads seen so far and each exchange's place in line.
#[derive(Clone, Default)]
pub(crate) struct Tracker(Arc<Mutex<Threads>>);

/// Gives one exchange's live lines and records their step and turn, and its records their step edges.
pub(crate) struct Steps {
    tracker: Option<Tracker>,
    place: Option<Place>,
    identity: Identity,
    exchange: u64,
    open: Option<u64>,
}

impl Steps {
    /// Puts the exchange in line among the exchanges of its thread, so its records wait for earlier ones.
    pub(crate) fn join(tracker: Option<&Tracker>, exchange: u64, identity: &Identity) -> Self {
        let place = tracker.zip(identity.thread.as_deref()).map(|(tracker, thread)| {
            tracker
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .join(thread, exchange)
        });
        Self {
            tracker: tracker.cloned(),
            place,
            identity: identity.clone(),
            exchange,
            open: None,
        }
    }

    /// The step of the record a live line is folded into, taken before the line is folded.
    pub(crate) fn of_line(&mut self, event: &Event) -> Option<u64> {
        match event {
            Event::Conversation(conversation) => {
                self.open = Some(claimed_step(conversation));
                self.open
            }
            Event::ReplyEnd(_) => self.open.take(),
            _ => self.open,
        }
    }

    /// Waits until the earlier exchanges of the thread have written their first record, stamps the
    /// record's step and turn, and returns the edges of its step.
    pub(crate) async fn link(&self, record: &mut Record) -> Vec<Edge> {
        if let Some(place) = &self.place {
            place.wait_for_turn(TURN_WAIT).await;
        }
        let Some(step) = Step::of_record(record, &self.identity, self.exchange) else {
            return Vec::new();
        };
        record.step = Some(step.n);
        record.turn = step.turn.clone();
        let Some(tracker) = &self.tracker else {
            return Vec::new();
        };
        tracker.0.lock().unwrap_or_else(PoisonError::into_inner).edges(&step)
    }

    /// Lets the later exchanges of the thread write their records.
    pub(crate) fn leave(&self) {
        if let Some(place) = &self.place {
            place.leave();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Conversation, ReplyEnd};
    use crate::graph::EdgeKind;
    use serde_json::Value;

    fn request(step: Option<u64>, turn: Option<&str>) -> Conversation {
        Conversation {
            step,
            turn: turn.map(Into::into),
            ..Conversation::new(Value::Null, Value::Null, Vec::new())
        }
    }

    fn record_of(conversation: Option<Conversation>) -> Record {
        Record {
            conversation,
            ..Record::empty()
        }
    }

    fn reply_end() -> Event {
        Event::ReplyEnd(ReplyEnd {
            message_id: None,
            stop_reason: Value::Null,
        })
    }

    fn header_turn(turn: &str) -> Identity {
        Identity {
            turn: Some(turn.into()),
            ..Identity::default()
        }
    }

    fn on_thread(thread: &str) -> Identity {
        Identity {
            thread: Some(thread.into()),
            ..Identity::default()
        }
    }

    #[test]
    fn a_live_line_carries_the_step_of_the_request_it_is_folded_into() {
        // The request line and every line up to its reply end show its step; a line with no request open shows none
        let mut steps = Steps::join(None, 7, &Identity::default());
        let usage = Event::Usage(Default::default());

        let before = steps.of_line(&usage);
        let opened = steps.of_line(&Event::Conversation(request(Some(3), None)));
        let during = steps.of_line(&usage);
        let ended = steps.of_line(&reply_end());
        let after = steps.of_line(&usage);

        assert_eq!(
            [before, opened, during, ended, after],
            [None, Some(3), Some(3), Some(3), None]
        );
    }

    #[test]
    fn a_request_that_states_no_step_opens_step_one() {
        // A Claude exchange is one request, so its lines show step 1
        let mut steps = Steps::join(None, 7, &Identity::default());

        assert_eq!(steps.of_line(&Event::Conversation(request(None, None))), Some(1));
    }

    #[tokio::test]
    async fn a_request_that_states_its_step_and_turn_gives_them_to_the_record() {
        // It copies the step and turn of the conversation onto the record
        let steps = Steps::join(None, 7, &header_turn("h1"));
        let mut record = record_of(Some(request(Some(3), Some("u2"))));

        steps.link(&mut record).await;

        assert_eq!((record.step, record.turn.as_deref()), (Some(3), Some("u2")));
    }

    #[tokio::test]
    async fn a_request_that_states_no_step_is_step_one_of_the_header_turn() {
        // A Claude exchange is one request, so the record is step 1 in the turn its headers name, or none
        let named = Steps::join(None, 7, &header_turn("h1"));
        let unnamed = Steps::join(None, 7, &Identity::default());
        let mut in_turn = record_of(Some(request(None, None)));
        let mut no_turn = record_of(Some(request(None, None)));

        named.link(&mut in_turn).await;
        unnamed.link(&mut no_turn).await;

        assert_eq!((in_turn.step, in_turn.turn.as_deref()), (Some(1), Some("h1")));
        assert_eq!((no_turn.step, no_turn.turn), (Some(1), None));
    }

    #[tokio::test]
    async fn a_record_without_a_request_names_no_step_and_no_turn() {
        // An unparsed request leaves no step, so it cannot collide with the real first one, and links nothing
        let steps = Steps::join(
            Some(&Tracker::default()),
            7,
            &Identity {
                thread: Some("t1".into()),
                ..header_turn("h1")
            },
        );
        let mut record = record_of(None);

        let edges = steps.link(&mut record).await;

        assert_eq!((record.step, record.turn), (None, None));
        assert!(edges.is_empty());
    }

    #[tokio::test]
    async fn a_later_exchange_of_a_thread_continues_the_step_of_an_earlier_one() {
        // Exchanges that share a tracker link their steps across exchanges
        let tracker = Tracker::default();
        let first = Steps::join(Some(&tracker), 1, &on_thread("t1"));
        let second = Steps::join(Some(&tracker), 2, &on_thread("t1"));

        first.link(&mut record_of(Some(request(None, None)))).await;
        first.leave();
        let edges = second.link(&mut record_of(Some(request(None, None)))).await;

        assert_eq!(edges, [Edge::new(EdgeKind::Continues, "step:2.1", "step:1.1")]);
    }

    #[tokio::test]
    async fn without_a_tracker_a_step_links_nothing() {
        // An exchange recorded alone has no thread history to link to
        let steps = Steps::join(
            None,
            7,
            &Identity {
                thread: Some("t1".into()),
                ..header_turn("h1")
            },
        );

        assert!(steps.link(&mut record_of(Some(request(None, None)))).await.is_empty());
    }
}
