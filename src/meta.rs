#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    pub trace_parent: Option<String>,
    pub trace_state: Option<String>,
    pub baggage: Option<String>,
}

impl RequestMeta {
    pub fn from_rmcp_meta(meta: &rmcp::model::Meta) -> Self {
        Self {
            trace_parent: meta.0.get("traceparent").and_then(|v| v.as_str()).map(String::from),
            trace_state: meta.0.get("tracestate").and_then(|v| v.as_str()).map(String::from),
            baggage: meta.0.get("baggage").and_then(|v| v.as_str()).map(String::from),
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
        let meta = rmcp::model::Meta(map);

        let rm = RequestMeta::from_rmcp_meta(&meta);
        assert_eq!(rm.trace_parent.as_deref(), Some("00-abc123-def456-01"));
        assert_eq!(rm.trace_state.as_deref(), Some("vendor=value"));
        assert_eq!(rm.baggage.as_deref(), Some("userId=alice"));
    }

    #[test]
    fn test_from_rmcp_meta_empty() {
        let meta = rmcp::model::Meta::default();
        let rm = RequestMeta::from_rmcp_meta(&meta);
        assert!(rm.trace_parent.is_none());
    }
}
