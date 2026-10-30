use crate::capture::MAX_BUFFER_BYTES;

pub fn is_encoded(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name == "content-encoding" && value != "identity")
}

pub struct RequestBuffer {
    bytes: Vec<u8>,
    truncated: bool,
    encoded: bool,
}

impl RequestBuffer {
    pub fn new(encoded: bool) -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
            encoded,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if self.bytes.len() + chunk.len() <= MAX_BUFFER_BYTES {
            self.bytes.extend_from_slice(chunk);
        } else {
            self.truncated = true;
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Hands the bytes over without copying; the buffer is empty afterwards.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }

    pub fn unreadable(&self) -> bool {
        self.truncated || self.encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_moves_the_bytes_out_and_leaves_the_buffer_empty() {
        let mut buffer = RequestBuffer::new(false);
        buffer.push(b"abc");

        assert_eq!(buffer.take(), b"abc");
        assert!(buffer.bytes().is_empty());
    }
}
