//! Shared constructors for events emitted by adapter parsers.

use super::TapEvent;
use crate::common::event::{
    BlockDelta, BlockStart, BlockStop, Conversation, Delta, Error, Event, ReplyEnd, Side, Unparsed, Usage,
};
use crate::common::message::Message;
use serde_json::Value;

pub fn conversation(system: Value, tools: Value, messages: Vec<Message>) -> TapEvent {
    TapEvent::from(Event::Conversation(Conversation::new(system, tools, messages)))
}

pub fn block_start(start: BlockStart) -> TapEvent {
    TapEvent::from(Event::BlockStart(start))
}

pub fn block_delta(index: u64, delta: Delta) -> TapEvent {
    TapEvent::from(Event::BlockDelta(BlockDelta { index, delta }))
}

pub fn block_stop(index: u64) -> TapEvent {
    TapEvent::from(Event::BlockStop(BlockStop { index }))
}

pub fn usage(usage: Usage) -> TapEvent {
    TapEvent::from(Event::Usage(usage))
}

pub fn reply_end_of_message(stop_reason: Value, message_id: Value) -> TapEvent {
    TapEvent::from(Event::ReplyEnd(ReplyEnd {
        message_id: Some(message_id).filter(|id| !id.is_null()),
        stop_reason,
    }))
}

pub fn error(error_type: Value) -> TapEvent {
    TapEvent::from(Event::Error(Error { error_type }))
}

pub fn unparsed(side: Side) -> TapEvent {
    TapEvent::from(Event::Unparsed(Unparsed { side }))
}
