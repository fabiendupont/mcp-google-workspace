use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug)]
pub struct JsonRpcRequest {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug)]
pub enum JsonRpcResponse {
    Result {
        id: Value,
        result: Value,
    },
    Error {
        id: Value,
        code: i64,
        message: String,
    },
    Notification(Value),
}

impl JsonRpcResponse {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Result { id, result } => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }),
            Self::Error { id, code, message } => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": code,
                    "message": message
                }
            }),
            Self::Notification(value) => value.clone(),
        }
    }
}

pub fn parse_request(line: &str) -> Result<JsonRpcRequest, JsonRpcResponse> {
    let val: Value = serde_json::from_str(line).map_err(|_| JsonRpcResponse::Error {
        id: Value::Null,
        code: PARSE_ERROR,
        message: "Parse error".to_string(),
    })?;

    let method = val
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or_else(|| JsonRpcResponse::Error {
            id: val.get("id").cloned().unwrap_or(Value::Null),
            code: INVALID_REQUEST,
            message: "Missing or invalid 'method' field".to_string(),
        })?
        .to_string();

    let id = val.get("id").cloned();
    let params = val.get("params").cloned().unwrap_or_else(|| json!({}));

    Ok(JsonRpcRequest { id, method, params })
}

pub fn success(id: &Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse::Result {
        id: id.clone(),
        result,
    }
}

pub fn method_not_found(id: &Value, method: &str) -> JsonRpcResponse {
    JsonRpcResponse::Error {
        id: id.clone(),
        code: METHOD_NOT_FOUND,
        message: format!("Method not found: {method}"),
    }
}

pub fn invalid_params(id: &Value, msg: &str) -> JsonRpcResponse {
    JsonRpcResponse::Error {
        id: id.clone(),
        code: INVALID_PARAMS,
        message: msg.to_string(),
    }
}

pub fn internal_error(id: &Value, msg: &str) -> JsonRpcResponse {
    JsonRpcResponse::Error {
        id: id.clone(),
        code: INTERNAL_ERROR,
        message: msg.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_request() {
        let req = parse_request(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#).unwrap();
        assert_eq!(req.method, "ping");
        assert_eq!(req.id, Some(json!(1)));
        assert!(!req.is_notification());
    }

    #[test]
    fn test_parse_notification() {
        let req =
            parse_request(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
                .unwrap();
        assert_eq!(req.method, "notifications/initialized");
        assert!(req.is_notification());
    }

    #[test]
    fn test_parse_missing_params_defaults_to_empty() {
        let req = parse_request(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(req.params, json!({}));
    }

    #[test]
    fn test_parse_invalid_json() {
        let err = parse_request("not json").unwrap_err();
        match err {
            JsonRpcResponse::Error { code, .. } => assert_eq!(code, PARSE_ERROR),
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn test_parse_missing_method() {
        let err = parse_request(r#"{"jsonrpc":"2.0","id":1}"#).unwrap_err();
        match err {
            JsonRpcResponse::Error { code, id, .. } => {
                assert_eq!(code, INVALID_REQUEST);
                assert_eq!(id, json!(1));
            }
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn test_success_response_shape() {
        let resp = success(&json!(42), json!({"ok": true}));
        let j = resp.to_json();
        assert_eq!(j["jsonrpc"], "2.0");
        assert_eq!(j["id"], 42);
        assert_eq!(j["result"]["ok"], true);
        assert!(j.get("error").is_none());
    }

    #[test]
    fn test_error_response_shape() {
        let resp = method_not_found(&json!("abc"), "foo/bar");
        let j = resp.to_json();
        assert_eq!(j["jsonrpc"], "2.0");
        assert_eq!(j["id"], "abc");
        assert_eq!(j["error"]["code"], METHOD_NOT_FOUND);
        assert!(j["error"]["message"].as_str().unwrap().contains("foo/bar"));
    }

    #[test]
    fn test_invalid_params_response() {
        let resp = invalid_params(&json!(1), "Missing 'name'");
        let j = resp.to_json();
        assert_eq!(j["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn test_internal_error_response() {
        let resp = internal_error(&json!(1), "something broke");
        let j = resp.to_json();
        assert_eq!(j["error"]["code"], INTERNAL_ERROR);
    }
}
