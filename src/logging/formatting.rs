use super::{Format, Record};

pub(super) const MAX_COLUMNS: usize = 160;
pub(super) const MAX_RECORD_BYTES: usize = 1024;

pub(super) fn encode(record: &Record, format: Format) -> Option<String> {
    let Ok(value) = serde_json::to_value(record) else {
        return None;
    };
    let mut line = match format {
        Format::Json => value.to_string(),
        Format::Human => {
            let secs = record.timestamp_ms / 1000 % 86400;
            let mut text = format!(
                "{:02}:{:02}:{:02}.{:03}Z {} {}",
                secs / 3600,
                secs / 60 % 60,
                secs % 60,
                record.timestamp_ms % 1000,
                record.level.as_str(),
                record.event
            );
            if let Some(id) = &record.run_id {
                text.push_str(&format!(" run={}", id.chars().take(8).collect::<String>()));
            }
            if let Some(exchange) = record.fields.exchange {
                text.push_str(&format!(" exchange={exchange}"));
            }
            if let Some(adapter) = record.fields.adapter {
                text.push_str(&format!(" adapter={adapter}"));
            }
            if let Some(messages) = record.fields.messages {
                text.push_str(&format!(" messages={messages}"));
            }
            if let Some(tools) = record.fields.tools {
                text.push_str(&format!(" tools={tools}"));
            }
            for (key, value) in value.as_object().unwrap() {
                if matches!(
                    key.as_str(),
                    "timestamp_ms" | "level" | "event" | "run_id" | "exchange" | "adapter" | "messages" | "tools"
                ) {
                    continue;
                }
                text.push_str(&format!(" {key}={value}"));
            }
            text.chars().filter(|c| !c.is_control()).take(MAX_COLUMNS).collect()
        }
    };
    if line.len() > MAX_RECORD_BYTES {
        return None;
    }
    line.push('\n');
    Some(line)
}
