use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    pub protocol_version: Option<String>,
    pub client_info: Option<ClientInfo>,
    pub trace_parent: Option<String>,
    pub trace_state: Option<String>,
    pub baggage: Option<String>,
}

impl RequestMeta {
    pub fn from_params(params: &Value) -> Self {
        let Some(meta) = params.get("_meta") else {
            return Self::default();
        };

        let protocol_version = meta
            .get("io.modelcontextprotocol/protocolVersion")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let client_info = meta
            .get("io.modelcontextprotocol/clientInfo")
            .and_then(|ci| {
                Some(ClientInfo {
                    name: ci.get("name")?.as_str()?.to_string(),
                    version: ci
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            });

        let trace_parent = meta
            .get("traceparent")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let trace_state = meta
            .get("tracestate")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let baggage = meta
            .get("baggage")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Self {
            protocol_version,
            client_info,
            trace_parent,
            trace_state,
            baggage,
        }
    }

    pub fn is_modern(&self) -> bool {
        self.protocol_version.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_empty_params() {
        let meta = RequestMeta::from_params(&json!({}));
        assert!(!meta.is_modern());
        assert!(meta.protocol_version.is_none());
        assert!(meta.client_info.is_none());
    }

    #[test]
    fn test_modern_meta() {
        let params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {
                    "name": "claude-code",
                    "version": "1.0.0"
                },
                "traceparent": "00-abc123-def456-01",
                "tracestate": "vendor=value"
            }
        });
        let meta = RequestMeta::from_params(&params);
        assert!(meta.is_modern());
        assert_eq!(meta.protocol_version.as_deref(), Some("2026-07-28"));
        let ci = meta.client_info.unwrap();
        assert_eq!(ci.name, "claude-code");
        assert_eq!(ci.version, "1.0.0");
        assert_eq!(meta.trace_parent.as_deref(), Some("00-abc123-def456-01"));
        assert_eq!(meta.trace_state.as_deref(), Some("vendor=value"));
    }

    #[test]
    fn test_legacy_params_no_meta() {
        let params = json!({ "protocolVersion": "2024-11-05" });
        let meta = RequestMeta::from_params(&params);
        assert!(!meta.is_modern());
    }

    #[test]
    fn test_meta_without_client_info() {
        let params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28"
            }
        });
        let meta = RequestMeta::from_params(&params);
        assert!(meta.is_modern());
        assert!(meta.client_info.is_none());
        assert!(meta.trace_parent.is_none());
    }

    #[test]
    fn test_meta_with_baggage() {
        let params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-11-25",
                "baggage": "userId=alice,requestId=123"
            }
        });
        let meta = RequestMeta::from_params(&params);
        assert_eq!(meta.baggage.as_deref(), Some("userId=alice,requestId=123"));
    }

    #[test]
    fn test_default_is_empty() {
        let meta = RequestMeta::default();
        assert!(!meta.is_modern());
        assert!(meta.protocol_version.is_none());
        assert!(meta.client_info.is_none());
        assert!(meta.trace_parent.is_none());
        assert!(meta.trace_state.is_none());
        assert!(meta.baggage.is_none());
    }
}
