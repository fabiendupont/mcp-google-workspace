use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::sync::{Mutex, RwLock};

use google_workspace::error::GwsError;

use crate::handler::GwsHandler;
use crate::policy::Policy;
use crate::shared::ServerState;
use crate::users::UserInstance;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

#[derive(Clone)]
struct AppState {
    /// Single-user mode: the shared state
    single: Option<Arc<Mutex<ServerState>>>,
    /// Multi-user mode: per-user instances
    users: Vec<Arc<UserInstance>>,
}

impl AppState {
    fn single(state: Arc<Mutex<ServerState>>) -> Self {
        Self {
            single: Some(state),
            users: Vec::new(),
        }
    }

    fn multi(users: Vec<UserInstance>) -> Self {
        Self {
            single: None,
            users: users.into_iter().map(Arc::new).collect(),
        }
    }

    /// Every state that may hold subscriptions, for webhook fan-out.
    fn all_states(&self) -> Vec<Arc<Mutex<ServerState>>> {
        let mut states: Vec<_> = self.users.iter().map(|u| u.state.clone()).collect();
        if let Some(s) = &self.single {
            states.push(s.clone());
        }
        states
    }
}

pub async fn serve(
    policy: Arc<RwLock<Policy>>,
    policy_path: Option<PathBuf>,
    state: Arc<Mutex<ServerState>>,
    addr: &str,
) -> Result<(), GwsError> {
    let ct = tokio_util::sync::CancellationToken::new();

    let config = StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token());

    let shared_state = state.clone();
    let shared_policy = policy.clone();

    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(GwsHandler::from_shared(
                shared_state.clone(),
                shared_policy.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let app_state = AppState::single(state);

    if let Some(ref reload_path) = policy_path {
        spawn_policy_reload(policy, app_state.single.clone(), reload_path.clone());
    }

    let app = Router::new()
        .nest_service("/mcp", mcp_service)
        .route("/webhooks/drive", post(handle_drive_webhook))
        .route("/healthz", get(handle_health))
        .route("/readyz", get(handle_readyz))
        .route("/livez", get(handle_livez))
        .route("/metrics", get(handle_metrics))
        .with_state(app_state);

    run_server(app, addr, ct).await
}

/// Multi-user mode: each user is served under `/mcp/<user>` with an
/// isolated state and policy. Users that configure `server.api_token`
/// must authenticate with `Authorization: Bearer <token>`.
pub async fn serve_users(
    users: Vec<UserInstance>,
    addr: &str,
    external_url: Option<String>,
) -> Result<(), GwsError> {
    let ct = tokio_util::sync::CancellationToken::new();

    let mut app = Router::new()
        .route("/webhooks/drive", post(handle_drive_webhook))
        .route("/healthz", get(handle_health))
        .route("/readyz", get(handle_readyz))
        .route("/livez", get(handle_livez))
        .route("/metrics", get(handle_metrics));

    for user in &users {
        let config =
            StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token());
        let user_state = user.state.clone();
        let user_policy = user.policy.clone();

        let mcp_service = StreamableHttpService::new(
            move || {
                Ok(GwsHandler::from_shared(
                    user_state.clone(),
                    user_policy.clone(),
                ))
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );

        let api_token = user.api_token().await;
        let auth_layer = middleware::from_fn_with_state(api_token.clone(), require_bearer_token);

        let user_router = Router::new()
            .fallback_service(mcp_service)
            .layer(auth_layer);

        app = app.nest_service(&format!("/mcp/{}", user.name), user_router);

        // Per-user policy reload on SIGHUP
        spawn_policy_reload(
            user.policy.clone(),
            Some(user.state.clone()),
            user.policy_path.clone(),
        );

        tracing::info!(
            user = %user.name,
            endpoint = format!("/mcp/{}", user.name),
            auth = if api_token.is_some() { "bearer token" } else { "none" },
            "User endpoint mounted"
        );
    }

    let _ = external_url; // webhook URL is already set per-user at load time
    let app_state = AppState::multi(users);
    let app = app.with_state(app_state);

    run_server(app, addr, ct).await
}

/// Spawn the SIGHUP policy reload task (single-user and per-user).
fn spawn_policy_reload(
    policy: Arc<RwLock<Policy>>,
    state: Option<Arc<Mutex<ServerState>>>,
    reload_path: PathBuf,
) {
    tokio::spawn(async move {
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("failed to register SIGHUP handler");
        loop {
            sighup.recv().await;
            tracing::info!("Received SIGHUP, reloading policy");
            match Policy::from_file(&reload_path) {
                Ok(new_policy) => {
                    let svcs = new_policy.allowed_services().join(", ");
                    *policy.write().await = new_policy;
                    if let Some(ref state) = state {
                        let mut st = state.lock().await;
                        st.tools = None;
                    }
                    tracing::info!(services = %svcs, "Policy reloaded, tools cache cleared");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Policy reload failed, keeping current policy");
                }
            }
        }
    });
}

async fn run_server(
    app: Router,
    addr: &str,
    ct: tokio_util::sync::CancellationToken,
) -> Result<(), GwsError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Failed to bind to {addr}: {e}")))?;

    tracing::info!(addr = addr, "HTTP server listening");

    let ct_shutdown = ct.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
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
        ct_shutdown.cancel();
    })
    .await
    .map_err(|e| GwsError::Other(anyhow::anyhow!("HTTP server error: {e}")))?;

    Ok(())
}

/// Extract the bearer token from an Authorization header.
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Bearer-token authentication middleware for per-user endpoints.
/// `expected == None` means the endpoint is unauthenticated.
async fn require_bearer_token(
    State(expected): State<Option<String>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = expected else {
        return next.run(req).await;
    };

    match extract_bearer_token(req.headers()) {
        Some(token) if token == expected => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "missing or invalid Authorization: Bearer token",
        )
            .into_response(),
    }
}

async fn handle_drive_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let channel_id = headers
        .get("x-goog-channel-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let resource_state = headers
        .get("x-goog-resource-state")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    if channel_id.is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    // Fan out to every state: in multi-user mode the channel belongs to
    // exactly one user, but channel IDs are globally unique so the
    // others simply find no match.
    for st in state.all_states() {
        let subs = st.lock().await.subscriptions.clone();
        crate::subscriptions::handle_webhook(channel_id, resource_state, &subs).await;
    }

    StatusCode::OK
}

async fn handle_health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn handle_readyz(State(state): State<AppState>) -> impl IntoResponse {
    let states = state.all_states();
    let mut ready = true;
    for st in &states {
        let st = st.lock().await;
        if st.tools.is_none() {
            ready = false;
        }
    }
    if states.is_empty() {
        ready = false;
    }
    if ready {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

async fn handle_livez() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn handle_metrics() -> impl IntoResponse {
    let body = crate::metrics::encode();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        assert_eq!(
            extract_bearer_token(&headers_with("Bearer secret123")),
            Some("secret123".to_string())
        );
    }

    #[test]
    fn test_extract_bearer_token_trims() {
        assert_eq!(
            extract_bearer_token(&headers_with("Bearer   secret  ")),
            Some("secret".to_string())
        );
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        assert_eq!(extract_bearer_token(&headers_with("Basic abc")), None);
    }

    #[test]
    fn test_extract_bearer_token_empty() {
        assert_eq!(extract_bearer_token(&headers_with("Bearer ")), None);
    }

    #[test]
    fn test_extract_bearer_token_missing_header() {
        assert_eq!(extract_bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn test_app_state_multi_all_states() {
        let dir = std::env::temp_dir().join("mcp-gws-test-http-multi");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("alice")).unwrap();
        std::fs::write(
            dir.join("alice/gws-policy.json"),
            r#"{"services": [{"name": "drive"}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("bob")).unwrap();
        std::fs::write(
            dir.join("bob/gws-policy.json"),
            r#"{"services": [{"name": "gmail"}]}"#,
        )
        .unwrap();

        let users = crate::users::load_users(&dir, &[], false, None).unwrap();
        let app_state = AppState::multi(users);
        assert!(app_state.single.is_none());
        assert_eq!(app_state.all_states().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_app_state_single_all_states() {
        let state = Arc::new(Mutex::new(ServerState::new()));
        let app_state = AppState::single(state);
        assert_eq!(app_state.all_states().len(), 1);
    }
}
