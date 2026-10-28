use serde::{Deserialize, Serialize};

/// The token counts one event reports; a count the upstream did not send is `null`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub reasoning: Option<u64>,
}

impl Usage {
    /// Takes every count the later report has and keeps the rest.
    pub fn merge(&mut self, later: Usage) {
        self.cache_read = later.cache_read.or(self.cache_read);
        self.cache_write = later.cache_write.or(self.cache_write);
        self.input = later.input.or(self.input);
        self.output = later.output.or(self.output);
        self.reasoning = later.reasoning.or(self.reasoning);
    }
}
