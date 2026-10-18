use hyper::HeaderMap;

const REDACTED: &str = "[redacted]";

fn is_secret(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "chatgpt-account-id"
    ) || name.ends_with("-token")
        || name.ends_with("-key")
        || ["auth", "secret", "password", "session-token", "api-key", "apikey"]
            .iter()
            .any(|marker| name.contains(marker))
}

pub fn redact_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = if is_secret(name.as_str()) {
                REDACTED.to_string()
            } else {
                String::from_utf8_lossy(value.as_bytes()).into_owned()
            };
            (name.as_str().to_string(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{HeaderName, HeaderValue};

    fn redacted(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        redact_headers(&headers)
    }

    fn value_of<'a>(out: &'a [(String, String)], name: &str) -> &'a str {
        &out.iter().find(|(n, _)| n == name).unwrap().1
    }

    #[test]
    fn hides_credential_values_and_keeps_the_rest() {
        // It redacts credential headers by name and leaves other values readable
        let out = redacted(&[
            ("Authorization", "Bearer abc"),
            ("Cookie", "a=b"),
            ("Set-Cookie", "a=b"),
            ("x-session-token", "abc"),
            ("x-api-key", "abc"),
            ("ChatGPT-Account-Id", "abc"),
            ("content-type", "text/event-stream"),
        ]);

        let secret_names = [
            "authorization",
            "cookie",
            "set-cookie",
            "x-session-token",
            "x-api-key",
            "chatgpt-account-id",
        ];
        assert!(secret_names.iter().all(|n| value_of(&out, n) == REDACTED));
        assert_eq!(value_of(&out, "content-type"), "text/event-stream");
    }

    #[test]
    fn hides_headers_whose_names_suggest_a_credential() {
        // It redacts by name fragment, whatever the case
        let out = redacted(&[
            ("X-Auth-Info", "v"),
            ("X-Client-Secret", "v"),
            ("X-Password-Hint", "v"),
            ("X-Session-Token-Id", "v"),
            ("X-ApiKey-Id", "v"),
            ("X-Api-Key-Id", "v"),
        ]);

        assert!(out.iter().all(|(_, v)| v == REDACTED));
    }
}
