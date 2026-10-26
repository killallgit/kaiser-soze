use super::request::{input_item, read_request};
use crate::adapter::TapEvent;
use crate::body::MAX_REQUEST_BODY;
use crate::common::conversation;
use crate::message::Message;
use serde_json::{json, Value};

/// The running conversation of one socket. Codex sends only the new items with
/// `previous_response_id`, so the parser joins them to what it stored. A request it cannot
/// chain (joined mid-socket, dropped frame, past the bound) is emitted as sent with
/// `partial: true`, and nothing chains from it until a request starts a new history.
#[derive(Default)]
pub(super) struct History {
    last_reply: Option<String>,
    system: Value,
    tools: Value,
    messages: Vec<Message>,
    bytes: usize,
    broken: bool,
    reply_id: Option<String>,
    reply_items: Vec<Message>,
}

impl History {
    pub(super) fn join(&mut self, json: &Value) -> TapEvent {
        let (system, tools, messages) = read_request(json);
        let previous = json["previous_response_id"].as_str();
        let partial = previous.is_some() && (self.broken || previous != self.last_reply.as_deref());
        let mut event = if partial {
            self.broken = true;
            conversation(system, tools, messages)
        } else {
            if previous.is_none() {
                *self = History::default();
            }
            let system = if system.is_null() { self.system.clone() } else { system };
            let tools = if tools.is_null() { self.tools.clone() } else { tools };
            self.bytes += size_of(&messages);
            append(&mut self.messages, messages);
            (self.system, self.tools) = (system.clone(), tools.clone());
            let event = conversation(system, tools, self.messages.clone());
            self.bound();
            event
        };
        if partial {
            event.payload["partial"] = json!(true);
        }
        if json["generate"] == json!(false) {
            event.payload["warmup"] = json!(true);
        }
        event
    }

    pub(super) fn request_unreadable(&mut self) {
        self.broken = true;
    }

    pub(super) fn watch(&mut self, data: &str) {
        let Ok(json) = serde_json::from_str::<Value>(data) else {
            return;
        };
        match json["type"].as_str().unwrap_or_default() {
            "response.created" => {
                self.reply_id = json["response"]["id"].as_str().map(String::from);
                self.reply_items.clear();
            }
            "response.output_item.done" => {
                let index = json["output_index"].as_u64().unwrap_or_default() as usize;
                self.reply_items.extend(input_item(index, &json["item"]));
            }
            "response.completed" | "response.incomplete" => self.commit(&json["response"]["output"]),
            _ => {}
        }
    }

    fn commit(&mut self, output: &Value) {
        let items = std::mem::take(&mut self.reply_items);
        let id = self.reply_id.take();
        let output_len = output.as_array().map_or(0, Vec::len);
        // shortcut: a dropped output_item.done frame is caught only when the reply lists its output
        if self.broken || (output_len > 0 && output_len != items.len()) {
            self.broken = true;
            return;
        }
        self.bytes += size_of(&items);
        append(&mut self.messages, items);
        self.last_reply = id;
        self.bound();
    }

    fn bound(&mut self) {
        if self.bytes > MAX_REQUEST_BODY {
            self.broken = true;
        }
        if self.broken {
            self.messages = Vec::new();
        }
    }
}

pub(super) fn size_of(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| serde_json::to_string(message).map_or(0, |written| written.len()))
        .sum()
}

/// Adds messages, joining one to the last when they share a role.
pub(super) fn append(into: &mut Vec<Message>, more: Vec<Message>) {
    for next in more {
        match into.last_mut() {
            Some(last) if last.role == next.role => last.blocks.extend(next.blocks),
            _ => into.push(next),
        }
    }
}
