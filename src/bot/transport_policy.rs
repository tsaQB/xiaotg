use serde_json::Value;
use std::time::Duration;

pub const MAX_TELEGRAM_ATTEMPTS: usize = 3;
pub const MAX_RETRY_AFTER_SECS: u64 = 60;

fn bounded_attempt(attempt: usize) -> bool {
    attempt + 1 < MAX_TELEGRAM_ATTEMPTS
}

fn backoff(attempt: usize) -> Duration {
    Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(4)))
}

pub fn retry_delay_for_http_status(
    status: u16,
    retry_after_secs: Option<u64>,
    attempt: usize,
) -> Option<Duration> {
    if !bounded_attempt(attempt) {
        return None;
    }
    if status == 429 {
        return retry_after_secs
            .map(|s| Duration::from_secs(s.min(MAX_RETRY_AFTER_SECS)))
            .or_else(|| Some(backoff(attempt)));
    }
    if (500..=599).contains(&status) {
        return Some(backoff(attempt));
    }
    None
}

pub fn retry_delay_from_response(response: &Value, attempt: usize) -> Option<Duration> {
    let code = response.get("error_code").and_then(Value::as_i64)?;
    let retry_after = response
        .pointer("/parameters/retry_after")
        .and_then(Value::as_u64);
    retry_delay_for_http_status(u16::try_from(code).ok()?, retry_after, attempt)
}

pub fn retry_delay_from_error(error: &str, attempt: usize) -> Option<Duration> {
    if !bounded_attempt(attempt) {
        return None;
    }
    if let Some(code) = normalized_api_error_code(error) {
        let retry_after = if code == 429 {
            error
                .rsplit_once(" retry_after=")
                .and_then(|(_, tail)| tail.strip_suffix('s'))
                .and_then(|seconds| seconds.parse::<u64>().ok())
        } else {
            None
        };
        return retry_delay_for_http_status(code, retry_after, attempt);
    }
    if error.to_ascii_lowercase().contains("connection failure") {
        return Some(backoff(attempt));
    }
    None
}

fn normalized_api_error_code(error: &str) -> Option<u16> {
    if let Some(error) = error.strip_prefix("Telegram API error [") {
        let (_, tail) = error.split_once("] code=")?;
        let (code, _) = tail.split_once(':')?;
        return code.parse().ok();
    }
    if let Some(rest) = error.strip_prefix("HTTP ") {
        let code_str = rest.split_whitespace().next()?;
        return code_str.parse().ok();
    }
    None
}

pub fn fallback_allowed_response(response: &Value) -> bool {
    response.get("error_code").and_then(Value::as_i64) == Some(400)
}

pub fn fallback_allowed_error(error: &str) -> bool {
    normalized_api_error_code(error) == Some(400)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn honors_retry_after_for_429() {
        let response = json!({
            "ok": false,
            "error_code": 429,
            "parameters": {"retry_after": 7}
        });
        assert_eq!(
            retry_delay_from_response(&response, 0),
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn retry_delay_for_http_status_behaves_consistently() {
        assert_eq!(
            retry_delay_for_http_status(429, Some(15), 0),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            retry_delay_for_http_status(429, Some(3600), 0),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SECS))
        );
        assert!(retry_delay_for_http_status(429, None, 0).is_some());
        assert!(retry_delay_for_http_status(500, None, 0).is_some());
        assert!(retry_delay_for_http_status(502, None, 0).is_some());
        assert!(retry_delay_for_http_status(503, None, 0).is_some());
        assert!(retry_delay_for_http_status(400, None, 0).is_none());
        assert!(retry_delay_for_http_status(401, None, 0).is_none());
        assert!(retry_delay_for_http_status(404, None, 0).is_none());
        assert!(retry_delay_for_http_status(503, None, MAX_TELEGRAM_ATTEMPTS).is_none());
    }

    #[test]
    fn retries_5xx_but_not_deterministic_4xx() {
        assert!(retry_delay_from_response(&json!({"error_code": 503}), 0).is_some());
        assert!(retry_delay_from_response(&json!({"error_code": 401}), 0).is_none());
        assert!(retry_delay_from_response(&json!({"error_code": 400}), 0).is_none());
    }

    #[test]
    fn retries_are_bounded() {
        assert!(retry_delay_from_response(&json!({"error_code": 503}), 1).is_some());
        assert!(retry_delay_from_response(&json!({"error_code": 503}), 2).is_none());
    }

    #[test]
    fn parses_retry_after_from_normalized_api_errors() {
        assert_eq!(
            retry_delay_from_error(
                "Telegram API error [sendMessage] code=429: Too Many Requests retry_after=9s",
                0,
            ),
            Some(Duration::from_secs(9))
        );
    }

    #[test]
    fn caps_retry_after_to_safe_upper_bound() {
        let response = json!({
            "ok": false,
            "error_code": 429,
            "parameters": {"retry_after": 3600}
        });
        assert_eq!(
            retry_delay_from_response(&response, 0),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SECS))
        );
        assert_eq!(
            retry_delay_from_error(
                "Telegram API error [sendMessage] code=429: Too Many Requests retry_after=3600s",
                0,
            ),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SECS))
        );
    }

    #[test]
    fn retries_on_html_502_503_error_string() {
        assert!(retry_delay_from_error("HTTP 502 Bad Gateway: <html>...</html>", 0).is_some());
        assert!(
            retry_delay_from_error("HTTP 503 Service Unavailable: <html>...</html>", 0).is_some()
        );
        assert!(retry_delay_from_error("Telegram API error [sendMessage] code=502: HTTP 502 Bad Gateway (invalid JSON: expected ident)", 0).is_some());
    }

    #[test]
    fn rich_or_media_fallback_is_only_for_bad_request() {
        assert!(fallback_allowed_response(&json!({"error_code": 400})));
        assert!(!fallback_allowed_response(&json!({"error_code": 429})));
        assert!(!fallback_allowed_response(&json!({"error_code": 503})));
        assert!(fallback_allowed_error(
            "Telegram API error [x] code=400: bad request"
        ));
        assert!(!fallback_allowed_error(
            "Telegram API error [x] code=429: rate limited"
        ));
    }

    #[test]
    fn normalized_error_policy_ignores_description_substrings() {
        assert!(!fallback_allowed_error(
            "Telegram API error [x] code=500: upstream code=400"
        ));
        assert!(!fallback_allowed_error(
            "Telegram API error [x] code=4000: unknown"
        ));
        assert!(
            retry_delay_from_error("Telegram API error [x] code=400: bad retry_after=9s", 0)
                .is_none()
        );
    }

    #[test]
    fn retries_only_transport_failures_known_to_precede_request_delivery() {
        assert!(retry_delay_from_error("connection failure", 0).is_some());
        assert!(retry_delay_from_error("timeout", 0).is_none());
        assert!(retry_delay_from_error("body failure", 0).is_none());
        assert!(retry_delay_from_error("transport failure", 0).is_none());
        assert!(retry_delay_from_error("request failure", 0).is_none());
    }
}
