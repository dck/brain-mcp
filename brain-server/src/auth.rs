use axum::Json;
use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response as HttpResponse};
use serde_json::json;

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator is available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn tokens_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']')
            .next()
            .map(|h| format!("[{h}]"))
            .unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default().to_string()
    };
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

pub enum Rejection {
    Origin,
    Host,
    Unauthorized,
}

pub fn check(headers: &HeaderMap, token: Option<&str>) -> Result<(), Rejection> {
    if headers.contains_key(ORIGIN) {
        return Err(Rejection::Origin);
    }
    let host = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !host_allowed(host) {
        return Err(Rejection::Host);
    }
    if let Some(expected) = token {
        let presented = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default();
        if !tokens_match(presented, expected) {
            return Err(Rejection::Unauthorized);
        }
    }
    Ok(())
}

impl IntoResponse for Rejection {
    fn into_response(self) -> HttpResponse {
        match self {
            Rejection::Origin => (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "origin not allowed"})),
            )
                .into_response(),
            Rejection::Host => (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "host not allowed"})),
            )
                .into_response(),
            Rejection::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
                Json(json!({"error": "unauthorized"})),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        for token in [&a, &b] {
            assert_eq!(token.len(), 64);
            assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn tokens_match_cases() {
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "abcd"));
        assert!(!tokens_match("", "abc"));
    }

    #[test]
    fn host_allowed_cases() {
        assert!(host_allowed("127.0.0.1:47200"));
        assert!(host_allowed("localhost"));
        assert!(host_allowed("localhost:1"));
        assert!(host_allowed("[::1]:47200"));
        assert!(!host_allowed("evil.com"));
        assert!(!host_allowed("127.0.0.1.evil.com:80"));
        assert!(!host_allowed(""));
        assert!(!host_allowed("localhost.evil:1"));
    }
}
