use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};

use google_workspace::error::GwsError;

use crate::handler::GwsHandler;
use crate::policy::Policy;
use crate::server::ServerState;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};

#[derive(Clone)]
struct AppState {
    server_state: Arc<Mutex<ServerState>>,
}

pub async fn serve(
    policy: Arc<RwLock<Policy>>,
    policy_path: Option<std::path::PathBuf>,
    state: Arc<Mutex<ServerState>>,
    addr: &str,
) -> Result<(), GwsError> {
    let ct = tokio_util::sync::CancellationToken::new();

    let config = StreamableHttpServerConfig::default()
        .with_cancellation_token(ct.child_token());

    let shared_state = state.clone();
    let shared_policy = policy.clone();

    let mcp_service = StreamableHttpService::new(
        move || Ok(GwsHandler::from_shared(shared_state.clone(), shared_policy.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let app_state = AppState {
        server_state: state,
    };

    if let Some(ref reload_path) = policy_path {
        let reload_policy = policy.clone();
        let reload_state = app_state.server_state.clone();
        let reload_path = reload_path.clone();
        tokio::spawn(async move {
            let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("failed to register SIGHUP handler");
            loop {
                sighup.recv().await;
                tracing::info!("Received SIGHUP, reloading policy");
                match Policy::from_file(&reload_path) {
                    Ok(new_policy) => {
                        let svcs = new_policy.allowed_services().join(", ");
                        *reload_policy.write().await = new_policy;
                        let mut st = reload_state.lock().await;
                        st.tools = None;
                        tracing::info!(services = %svcs, "Policy reloaded, tools cache cleared");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Policy reload failed, keeping current policy");
                    }
                }
            }
        });
    }

    let app = Router::new()
        .nest_service("/mcp", mcp_service)
        .route("/healthz", get(handle_health))
        .route("/readyz", get(handle_readyz))
        .route("/livez", get(handle_livez))
        .route("/metrics", get(handle_metrics))
        .with_state(app_state);

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
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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

async fn handle_health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn handle_readyz(State(state): State<AppState>) -> impl IntoResponse {
    let st = state.server_state.lock().await;
    if st.tools.is_some() {
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
