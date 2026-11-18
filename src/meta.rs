/// Longest `traceparent` / `tracestate` value accepted from a client.
///
/// W3C trace-context headers are short; the spec's grammar keeps `tracestate`
/// to 32 list members and `traceparent` to 55 characters. The cap is generous
/// but bounded so an attacker cannot make the server copy megabytes per request.
const TRACE_HEADER_MAX_LEN: usize = 1024;

/// Longest `baggage` value accepted from a client.
///
/// Baggage carries arbitrary key/value pairs, so it gets a larger budget than
/// the trace-context headers while staying bounded.
const BAGGAGE_MAX_LEN: usize = 8192;

/// Whether `byte` may appear in an HTTP field value / trace-context header.
///
/// RFC 9110 field values and the W3C trace-context grammar allow visible ASCII
/// (`0x20..=0x7e`) plus horizontal tab. C0 controls, DEL (`0x7f`) and every
/// byte `>= 0x80` are rejected: they cannot be converted to a `HeaderValue`
/// and are not valid trace-context.
fn is_valid_trace_header_byte(byte: u8) -> bool {
    byte == b'\t' || (0x20..=0x7e).contains(&byte)
}

/// Validate a client-supplied `_meta` trace header value.
///
/// Returns `None` — dropping the value — when it is empty, longer than
/// `max_len`, or contains a byte outside the trace-context grammar. Dropping is
/// deliberate: `reqwest::RequestBuilder::header` stores a conversion error
/// instead of failing, and a request built from such a value returns `None`
/// from `try_clone()`. Release builds are `panic = "abort"`, so propagating a
/// bad value turns a client-controlled `_meta` field into a SIGABRT.
///
/// The warning names the field but never the value: it is attacker-controlled
/// and may contain control bytes.
fn sanitize_trace_header(field: &str, value: &str, max_len: usize) -> Option<String> {
    if value.is_empty() {
        tracing::warn!(field, "dropping empty _meta trace header value");
        return None;
    }
    if value.len() > max_len {
        tracing::warn!(
            field,
            len = value.len(),
            max_len,
            "dropping oversized _meta trace header value"
        );
        return None;
    }
    if !value.bytes().all(is_valid_trace_header_byte) {
        tracing::warn!(
            field,
            "dropping _meta trace header value with bytes outside the trace-context grammar"
        );
        return None;
    }
    Some(value.to_string())
}

#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    pub trace_parent: Option<String>,
    pub trace_state: Option<String>,
    pub baggage: Option<String>,
    pub user_email: Option<String>,
}

impl RequestMeta {
    pub fn from_rmcp_meta(meta: &rmcp::model::RequestMetaObject) -> Self {
        let field = |name: &str| meta.0.0.get(name).and_then(|v| v.as_str());

        Self {
            trace_parent: field("traceparent").and_then(|value| {
                sanitize_trace_header("traceparent", value, TRACE_HEADER_MAX_LEN)
            }),
            trace_state: field("tracestate")
                .and_then(|value| sanitize_trace_header("tracestate", value, TRACE_HEADER_MAX_LEN)),
            baggage: field("baggage")
                .and_then(|value| sanitize_trace_header("baggage", value, BAGGAGE_MAX_LEN)),
            user_email: field("user_email").map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_default_is_empty() {
        let meta = RequestMeta::default();
        assert!(meta.trace_parent.is_none());
        assert!(meta.trace_state.is_none());
        assert!(meta.baggage.is_none());
    }

    #[test]
    fn test_from_rmcp_meta_with_trace() {
        let mut map = serde_json::Map::new();
        map.insert("traceparent".to_string(), json!("00-abc123-def456-01"));
        map.insert("tracestate".to_string(), json!("vendor=value"));
        map.insert("baggage".to_string(), json!("userId=alice"));
        let meta = rmcp::model::RequestMetaObject(rmcp::model::MetaObject(map));

        let rm = RequestMeta::from_rmcp_meta(&meta);
        assert_eq!(rm.trace_parent.as_deref(), Some("00-abc123-def456-01"));
        assert_eq!(rm.trace_state.as_deref(), Some("vendor=value"));
        assert_eq!(rm.baggage.as_deref(), Some("userId=alice"));
    }

    #[test]
    fn test_from_rmcp_meta_empty() {
        let meta = rmcp::model::RequestMetaObject::default();
        let rm = RequestMeta::from_rmcp_meta(&meta);
        assert!(rm.trace_parent.is_none());
    }

    #[test]
    fn test_trace_header_rejects_control_and_non_ascii_bytes() {
        // CR, LF, NUL, DEL and non-ASCII bytes are all outside the grammar.
        for bad in [
            "",
            "00-abc\rdef-01",
            "00-abc\ndef-01",
            "00-abc\0def-01",
            "00-abc\u{7f}def-01",
            "00-caf\u{e9}-def-01",
        ] {
            assert!(
                sanitize_trace_header("traceparent", bad, TRACE_HEADER_MAX_LEN).is_none(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn test_trace_header_accepts_w3c_traceparent() {
        assert_eq!(
            sanitize_trace_header("traceparent", "00-abc-def-01", TRACE_HEADER_MAX_LEN).as_deref(),
            Some("00-abc-def-01")
        );
        // Horizontal tab is a valid field-value byte.
        assert!(
            sanitize_trace_header("tracestate", "vendor=value\tother=1", TRACE_HEADER_MAX_LEN)
                .is_some()
        );
    }

    #[test]
    fn test_trace_header_length_caps() {
        let long = "a".repeat(2000);
        assert!(sanitize_trace_header("traceparent", &long, TRACE_HEADER_MAX_LEN).is_none());
        assert!(sanitize_trace_header("tracestate", &long, TRACE_HEADER_MAX_LEN).is_none());
        // 2000 bytes is still within the baggage budget.
        assert!(sanitize_trace_header("baggage", &long, BAGGAGE_MAX_LEN).is_some());

        let too_big = "a".repeat(BAGGAGE_MAX_LEN + 1);
        assert!(sanitize_trace_header("baggage", &too_big, BAGGAGE_MAX_LEN).is_none());
    }

    #[test]
    fn test_from_rmcp_meta_drops_poisoned_trace_headers() {
        let mut map = serde_json::Map::new();
        map.insert("traceparent".to_string(), json!("00-abc\r\ndef-01"));
        map.insert("tracestate".to_string(), json!("vendor=value"));
        map.insert("baggage".to_string(), json!("user\0=alice"));
        let meta = rmcp::model::RequestMetaObject(rmcp::model::MetaObject(map));

        let rm = RequestMeta::from_rmcp_meta(&meta);
        assert!(rm.trace_parent.is_none(), "CR/LF must be dropped");
        assert!(rm.baggage.is_none(), "NUL must be dropped");
        assert_eq!(rm.trace_state.as_deref(), Some("vendor=value"));
    }
}

#[cfg(test)]
mod delegation_tests {
    use super::*;
    use serde_json::json;

    fn meta_with_user_email(email: &str) -> RequestMeta {
        let mut map = serde_json::Map::new();
        map.insert("user_email".to_string(), json!(email));
        let rm = rmcp::model::RequestMetaObject(rmcp::model::MetaObject(map));
        RequestMeta::from_rmcp_meta(&rm)
    }

    #[test]
    fn test_user_email_is_extracted() {
        let meta = meta_with_user_email("alice@example.com");
        assert_eq!(meta.user_email.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn test_unlisted_subject_is_rejected_with_fix_hint() {
        // Empty allowed_subjects: client-supplied identities fail closed.
        let policy = crate::policy::Policy::from_services(&["gmail".to_string()]);
        let meta = meta_with_user_email("attacker@example.com");

        let err = policy
            .resolve_delegation_subject(meta.user_email.as_deref())
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not in server.allowed_subjects"), "{msg}");
        assert!(msg.contains("Fix: add \"attacker@example.com\""), "{msg}");
        assert!(msg.contains("allowed_subjects"), "{msg}");
    }

    #[test]
    fn test_listed_subject_resolves() {
        let mut file = crate::policy::PolicyFile::default();
        file.server.allowed_subjects = vec!["alice@example.com".to_string()];
        let policy = crate::policy::Policy::from_policy_file(file);
        let meta = meta_with_user_email("alice@example.com");

        assert_eq!(
            policy
                .resolve_delegation_subject(meta.user_email.as_deref())
                .unwrap()
                .as_deref(),
            Some("alice@example.com")
        );
    }
}
