use hyper::header::{HeaderMap, PROXY_AUTHORIZATION};
use serde::Serialize;
use serde_json::{json, Value};

const MAX_COMMAND: usize = 32;
const MAX_DIR: usize = 4096;

/// Which agent command and directory started a connection, as the wrapper told the proxy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Label {
    pub command: String,
    pub dir: String,
}

impl Label {
    pub fn to_json(&self) -> Value {
        json!({"command": self.command, "dir": self.dir})
    }
}

/// Reads the label from the `Proxy-Authorization: Basic` header of a CONNECT request.
/// Anything wrong with it, in any way, gives `None`: a label never fails a connection.
pub fn from_headers(headers: &HeaderMap) -> Option<Label> {
    let mut values = headers.get_all(PROXY_AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(decode_base64(encoded.trim())?).ok()?;
    let (command, password) = decoded.split_once(':')?;
    // The wrapper sends every byte as %XX; a client that already decoded it sends the path itself.
    let dir = if password.starts_with('%') {
        String::from_utf8(decode_percent(password)?).ok()?
    } else {
        password.to_string()
    };
    let command_ok = (1..=MAX_COMMAND).contains(&command.len())
        && command
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let dir_ok = dir.len() <= MAX_DIR && dir.starts_with('/') && !dir.chars().any(char::is_control);
    (command_ok && dir_ok).then(|| Label {
        command: command.to_string(),
        dir,
    })
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut bits, mut width) = (0u32, 0);
    for byte in text.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = bits << 6 | value as u32;
        width += 6;
        if width >= 8 {
            width -= 8;
            out.push((bits >> width) as u8);
            bits &= (1 << width) - 1;
        }
    }
    Some(out)
}

fn decode_percent(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let pair = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(pair, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
            out.push_str(&"=".repeat(3 - chunk.len()));
        }
        out
    }

    fn encoded(dir: &str) -> String {
        dir.bytes().map(|b| format!("%{b:02X}")).collect()
    }

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(PROXY_AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn basic(user_and_password: &str) -> HeaderMap {
        headers(&format!("Basic {}", base64(user_and_password.as_bytes())))
    }

    fn label(command: &str, dir: &str) -> Option<Label> {
        Some(Label {
            command: command.into(),
            dir: dir.into(),
        })
    }

    #[test]
    fn reads_a_percent_encoded_directory() {
        let found = from_headers(&basic(&format!("claude:{}", encoded("/Users/me/Code/kaiser soze"))));

        assert_eq!(found, label("claude", "/Users/me/Code/kaiser soze"));
    }

    #[test]
    fn reads_a_directory_the_client_already_decoded() {
        assert_eq!(from_headers(&basic("codex:/Users/me/é")), label("codex", "/Users/me/é"));
    }

    #[test]
    fn the_scheme_is_case_insensitive_and_padding_is_optional() {
        let value = base64(b"agent:/tmp").trim_end_matches('=').to_string();

        assert_eq!(
            from_headers(&headers(&format!("basic {value}"))),
            label("agent", "/tmp")
        );
    }

    #[test]
    fn anything_wrong_gives_none() {
        let long_dir = format!("/{}", "a".repeat(MAX_DIR));
        let cases = [
            ("no header", HeaderMap::new()),
            ("bearer", headers("Bearer abc")),
            ("bad base64", headers("Basic !!!")),
            ("no colon", basic("claude")),
            ("empty command", basic(":/tmp")),
            ("command with a space", basic("cl aude:/tmp")),
            (
                "command too long",
                basic(&format!("{}:/tmp", "a".repeat(MAX_COMMAND + 1))),
            ),
            ("relative directory", basic("claude:tmp")),
            ("newline in directory", basic("claude:/tmp/a\nb")),
            ("escape in directory", basic("claude:/tmp/\x1b[31m")),
            ("encoded escape", basic(&format!("claude:{}", encoded("/tmp/\x1b")))),
            ("directory too long", basic(&format!("claude:{long_dir}"))),
            ("bad percent escape", basic("claude:%2Ftmp%zz")),
            (
                "invalid utf8",
                headers(&format!("Basic {}", base64(b"claude:/tmp/\xff"))),
            ),
        ];
        for (name, headers) in cases {
            assert_eq!(from_headers(&headers), None, "{name}");
        }
    }

    #[test]
    fn two_authorization_headers_give_none() {
        let mut headers = basic("claude:/tmp");
        headers.append(PROXY_AUTHORIZATION, HeaderValue::from_static("Basic Zm9v"));

        assert_eq!(from_headers(&headers), None);
    }

    #[test]
    fn the_longest_allowed_command_and_directory_are_kept() {
        let command = "a".repeat(MAX_COMMAND);
        let dir = format!("/{}", "b".repeat(MAX_DIR - 1));

        assert_eq!(from_headers(&basic(&format!("{command}:{dir}"))), label(&command, &dir));
    }

    #[test]
    fn a_label_serialises_its_command_then_its_directory() {
        let found = label("claude", "/tmp").unwrap();

        assert_eq!(
            serde_json::to_string(&found).unwrap(),
            r#"{"command":"claude","dir":"/tmp"}"#
        );
        assert_eq!(serde_json::to_value(&found).unwrap(), found.to_json());
    }
}
