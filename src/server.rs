use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use google_workspace::discovery::RestDescription;
use google_workspace::error::GwsError;

use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::protocol::{self, JsonRpcResponse};
use crate::tasks;
use crate::tools;

pub(crate) struct ServerState {
    pub tools_json: Option<Vec<Value>>,
    pub docs: HashMap<String, RestDescription>,
    pub era: ClientEra,
    pub active_ids: HashSet<String>,
    pub tasks: HashMap<String, tasks::Task>,
    pub token_cache: Option<crate::auth::TokenCache>,
    pub audit: Option<Arc<crate::audit::AuditLogger>>,
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

    pub(crate) async fn get_doc(&mut self, svc_alias: &str) -> Result<&RestDescription, GwsError> {
        tools::get_or_fetch_doc(&mut self.docs, svc_alias).await
    }

    pub(crate) fn clean_expired_sessions(&mut self) {
        tasks::clean_expired_tasks(&mut self.tasks);
    }
}

pub async fn run_stdio(
    policy: Policy,
    audit: Option<Arc<crate::audit::AuditLogger>>,
) -> Result<(), GwsError> {
    let svc_list = policy.allowed_services();
    if svc_list.is_empty() {
        tracing::warn!("No services configured. Zero tools will be exposed.");
    } else {
        tracing::info!(services = %svc_list.join(", "), "Starting MCP server");
    }

    let mut state = ServerState::new();
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

        let req = match protocol::parse_request(&line) {
            Ok(r) => r,
            Err(resp) => {
                write_stdio_response(&mut stdout, &resp).await;
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
            write_stdio_response(&mut stdout, &resp).await;
            continue;
        }

        let meta = RequestMeta::from_params(&req.params);
        detect_era(&mut state.era, &req.method, &meta);

        if let Some(resp) = check_pre_init(&state.era, &req.method, id) {
            state.release_id(id);
            write_stdio_response(&mut stdout, &resp).await;
            continue;
        }

        let (response, notifications) =
            handle_request(&req.method, &req.params, &meta, &policy, &mut state).await;

        for notif in &notifications {
            let n = JsonRpcResponse::Notification(notif.clone());
            write_stdio_response(&mut stdout, &n).await;
        }

        let resp = match response {
            Ok(result) => protocol::success(id, result),
            Err(err_resp) => err_resp,
        };

        state.release_id(id);
        write_stdio_response(&mut stdout, &resp).await;
    }

    Ok(())
}

pub async fn run_http(
    policy: Arc<Policy>,
    addr: &str,
    audit: Option<Arc<crate::audit::AuditLogger>>,
) -> Result<(), GwsError> {
    let mut s = ServerState::new();
    s.audit = audit;
    s.era = ClientEra::Modern;
    let state = Arc::new(Mutex::new(s));

    crate::http::serve(policy, state, addr).await
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

async fn write_stdio_response(stdout: &mut tokio::io::Stdout, resp: &JsonRpcResponse) {
    let json = resp.to_json();
    let Ok(mut out) = serde_json::to_string(&json) else {
        tracing::error!("Failed to serialize JSON-RPC response");
        return;
    };
    out.push('\n');
    let _ = stdout.write_all(out.as_bytes()).await;
    let _ = stdout.flush().await;
}

pub(crate) async fn handle_request(
    method: &str,
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &mut ServerState,
) -> (Result<Value, JsonRpcResponse>, Vec<Value>) {
    let start = Instant::now();
    let dummy_id = json!(null);

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
                        return (
                            Err(protocol::internal_error(&dummy_id, &e.to_string())),
                            vec![],
                        );
                    }
                }
            }
            let all_tools = state.tools_json.as_ref().unwrap();
            let cursor = params.get("cursor").and_then(|v| v.as_str());
            paginate_tools(all_tools, cursor)
                .map_err(|e| protocol::invalid_params(&dummy_id, &e.to_string()))
        }

        "tools/call" => {
            let (result, notifications) = handle_tool_call(params, meta, policy, state).await;
            let mapped = result.map_err(|e| protocol::internal_error(&dummy_id, &e.to_string()));
            crate::metrics::record_request(method, mapped.is_err(), start.elapsed().as_secs_f64());
            crate::metrics::set_active_tasks(state.tasks.len() as i64);
            return (mapped, notifications);
        }

        "tasks/get" => tasks::handle_tasks_get(params, &state.tasks)
            .map_err(|e| protocol::invalid_params(&dummy_id, &e.to_string())),

        "tasks/result" => tasks::handle_tasks_result(params, &state.tasks)
            .map_err(|e| protocol::invalid_params(&dummy_id, &e.to_string())),

        "tasks/cancel" => tasks::handle_tasks_cancel(params, &mut state.tasks)
            .map_err(|e| protocol::invalid_params(&dummy_id, &e.to_string())),

        "tasks/list" => tasks::handle_tasks_list(params, &state.tasks)
            .map_err(|e| protocol::internal_error(&dummy_id, &e.to_string())),

        _ => Err(protocol::method_not_found(&dummy_id, method)),
    };

    crate::metrics::record_request(method, result.is_err(), start.elapsed().as_secs_f64());
    crate::metrics::set_active_tasks(state.tasks.len() as i64);
    (result, vec![])
}

async fn handle_tool_call(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &mut ServerState,
) -> (Result<Value, GwsError>, Vec<Value>) {
    match handle_tool_call_inner(params, meta, policy, state).await {
        Ok((result, notifications)) => (Ok(result), notifications),
        Err(e) => (Err(e), vec![]),
    }
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
    let arguments = params.get("arguments").unwrap_or(&default_args);

    if tool_name == "gws_discover" {
        let result = tools::handle_discover(arguments, policy, &mut state.docs).await?;
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
        return Err(GwsError::Validation(format!(
            "Service '{svc_alias}' is not enabled"
        )));
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
            doc, method, arguments, svc_alias, policy, meta, &mut tc,
        )
        .await;
        state.token_cache = tc;
        let init_result = init_result?;

        let session_uri = init_result["sessionUri"]
            .as_str()
            .ok_or_else(|| {
                GwsError::Other(anyhow::anyhow!("No session URI in upload init response"))
            })?
            .to_string();

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
                    content_type,
                }),
            ),
        );

        let result = json!({
            "content": [{ "type": "text", "text": format!("Upload session started. Handle: {handle}. Send chunks with upload_handle + media_chunk.") }],
            "structuredContent": {
                "upload_handle": handle,
                "total_size": total_size,
                "status": "initiated"
            },
            "isError": false
        });
        return Ok((result, vec![]));
    }

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    let exec_start = Instant::now();
    let result = crate::execute::execute_tool(
        doc,
        method,
        resource_path,
        method_name,
        arguments,
        svc_alias,
        policy,
        meta,
        Some(&notify_tx),
        false,
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
                    200,
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

    let mcp_result = if let Some(mcp_content) = result.get("_mcp_content") {
        json!({
            "content": mcp_content,
            "isError": false
        })
    } else if let Some(dl) = result.get("_mcp_large_download") {
        let b64_data = dl["b64_data"].as_str().unwrap_or("").to_string();
        let content_type = dl["content_type"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string();
        let total_size = dl["total_size"].as_u64().unwrap_or(0) as usize;

        let handle = format!(
            "download_{:016x}",
            crate::execute::simple_hash(b64_data.as_bytes())
        );

        state.clean_expired_sessions();
        state.tasks.insert(
            handle.clone(),
            tasks::Task::new(
                handle.clone(),
                3_600_000,
                tasks::TaskKind::Download(tasks::DownloadData {
                    b64_data,
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
    } else if let Some(ar) = result.get("_mcp_auto_resumable") {
        let total_size = ar["total_size"].as_u64().unwrap_or(0);
        let content_type = ar["content_type"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string();

        let init_result = crate::execute::initiate_resumable_upload(
            doc, method, arguments, svc_alias, policy, meta, &mut tc,
        )
        .await?;

        let session_uri = init_result["sessionUri"]
            .as_str()
            .ok_or_else(|| {
                GwsError::Other(anyhow::anyhow!("No session URI in upload init response"))
            })?
            .to_string();

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
    } else {
        let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
        json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": result,
            "isError": false
        })
    };

    state.token_cache = tc;
    Ok((mcp_result, notifications))
}

const DOWNLOAD_CHUNK_B64_SIZE: usize = 10 * 1024 * 1024 * 4 / 3 + 4;

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

    let b64_len = d.b64_data.len();
    let total_size = d.total_size;
    let content_type = d.content_type.clone();

    if chunk_offset >= b64_len {
        if let Some(t) = state.tasks.get_mut(task_id) {
            t.complete(json!({"content": [{"type": "text", "text": "Download complete"}]}));
        }
        return Ok(chunk_response(
            task_id,
            "",
            0,
            0,
            b64_len,
            total_size,
            &content_type,
            true,
        ));
    }

    let end = (chunk_offset + DOWNLOAD_CHUNK_B64_SIZE).min(b64_len);
    let chunk = d.b64_data[chunk_offset..end].to_string();
    let is_last = end >= b64_len;

    if is_last && let Some(t) = state.tasks.get_mut(task_id) {
        t.complete(json!({"content": [{"type": "text", "text": "Download complete"}]}));
    }

    Ok(chunk_response(
        task_id,
        &chunk,
        chunk_offset,
        end,
        b64_len,
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

    if start > tools.len() {
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
        let b64_data = "A".repeat(100);
        state.tasks.insert(
            "dl1".to_string(),
            tasks::Task::new(
                "dl1".to_string(),
                3_600_000,
                tasks::TaskKind::Download(tasks::DownloadData {
                    b64_data,
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
        let (result, notifs) = handle_request("ping", &json!({}), &meta, &policy, &mut state).await;
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
        let (result, _) =
            handle_request("server/discover", &json!({}), &meta, &policy, &mut state).await;
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
        let (result, _) = handle_request("initialize", &params, &meta, &policy, &mut state).await;
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
        let (result, _) =
            handle_request("nonexistent/method", &json!({}), &meta, &policy, &mut state).await;
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
        let (result, _) =
            handle_request("tasks/list", &json!({}), &meta, &policy, &mut state).await;
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
}
