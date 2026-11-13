use super::place::Place;
use super::step::{claimed_step, Step};
use super::{Identity, Thread, Threads};
use crate::common::Edge;
use crate::event::Event;
use crate::record::Record;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long a record waits for the earlier exchanges of its thread before it goes ahead regardless.
const TURN_WAIT: Duration = Duration::from_secs(2);

/// What one proxy shares: exchange admission order, thread history and each exchange's place in line.
#[derive(Clone, Default)]
pub(crate) struct Tracker(Arc<Mutex<Threads>>);

/// Gives one exchange's live lines and records their step and turn, and its records their step edges.
pub(crate) struct Steps {
    tracker: Tracker,
    place: Option<Place>,
    thread: Option<Thread>,
    header_turn: Option<String>,
    exchange: u64,
    open: Option<u64>,
}

impl Tracker {
    /// Assigns an id and a place in line together, after the adapter has supplied its identity.
    pub(crate) fn admit(&self, adapter: &'static str, identity: &Identity) -> Steps {
        let thread = identity.thread.as_deref().map(|id| Thread::new(adapter, id));
        let (exchange, place) = {
            let mut threads = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            let exchange = threads.next_exchange();
            let place = thread.as_ref().map(|thread| threads.join(thread, exchange));
            (exchange, place)
        };
        Steps {
            tracker: self.clone(),
            place,
            thread,
            header_turn: identity.turn.clone(),
            exchange,
            open: None,
        }
    }

    /// Names a standalone note without reserving a place in any thread's record queue.
    pub(crate) fn note_exchange(&self) -> u64 {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).next_exchange()
    }
}

impl Steps {
    pub(crate) fn exchange_id(&self) -> u64 {
        self.exchange
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
        let Some(step) = Step::of_record(record, self.thread.as_ref(), self.header_turn.as_deref(), self.exchange)
        else {
            return Vec::new();
        };
        record.step = Some(step.n);
        record.turn = step.turn.clone();
        self.tracker
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .edges(&step)
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
    use crate::common::EdgeKind;
    use crate::event::{Conversation, ReplyEnd};
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
        let mut steps = Tracker::default().admit("test", &Identity::default());
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
        let mut steps = Tracker::default().admit("test", &Identity::default());

        assert_eq!(steps.of_line(&Event::Conversation(request(None, None))), Some(1));
    }

    #[tokio::test]
    async fn a_request_that_states_its_step_and_turn_gives_them_to_the_record() {
        // It copies the step and turn of the conversation onto the record
        let steps = Tracker::default().admit("test", &header_turn("h1"));
        let mut record = record_of(Some(request(Some(3), Some("u2"))));

        steps.link(&mut record).await;

        assert_eq!((record.step, record.turn.as_deref()), (Some(3), Some("u2")));
    }

    #[tokio::test]
    async fn a_request_that_states_no_step_is_step_one_of_the_header_turn() {
        // A Claude exchange is one request, so the record is step 1 in the turn its headers name, or none
        let named = Tracker::default().admit("test", &header_turn("h1"));
        let unnamed = Tracker::default().admit("test", &Identity::default());
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
        let steps = Tracker::default().admit(
            "test",
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
        let first = tracker.admit("test", &on_thread("t1"));
        let second = tracker.admit("test", &on_thread("t1"));

        first.link(&mut record_of(Some(request(None, None)))).await;
        first.leave();
        let edges = second.link(&mut record_of(Some(request(None, None)))).await;

        assert_eq!(edges, [Edge::new(EdgeKind::Continues, "step:2.1", "step:1.1")]);
    }

    #[tokio::test]
    async fn the_thread_bound_is_shared_across_adapters() {
        // Activity across both adapters evicts the oldest thread under one global bound.
        let tracker = Tracker::default();
        let first = tracker.admit("claude", &on_thread("first"));
        first.link(&mut record_of(Some(request(None, None)))).await;
        first.leave();

        for exchange in 2..=super::super::THREAD_BOUND as u64 + 1 {
            let adapter = if exchange % 2 == 0 { "claude" } else { "codex" };
            let identity = on_thread(&format!("t{}", exchange / 2));
            let steps = tracker.admit(adapter, &identity);
            steps.link(&mut record_of(Some(request(None, None)))).await;
            steps.leave();
        }
        let revisited = tracker.admit("claude", &on_thread("first"));
        let edges = revisited.link(&mut record_of(Some(request(None, None)))).await;

        assert!(edges.is_empty());
    }
}
