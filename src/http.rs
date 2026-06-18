use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use google_workspace::error::GwsError;

use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::protocol;
use crate::server::ServerState;

struct RateLimiter {
    requests: HashMap<String, Vec<Instant>>,
    max_rpm: u32,
}

impl RateLimiter {
    fn new(max_rpm: u32) -> Self {
        Self {
            requests: HashMap::new(),
            max_rpm,
        }
    }

    fn check(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let one_minute = std::time::Duration::from_secs(60);
        let entries = self.requests.entry(key.to_string()).or_default();
        entries.retain(|t| now.duration_since(*t) < one_minute);
        if entries.len() >= self.max_rpm as usize {
            return false;
        }
        entries.push(now);
        true
    }
}

#[derive(Clone)]
struct AppState {
    policy: Arc<Policy>,
    state: Arc<Mutex<ServerState>>,
    notify_tx: broadcast::Sender<Value>,
    rate_limiter: Option<Arc<Mutex<RateLimiter>>>,
    session_id: Arc<String>,
}

pub async fn serve(
    policy: Arc<Policy>,
    state: Arc<Mutex<ServerState>>,
    addr: &str,
) -> Result<(), GwsError> {
    let (notify_tx, _) = broadcast::channel::<Value>(256);

    let rate_limiter = policy.rate_limit_rpm.map(|rpm| {
        tracing::info!(rate_limit_rpm = rpm, "Rate limiting enabled");
        Arc::new(Mutex::new(RateLimiter::new(rpm)))
    });

    let max_body = policy.max_request_bytes;

    let session_id = Arc::new(format!(
        "{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let app_state = AppState {
        policy,
        state,
        notify_tx,
        rate_limiter,
        session_id,
    };

    let app = Router::new()
        .route("/mcp", post(handle_post))
        .route("/mcp", get(handle_get))
        .route("/healthz", get(handle_health))
        .route("/readyz", get(handle_readyz))
        .route("/livez", get(handle_livez))
        .route("/metrics", get(handle_metrics))
        .layer(axum::extract::DefaultBodyLimit::max(max_body))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Failed to bind to {addr}: {e}")))?;

    tracing::info!(addr = addr, "HTTP server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("HTTP server error: {e}")))?;

    Ok(())
}

async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down HTTP server");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down HTTP server");
        }
    }
}

#[axum::debug_handler]
async fn handle_post(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Err(resp) = validate_origin(&headers, &app.policy) {
        return *resp;
    }

    if let Some(ref rl) = app.rate_limiter {
        let client_ip = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        let allowed = rl.lock().await.check(&client_ip);
        if !allowed {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32603,
                        "message": "Rate limit exceeded"
                    }
                })),
            )
                .into_response();
        }
    }

    let req = match protocol::parse_request(&body) {
        Ok(r) => r,
        Err(resp) => {
            let json = resp.to_json();
            return (StatusCode::BAD_REQUEST, Json(json)).into_response();
        }
    };

    if req.is_notification() {
        return StatusCode::ACCEPTED.into_response();
    }

    let id = req.id.as_ref().unwrap();
    let meta = RequestMeta::from_params(&req.params);

    if let Err(msg) = validate_mcp_headers(&headers, &req.method, &req.params) {
        let resp = protocol::invalid_params(id, &msg);
        return (StatusCode::BAD_REQUEST, Json(resp.to_json())).into_response();
    }

    let accepts_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/event-stream"));

    if accepts_sse {
        return handle_post_streaming(app, req, meta).await;
    }

    let mut state = app.state.lock().await;

    let (response, notifications) =
        crate::server::handle_request(&req.method, &req.params, &meta, &app.policy, &mut state)
            .await;

    for notif in &notifications {
        let _ = app.notify_tx.send(notif.clone());
    }

    let resp = match response {
        Ok(result) => protocol::success(id, result),
        Err(err_resp) => err_resp,
    };

    let json = resp.to_json();
    let mut response = (StatusCode::OK, Json(json)).into_response();
    if let Ok(val) = axum::http::HeaderValue::from_str(&app.session_id) {
        response.headers_mut().insert("mcp-session-id", val);
    }
    response
}

async fn handle_post_streaming(
    app: AppState,
    req: protocol::JsonRpcRequest,
    meta: RequestMeta,
) -> axum::response::Response {
    let id = req.id.clone().unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        let mut state = app.state.lock().await;

        let (response, notifications) =
            crate::server::handle_request(&req.method, &req.params, &meta, &app.policy, &mut state)
                .await;

        for notif in &notifications {
            let _ = app.notify_tx.send(notif.clone());
            let data = serde_json::to_string(notif).unwrap_or_default();
            let _ = tx
                .send(Ok(Event::default().event("notification").data(data)))
                .await;
        }

        let resp = match response {
            Ok(result) => protocol::success(&id, result),
            Err(err_resp) => err_resp,
        };
        let data = serde_json::to_string(&resp.to_json()).unwrap_or_default();
        let _ = tx
            .send(Ok(Event::default().event("message").data(data)))
            .await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[axum::debug_handler]
async fn handle_get(State(app): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(resp) = validate_origin(&headers, &app.policy) {
        return *resp;
    }

    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !accept.contains("text/event-stream") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32600,
                    "message": "GET /mcp requires Accept: text/event-stream"
                }
            })),
        )
            .into_response();
    }

    let rx = app.notify_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(value) => {
            let data = serde_json::to_string(&value).unwrap_or_default();
            Some(Ok::<_, Infallible>(Event::default().data(data)))
        }
        Err(_) => None,
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn handle_health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

async fn handle_readyz(State(app): State<AppState>) -> impl IntoResponse {
    let state = app.state.lock().await;
    let has_tools = state.tools_json.is_some();
    if has_tools {
        (StatusCode::OK, Json(json!({"status": "ready"})))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "not_ready", "reason": "discovery docs not yet loaded"})),
        )
    }
}

async fn handle_livez() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "alive"})))
}

async fn handle_metrics() -> impl IntoResponse {
    let body = crate::metrics::encode();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

fn validate_origin(
    headers: &HeaderMap,
    policy: &Policy,
) -> Result<(), Box<axum::response::Response>> {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok())
        && !policy.is_origin_allowed(origin)
    {
        return Err(Box::new(
            (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32600,
                        "message": format!("Origin '{origin}' not allowed")
                    }
                })),
            )
                .into_response(),
        ));
    }
    Ok(())
}

fn validate_mcp_headers(headers: &HeaderMap, method: &str, params: &Value) -> Result<(), String> {
    if let Some(mcp_method) = headers.get("mcp-method").and_then(|v| v.to_str().ok())
        && mcp_method != method
    {
        return Err(format!(
            "Mcp-Method header '{mcp_method}' does not match request method '{method}'"
        ));
    }

    if let Some(mcp_name) = headers.get("mcp-name").and_then(|v| v.to_str().ok()) {
        let expected_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if !expected_name.is_empty() && mcp_name != expected_name {
            return Err(format!(
                "Mcp-Name header '{mcp_name}' does not match request name '{expected_name}'"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn test_policy() -> Arc<Policy> {
        Arc::new(Policy::from_services(&["drive".to_string()]))
    }

    fn policy_with_origins(origins: Vec<String>) -> Arc<Policy> {
        let json_str = serde_json::json!({
            "server": { "allowed_origins": origins },
            "services": [{ "name": "drive" }]
        });
        let file: crate::policy::PolicyFile = serde_json::from_value(json_str).unwrap();
        Arc::new(Policy::from_policy_file(file))
    }

    #[test]
    fn test_validate_origin_localhost_allowed() {
        let policy = test_policy();
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://localhost:3000"));
        assert!(validate_origin(&headers, &policy).is_ok());
    }

    #[test]
    fn test_validate_origin_127_allowed() {
        let policy = test_policy();
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:8080"));
        assert!(validate_origin(&headers, &policy).is_ok());
    }

    #[test]
    fn test_validate_origin_remote_rejected() {
        let policy = test_policy();
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://evil.com"));
        assert!(validate_origin(&headers, &policy).is_err());
    }

    #[test]
    fn test_validate_origin_no_header_allowed() {
        let policy = test_policy();
        let headers = HeaderMap::new();
        assert!(validate_origin(&headers, &policy).is_ok());
    }

    #[test]
    fn test_validate_origin_custom_allowlist() {
        let policy = policy_with_origins(vec!["internal.corp.com".to_string()]);
        let mut headers = HeaderMap::new();
        headers.insert(
            "origin",
            HeaderValue::from_static("https://internal.corp.com"),
        );
        assert!(validate_origin(&headers, &policy).is_ok());
    }

    #[test]
    fn test_validate_origin_custom_allowlist_rejects_others() {
        let policy = policy_with_origins(vec!["internal.corp.com".to_string()]);
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("http://localhost:3000"));
        assert!(validate_origin(&headers, &policy).is_err());
    }

    #[test]
    fn test_validate_mcp_headers_matching() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-method", HeaderValue::from_static("tools/call"));
        headers.insert("mcp-name", HeaderValue::from_static("drive"));
        let params = json!({"name": "drive"});
        assert!(validate_mcp_headers(&headers, "tools/call", &params).is_ok());
    }

    #[test]
    fn test_validate_mcp_headers_method_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-method", HeaderValue::from_static("tools/list"));
        let params = json!({});
        assert!(validate_mcp_headers(&headers, "tools/call", &params).is_err());
    }

    #[test]
    fn test_validate_mcp_headers_name_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert("mcp-name", HeaderValue::from_static("gmail"));
        let params = json!({"name": "drive"});
        assert!(validate_mcp_headers(&headers, "tools/call", &params).is_err());
    }

    #[test]
    fn test_validate_mcp_headers_absent_ok() {
        let headers = HeaderMap::new();
        let params = json!({});
        assert!(validate_mcp_headers(&headers, "tools/call", &params).is_ok());
    }

    #[tokio::test]
    async fn test_rate_limiter_allows_under_limit() {
        let mut rl = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(rl.check("client1"));
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_blocks_over_limit() {
        let mut rl = RateLimiter::new(3);
        assert!(rl.check("client1"));
        assert!(rl.check("client1"));
        assert!(rl.check("client1"));
        assert!(!rl.check("client1"));
    }

    #[tokio::test]
    async fn test_rate_limiter_separate_clients() {
        let mut rl = RateLimiter::new(1);
        assert!(rl.check("client1"));
        assert!(!rl.check("client1"));
        assert!(rl.check("client2"));
    }

    #[test]
    fn test_accepts_sse_detection() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("text/event-stream"));
        let accepts = headers
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/event-stream"));
        assert!(accepts);

        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("application/json"));
        let accepts = headers
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/event-stream"));
        assert!(!accepts);

        let headers = HeaderMap::new();
        let accepts = headers
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/event-stream"));
        assert!(!accepts);
    }
}
