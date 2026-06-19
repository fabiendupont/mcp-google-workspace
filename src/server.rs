use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use google_workspace::discovery::RestDescription;
use google_workspace::error::GwsError;

use crate::helpers::{self, ParagraphStyle, Position, TextStyle};
use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::protocol::{self, JsonRpcResponse};
use crate::tasks;
use crate::tools;

pub(crate) struct ServerState {
    pub tools_json: Option<Vec<Value>>,
    pub docs: HashMap<String, Arc<RestDescription>>,
    pub era: ClientEra,
    pub active_ids: HashSet<String>,
    pub tasks: HashMap<String, tasks::Task>,
    pub token_cache: Option<crate::auth::TokenCache>,
    pub audit: Option<Arc<crate::audit::AuditLogger>>,
    pub prompts: Vec<crate::prompts::Prompt>,
}

#[derive(Debug)]
pub(crate) enum ClientEra {
    Unknown,
    Legacy { initialized: bool },
    Modern,
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self {
            tools_json: None,
            docs: HashMap::new(),
            era: ClientEra::Unknown,
            active_ids: HashSet::new(),
            tasks: HashMap::new(),
            token_cache: None,
            audit: None,
            prompts: Vec::new(),
        }
    }

    pub(crate) fn track_id(&mut self, id: &Value) -> Result<(), JsonRpcResponse> {
        let key = id.to_string();
        if !self.active_ids.insert(key) {
            return Err(protocol::invalid_params(id, "Duplicate request ID"));
        }
        Ok(())
    }

    pub(crate) fn release_id(&mut self, id: &Value) {
        self.active_ids.remove(&id.to_string());
    }

    pub(crate) async fn get_doc(
        &mut self,
        svc_alias: &str,
    ) -> Result<Arc<RestDescription>, GwsError> {
        tools::get_or_fetch_doc(&mut self.docs, svc_alias).await
    }

    pub(crate) fn clean_expired_sessions(&mut self) {
        tasks::clean_expired_tasks(&mut self.tasks);
    }
}

pub async fn run_stdio(
    policy: Policy,
    prompts: Vec<crate::prompts::Prompt>,
    audit: Option<Arc<crate::audit::AuditLogger>>,
) -> Result<(), GwsError> {
    let svc_list = policy.allowed_services();
    if svc_list.is_empty() {
        tracing::warn!("No services configured. Zero tools will be exposed.");
    } else {
        tracing::info!(services = %svc_list.join(", "), "Starting MCP server");
    }

    let mut state = ServerState::new();
    state.prompts = prompts;
    state.audit = audit;

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Failed to register SIGTERM handler: {e}")))?;

    loop {
        let line = tokio::select! {
            result = stdin.next_line() => {
                match result {
                    Ok(Some(line)) => line,
                    _ => break,
                }
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, shutting down");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        if line.len() > MAX_REQUEST_SIZE {
            tracing::warn!(
                size = line.len(),
                limit = MAX_REQUEST_SIZE,
                "Oversized request rejected"
            );
            let resp = JsonRpcResponse::Error {
                id: Value::Null,
                code: protocol::INVALID_REQUEST,
                message: "Request too large".to_string(),
            };
            if let Err(e) = write_stdio_response(&mut stdout, &resp).await {
                tracing::error!(error = %e, "stdout write failed");
                break;
            }
            continue;
        }

        let req = match protocol::parse_request(&line) {
            Ok(r) => r,
            Err(resp) => {
                if let Err(e) = write_stdio_response(&mut stdout, &resp).await {
                    tracing::error!(error = %e, "stdout write failed");
                    break;
                }
                continue;
            }
        };

        if req.is_notification() {
            detect_era(
                &mut state.era,
                &req.method,
                &RequestMeta::from_params(&req.params),
            );
            continue;
        }

        let id = req.id.as_ref().unwrap();

        if let Err(resp) = state.track_id(id) {
            if let Err(e) = write_stdio_response(&mut stdout, &resp).await {
                tracing::error!(error = %e, "stdout write failed");
                break;
            }
            continue;
        }

        let meta = RequestMeta::from_params(&req.params);
        detect_era(&mut state.era, &req.method, &meta);

        if let Some(resp) = check_pre_init(&state.era, &req.method, id) {
            state.release_id(id);
            if let Err(e) = write_stdio_response(&mut stdout, &resp).await {
                tracing::error!(error = %e, "stdout write failed");
                break;
            }
            continue;
        }

        let (response, notifications) =
            handle_request(&req.method, &req.params, &meta, &policy, &mut state, id).await;

        let mut write_failed = false;
        for notif in &notifications {
            let n = JsonRpcResponse::Notification(notif.clone());
            if let Err(e) = write_stdio_response(&mut stdout, &n).await {
                tracing::error!(error = %e, "stdout write failed");
                write_failed = true;
                break;
            }
        }

        let resp = match response {
            Ok(result) => protocol::success(id, result),
            Err(err_resp) => err_resp,
        };

        state.release_id(id);
        if write_failed {
            break;
        }
        if let Err(e) = write_stdio_response(&mut stdout, &resp).await {
            tracing::error!(error = %e, "stdout write failed");
            break;
        }
    }

    Ok(())
}

pub async fn run_http(
    policy: Policy,
    prompts: Vec<crate::prompts::Prompt>,
    policy_path: Option<std::path::PathBuf>,
    addr: &str,
    audit: Option<Arc<crate::audit::AuditLogger>>,
) -> Result<(), GwsError> {
    let mut s = ServerState::new();
    s.prompts = prompts;
    s.audit = audit;
    s.era = ClientEra::Modern;
    let state = Arc::new(Mutex::new(s));
    let policy = Arc::new(tokio::sync::RwLock::new(policy));

    crate::http::serve(policy, policy_path, state, addr).await
}

pub(crate) fn detect_era(era: &mut ClientEra, method: &str, meta: &RequestMeta) {
    if matches!(era, ClientEra::Unknown) {
        if meta.is_modern() || method == "server/discover" {
            *era = ClientEra::Modern;
        } else if method == "initialize" {
            *era = ClientEra::Legacy { initialized: false };
        }
        if let Some(ref ci) = meta.client_info {
            tracing::info!(client.name = %ci.name, client.version = %ci.version, "Client connected");
        }
    }
    if let ClientEra::Legacy { initialized } = era
        && method == "notifications/initialized"
    {
        *initialized = true;
    }
}

pub(crate) fn check_pre_init(era: &ClientEra, method: &str, id: &Value) -> Option<JsonRpcResponse> {
    match era {
        ClientEra::Legacy { initialized: false } => {
            if method != "initialize" && method != "ping" {
                return Some(protocol::invalid_params(
                    id,
                    "Server not initialized. Send 'initialize' first.",
                ));
            }
        }
        ClientEra::Unknown
            if method != "initialize" && method != "server/discover" && method != "ping" =>
        {
            return Some(protocol::invalid_params(
                id,
                "Send 'initialize' or 'server/discover' first.",
            ));
        }
        _ => {}
    }
    None
}

async fn write_stdio_response(
    stdout: &mut tokio::io::Stdout,
    resp: &JsonRpcResponse,
) -> Result<(), std::io::Error> {
    let json = resp.to_json();
    let Ok(mut out) = serde_json::to_string(&json) else {
        tracing::error!("Failed to serialize JSON-RPC response");
        return Ok(());
    };
    out.push('\n');
    stdout.write_all(out.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

pub(crate) async fn handle_request(
    method: &str,
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &mut ServerState,
    id: &Value,
) -> (Result<Value, JsonRpcResponse>, Vec<Value>) {
    let start = Instant::now();

    let result = match method {
        "initialize" => {
            let client_version = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2024-11-05");

            let negotiated = negotiate_version(client_version);

            Ok(json!({
                "protocolVersion": negotiated,
                "serverInfo": server_info(),
                "capabilities": server_capabilities(),
                "instructions": server_instructions()
            }))
        }

        "server/discover" => Ok(json!({
            "capabilities": server_capabilities(),
            "serverInfo": server_info(),
            "supportedVersions": SUPPORTED_VERSIONS,
            "instructions": server_instructions()
        })),

        "ping" => Ok(json!({})),

        "tools/list" => {
            if state.tools_json.is_none() {
                match tools::build_tools_list(policy, &mut state.docs).await {
                    Ok(tools) => state.tools_json = Some(tools),
                    Err(e) => {
                        return (Err(protocol::internal_error(id, &e.to_string())), vec![]);
                    }
                }
            }
            let all_tools = state.tools_json.as_ref().unwrap();
            let cursor = params.get("cursor").and_then(|v| v.as_str());
            paginate_tools(all_tools, cursor)
                .map_err(|e| protocol::invalid_params(id, &e.to_string()))
        }

        "tools/call" => {
            let (result, notifications) = handle_tool_call(params, meta, policy, state).await;
            let mapped = result.map_err(|e| protocol::internal_error(id, &e.to_string()));
            crate::metrics::record_request(method, mapped.is_err(), start.elapsed().as_secs_f64());
            crate::metrics::set_active_tasks(state.tasks.len() as i64);
            return (mapped, notifications);
        }

        "tasks/get" => tasks::handle_tasks_get(params, &state.tasks)
            .map_err(|e| protocol::invalid_params(id, &e.to_string())),

        "tasks/result" => tasks::handle_tasks_result(params, &state.tasks)
            .map_err(|e| protocol::invalid_params(id, &e.to_string())),

        "tasks/cancel" => tasks::handle_tasks_cancel(params, &mut state.tasks)
            .map_err(|e| protocol::invalid_params(id, &e.to_string())),

        "tasks/list" => tasks::handle_tasks_list(params, &state.tasks)
            .map_err(|e| protocol::internal_error(id, &e.to_string())),

        "prompts/list" => Ok(crate::prompts::list_prompts(&state.prompts)),

        "prompts/get" => {
            let name = match params.get("name").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => {
                    return (
                        Err(protocol::invalid_params(id, "Missing 'name' parameter")),
                        vec![],
                    );
                }
            };
            let default_args = json!({});
            let args = params.get("arguments").unwrap_or(&default_args);
            crate::prompts::get_prompt(&state.prompts, name, args)
                .map_err(|msg| protocol::invalid_params(id, &msg))
        }

        _ => Err(protocol::method_not_found(id, method)),
    };

    crate::metrics::record_request(method, result.is_err(), start.elapsed().as_secs_f64());
    crate::metrics::set_active_tasks(state.tasks.len() as i64);
    (result, vec![])
}

pub(crate) async fn handle_request_concurrent(
    method: &str,
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &Arc<Mutex<ServerState>>,
    id: &Value,
) -> (Result<Value, JsonRpcResponse>, Vec<Value>) {
    let start = Instant::now();

    let result = match method {
        "initialize" => {
            let client_version = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2024-11-05");
            let negotiated = negotiate_version(client_version);
            Ok(json!({
                "protocolVersion": negotiated,
                "serverInfo": server_info(),
                "capabilities": server_capabilities(),
                "instructions": server_instructions()
            }))
        }

        "server/discover" => Ok(json!({
            "capabilities": server_capabilities(),
            "serverInfo": server_info(),
            "supportedVersions": SUPPORTED_VERSIONS,
            "instructions": server_instructions()
        })),

        "ping" => Ok(json!({})),

        "tools/list" => {
            let mut st = state.lock().await;
            if st.tools_json.is_none() {
                match tools::build_tools_list(policy, &mut st.docs).await {
                    Ok(tools) => st.tools_json = Some(tools),
                    Err(e) => {
                        return (Err(protocol::internal_error(id, &e.to_string())), vec![]);
                    }
                }
            }
            let all_tools = st.tools_json.as_ref().unwrap();
            let cursor = params.get("cursor").and_then(|v| v.as_str());
            paginate_tools(all_tools, cursor)
                .map_err(|e| protocol::invalid_params(id, &e.to_string()))
        }

        "tools/call" => {
            let (result, notifications) =
                handle_tool_call_concurrent(params, meta, policy, state).await;
            let task_count = state.lock().await.tasks.len();
            let mapped = result.map_err(|e| protocol::internal_error(id, &e.to_string()));
            crate::metrics::record_request(method, mapped.is_err(), start.elapsed().as_secs_f64());
            crate::metrics::set_active_tasks(task_count as i64);
            return (mapped, notifications);
        }

        "tasks/get" | "tasks/result" | "tasks/cancel" | "tasks/list" => {
            let mut st = state.lock().await;
            match method {
                "tasks/get" => tasks::handle_tasks_get(params, &st.tasks)
                    .map_err(|e| protocol::invalid_params(id, &e.to_string())),
                "tasks/result" => tasks::handle_tasks_result(params, &st.tasks)
                    .map_err(|e| protocol::invalid_params(id, &e.to_string())),
                "tasks/cancel" => tasks::handle_tasks_cancel(params, &mut st.tasks)
                    .map_err(|e| protocol::invalid_params(id, &e.to_string())),
                "tasks/list" => tasks::handle_tasks_list(params, &st.tasks)
                    .map_err(|e| protocol::internal_error(id, &e.to_string())),
                _ => unreachable!(),
            }
        }

        "prompts/list" => {
            let st = state.lock().await;
            Ok(crate::prompts::list_prompts(&st.prompts))
        }

        "prompts/get" => {
            let name = match params.get("name").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => {
                    return (
                        Err(protocol::invalid_params(id, "Missing 'name' parameter")),
                        vec![],
                    );
                }
            };
            let default_args = json!({});
            let args = params.get("arguments").unwrap_or(&default_args);
            let st = state.lock().await;
            crate::prompts::get_prompt(&st.prompts, name, args)
                .map_err(|msg| protocol::invalid_params(id, &msg))
        }

        _ => Err(protocol::method_not_found(id, method)),
    };

    let task_count = state.lock().await.tasks.len();
    crate::metrics::record_request(method, result.is_err(), start.elapsed().as_secs_f64());
    crate::metrics::set_active_tasks(task_count as i64);
    (result, vec![])
}

async fn handle_tool_call_concurrent(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &Arc<Mutex<ServerState>>,
) -> (Result<Value, GwsError>, Vec<Value>) {
    match handle_tool_call_inner_concurrent(params, meta, policy, state).await {
        Ok((result, notifications)) => (Ok(result), notifications),
        Err(e) => {
            let msg = e.to_string();
            if is_policy_denial(&msg) {
                tracing::warn!(reason = %msg, "Policy denied tool call");
                (
                    Err(GwsError::Validation(
                        "Operation not allowed by policy".to_string(),
                    )),
                    vec![],
                )
            } else {
                (Err(e), vec![])
            }
        }
    }
}

async fn handle_tool_call_inner_concurrent(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &Arc<Mutex<ServerState>>,
) -> Result<(Value, Vec<Value>), GwsError> {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'name' in tools/call".to_string()))?;

    tracing::info!(tool = tool_name, "Tool call");

    let default_args = json!({});
    let raw_arguments = params.get("arguments").unwrap_or(&default_args);
    let dry_run = raw_arguments
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let arguments = &strip_key(raw_arguments, "dry_run");

    if tool_name == "gws_discover" {
        let mut st = state.lock().await;
        let result = tools::handle_discover(arguments, policy, &mut st.docs).await?;
        return Ok((result, vec![]));
    }

    if tool_name == "gws_batch" {
        let service = arguments
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GwsError::Validation("Missing 'service' in gws_batch".to_string()))?;
        let requests = arguments
            .get("requests")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                GwsError::Validation("Missing 'requests' array in gws_batch".to_string())
            })?;
        let mut st = state.lock().await;
        let result = execute_batch(service, requests, policy, meta, &mut st).await?;
        return Ok((result, vec![]));
    }

    if tool_name.starts_with("gws_docs_") {
        let mut st = state.lock().await;
        let result =
            execute_docs_helper(tool_name, arguments, policy, meta, &mut st, dry_run).await?;
        return Ok((result, vec![]));
    }

    let task_id = arguments
        .get("upload_handle")
        .or_else(|| arguments.get("download_handle"))
        .or_else(|| arguments.get("task_id"))
        .and_then(|v| v.as_str());
    if let Some(tid) = task_id {
        let mut st = state.lock().await;
        let result = handle_task_chunk(tid, arguments, &mut st).await?;
        return Ok((result, vec![]));
    }

    let svc_alias = tool_name;
    if !policy.is_service_allowed(svc_alias) {
        tracing::warn!(service = svc_alias, "Policy denied: service not enabled");
        return Err(GwsError::Validation(
            "Operation not allowed by policy".to_string(),
        ));
    }

    let resource_path = arguments
        .get("resource")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'resource' argument".to_string()))?;
    let method_name = arguments
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'method' argument".to_string()))?;

    let (mut tc, audit, doc) = {
        let mut st = state.lock().await;
        let tc = st.token_cache.take();
        let audit = st.audit.clone();
        let doc = st.get_doc(svc_alias).await?;
        (tc, audit, doc)
    };

    let resource = tools::find_resource(&doc.resources, resource_path).ok_or_else(|| {
        GwsError::Validation(format!(
            "Resource '{resource_path}' not found in {svc_alias}"
        ))
    })?;

    let method = resource.methods.get(method_name).ok_or_else(|| {
        GwsError::Validation(format!(
            "Method '{method_name}' not found in {svc_alias}.{resource_path}"
        ))
    })?;

    if arguments
        .get("media_upload_init")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let init_result = crate::execute::initiate_resumable_upload(
            &doc, method, arguments, svc_alias, policy, meta, &mut tc,
        )
        .await;

        let mut st = state.lock().await;
        st.token_cache = tc;
        let init_result = init_result?;

        let session_uri = extract_session_uri(&init_result)?;
        let total_size = arguments
            .get("media_total_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let content_type = arguments
            .get("media_content_type")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();

        let handle = format!(
            "upload_{:016x}",
            crate::execute::simple_hash(session_uri.as_bytes())
        );

        let result = create_upload_task(&mut st, &handle, session_uri, total_size, content_type);
        return Ok((result, vec![]));
    }

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    let exec_start = Instant::now();
    let result = crate::execute::execute_tool(
        &doc,
        method,
        resource_path,
        method_name,
        arguments,
        svc_alias,
        policy,
        meta,
        Some(&notify_tx),
        dry_run,
        &mut tc,
    )
    .await;
    let duration_ms = exec_start.elapsed().as_millis() as u64;

    match &result {
        Ok(_) => {
            if let Some(ref a) = audit {
                a.log_allowed(
                    svc_alias,
                    resource_path,
                    method_name,
                    &method.http_method,
                    0,
                    duration_ms,
                );
            }
        }
        Err(e) => {
            if let Some(ref a) = audit {
                a.log_denied(svc_alias, resource_path, method_name, &e.to_string());
            }
        }
    }
    let result = result?;

    drop(notify_tx);
    let mut notifications = Vec::new();
    while let Ok(notification) = notify_rx.try_recv() {
        notifications.push(notification);
    }

    let mut st = state.lock().await;
    let mcp_result = format_execute_result(
        result,
        method,
        svc_alias,
        resource_path,
        method_name,
        arguments,
        &doc,
        policy,
        meta,
        &mut tc,
        &mut st,
    )
    .await?;

    st.token_cache = tc;
    Ok((mcp_result, notifications))
}

async fn handle_tool_call(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &mut ServerState,
) -> (Result<Value, GwsError>, Vec<Value>) {
    match handle_tool_call_inner(params, meta, policy, state).await {
        Ok((result, notifications)) => (Ok(result), notifications),
        Err(e) => {
            let msg = e.to_string();
            if is_policy_denial(&msg) {
                tracing::warn!(reason = %msg, "Policy denied tool call");
                (
                    Err(GwsError::Validation(
                        "Operation not allowed by policy".to_string(),
                    )),
                    vec![],
                )
            } else {
                (Err(e), vec![])
            }
        }
    }
}

fn create_upload_task(
    state: &mut ServerState,
    handle: &str,
    session_uri: String,
    total_size: u64,
    content_type: String,
) -> Value {
    state.clean_expired_sessions();
    state.tasks.insert(
        handle.to_string(),
        tasks::Task::new(
            handle.to_string(),
            3_600_000,
            tasks::TaskKind::Upload(tasks::UploadData {
                session_uri,
                total_size,
                bytes_uploaded: 0,
                content_type,
            }),
        ),
    );
    json!({
        "content": [{ "type": "text", "text": format!("Upload session started. Handle: {handle}. Send chunks with upload_handle + media_chunk.") }],
        "structuredContent": {
            "upload_handle": handle,
            "total_size": total_size,
            "status": "initiated"
        },
        "isError": false
    })
}

fn create_download_task(
    state: &mut ServerState,
    raw_data: Vec<u8>,
    content_type: String,
    total_size: usize,
) -> Value {
    let handle = format!("download_{:016x}", crate::execute::simple_hash(&raw_data));
    state.clean_expired_sessions();
    state.tasks.insert(
        handle.clone(),
        tasks::Task::new(
            handle.clone(),
            3_600_000,
            tasks::TaskKind::Download(tasks::DownloadData {
                raw_data,
                content_type: content_type.clone(),
                total_size,
            }),
        ),
    );
    json!({
        "content": [{ "type": "text", "text": format!("File ready for download: {} bytes of {}. Use download_handle=\"{handle}\" or tasks/get with taskId=\"{handle}\" to retrieve chunks.", total_size, content_type) }],
        "structuredContent": {
            "download_handle": handle,
            "taskId": handle,
            "total_size": total_size,
            "content_type": content_type,
            "status": "ready"
        },
        "isError": false
    })
}

fn create_auto_resumable_task(
    state: &mut ServerState,
    session_uri: String,
    total_size: u64,
    content_type: String,
) -> Value {
    let handle = format!(
        "upload_{:016x}",
        crate::execute::simple_hash(session_uri.as_bytes())
    );
    state.clean_expired_sessions();
    state.tasks.insert(
        handle.clone(),
        tasks::Task::new(
            handle.clone(),
            3_600_000,
            tasks::TaskKind::Upload(tasks::UploadData {
                session_uri,
                total_size,
                bytes_uploaded: 0,
                content_type: content_type.clone(),
            }),
        ),
    );
    json!({
        "content": [{ "type": "text", "text": format!(
            "File too large for simple upload ({total_size} bytes). \
             Resumable session started. Send chunks with upload_handle=\"{handle}\" + media_chunk."
        ) }],
        "structuredContent": {
            "upload_handle": handle,
            "taskId": handle,
            "total_size": total_size,
            "content_type": content_type,
            "status": "initiated",
            "auto_resumable": true
        },
        "isError": false
    })
}

fn format_tool_result(
    result: Value,
    method: &google_workspace::discovery::RestMethod,
    service: &str,
    resource_path: &str,
    method_name: &str,
    arguments: &Value,
) -> Value {
    if let Some(mcp_content) = result.get("_mcp_content") {
        return json!({
            "content": mcp_content,
            "isError": false
        });
    }

    let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    let mut structured = result;
    if method.http_method != "GET" {
        let explanation = explain_request(service, resource_path, method_name, method, arguments);
        structured["_explanation"] = json!(explanation);
    }
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn extract_session_uri(init_result: &Value) -> Result<String, GwsError> {
    init_result["sessionUri"]
        .as_str()
        .ok_or_else(|| GwsError::Other(anyhow::anyhow!("No session URI in upload init response")))
        .map(|s| s.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn format_execute_result(
    result: Value,
    method: &google_workspace::discovery::RestMethod,
    service: &str,
    resource_path: &str,
    method_name: &str,
    arguments: &Value,
    doc: &RestDescription,
    policy: &Policy,
    meta: &RequestMeta,
    tc: &mut Option<crate::auth::TokenCache>,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    if result.get("_mcp_content").is_some() {
        return Ok(format_tool_result(
            result,
            method,
            service,
            resource_path,
            method_name,
            arguments,
        ));
    }

    if let Some(dl) = result.get("_mcp_large_download") {
        let b64_str = dl["b64_data"].as_str().unwrap_or("");
        let raw_data = base64::engine::general_purpose::STANDARD
            .decode(b64_str)
            .map_err(|_| GwsError::Validation("Invalid base64 in download data".to_string()))?;
        let content_type = dl["content_type"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string();
        let total_size = raw_data.len();
        return Ok(create_download_task(
            state,
            raw_data,
            content_type,
            total_size,
        ));
    }

    if let Some(ar) = result.get("_mcp_auto_resumable") {
        let total_size = ar["total_size"].as_u64().unwrap_or(0);
        let content_type = ar["content_type"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string();

        let init_result = crate::execute::initiate_resumable_upload(
            doc, method, arguments, service, policy, meta, tc,
        )
        .await?;

        let session_uri = extract_session_uri(&init_result)?;
        return Ok(create_auto_resumable_task(
            state,
            session_uri,
            total_size,
            content_type,
        ));
    }

    Ok(format_tool_result(
        result,
        method,
        service,
        resource_path,
        method_name,
        arguments,
    ))
}

fn strip_key(value: &Value, key: &str) -> Value {
    match value.as_object() {
        Some(map) => {
            let filtered: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(k, _)| k.as_str() != key)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Value::Object(filtered)
        }
        None => value.clone(),
    }
}

fn parse_position(arguments: &Value) -> Position {
    if let Some(idx) = arguments.get("index").and_then(|v| v.as_i64()) {
        return Position::Index(idx as i32);
    }
    match arguments.get("position").and_then(|v| v.as_str()) {
        Some("start") => Position::Start,
        _ => Position::End,
    }
}

fn parse_text_style(arguments: &Value) -> TextStyle {
    TextStyle {
        bold: arguments.get("bold").and_then(|v| v.as_bool()),
        italic: arguments.get("italic").and_then(|v| v.as_bool()),
        font_size_pt: arguments.get("font_size_pt").and_then(|v| v.as_f64()),
        font_family: arguments
            .get("font_family")
            .and_then(|v| v.as_str())
            .map(String::from),
        foreground_color: arguments
            .get("foreground_color")
            .and_then(|v| v.as_str())
            .map(String::from),
        background_color: arguments
            .get("background_color")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

async fn execute_docs_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    if tool_name == "gws_docs_import_markdown" {
        return execute_docs_import_markdown(arguments, policy, meta, state, dry_run).await;
    }

    let doc_id = arguments
        .get("document_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'document_id'".into()))?;

    let needs_end_index = |tool: &str, args: &Value| -> bool {
        match tool {
            "gws_docs_insert_text" => {
                args.get("index").is_none()
                    && args.get("position").and_then(|v| v.as_str()) != Some("start")
                    && (args.get("bold").is_some()
                        || args.get("italic").is_some()
                        || args.get("font_size_pt").is_some()
                        || args.get("font_family").is_some()
                        || args.get("foreground_color").is_some()
                        || args.get("background_color").is_some()
                        || args.get("paragraph_style").is_some())
            }
            _ => false,
        }
    };

    let resolve_end_position = |position: Position, end_index: Option<i32>| -> Position {
        match (&position, end_index) {
            (Position::End, Some(idx)) => Position::Index(idx),
            _ => position,
        }
    };

    let end_index = if needs_end_index(tool_name, arguments) && !dry_run {
        let doc_ref = state.get_doc("docs").await?;
        let resource = tools::find_resource(&doc_ref.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": doc_id}});
        let doc_content = crate::execute::execute_tool(
            &doc_ref,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;
        doc_content["body"]["content"]
            .as_array()
            .and_then(|arr| arr.last())
            .and_then(|el| el["endIndex"].as_i64())
            .map(|idx| (idx - 1) as i32)
    } else {
        None
    };

    let requests: Vec<Value> = match tool_name {
        "gws_docs_insert_text" => {
            let text = arguments
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'text'".into()))?;
            let position = resolve_end_position(parse_position(arguments), end_index);
            let style = parse_text_style(arguments);
            let has_style = style.bold.is_some()
                || style.italic.is_some()
                || style.font_size_pt.is_some()
                || style.font_family.is_some()
                || style.foreground_color.is_some()
                || style.background_color.is_some();
            let para = arguments
                .get("paragraph_style")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            helpers::build_insert_text_requests(
                text,
                position,
                if has_style { Some(style) } else { None },
                para.as_deref(),
            )
        }
        "gws_docs_insert_table" => {
            let rows = arguments
                .get("rows")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| GwsError::Validation("Missing 'rows'".into()))?
                as u32;
            let columns = arguments
                .get("columns")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| GwsError::Validation("Missing 'columns'".into()))?
                as u32;
            let position = parse_position(arguments);
            vec![helpers::build_insert_table_request(rows, columns, position)]
        }
        "gws_docs_insert_image" => {
            let image_url = arguments.get("image_url").and_then(|v| v.as_str());
            let drive_file_id = arguments.get("drive_file_id").and_then(|v| v.as_str());
            let image_data = arguments.get("image_data").and_then(|v| v.as_str());

            let uri = if let Some(url) = image_url {
                url.to_string()
            } else if let Some(fid) = drive_file_id {
                let drive_doc = state.get_doc("drive").await?;
                let files_resource = tools::find_resource(&drive_doc.resources, "files")
                    .ok_or_else(|| GwsError::Validation("files resource not found".into()))?;
                let get_method = files_resource
                    .methods
                    .get("get")
                    .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                let get_args = json!({"params": {"fileId": fid, "alt": "media"}});
                let dl_result = crate::execute::execute_tool(
                    &drive_doc,
                    get_method,
                    "files",
                    "get",
                    &get_args,
                    "drive",
                    policy,
                    meta,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                let mcp_content = dl_result["_mcp_content"].as_array().ok_or_else(|| {
                    GwsError::Validation("Failed to download image from Drive".into())
                })?;
                let image_entry = mcp_content
                    .iter()
                    .find(|e| e["type"].as_str() == Some("image"))
                    .ok_or_else(|| GwsError::Validation("Drive file is not an image".into()))?;
                let b64 = image_entry["data"]
                    .as_str()
                    .ok_or_else(|| GwsError::Validation("No image data in download".into()))?;
                let mime = image_entry["mimeType"].as_str().unwrap_or("image/png");
                format!("data:{mime};base64,{b64}")
            } else if let Some(b64) = image_data {
                let mime = arguments
                    .get("image_content_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("image/png");
                format!("data:{mime};base64,{b64}")
            } else {
                return Err(GwsError::Validation(
                    "One of 'image_url', 'drive_file_id', or 'image_data' is required".into(),
                ));
            };

            let position = parse_position(arguments);
            let w = arguments.get("width_pt").and_then(|v| v.as_f64());
            let h = arguments.get("height_pt").and_then(|v| v.as_f64());
            vec![helpers::build_insert_image_request(&uri, position, w, h)]
        }
        "gws_docs_format_text" => {
            let start = arguments
                .get("start_index")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| GwsError::Validation("Missing 'start_index'".into()))?
                as i32;
            let end = arguments
                .get("end_index")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| GwsError::Validation("Missing 'end_index'".into()))?
                as i32;
            let style = parse_text_style(arguments);
            let para =
                if arguments.get("named_style").is_some() || arguments.get("alignment").is_some() {
                    Some(ParagraphStyle {
                        named_style: arguments
                            .get("named_style")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        alignment: arguments
                            .get("alignment")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    })
                } else {
                    None
                };
            helpers::build_format_text_requests(start, end, style, para)
        }
        "gws_docs_add_bullets" => {
            let start = arguments
                .get("start_index")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| GwsError::Validation("Missing 'start_index'".into()))?
                as i32;
            let end = arguments
                .get("end_index")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| GwsError::Validation("Missing 'end_index'".into()))?
                as i32;
            let preset = arguments
                .get("bullet_preset")
                .and_then(|v| v.as_str())
                .unwrap_or("BULLET_DISC_CIRCLE_SQUARE");
            vec![helpers::build_add_bullets_request(start, end, preset)]
        }
        _ => {
            return Err(GwsError::Validation(format!(
                "Unknown helper tool: {tool_name}"
            )));
        }
    };

    let batch_args = json!({
        "params": { "documentId": doc_id },
        "body": { "requests": requests }
    });

    let doc = state.get_doc("docs").await?;
    let resource = tools::find_resource(&doc.resources, "documents")
        .ok_or_else(|| GwsError::Validation("documents resource not found in docs API".into()))?;
    let method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;

    crate::execute::execute_tool(
        &doc,
        method,
        "documents",
        "batchUpdate",
        &batch_args,
        "docs",
        policy,
        meta,
        None,
        dry_run,
        &mut state.token_cache,
    )
    .await
}

fn heading_level(style: &str) -> Option<u32> {
    match style {
        "HEADING_1" => Some(1),
        "HEADING_2" => Some(2),
        "HEADING_3" => Some(3),
        "HEADING_4" => Some(4),
        "HEADING_5" => Some(5),
        "HEADING_6" => Some(6),
        _ => None,
    }
}

fn find_section_range(doc: &Value, section: &str) -> Option<(i32, i32)> {
    let content = doc["body"]["content"].as_array()?;
    let mut section_start = None;
    let mut section_level = None;

    for element in content {
        if let Some(para) = element.get("paragraph") {
            let style_type = para["paragraphStyle"]["namedStyleType"]
                .as_str()
                .unwrap_or("");
            let text: String = para["elements"]
                .as_array()
                .map(|els| {
                    els.iter()
                        .filter_map(|e| e["textRun"]["content"].as_str())
                        .collect::<String>()
                })
                .unwrap_or_default();
            let text_trimmed = text.trim();

            if let Some(level) = heading_level(style_type) {
                if let Some(start_level) = section_level
                    && level <= start_level
                {
                    let start = section_start.unwrap();
                    let end = element["startIndex"].as_i64().unwrap() as i32;
                    return Some((start, end));
                }
                if text_trimmed == section {
                    section_start = Some(element["startIndex"].as_i64().unwrap() as i32);
                    section_level = Some(level);
                }
            }
        }
    }

    if let Some(start) = section_start {
        let last = content.last()?;
        let end = last["endIndex"].as_i64().unwrap_or(start as i64) as i32;
        return Some((start, end));
    }
    None
}

async fn execute_docs_import_markdown(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let markdown = arguments
        .get("markdown")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'markdown'".into()))?;

    let doc_id_arg = arguments.get("document_id").and_then(|v| v.as_str());
    let title = arguments.get("title").and_then(|v| v.as_str());
    let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());
    let section = arguments.get("section").and_then(|v| v.as_str());
    let template_id = arguments.get("template_id").and_then(|v| v.as_str());

    // Step A: resolve or create the document
    let (doc_id, created_new_doc) = if let Some(id) = doc_id_arg {
        (id.to_string(), false)
    } else if title.is_some() || folder_id.is_some() {
        let doc_title = title.unwrap_or("Untitled");
        let mut body = json!({
            "name": doc_title,
            "mimeType": "application/vnd.google-apps.document"
        });
        if let Some(fid) = folder_id {
            body["parents"] = json!([fid]);
        }
        let create_args = json!({"body": body});

        let drive_doc = state.get_doc("drive").await?;
        let drive_resource = tools::find_resource(&drive_doc.resources, "files")
            .ok_or_else(|| GwsError::Validation("files resource not found in drive API".into()))?;
        let create_method = drive_resource
            .methods
            .get("create")
            .ok_or_else(|| GwsError::Validation("create method not found in drive files".into()))?;
        let result = crate::execute::execute_tool(
            &drive_doc,
            create_method,
            "files",
            "create",
            &create_args,
            "drive",
            policy,
            meta,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await?;
        let new_id = result["id"]
            .as_str()
            .ok_or_else(|| {
                GwsError::Other(anyhow::anyhow!("No 'id' in drive.files.create response"))
            })?
            .to_string();
        (new_id, true)
    } else {
        return Err(GwsError::Validation(
            "Either 'document_id' or 'title' is required".into(),
        ));
    };

    // Step B: handle template (apply named styles from another doc)
    let template_requests = if let Some(tmpl_id) = template_id {
        let docs_doc = state.get_doc("docs").await?;
        let resource = tools::find_resource(&docs_doc.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": tmpl_id}});
        let tmpl_result = crate::execute::execute_tool(
            &docs_doc,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;

        let mut style_reqs = Vec::new();
        if let Some(styles) = tmpl_result["namedStyles"]["styles"].as_array() {
            for style in styles {
                if let (Some(props), Some(style_type)) = (
                    style.get("textStyle"),
                    style.get("namedStyleType").and_then(|v| v.as_str()),
                ) {
                    let mut ns_props = serde_json::Map::new();
                    ns_props.insert("namedStyleType".to_string(), json!(style_type));
                    ns_props.insert("textStyle".to_string(), props.clone());
                    if let Some(para) = style.get("paragraphStyle") {
                        ns_props.insert("paragraphStyle".to_string(), para.clone());
                    }
                    style_reqs.push(json!({
                        "updateNamedStyle": {
                            "namedStyle": Value::Object(ns_props),
                            "fields": "*"
                        }
                    }));
                }
            }
        }
        if style_reqs.is_empty() {
            None
        } else {
            Some(style_reqs)
        }
    } else {
        None
    };

    // Step C: handle section replacement
    let (section_delete, insert_index) = if let Some(section_text) = section {
        let docs_doc = state.get_doc("docs").await?;
        let resource = tools::find_resource(&docs_doc.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let get_method = resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
        let get_args = json!({"params": {"documentId": doc_id}});
        let doc_content = crate::execute::execute_tool(
            &docs_doc,
            get_method,
            "documents",
            "get",
            &get_args,
            "docs",
            policy,
            meta,
            None,
            false,
            &mut state.token_cache,
        )
        .await?;

        match find_section_range(&doc_content, section_text) {
            Some((start, end)) => (
                Some(json!({
                    "deleteContentRange": {
                        "range": { "startIndex": start, "endIndex": end }
                    }
                })),
                start,
            ),
            None => {
                return Err(GwsError::Validation(format!(
                    "Section '{}' not found in document",
                    section_text
                )));
            }
        }
    } else {
        let idx = if let Some(i) = arguments.get("index").and_then(|v| v.as_i64()) {
            i as i32
        } else {
            match arguments.get("position").and_then(|v| v.as_str()) {
                Some("start") => 1,
                _ => {
                    // Fetch document to find end index
                    let docs_doc = state.get_doc("docs").await?;
                    let resource = tools::find_resource(&docs_doc.resources, "documents")
                        .ok_or_else(|| {
                            GwsError::Validation("documents resource not found".into())
                        })?;
                    let get_method = resource
                        .methods
                        .get("get")
                        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                    let get_args = json!({"params": {"documentId": doc_id}});
                    let doc_content = crate::execute::execute_tool(
                        &docs_doc,
                        get_method,
                        "documents",
                        "get",
                        &get_args,
                        "docs",
                        policy,
                        meta,
                        None,
                        false,
                        &mut state.token_cache,
                    )
                    .await?;
                    doc_content["body"]["content"]
                        .as_array()
                        .and_then(|arr| arr.last())
                        .and_then(|el| el["endIndex"].as_i64())
                        .map(|idx| (idx - 1) as i32)
                        .unwrap_or(1)
                }
            }
        };
        (None, idx)
    };

    // Step D: execute batchUpdate(s)
    let docs_doc = state.get_doc("docs").await?;
    let resource = tools::find_resource(&docs_doc.resources, "documents")
        .ok_or_else(|| GwsError::Validation("documents resource not found in docs API".into()))?;
    let batch_method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;

    if let Some(style_reqs) = template_requests {
        let style_args = json!({
            "params": { "documentId": doc_id },
            "body": { "requests": style_reqs }
        });
        crate::execute::execute_tool(
            &docs_doc,
            batch_method,
            "documents",
            "batchUpdate",
            &style_args,
            "docs",
            policy,
            meta,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await?;
    }

    let mut content_requests: Vec<Value> = Vec::new();
    if let Some(delete_req) = section_delete {
        content_requests.push(delete_req);
    }
    content_requests.extend(helpers::markdown_to_batch_requests(markdown, insert_index));

    let content_args = json!({
        "params": { "documentId": doc_id },
        "body": { "requests": content_requests }
    });

    let mut result = crate::execute::execute_tool(
        &docs_doc,
        batch_method,
        "documents",
        "batchUpdate",
        &content_args,
        "docs",
        policy,
        meta,
        None,
        dry_run,
        &mut state.token_cache,
    )
    .await?;

    // Step E: include doc ID in result (especially useful when a new doc was created)
    if created_new_doc {
        result["created_document_id"] = json!(doc_id);
    }
    result["document_id"] = json!(doc_id);

    Ok(result)
}

const MAX_BATCH_SIZE: usize = 100;

async fn execute_batch(
    service: &str,
    requests: &[Value],
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    if requests.is_empty() {
        return Err(GwsError::Validation(
            "Batch 'requests' array is empty".to_string(),
        ));
    }
    if requests.len() > MAX_BATCH_SIZE {
        return Err(GwsError::Validation(format!(
            "Batch size {} exceeds maximum of {MAX_BATCH_SIZE}",
            requests.len()
        )));
    }

    if !policy.is_service_allowed(service) {
        return Err(GwsError::Validation(
            "Operation not allowed by policy".to_string(),
        ));
    }

    let doc = state.get_doc(service).await?;

    let mut policy_errors: Vec<Value> = Vec::new();
    for (i, req) in requests.iter().enumerate() {
        let resource_path = req.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let method_name = req.get("method").and_then(|v| v.as_str()).unwrap_or("");

        let resource = match tools::find_resource(&doc.resources, resource_path) {
            Some(r) => r,
            None => {
                policy_errors.push(json!({
                    "index": i,
                    "error": format!("Resource '{resource_path}' not found in {service}")
                }));
                continue;
            }
        };
        let method = match resource.methods.get(method_name) {
            Some(m) => m,
            None => {
                policy_errors.push(json!({
                    "index": i,
                    "error": format!("Method '{method_name}' not found in {service}.{resource_path}")
                }));
                continue;
            }
        };

        let mut params = req
            .get("params")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let body = req
            .get("body")
            .filter(|v| !v.as_object().is_some_and(|m| m.is_empty()))
            .cloned();

        if let Err(e) = policy.check_method(service, resource_path, method_name, method) {
            policy_errors.push(json!({"index": i, "error": e.to_string()}));
            continue;
        }
        if let Err(e) = policy.enforce_constraints(service, method, &mut params, &body) {
            policy_errors.push(json!({"index": i, "error": e.to_string()}));
        }
    }

    if !policy_errors.is_empty() {
        return Err(GwsError::Validation(format!(
            "Batch rejected: {} sub-request(s) failed policy validation: {}",
            policy_errors.len(),
            serde_json::to_string(&policy_errors).unwrap_or_default()
        )));
    }

    let audit = state.audit.clone();
    let mut results: Vec<Value> = Vec::new();
    let mut succeeded = 0u32;
    let mut failed = 0u32;

    for (i, req) in requests.iter().enumerate() {
        let resource_path = req.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let method_name = req.get("method").and_then(|v| v.as_str()).unwrap_or("");

        let resource = tools::find_resource(&doc.resources, resource_path).unwrap();
        let method = resource.methods.get(method_name).unwrap();

        let sub_args = json!({
            "resource": resource_path,
            "method": method_name,
            "params": req.get("params").unwrap_or(&json!({})),
            "body": req.get("body").unwrap_or(&json!({}))
        });

        let exec_start = std::time::Instant::now();
        let exec_result = crate::execute::execute_tool(
            &doc,
            method,
            resource_path,
            method_name,
            &sub_args,
            service,
            policy,
            meta,
            None,
            false,
            &mut state.token_cache,
        )
        .await;
        let duration_ms = exec_start.elapsed().as_millis() as u64;

        match exec_result {
            Ok(result) => {
                if let Some(ref a) = audit {
                    a.log_allowed(
                        service,
                        resource_path,
                        method_name,
                        &method.http_method,
                        0,
                        duration_ms,
                    );
                }
                let text =
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
                results.push(json!({
                    "index": i,
                    "status": "success",
                    "result": text
                }));
                succeeded += 1;
            }
            Err(e) => {
                if let Some(ref a) = audit {
                    a.log_denied(service, resource_path, method_name, &e.to_string());
                }
                results.push(json!({
                    "index": i,
                    "status": "error",
                    "error": e.to_string()
                }));
                failed += 1;
            }
        }
    }

    let total = succeeded + failed;
    let summary_text =
        format!("Batch complete: {succeeded}/{total} succeeded, {failed}/{total} failed");

    Ok(json!({
        "content": [{ "type": "text", "text": summary_text }],
        "structuredContent": {
            "batch_results": results,
            "summary": {
                "total": total,
                "succeeded": succeeded,
                "failed": failed
            }
        },
        "isError": false
    }))
}

fn is_policy_denial(msg: &str) -> bool {
    msg.contains("not allowed by policy")
        || msg.contains("denied by policy")
        || msg.contains("is read-only;")
        || msg.contains("Write denied")
}

async fn handle_tool_call_inner(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &mut ServerState,
) -> Result<(Value, Vec<Value>), GwsError> {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'name' in tools/call".to_string()))?;

    tracing::info!(tool = tool_name, "Tool call");

    let default_args = json!({});
    let raw_arguments = params.get("arguments").unwrap_or(&default_args);
    let dry_run = raw_arguments
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let arguments = &strip_key(raw_arguments, "dry_run");

    if tool_name == "gws_discover" {
        let result = tools::handle_discover(arguments, policy, &mut state.docs).await?;
        return Ok((result, vec![]));
    }

    if tool_name == "gws_batch" {
        let service = arguments
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GwsError::Validation("Missing 'service' in gws_batch".to_string()))?;
        let requests = arguments
            .get("requests")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                GwsError::Validation("Missing 'requests' array in gws_batch".to_string())
            })?;
        let result = execute_batch(service, requests, policy, meta, state).await?;
        return Ok((result, vec![]));
    }

    if tool_name.starts_with("gws_docs_") {
        let result =
            execute_docs_helper(tool_name, arguments, policy, meta, state, dry_run).await?;
        return Ok((result, vec![]));
    }

    let task_id = arguments
        .get("upload_handle")
        .or_else(|| arguments.get("download_handle"))
        .or_else(|| arguments.get("task_id"))
        .and_then(|v| v.as_str());
    if let Some(tid) = task_id {
        let result = handle_task_chunk(tid, arguments, state).await?;
        return Ok((result, vec![]));
    }

    let svc_alias = tool_name;
    if !policy.is_service_allowed(svc_alias) {
        tracing::warn!(service = svc_alias, "Policy denied: service not enabled");
        return Err(GwsError::Validation(
            "Operation not allowed by policy".to_string(),
        ));
    }

    let resource_path = arguments
        .get("resource")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'resource' argument".to_string()))?;
    let method_name = arguments
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'method' argument".to_string()))?;

    let mut tc = state.token_cache.take();
    let audit = state.audit.clone();
    let doc = state.get_doc(svc_alias).await?;

    let resource = tools::find_resource(&doc.resources, resource_path).ok_or_else(|| {
        GwsError::Validation(format!(
            "Resource '{resource_path}' not found in {svc_alias}"
        ))
    })?;

    let method = resource.methods.get(method_name).ok_or_else(|| {
        GwsError::Validation(format!(
            "Method '{method_name}' not found in {svc_alias}.{resource_path}"
        ))
    })?;

    if arguments
        .get("media_upload_init")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let init_result = crate::execute::initiate_resumable_upload(
            &doc, method, arguments, svc_alias, policy, meta, &mut tc,
        )
        .await;
        state.token_cache = tc;
        let init_result = init_result?;

        let session_uri = extract_session_uri(&init_result)?;
        let total_size = arguments
            .get("media_total_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let content_type = arguments
            .get("media_content_type")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();

        let handle = format!(
            "upload_{:016x}",
            crate::execute::simple_hash(session_uri.as_bytes())
        );

        let result = create_upload_task(state, &handle, session_uri, total_size, content_type);
        return Ok((result, vec![]));
    }

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    let exec_start = Instant::now();
    let result = crate::execute::execute_tool(
        &doc,
        method,
        resource_path,
        method_name,
        arguments,
        svc_alias,
        policy,
        meta,
        Some(&notify_tx),
        dry_run,
        &mut tc,
    )
    .await;
    let duration_ms = exec_start.elapsed().as_millis() as u64;

    match &result {
        Ok(_) => {
            if let Some(ref a) = audit {
                a.log_allowed(
                    svc_alias,
                    resource_path,
                    method_name,
                    &method.http_method,
                    0,
                    duration_ms,
                );
            }
        }
        Err(e) => {
            if let Some(ref a) = audit {
                a.log_denied(svc_alias, resource_path, method_name, &e.to_string());
            }
        }
    }
    let result = result?;

    drop(notify_tx);
    let mut notifications = Vec::new();
    while let Ok(notification) = notify_rx.try_recv() {
        notifications.push(notification);
    }

    let mcp_result = format_execute_result(
        result,
        method,
        svc_alias,
        resource_path,
        method_name,
        arguments,
        &doc,
        policy,
        meta,
        &mut tc,
        state,
    )
    .await?;

    state.token_cache = tc;
    Ok((mcp_result, notifications))
}

fn explain_request(
    service: &str,
    resource: &str,
    method_name: &str,
    method: &google_workspace::discovery::RestMethod,
    arguments: &Value,
) -> String {
    let verb = match method.http_method.as_str() {
        "POST" => "Create",
        "PUT" => "Replace",
        "PATCH" => "Update",
        "DELETE" => "Delete",
        _ => "Modify",
    };

    let params = arguments
        .get("params")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let body = arguments.get("body");

    let mut details = Vec::new();

    // Extract key identifiers from params
    for key in [
        "fileId",
        "messageId",
        "eventId",
        "spreadsheetId",
        "documentId",
        "presentationId",
    ] {
        if let Some(Value::String(val)) = params.get(key) {
            details.push(format!("{key}={val}"));
        }
    }

    // Extract names and subjects from body
    if let Some(b) = body {
        if let Some(Value::String(name)) = b.get("name") {
            details.push(format!("name=\"{name}\""));
        }
        if let Some(Value::String(subj)) = b.get("subject") {
            details.push(format!("subject=\"{subj}\""));
        }
        if let Some(Value::String(summary)) = b.get("summary") {
            details.push(format!("summary=\"{summary}\""));
        }
        if let Some(Value::Array(parents)) = b.get("parents") {
            let ids: Vec<&str> = parents.iter().filter_map(|v| v.as_str()).collect();
            if !ids.is_empty() {
                details.push(format!("in folder {}", ids.join(", ")));
            }
        }
        if let Some(Value::Array(to)) = b.get("to") {
            let addrs: Vec<&str> = to.iter().filter_map(|v| v.as_str()).collect();
            if !addrs.is_empty() {
                details.push(format!("to {}", addrs.join(", ")));
            }
        }
    }

    // Calendar-specific
    if let Some(Value::String(cal)) = params.get("calendarId") {
        details.push(format!("on calendar \"{cal}\""));
    }

    let detail_str = if details.is_empty() {
        String::new()
    } else {
        format!(": {}", details.join(", "))
    };

    format!(
        "{verb} {service}/{resource}.{method_name} ({}){detail_str}",
        method.http_method
    )
}

const DOWNLOAD_CHUNK_RAW_SIZE: usize = 10 * 1024 * 1024;

async fn handle_task_chunk(
    task_id: &str,
    arguments: &Value,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let task = state
        .tasks
        .get(task_id)
        .ok_or_else(|| GwsError::Validation(format!("Task '{task_id}' not found or expired")))?;

    match &task.kind {
        tasks::TaskKind::Upload(_) => handle_upload_chunk(task_id, arguments, state).await,
        tasks::TaskKind::Download(_) => handle_download_chunk(task_id, arguments, state),
        tasks::TaskKind::Generic => Err(GwsError::Validation(format!(
            "Task '{task_id}' does not support chunked operations"
        ))),
    }
}

async fn handle_upload_chunk(
    task_id: &str,
    arguments: &Value,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let task = state.tasks.get(task_id).unwrap();
    let tasks::TaskKind::Upload(u) = &task.kind else {
        unreachable!()
    };
    let session_uri = u.session_uri.clone();
    let bytes_uploaded = u.bytes_uploaded;
    let total_size = u.total_size;
    let content_type = u.content_type.clone();

    let chunk_b64 = arguments
        .get("media_chunk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'media_chunk' argument".to_string()))?;

    let chunk_offset = arguments
        .get("media_chunk_offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(bytes_uploaded);

    let chunk_bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk_b64)
        .map_err(|_| GwsError::Validation("Invalid base64 in media_chunk".to_string()))?;

    let api_result = crate::execute::upload_chunk(
        &session_uri,
        &chunk_bytes,
        chunk_offset,
        total_size,
        &content_type,
    )
    .await?;

    let is_complete = api_result
        .get("complete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let new_offset = chunk_offset + chunk_bytes.len() as u64;

    complete_or_progress(
        state,
        task_id,
        is_complete,
        new_offset,
        total_size as usize,
        &api_result,
    )
}

fn handle_download_chunk(
    task_id: &str,
    arguments: &Value,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let task = state.tasks.get(task_id).unwrap();
    let tasks::TaskKind::Download(d) = &task.kind else {
        unreachable!()
    };

    let chunk_offset = arguments
        .get("download_chunk_offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let raw_len = d.raw_data.len();
    let total_size = d.total_size;
    let content_type = d.content_type.clone();

    if chunk_offset >= raw_len {
        if let Some(t) = state.tasks.get_mut(task_id) {
            t.complete(json!({"content": [{"type": "text", "text": "Download complete"}]}));
        }
        return Ok(chunk_response(
            task_id,
            "",
            0,
            0,
            raw_len,
            total_size,
            &content_type,
            true,
        ));
    }

    let end = (chunk_offset + DOWNLOAD_CHUNK_RAW_SIZE).min(raw_len);
    let chunk_b64 =
        base64::engine::general_purpose::STANDARD.encode(&d.raw_data[chunk_offset..end]);
    let is_last = end >= raw_len;

    if is_last && let Some(t) = state.tasks.get_mut(task_id) {
        t.complete(json!({"content": [{"type": "text", "text": "Download complete"}]}));
    }

    Ok(chunk_response(
        task_id,
        &chunk_b64,
        chunk_offset,
        end,
        raw_len,
        total_size,
        &content_type,
        is_last,
    ))
}

fn complete_or_progress(
    state: &mut ServerState,
    task_id: &str,
    is_complete: bool,
    new_offset: u64,
    total_size: usize,
    api_result: &Value,
) -> Result<Value, GwsError> {
    if is_complete {
        let text = serde_json::to_string_pretty(api_result).unwrap_or_else(|_| "{}".to_string());
        if let Some(t) = state.tasks.get_mut(task_id) {
            t.complete(json!({
                "content": [{ "type": "text", "text": text }],
                "structuredContent": api_result
            }));
        }
        Ok(json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": api_result,
            "isError": false
        }))
    } else {
        if let Some(t) = state.tasks.get_mut(task_id) {
            if let tasks::TaskKind::Upload(ref mut u) = t.kind {
                u.bytes_uploaded = new_offset;
            }
            t.status_message = format!("{new_offset} of {total_size} bytes");
            t.updated_at = Instant::now();
        }
        Ok(json!({
            "content": [{ "type": "text", "text": format!("Transferred {new_offset} of {total_size} bytes") }],
            "structuredContent": {
                "taskId": task_id,
                "bytes_transferred": new_offset,
                "total_size": total_size,
                "status": "working"
            },
            "isError": false
        }))
    }
}

#[allow(clippy::too_many_arguments)]
fn chunk_response(
    task_id: &str,
    chunk_data: &str,
    offset: usize,
    end: usize,
    b64_len: usize,
    total_size: usize,
    content_type: &str,
    is_last: bool,
) -> Value {
    let status = if is_last { "complete" } else { "working" };
    let pct = (end * 100).checked_div(b64_len).unwrap_or(100);
    json!({
        "content": [{ "type": "text", "text": format!("{pct}% ({end}/{b64_len} base64 chars)") }],
        "structuredContent": {
            "taskId": task_id,
            "chunk_data": chunk_data,
            "chunk_offset": offset,
            "next_offset": end,
            "total_b64_size": b64_len,
            "total_size": total_size,
            "content_type": content_type,
            "is_last": is_last,
            "status": status
        },
        "isError": false
    })
}

fn server_info() -> Value {
    json!({
        "name": "mcp-google-workspace",
        "version": env!("CARGO_PKG_VERSION")
    })
}

fn server_capabilities() -> Value {
    json!({
        "tools": {},
        "prompts": {},
        "extensions": {
            "io.modelcontextprotocol/tasks": {}
        }
    })
}

fn server_instructions() -> &'static str {
    "MCP server for Google Workspace APIs with per-project safety policies. \
     Use gws_discover to explore available services, resources, and methods. \
     Each enabled Google service is exposed as a tool."
}

const SUPPORTED_VERSIONS: &[&str] = &["2026-07-28", "2025-11-25", "2024-11-05"];

fn negotiate_version(client_version: &str) -> &'static str {
    SUPPORTED_VERSIONS
        .iter()
        .find(|&&v| v == client_version)
        .copied()
        .unwrap_or(SUPPORTED_VERSIONS.last().unwrap())
}

const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;
const TOOLS_PAGE_SIZE: usize = 50;
const TOOLS_TTL_MS: u64 = 300_000;

fn paginate_tools(tools: &[Value], cursor: Option<&str>) -> Result<Value, GwsError> {
    let start = match cursor {
        None => 0,
        Some(c) => {
            let decoded = String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(c)
                    .map_err(|_| GwsError::Validation("Invalid cursor".to_string()))?,
            )
            .map_err(|_| GwsError::Validation("Invalid cursor".to_string()))?;
            decoded
                .parse::<usize>()
                .map_err(|_| GwsError::Validation("Invalid cursor".to_string()))?
        }
    };

    if cursor.is_some() && start >= tools.len() {
        return Err(GwsError::Validation("Invalid cursor".to_string()));
    }

    let end = (start + TOOLS_PAGE_SIZE).min(tools.len());
    let page = &tools[start..end];

    let mut result = json!({
        "tools": page,
        "ttlMs": TOOLS_TTL_MS,
        "cacheScope": "instance"
    });

    if end < tools.len() {
        let next = base64::engine::general_purpose::STANDARD.encode(end.to_string().as_bytes());
        result["nextCursor"] = json!(next);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::RequestMeta;

    #[test]
    fn test_detect_era_initialize_sets_legacy() {
        let mut era = ClientEra::Unknown;
        let meta = RequestMeta::default();
        detect_era(&mut era, "initialize", &meta);
        assert!(matches!(era, ClientEra::Legacy { initialized: false }));
    }

    #[test]
    fn test_detect_era_modern_meta_sets_modern() {
        let mut era = ClientEra::Unknown;
        let params = json!({
            "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
        });
        let meta = RequestMeta::from_params(&params);
        detect_era(&mut era, "tools/list", &meta);
        assert!(matches!(era, ClientEra::Modern));
    }

    #[test]
    fn test_detect_era_server_discover_sets_modern() {
        let mut era = ClientEra::Unknown;
        let meta = RequestMeta::default();
        detect_era(&mut era, "server/discover", &meta);
        assert!(matches!(era, ClientEra::Modern));
    }

    #[test]
    fn test_detect_era_initialized_notification() {
        let mut era = ClientEra::Legacy { initialized: false };
        let meta = RequestMeta::default();
        detect_era(&mut era, "notifications/initialized", &meta);
        assert!(matches!(era, ClientEra::Legacy { initialized: true }));
    }

    #[test]
    fn test_detect_era_unknown_stays_unknown_on_random_method() {
        let mut era = ClientEra::Unknown;
        let meta = RequestMeta::default();
        detect_era(&mut era, "tools/list", &meta);
        assert!(matches!(era, ClientEra::Unknown));
    }

    #[test]
    fn test_pre_init_blocks_tools_list_in_unknown() {
        let resp = check_pre_init(&ClientEra::Unknown, "tools/list", &json!(1));
        assert!(resp.is_some());
    }

    #[test]
    fn test_pre_init_allows_initialize_in_unknown() {
        let resp = check_pre_init(&ClientEra::Unknown, "initialize", &json!(1));
        assert!(resp.is_none());
    }

    #[test]
    fn test_pre_init_allows_server_discover_in_unknown() {
        let resp = check_pre_init(&ClientEra::Unknown, "server/discover", &json!(1));
        assert!(resp.is_none());
    }

    #[test]
    fn test_pre_init_allows_ping_in_unknown() {
        let resp = check_pre_init(&ClientEra::Unknown, "ping", &json!(1));
        assert!(resp.is_none());
    }

    #[test]
    fn test_pre_init_blocks_tools_call_before_initialized() {
        let era = ClientEra::Legacy { initialized: false };
        let resp = check_pre_init(&era, "tools/call", &json!(1));
        assert!(resp.is_some());
    }

    #[test]
    fn test_pre_init_allows_tools_list_after_initialized() {
        let era = ClientEra::Legacy { initialized: true };
        let resp = check_pre_init(&era, "tools/list", &json!(1));
        assert!(resp.is_none());
    }

    #[test]
    fn test_pre_init_allows_all_in_modern() {
        let resp = check_pre_init(&ClientEra::Modern, "tools/call", &json!(1));
        assert!(resp.is_none());
    }

    #[test]
    fn test_negotiate_known_version() {
        assert_eq!(negotiate_version("2026-07-28"), "2026-07-28");
        assert_eq!(negotiate_version("2025-11-25"), "2025-11-25");
        assert_eq!(negotiate_version("2024-11-05"), "2024-11-05");
    }

    #[test]
    fn test_negotiate_unknown_version_falls_back() {
        assert_eq!(negotiate_version("1999-01-01"), "2024-11-05");
    }

    #[test]
    fn test_paginate_no_cursor_returns_all_small_list() {
        let tools = vec![json!({"name": "a"}), json!({"name": "b"})];
        let result = paginate_tools(&tools, None).unwrap();
        assert_eq!(result["tools"].as_array().unwrap().len(), 2);
        assert!(result.get("nextCursor").is_none());
        assert_eq!(result["ttlMs"], TOOLS_TTL_MS);
        assert_eq!(result["cacheScope"], "instance");
    }

    #[test]
    fn test_paginate_with_cursor_roundtrip() {
        let tools: Vec<Value> = (0..60)
            .map(|i| json!({"name": format!("tool-{i}")}))
            .collect();
        let page1 = paginate_tools(&tools, None).unwrap();
        assert_eq!(page1["tools"].as_array().unwrap().len(), 50);
        let cursor = page1["nextCursor"].as_str().unwrap();

        let page2 = paginate_tools(&tools, Some(cursor)).unwrap();
        assert_eq!(page2["tools"].as_array().unwrap().len(), 10);
        assert!(page2.get("nextCursor").is_none());
    }

    #[test]
    fn test_paginate_invalid_cursor() {
        let tools = vec![json!({"name": "a"})];
        let result = paginate_tools(&tools, Some("!!!invalid!!!"));
        assert!(result.is_err());
    }

    #[test]
    fn test_track_id_unique() {
        let mut state = ServerState::new();
        assert!(state.track_id(&json!(1)).is_ok());
        assert!(state.track_id(&json!(2)).is_ok());
    }

    #[test]
    fn test_track_id_duplicate_rejected() {
        let mut state = ServerState::new();
        assert!(state.track_id(&json!(1)).is_ok());
        assert!(state.track_id(&json!(1)).is_err());
    }

    #[test]
    fn test_track_id_released_can_reuse() {
        let mut state = ServerState::new();
        assert!(state.track_id(&json!(1)).is_ok());
        state.release_id(&json!(1));
        assert!(state.track_id(&json!(1)).is_ok());
    }

    #[test]
    fn test_server_info_shape() {
        let info = server_info();
        assert_eq!(info["name"], "mcp-google-workspace");
        assert!(info["version"].as_str().is_some());
    }

    #[test]
    fn test_supported_versions_not_empty() {
        let versions = SUPPORTED_VERSIONS;
        assert!(versions.contains(&"2026-07-28"));
        assert!(versions.contains(&"2024-11-05"));
    }

    #[test]
    fn test_task_cleanup_removes_expired() {
        let mut state = ServerState::new();
        let old_task = tasks::Task::new("old".to_string(), 0, tasks::TaskKind::Generic);
        std::thread::sleep(std::time::Duration::from_millis(1));
        state.tasks.insert("old".to_string(), old_task);
        state.tasks.insert(
            "recent".to_string(),
            tasks::Task::new("recent".to_string(), 3_600_000, tasks::TaskKind::Generic),
        );
        state.clean_expired_sessions();
        assert!(!state.tasks.contains_key("old"));
        assert!(state.tasks.contains_key("recent"));
    }

    #[test]
    fn test_download_chunk_lifecycle() {
        let mut state = ServerState::new();
        let raw_data = vec![0x41u8; 75];
        state.tasks.insert(
            "dl1".to_string(),
            tasks::Task::new(
                "dl1".to_string(),
                3_600_000,
                tasks::TaskKind::Download(tasks::DownloadData {
                    raw_data,
                    content_type: "application/pdf".to_string(),
                    total_size: 75,
                }),
            ),
        );

        let args = json!({ "download_chunk_offset": 0 });
        let result = handle_download_chunk("dl1", &args, &mut state).unwrap();
        assert_eq!(result["structuredContent"]["status"], "complete");
        assert!(result["structuredContent"]["is_last"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_task_chunk_unknown_handle() {
        let mut state = ServerState::new();
        let args = json!({ "download_chunk_offset": 0 });
        assert!(
            handle_task_chunk("nonexistent", &args, &mut state)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_task_chunk_wrong_kind() {
        let mut state = ServerState::new();
        state.tasks.insert(
            "t1".to_string(),
            tasks::Task::new("t1".to_string(), 60000, tasks::TaskKind::Generic),
        );
        let args = json!({ "download_chunk_offset": 0 });
        assert!(handle_task_chunk("t1", &args, &mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_handle_request_ping() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Modern;
        let meta = RequestMeta::default();
        let (result, notifs) =
            handle_request("ping", &json!({}), &meta, &policy, &mut state, &json!(1)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), json!({}));
        assert!(notifs.is_empty());
    }

    #[tokio::test]
    async fn test_handle_request_server_discover() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Modern;
        let meta = RequestMeta::default();
        let (result, _) = handle_request(
            "server/discover",
            &json!({}),
            &meta,
            &policy,
            &mut state,
            &json!(1),
        )
        .await;
        let val = result.unwrap();
        assert!(val.get("capabilities").is_some());
        assert!(val.get("serverInfo").is_some());
        assert!(val.get("supportedVersions").is_some());
    }

    #[tokio::test]
    async fn test_handle_request_initialize() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Legacy { initialized: true };
        let meta = RequestMeta::default();
        let params = json!({"protocolVersion": "2024-11-05"});
        let (result, _) =
            handle_request("initialize", &params, &meta, &policy, &mut state, &json!(1)).await;
        let val = result.unwrap();
        assert_eq!(val["protocolVersion"], "2024-11-05");
        assert!(val.get("serverInfo").is_some());
    }

    #[tokio::test]
    async fn test_handle_request_unknown_method() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Modern;
        let meta = RequestMeta::default();
        let (result, _) = handle_request(
            "nonexistent/method",
            &json!({}),
            &meta,
            &policy,
            &mut state,
            &json!(1),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_request_tasks_get() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Modern;
        state.tasks.insert(
            "t1".to_string(),
            tasks::Task::new("t1".to_string(), 60000, tasks::TaskKind::Generic),
        );
        let meta = RequestMeta::default();
        let (result, _) = handle_request(
            "tasks/get",
            &json!({"taskId": "t1"}),
            &meta,
            &policy,
            &mut state,
            &json!(1),
        )
        .await;
        let val = result.unwrap();
        assert_eq!(val["taskId"], "t1");
        assert_eq!(val["status"], "working");
    }

    #[tokio::test]
    async fn test_handle_request_tasks_list() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Modern;
        let meta = RequestMeta::default();
        let (result, _) = handle_request(
            "tasks/list",
            &json!({}),
            &meta,
            &policy,
            &mut state,
            &json!(1),
        )
        .await;
        let val = result.unwrap();
        assert_eq!(val["tasks"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handle_request_tasks_cancel() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string()]);
        let mut state = ServerState::new();
        state.era = ClientEra::Modern;
        state.tasks.insert(
            "t1".to_string(),
            tasks::Task::new("t1".to_string(), 60000, tasks::TaskKind::Generic),
        );
        let meta = RequestMeta::default();
        let (result, _) = handle_request(
            "tasks/cancel",
            &json!({"taskId": "t1"}),
            &meta,
            &policy,
            &mut state,
            &json!(1),
        )
        .await;
        let val = result.unwrap();
        assert_eq!(val["status"], "cancelled");
    }

    #[test]
    fn test_chunk_response_shape() {
        let resp = chunk_response("dl1", "AAAA", 0, 4, 100, 75, "application/pdf", false);
        assert_eq!(resp["structuredContent"]["taskId"], "dl1");
        assert_eq!(resp["structuredContent"]["status"], "working");
        assert_eq!(resp["structuredContent"]["chunk_offset"], 0);
        assert_eq!(resp["structuredContent"]["next_offset"], 4);
        assert!(!resp["structuredContent"]["is_last"].as_bool().unwrap());
    }

    #[test]
    fn test_chunk_response_last() {
        let resp = chunk_response("dl1", "AA", 98, 100, 100, 75, "text/plain", true);
        assert_eq!(resp["structuredContent"]["status"], "complete");
        assert!(resp["structuredContent"]["is_last"].as_bool().unwrap());
    }

    #[test]
    fn test_explain_create_file() {
        let method = google_workspace::discovery::RestMethod {
            http_method: "POST".to_string(),
            ..Default::default()
        };
        let args = json!({
            "resource": "files",
            "method": "create",
            "body": { "name": "report.pdf", "parents": ["folder-123"] }
        });
        let explanation = explain_request("drive", "files", "create", &method, &args);
        assert!(explanation.contains("Create"));
        assert!(explanation.contains("drive/files.create"));
        assert!(explanation.contains("report.pdf"));
        assert!(explanation.contains("folder-123"));
    }

    #[test]
    fn test_explain_delete() {
        let method = google_workspace::discovery::RestMethod {
            http_method: "DELETE".to_string(),
            ..Default::default()
        };
        let args = json!({
            "resource": "files",
            "method": "delete",
            "params": { "fileId": "abc123" }
        });
        let explanation = explain_request("drive", "files", "delete", &method, &args);
        assert!(explanation.contains("Delete"));
        assert!(explanation.contains("fileId=abc123"));
    }

    #[test]
    fn test_explain_calendar_event() {
        let method = google_workspace::discovery::RestMethod {
            http_method: "POST".to_string(),
            ..Default::default()
        };
        let args = json!({
            "resource": "events",
            "method": "insert",
            "params": { "calendarId": "primary" },
            "body": { "summary": "Team standup" }
        });
        let explanation = explain_request("calendar", "events", "insert", &method, &args);
        assert!(explanation.contains("Create"));
        assert!(explanation.contains("Team standup"));
        assert!(explanation.contains("primary"));
    }

    #[test]
    fn test_heading_level_known() {
        assert_eq!(heading_level("HEADING_1"), Some(1));
        assert_eq!(heading_level("HEADING_3"), Some(3));
        assert_eq!(heading_level("HEADING_6"), Some(6));
    }

    #[test]
    fn test_heading_level_unknown() {
        assert_eq!(heading_level("NORMAL_TEXT"), None);
        assert_eq!(heading_level("TITLE"), None);
    }

    #[test]
    fn test_find_section_range_basic() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "Introduction\n" } }]
                    }},
                    { "startIndex": 10, "endIndex": 30, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                        "elements": [{ "textRun": { "content": "Some body text\n" } }]
                    }},
                    { "startIndex": 30, "endIndex": 45, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "Next Section\n" } }]
                    }}
                ]
            }
        });
        let range = find_section_range(&doc, "Introduction");
        assert_eq!(range, Some((1, 30)));
    }

    #[test]
    fn test_find_section_range_to_end() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_2" },
                        "elements": [{ "textRun": { "content": "Only Section\n" } }]
                    }},
                    { "startIndex": 10, "endIndex": 50, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                        "elements": [{ "textRun": { "content": "Content goes here\n" } }]
                    }}
                ]
            }
        });
        let range = find_section_range(&doc, "Only Section");
        assert_eq!(range, Some((1, 50)));
    }

    #[test]
    fn test_find_section_range_not_found() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "Existing\n" } }]
                    }}
                ]
            }
        });
        assert!(find_section_range(&doc, "Missing").is_none());
    }

    #[test]
    fn test_find_section_range_subsection_not_terminated_by_lower() {
        let doc = json!({
            "body": {
                "content": [
                    { "startIndex": 1, "endIndex": 10, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_2" },
                        "elements": [{ "textRun": { "content": "Parent\n" } }]
                    }},
                    { "startIndex": 10, "endIndex": 20, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_3" },
                        "elements": [{ "textRun": { "content": "Child\n" } }]
                    }},
                    { "startIndex": 20, "endIndex": 30, "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_2" },
                        "elements": [{ "textRun": { "content": "Sibling\n" } }]
                    }}
                ]
            }
        });
        // H2 "Parent" should include the H3 child, stopping at the next H2
        let range = find_section_range(&doc, "Parent");
        assert_eq!(range, Some((1, 20)));
    }

    #[test]
    fn test_explain_get_no_explanation() {
        let method = google_workspace::discovery::RestMethod {
            http_method: "GET".to_string(),
            ..Default::default()
        };
        let args = json!({
            "resource": "files",
            "method": "list"
        });
        let explanation = explain_request("drive", "files", "list", &method, &args);
        assert!(explanation.contains("Modify"));
    }
}
