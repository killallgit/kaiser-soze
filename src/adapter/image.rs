//! Shared image sizing for adapter request normalization.

pub(crate) fn base64_size(data: &str) -> usize {
    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
    (data.len() / 4 * 3).saturating_sub(padding)
}
