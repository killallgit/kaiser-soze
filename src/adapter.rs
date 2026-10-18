use bytes::Bytes;
use serde_json::Value;

pub struct RequestHead {
    pub method: String,
    pub host: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

pub enum TapInput {
    RequestBody(Bytes),
    RequestEnd,
    ResponseHead(ResponseHead),
    ResponseBody(Bytes),
    ResponseEnd,
    WsMessage {
        from_client: bool,
        text: String,
        start: u64,
        end: u64,
    },
}

pub struct TapEvent {
    pub kind: String,
    pub payload: Value,
    pub start: u64,
    pub end: u64,
}

/// Teaches the proxy about one tool: which hosts to decrypt and which exchanges to parse.
pub trait Adapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn hosts(&self) -> &[String];
    fn claim(&self, head: &RequestHead) -> Option<Box<dyn ExchangeParser>>;
}

pub trait ExchangeParser: Send {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent>;
}

pub fn claims_host(adapter: &dyn Adapter, host: &str) -> bool {
    adapter
        .hosts()
        .iter()
        .any(|claimed| host == claimed || host.ends_with(&format!(".{claimed}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OneHost(Vec<String>);

    impl Adapter for OneHost {
        fn name(&self) -> &'static str {
            "test"
        }
        fn hosts(&self) -> &[String] {
            &self.0
        }
        fn claim(&self, _head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
            None
        }
    }

    #[test]
    fn claims_the_host_and_its_subdomains_but_not_lookalikes() {
        // It matches whole labels only
        let adapter = OneHost(vec!["chatgpt.com".to_string()]);

        assert!(claims_host(&adapter, "chatgpt.com"));
        assert!(claims_host(&adapter, "api.chatgpt.com"));
        assert!(!claims_host(&adapter, "evilchatgpt.com"));
        assert!(!claims_host(&adapter, "chatgpt.com.evil"));
    }
}
