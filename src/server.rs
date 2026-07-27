use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use google_workspace::discovery::RestDescription;
use google_workspace::error::GwsError;

use crate::helpers::{self, ParagraphStyle, Position, TextStyle};
use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::tasks;
use crate::tools;

pub(crate) struct ServerState {
    pub tools: Option<Vec<rmcp::model::Tool>>,
    pub docs: HashMap<String, Arc<RestDescription>>,
    pub tasks: HashMap<String, tasks::Task>,
    pub token_cache: Option<crate::auth::TokenCache>,
    pub audit: Option<Arc<crate::audit::AuditLogger>>,
    pub prompts: Vec<crate::prompts::Prompt>,
    pub subscriptions: Arc<tokio::sync::Mutex<crate::subscriptions::SubscriptionMap>>,
    pub webhook_url: Option<String>,
    pub sheet_cache: crate::cache::SheetCache,
    pub activated_services: std::collections::HashSet<String>,
    pub eager_tools: bool,
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self {
            tools: None,
            docs: HashMap::new(),
            tasks: HashMap::new(),
            token_cache: None,
            audit: None,
            prompts: Vec::new(),
            subscriptions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            webhook_url: None,
            sheet_cache: crate::cache::SheetCache::new(20, 300),
            activated_services: std::collections::HashSet::new(),
            eager_tools: false,
        }
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

pub(crate) async fn handle_tool_call_concurrent(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &Arc<Mutex<ServerState>>,
    peer: Option<&rmcp::Peer<rmcp::RoleServer>>,
    progress_token: Option<&rmcp::model::ProgressToken>,
) -> Result<Value, GwsError> {
    match handle_tool_call_inner_concurrent(params, meta, policy, state, peer, progress_token).await
    {
        Ok(result) => Ok(result),
        Err(e) => {
            let msg = e.to_string();
            if is_policy_denial(&msg) {
                tracing::warn!(reason = %msg, "Policy denied tool call");
                Err(GwsError::Validation(
                    "Operation not allowed by policy".to_string(),
                ))
            } else {
                Err(e)
            }
        }
    }
}

async fn handle_tool_call_inner_concurrent(
    params: &Value,
    meta: &RequestMeta,
    policy: &Policy,
    state: &Arc<Mutex<ServerState>>,
    peer: Option<&rmcp::Peer<rmcp::RoleServer>>,
    progress_token: Option<&rmcp::model::ProgressToken>,
) -> Result<Value, GwsError> {
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
        // Activate service for lazy tool discovery
        if let Some(svc) = arguments.get("service").and_then(|v| v.as_str()) {
            if !st.eager_tools && st.activated_services.insert(svc.to_string()) {
                st.tools = None; // Force rebuild on next list_tools
                tracing::info!(service = svc, "Lazy discovery: service activated");
            }
        }
        let result = tools::handle_discover(arguments, policy, &mut st.docs).await?;
        return Ok(result);
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
        return Ok(result);
    }

    if tool_name.starts_with("gws_drive_") {
        let mut st = state.lock().await;
        let result = execute_drive_helper(tool_name, arguments, policy, meta, &mut st).await?;
        return Ok(result);
    }

    if tool_name.starts_with("gws_docs_") {
        let mut st = state.lock().await;
        let result =
            execute_docs_helper(tool_name, arguments, policy, meta, &mut st, dry_run).await?;
        return Ok(result);
    }

    if tool_name.starts_with("gws_sheets_") {
        let mut st = state.lock().await;
        let result = execute_sheets_helper(tool_name, arguments, policy, meta, &mut st).await?;
        return Ok(result);
    }

    if tool_name == "gws_templates" {
        let mut st = state.lock().await;
        let result = execute_list_templates(Some(arguments), policy, meta, &mut st).await;
        return Ok(result);
    }

    if tool_name.starts_with("gws_slides_") {
        let mut st = state.lock().await;
        let result =
            execute_slides_helper(tool_name, arguments, policy, meta, &mut st, dry_run).await?;
        return Ok(result);
    }

    if tool_name == "gws_generate_image" {
        let mut st = state.lock().await;
        let result = execute_generate_image(arguments, policy, meta, &mut st, dry_run).await?;
        return Ok(result);
    }

    let task_id = arguments
        .get("upload_handle")
        .or_else(|| arguments.get("download_handle"))
        .or_else(|| arguments.get("task_id"))
        .and_then(|v| v.as_str());
    if let Some(tid) = task_id {
        let mut st = state.lock().await;
        let result = handle_task_chunk(tid, arguments, &mut st).await?;
        return Ok(result);
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
        return Ok(result);
    }

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
        peer,
        progress_token,
        dry_run,
        &mut tc,
    )
    .await;
    let duration_ms = exec_start.elapsed().as_millis() as u64;

    match &result {
        Ok(_) => {
            if let Some(ref a) = audit {
                a.log_allowed_with_tool(
                    Some(svc_alias),
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
    Ok(mcp_result)
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

    let mut cleaned = result;
    crate::execute::strip_google_metadata(&mut cleaned);

    let summary = build_list_summary(&cleaned);
    let text = if let Some(ref summary) = summary {
        let json_str = serde_json::to_string_pretty(&cleaned).unwrap_or_else(|_| "{}".to_string());
        format!("{summary}\n\n{json_str}")
    } else {
        serde_json::to_string_pretty(&cleaned).unwrap_or_else(|_| "{}".to_string())
    };

    let mut structured = cleaned;
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

fn build_list_summary(result: &Value) -> Option<String> {
    for key in &[
        "files",
        "messages",
        "threads",
        "items",
        "drafts",
        "labels",
        "permissions",
        "revisions",
        "comments",
        "drives",
        "events",
    ] {
        if let Some(arr) = result.get(*key).and_then(|v| v.as_array()) {
            let has_more = result.get("nextPageToken").is_some();
            let more_text = if has_more { " (more available)" } else { "" };
            return Some(format!("Found {} {}{more_text}.", arr.len(), key));
        }
    }
    None
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

fn check_api_result(result: &Value) -> Result<(), GwsError> {
    if let Some(err) = result.get("error") {
        let msg = if let Some(s) = err.as_str() {
            s.to_string()
        } else {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            let status = err.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status.is_empty() {
                format!("API error {code}: {message}")
            } else {
                format!("API error {code} ({status}): {message}")
            }
        };
        return Err(GwsError::Validation(msg));
    }
    if result.get("validation_error").is_some() {
        let msg = result["errors"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e["hint"].as_str())
            .unwrap_or("Validation failed");
        return Err(GwsError::Validation(msg.to_string()));
    }
    Ok(())
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

async fn is_descendant_of(
    folder_id: &str,
    allowed_roots: &[&str],
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> bool {
    if allowed_roots.is_empty() || allowed_roots.contains(&folder_id) {
        return true;
    }
    let drive_doc = match state.get_doc("drive").await {
        Ok(d) => d,
        Err(_) => return false,
    };
    let files_resource = match tools::find_resource(&drive_doc.resources, "files") {
        Some(r) => r,
        None => return false,
    };
    let get_method = match files_resource.methods.get("get") {
        Some(m) => m,
        None => return false,
    };

    let mut current = folder_id.to_string();
    for _ in 0..10 {
        if allowed_roots.contains(&current.as_str()) {
            return true;
        }
        let args = json!({"params": {"fileId": current}, "fields": "parents"});
        match crate::execute::execute_tool(
            &drive_doc, get_method, "files", "get", &args, "drive",
            policy, meta, None, None, false, &mut state.token_cache,
        ).await {
            Ok(result) => {
                let parent = result
                    .get("parents")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str());
                match parent {
                    Some(p) => current = p.to_string(),
                    None => return false,
                }
            }
            Err(_) => return false,
        }
    }
    false
}

async fn policy_for_folder(
    folder_id: Option<&str>,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Policy, GwsError> {
    let Some(fid) = folder_id else {
        return Ok(policy.clone());
    };
    let roots = policy.recursive_parent_values("drive");
    if roots.is_empty() || roots.contains(&fid) {
        return Ok(policy.clone());
    }
    if is_descendant_of(fid, &roots, state, policy, meta).await {
        Ok(policy.with_extra_parent("drive", fid))
    } else {
        Err(GwsError::Validation(format!(
            "Folder '{fid}' is not inside an allowed root folder. \
             Allowed roots: {}",
            roots.join(", ")
        )))
    }
}

fn extract_file_id<'a>(arguments: &'a Value, param: &str) -> Result<&'a str, GwsError> {
    let id = arguments
        .get(param)
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation(format!("Missing '{param}'")))?;
    crate::drive_helpers::validate_file_id(id).map_err(GwsError::Validation)?;
    Ok(id)
}

async fn execute_drive_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let drive_doc = state.get_doc("drive").await?;
    let files_resource = tools::find_resource(&drive_doc.resources, "files")
        .ok_or_else(|| GwsError::Validation("files resource not found in drive API".into()))?;

    match tool_name {
        "gws_drive_list" => {
            let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());
            let query = arguments.get("query").and_then(|v| v.as_str());
            let file_type = arguments.get("type").and_then(|v| v.as_str());
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(20);
            let q = crate::drive_helpers::build_drive_query(folder_id, query, file_type);
            let list_method = files_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("list method not found".into()))?;
            let args = json!({
                "params": { "q": q, "pageSize": max_results, "orderBy": "modifiedTime desc" },
                "fields": "files(id,name,mimeType,modifiedTime,size,parents)"
            });
            let result = crate::execute::execute_tool(
                &drive_doc,
                list_method,
                "files",
                "list",
                &args,
                "drive",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_drive_find_folder" => {
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'name'".into()))?;
            let parent_id = arguments.get("parent_id").and_then(|v| v.as_str());
            let list_method = files_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("list method not found".into()))?;
            let mut q = format!(
                "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
                name.replace('\'', "\\'")
            );
            if let Some(pid) = parent_id {
                q.push_str(&format!(" and '{}' in parents", pid));
            }
            let args =
                json!({ "params": { "q": q, "pageSize": 5 }, "fields": "files(id,name,parents)" });
            let result = crate::execute::execute_tool(
                &drive_doc,
                list_method,
                "files",
                "list",
                &args,
                "drive",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            if result["files"].as_array().is_none_or(|a| a.is_empty()) {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!("No folder named '{}' found", name) }],
                    "isError": true
                }));
            }
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_drive_info" => {
            let file_id = extract_file_id(arguments, "file_id")?;
            let get_method = files_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
            let args = json!({
                "params": { "fileId": file_id },
                "fields": "id,name,mimeType,modifiedTime,createdTime,size,owners,sharingUser,shared,trashed,webViewLink,parents,permissions(emailAddress,role,type,domain)"
            });
            let result = crate::execute::execute_tool(
                &drive_doc,
                get_method,
                "files",
                "get",
                &args,
                "drive",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_drive_create_folder" => {
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'name'".into()))?;
            let parent_id = arguments.get("parent_id").and_then(|v| v.as_str());
            let effective_policy = policy_for_folder(parent_id, policy, meta, state).await?;
            let drive_doc = state.get_doc("drive").await?;
            let files_resource = tools::find_resource(&drive_doc.resources, "files")
                .ok_or_else(|| GwsError::Validation("files resource not found".into()))?;
            let create_method = files_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
            let mut body =
                json!({ "name": name, "mimeType": "application/vnd.google-apps.folder" });
            if let Some(pid) = parent_id {
                body["parents"] = json!([pid]);
            }
            let result = crate::execute::execute_tool(
                &drive_doc,
                create_method,
                "files",
                "create",
                &json!({ "body": body }),
                "drive",
                &effective_policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let folder_id = result["id"].as_str().unwrap_or("");
            Ok(json!({
                "content": [{ "type": "text", "text": format!("Folder '{}' created.\nfolder_id: {}", name, folder_id) }],
                "structuredContent": { "folder_id": folder_id, "name": name },
                "isError": false
            }))
        }

        "gws_drive_move" => {
            let file_id = extract_file_id(arguments, "file_id")?;
            let to_folder = arguments
                .get("to_folder_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'to_folder_id'".into()))?;
            let effective_policy = policy_for_folder(Some(to_folder), policy, meta, state).await?;
            let drive_doc = state.get_doc("drive").await?;
            let files_resource = tools::find_resource(&drive_doc.resources, "files")
                .ok_or_else(|| GwsError::Validation("files resource not found".into()))?;
            let get_method = files_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
            let file_meta = crate::execute::execute_tool(
                &drive_doc,
                get_method,
                "files",
                "get",
                &json!({"params": {"fileId": file_id}, "fields": "parents"}),
                "drive",
                &effective_policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let remove_parents = file_meta["parents"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let update_method = files_resource
                .methods
                .get("update")
                .ok_or_else(|| GwsError::Validation("update method not found".into()))?;
            crate::execute::execute_tool(
                &drive_doc, update_method, "files", "update",
                &json!({"params": {"fileId": file_id, "addParents": to_folder, "removeParents": remove_parents}}),
                "drive", &effective_policy, meta, None, None, false, &mut state.token_cache,
            ).await?;
            let verify = crate::execute::execute_tool(
                &drive_doc,
                get_method,
                "files",
                "get",
                &json!({"params": {"fileId": file_id}, "fields": "parents"}),
                "drive",
                &effective_policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let in_target = verify["parents"]
                .as_array()
                .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some(to_folder)));
            if !in_target {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!("Failed to move file to folder {}", to_folder) }],
                    "isError": true
                }));
            }
            Ok(json!({
                "content": [{ "type": "text", "text": format!("File moved to folder {}", to_folder) }],
                "isError": false
            }))
        }

        "gws_drive_share" => {
            let file_id = extract_file_id(arguments, "file_id")?;
            let email = arguments.get("email").and_then(|v| v.as_str());
            let domain = arguments.get("domain").and_then(|v| v.as_str());
            let role = arguments
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("reader");
            let perm_resource = tools::find_resource(&drive_doc.resources, "permissions")
                .ok_or_else(|| GwsError::Validation("permissions resource not found".into()))?;
            let create_method = perm_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
            let body = if let Some(e) = email {
                json!({ "role": role, "type": "user", "emailAddress": e })
            } else if let Some(d) = domain {
                json!({ "role": role, "type": "domain", "domain": d })
            } else {
                return Err(GwsError::Validation(
                    "Either 'email' or 'domain' is required".into(),
                ));
            };
            let result = crate::execute::execute_tool(
                &drive_doc,
                create_method,
                "permissions",
                "create",
                &json!({"params": {"fileId": file_id}, "body": body}),
                "drive",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let target = email.unwrap_or_else(|| domain.unwrap_or("unknown"));
            if result.get("error").is_some() {
                let msg = result["error"].as_str().unwrap_or("unknown error");
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!("Failed to share with {target}: {msg}") }],
                    "isError": true
                }));
            }
            if result.get("id").is_none() {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!("Failed to share with {target}: no permission ID returned") }],
                    "isError": true
                }));
            }
            Ok(json!({
                "content": [{ "type": "text", "text": format!("Shared with {} as {}", target, role) }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_drive_trash" => {
            let file_id = extract_file_id(arguments, "file_id")?;
            let update_method = files_resource
                .methods
                .get("update")
                .ok_or_else(|| GwsError::Validation("update method not found".into()))?;
            let result = crate::execute::execute_tool(
                &drive_doc,
                update_method,
                "files",
                "update",
                &json!({"params": {"fileId": file_id}, "body": {"trashed": true}}),
                "drive",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            if result.get("error").is_some() {
                let msg = result["error"].as_str().unwrap_or("unknown error");
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!("Failed to move file to trash: {msg}") }],
                    "isError": true
                }));
            }
            Ok(json!({
                "content": [{ "type": "text", "text": format!("File {} moved to trash", file_id) }],
                "isError": false
            }))
        }

        "gws_drive_copy" => {
            let file_id = extract_file_id(arguments, "file_id")?;
            let name = arguments.get("name").and_then(|v| v.as_str());
            let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());
            let effective_policy = policy_for_folder(folder_id, policy, meta, state).await?;
            let drive_doc = state.get_doc("drive").await?;
            let files_resource = tools::find_resource(&drive_doc.resources, "files")
                .ok_or_else(|| GwsError::Validation("files resource not found".into()))?;
            let copy_method = files_resource
                .methods
                .get("copy")
                .ok_or_else(|| GwsError::Validation("copy method not found".into()))?;
            let mut body = json!({});
            if let Some(n) = name {
                body["name"] = json!(n);
            }
            if let Some(fid) = folder_id {
                body["parents"] = json!([fid]);
            }
            let result = crate::execute::execute_tool(
                &drive_doc,
                copy_method,
                "files",
                "copy",
                &json!({"params": {"fileId": file_id}, "body": body}),
                "drive",
                &effective_policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let new_id = result["id"].as_str().unwrap_or("");
            Ok(json!({
                "content": [{ "type": "text", "text": format!("File copied.\nfile_id: {}", new_id) }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_drive_rename" => {
            let file_id = extract_file_id(arguments, "file_id")?;
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'name'".into()))?;
            let update_method = files_resource
                .methods
                .get("update")
                .ok_or_else(|| GwsError::Validation("update method not found".into()))?;
            crate::execute::execute_tool(
                &drive_doc,
                update_method,
                "files",
                "update",
                &json!({"params": {"fileId": file_id}, "body": {"name": name}}),
                "drive",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            Ok(json!({
                "content": [{ "type": "text", "text": format!("File renamed to '{}'", name) }],
                "isError": false
            }))
        }

        _ => Err(GwsError::Validation(format!(
            "Unknown drive helper tool: {tool_name}"
        ))),
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
    if tool_name == "gws_docs_write" || tool_name == "gws_docs_replace_section" {
        tracing::info!(
            tool = tool_name,
            has_content = arguments.get("content").is_some(),
            has_document_id = arguments.get("document_id").is_some() || arguments.get("documentId").is_some(),
            has_title = arguments.get("title").is_some(),
            content_type = ?arguments.get("content").map(|v| v.is_string()),
            arg_keys = ?arguments.as_object().map(|m| m.keys().collect::<Vec<_>>()),
            "docs_write dispatch"
        );
        // gws_docs_replace_section requires section — validate early
        if tool_name == "gws_docs_replace_section" {
            if arguments.get("section").and_then(|v| v.as_str()).is_none() {
                return Err(GwsError::Validation(
                    "Missing 'section' — specify the heading text of the section to replace."
                        .into(),
                ));
            }
            if arguments
                .get("document_id")
                .and_then(|v| v.as_str())
                .is_none()
                && arguments
                    .get("documentId")
                    .and_then(|v| v.as_str())
                    .is_none()
            {
                return Err(GwsError::Validation(
                    "Missing 'document_id' — specify the doc to update.".into(),
                ));
            }
        }
        let format = crate::format::parse_format(arguments.get("format").and_then(|v| v.as_str()));
        return execute_docs_write(arguments, policy, meta, state, dry_run, format).await;
    }

    let doc_id = arguments
        .get("document_id")
        .or_else(|| arguments.get("documentId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GwsError::Validation(format!(
                "Missing 'document_id' in {tool_name}. Pass the Google Docs document ID."
            ))
        })?;

    if tool_name == "gws_docs_read_table" {
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
            None,
            false,
            &mut state.token_cache,
        )
        .await?;
        let table_index = arguments
            .get("table_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let result = helpers::read_table_from_doc(&doc_content, table_index);
        return Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
            "structuredContent": result,
            "isError": false
        }));
    }

    if tool_name == "gws_docs_read"
        || tool_name == "gws_docs_outline"
        || tool_name == "gws_docs_find"
    {
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
            None,
            false,
            &mut state.token_cache,
        )
        .await?;

        if tool_name == "gws_docs_find" {
            let needle = arguments
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'text'".into()))?;
            let occurrence = arguments
                .get("occurrence")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize;
            let result = helpers::find_text_in_doc(&doc_content, needle, occurrence);
            return Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }));
        }

        if tool_name == "gws_docs_outline" {
            let structure = helpers::parse_doc_structure(&doc_content);
            return Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&structure).unwrap_or_default() }],
                "structuredContent": structure,
                "isError": false
            }));
        }

        // gws_docs_read: section-level or full doc
        let section = arguments.get("section").and_then(|v| v.as_str());
        let format = arguments
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let content_to_convert = if let Some(heading) = section {
            if let Some((start, end)) = find_section_range(&doc_content, heading) {
                extract_section_doc(&doc_content, start, end)
            } else {
                return Err(GwsError::Validation(format!(
                    "Section '{heading}' not found. Use gws_docs_outline to see available headings."
                )));
            }
        } else {
            doc_content.clone()
        };

        let tables = helpers::extract_all_tables(&content_to_convert);
        let has_tables = !tables.is_empty();

        return match format {
            "plain" => {
                let plain = crate::format::doc_to_plain(&content_to_convert);
                if has_tables {
                    Ok(json!({
                        "content": [{ "type": "text", "text": plain }],
                        "structuredContent": { "text": plain, "tables": tables },
                        "isError": false
                    }))
                } else {
                    Ok(json!({
                        "content": [{ "type": "text", "text": plain }],
                        "isError": false
                    }))
                }
            }
            _ => {
                let md = crate::format::doc_to_markdown(&content_to_convert);
                if has_tables {
                    Ok(json!({
                        "content": [{ "type": "text", "text": md }],
                        "structuredContent": { "text": md, "tables": tables },
                        "isError": false
                    }))
                } else {
                    Ok(json!({
                        "content": [{ "type": "text", "text": md }],
                        "isError": false
                    }))
                }
            }
        };
    }

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
            if let Some(sections) = arguments.get("sections").and_then(|v| v.as_array()) {
                let mut all_requests = Vec::new();
                for section in sections {
                    let text = section.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if text.is_empty() {
                        continue;
                    }
                    let style = parse_text_style(section);
                    let has_style = style.bold.is_some()
                        || style.foreground_color.is_some()
                        || style.italic.is_some()
                        || style.font_size_pt.is_some();
                    let para = section.get("paragraph_style").and_then(|v| v.as_str());
                    all_requests.extend(helpers::build_insert_text_requests(
                        text,
                        Position::End,
                        if has_style { Some(style) } else { None },
                        para,
                    ));
                }
                all_requests
            } else {
                let text = arguments
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| GwsError::Validation("Missing 'text' or 'sections'".into()))?;
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
        }
        "gws_docs_insert_table" => {
            let headers: Option<Vec<String>> = arguments
                .get("headers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });
            let data_rows: Option<Vec<Vec<String>>> =
                arguments.get("rows").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|row| {
                            row.as_array().map(|cells| {
                                cells
                                    .iter()
                                    .filter_map(|c| c.as_str().map(String::from))
                                    .collect()
                            })
                        })
                        .collect()
                });

            if headers.is_some() || data_rows.is_some() {
                let num_cols = headers
                    .as_ref()
                    .map(|h| h.len())
                    .or_else(|| {
                        data_rows
                            .as_ref()
                            .and_then(|r| r.first().map(|row| row.len()))
                    })
                    .unwrap_or(1) as u32;
                let num_rows = (if headers.is_some() { 1 } else { 0 }
                    + data_rows.as_ref().map(|r| r.len()).unwrap_or(0))
                    as u32;

                let position = parse_position(arguments);
                let insert_req = helpers::build_insert_table_request(num_rows, num_cols, position);

                let doc_ref = state.get_doc("docs").await?;
                let resource = tools::find_resource(&doc_ref.resources, "documents")
                    .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
                let batch_method = resource
                    .methods
                    .get("batchUpdate")
                    .ok_or_else(|| GwsError::Validation("batchUpdate not found".into()))?;

                let create_args = json!({
                    "params": { "documentId": doc_id },
                    "body": { "requests": [insert_req] }
                });
                crate::execute::execute_tool(
                    &doc_ref,
                    batch_method,
                    "documents",
                    "batchUpdate",
                    &create_args,
                    "docs",
                    policy,
                    meta,
                    None,
                    None,
                    dry_run,
                    &mut state.token_cache,
                )
                .await?;

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
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;

                let empty_rows: Vec<Vec<String>> = Vec::new();
                let populate_reqs = helpers::build_table_populate_requests(
                    &doc_content,
                    headers.as_deref(),
                    data_rows.as_ref().unwrap_or(&empty_rows),
                );

                if populate_reqs.is_empty() {
                    return Ok(json!({
                        "content": [{ "type": "text", "text": "Table created (no data to populate)" }],
                        "isError": false
                    }));
                }

                let populate_args = json!({
                    "params": { "documentId": doc_id },
                    "body": { "requests": populate_reqs }
                });
                let result = crate::execute::execute_tool(
                    &doc_ref,
                    batch_method,
                    "documents",
                    "batchUpdate",
                    &populate_args,
                    "docs",
                    policy,
                    meta,
                    None,
                    None,
                    dry_run,
                    &mut state.token_cache,
                )
                .await?;
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!("Table created and populated ({} rows, {} columns)", num_rows, num_cols) }],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            let rows = arguments
                .get("rows")
                .and_then(|v| v.as_u64())
                .or_else(|| arguments.get("row_count").and_then(|v| v.as_u64()))
                .ok_or_else(|| GwsError::Validation("Missing 'rows' or 'headers'".into()))?
                as u32;
            let columns = arguments
                .get("columns")
                .and_then(|v| v.as_u64())
                .or_else(|| arguments.get("column_count").and_then(|v| v.as_u64()))
                .ok_or_else(|| GwsError::Validation("Missing 'columns' or 'headers'".into()))?
                as u32;
            let position = parse_position(arguments);
            vec![helpers::build_insert_table_request(rows, columns, position)]
        }
        "gws_docs_insert_image" => {
            let image_url = arguments.get("image_url").and_then(|v| v.as_str());
            let drive_file_id = arguments.get("drive_file_id").and_then(|v| v.as_str());
            let image_data = arguments.get("image_data").and_then(|v| v.as_str());

            let content_type = arguments
                .get("image_content_type")
                .and_then(|v| v.as_str())
                .unwrap_or("image/png");
            let uri = if let Some(url) = image_url {
                url.to_string()
            } else if let Some(fid) = drive_file_id {
                let (url, _perm_id) = make_image_insertable(fid, policy, meta, state).await?;
                url
            } else if let Some(data) = image_data {
                format!("data:{content_type};base64,{data}")
            } else {
                return Err(GwsError::Validation(
                    "One of 'image_url', 'drive_file_id', or 'image_data' is required".into(),
                ));
            };

            let position = parse_position(arguments);
            let w = arguments.get("width_pt").and_then(|v| v.as_f64());
            let h = arguments.get("height_pt").and_then(|v| v.as_f64());
            let mut reqs = vec![helpers::build_insert_image_request(&uri, position, w, h)];
            reqs.push(json!({
                "insertText": {
                    "text": "\n",
                    "endOfSegmentLocation": { "segmentId": "" }
                }
            }));
            reqs
        }
        "gws_docs_format" | "gws_docs_format_text" => {
            let (start, end) = if let Some(text_match) =
                arguments.get("text").and_then(|v| v.as_str())
            {
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
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                let occurrence = arguments
                    .get("occurrence")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as usize;
                let result = helpers::find_text_in_doc(&doc_content, text_match, occurrence);
                if result.get("found") != Some(&json!(true)) {
                    return Err(GwsError::Validation(format!(
                        "Text '{}' not found in document",
                        text_match
                    )));
                }
                let s = result["startIndex"].as_i64().unwrap() as i32;
                let e = result["endIndex"].as_i64().unwrap() as i32;
                (s, e)
            } else {
                let s = arguments
                    .get("start_index")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| GwsError::Validation("Missing 'start_index' or 'text'".into()))?
                    as i32;
                let e = arguments
                    .get("end_index")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| GwsError::Validation("Missing 'end_index'".into()))?
                    as i32;
                (s, e)
            };
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
        "gws_docs_append_section" => {
            let heading = arguments.get("heading").and_then(|v| v.as_str());
            let level = arguments
                .get("heading_level")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            let body = arguments.get("body").and_then(|v| v.as_str());
            let items: Option<Vec<String>> =
                arguments
                    .get("items")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    });
            let preset = arguments
                .get("bullet_preset")
                .and_then(|v| v.as_str())
                .unwrap_or("BULLET_DISC_CIRCLE_SQUARE");
            helpers::build_append_section_requests(heading, level, body, items.as_deref(), preset)
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

    let result = crate::execute::execute_tool(
        &doc,
        method,
        "documents",
        "batchUpdate",
        &batch_args,
        "docs",
        policy,
        meta,
        None,
        None,
        dry_run,
        &mut state.token_cache,
    )
    .await
    .map_err(|e| {
        GwsError::Other(anyhow::anyhow!(
            "{tool_name}: batchUpdate failed for document '{doc_id}': {e}"
        ))
    })?;
    check_api_result(&result).map_err(|e| {
        GwsError::Other(anyhow::anyhow!(
            "{tool_name}: Google Docs API error on document '{doc_id}': {e}"
        ))
    })?;
    Ok(result)
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
        return Some((start, end - 1));
    }
    None
}

async fn make_image_insertable(
    file_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<(String, Option<String>), GwsError> {
    let drive_doc = state.get_doc("drive").await?;
    let resource = tools::find_resource(&drive_doc.resources, "permissions")
        .ok_or_else(|| GwsError::Validation("permissions resource not found".into()))?;
    let create_method = resource
        .methods
        .get("create")
        .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
    let perm_args = json!({
        "params": { "fileId": file_id },
        "body": { "role": "reader", "type": "anyone" }
    });
    let perm_result = crate::execute::execute_tool(
        &drive_doc,
        create_method,
        "permissions",
        "create",
        &perm_args,
        "drive",
        policy,
        meta,
        None,
        None,
        false,
        &mut state.token_cache,
    )
    .await?;
    if let Some(err) = perm_result.get("error") {
        let msg = err.as_str().unwrap_or("unknown error");
        return Err(GwsError::Api {
            code: perm_result["status"].as_u64().unwrap_or(403) as u16,
            message: format!(
                "Cannot make image publicly accessible for Docs insertion: {msg}. \
                 The image is in Drive (file ID: {file_id}). Insert via Docs UI: Insert > Image > Drive."
            ),
            reason: "sharingBlocked".into(),
            enable_url: None,
        });
    }
    let perm_id = perm_result["id"]
        .as_str()
        .unwrap_or("anyoneWithLink")
        .to_string();
    let url = format!("https://drive.google.com/uc?export=download&id={file_id}");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    Ok((url, Some(perm_id)))
}

async fn revoke_image_sharing(
    file_id: &str,
    permission_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) {
    if let Ok(drive_doc) = state.get_doc("drive").await {
        if let Some(resource) = tools::find_resource(&drive_doc.resources, "permissions") {
            if let Some(delete_method) = resource.methods.get("delete") {
                let _ = crate::execute::execute_tool(
                    &drive_doc,
                    delete_method,
                    "permissions",
                    "delete",
                    &json!({"params": {"fileId": file_id, "permissionId": permission_id}}),
                    "drive",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await;
            }
        }
    }
}

fn shift_request_indexes(requests: &[Value], shift: i32) -> Vec<Value> {
    if shift == 0 {
        return requests.to_vec();
    }
    requests
        .iter()
        .map(|req| {
            let mut r = req.clone();
            for path in &[
                "/insertText/location/index",
                "/insertTable/location/index",
                "/updateParagraphStyle/range/startIndex",
                "/updateParagraphStyle/range/endIndex",
                "/updateTextStyle/range/startIndex",
                "/updateTextStyle/range/endIndex",
                "/createParagraphBullets/range/startIndex",
                "/createParagraphBullets/range/endIndex",
            ] {
                if let Some(idx) = r.pointer_mut(path) {
                    if let Some(v) = idx.as_i64() {
                        *idx = json!(v + shift as i64);
                    }
                }
            }
            r
        })
        .collect()
}

fn extract_section_doc(doc: &Value, start: i32, end: i32) -> Value {
    let mut section_doc = doc.clone();
    if let Some(content) = doc["body"]["content"].as_array() {
        let filtered: Vec<Value> = content
            .iter()
            .filter(|elem| {
                let elem_start = elem["startIndex"].as_i64().unwrap_or(0) as i32;
                let elem_end = elem["endIndex"].as_i64().unwrap_or(0) as i32;
                elem_start >= start && elem_end <= end
            })
            .cloned()
            .collect();
        section_doc["body"]["content"] = json!(filtered);
    }
    section_doc
}

async fn execute_docs_write(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
    format: crate::format::ContentFormat,
) -> Result<Value, GwsError> {
    let content = arguments
        .get("content")
        .or_else(|| arguments.get("markdown"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GwsError::Validation(
                "Missing 'content' parameter (must be a string). Pass the content to write.".into(),
            )
        })?;

    let doc_id_arg = arguments
        .get("document_id")
        .or_else(|| arguments.get("documentId"))
        .and_then(|v| v.as_str());
    let title = arguments.get("title").and_then(|v| v.as_str());
    let mut folder_id = arguments
        .get("folder_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let section = arguments.get("section").and_then(|v| v.as_str());
    let template_id = arguments.get("template_id").and_then(|v| v.as_str());

    // If document_id looks like a folder (title also provided), treat it as folder_id.
    let doc_id_arg = if let (Some(id), Some(_)) = (doc_id_arg, title) {
        if folder_id.is_none() {
            if let Ok(drive_doc) = state.get_doc("drive").await {
                if let Some(resource) = tools::find_resource(&drive_doc.resources, "files") {
                    if let Some(gm) = resource.methods.get("get") {
                        let args = json!({"params": {"fileId": id}, "fields": "mimeType"});
                        if let Ok(file_meta) = crate::execute::execute_tool(
                            &drive_doc,
                            gm,
                            "files",
                            "get",
                            &args,
                            "drive",
                            policy,
                            meta,
                            None,
                            None,
                            false,
                            &mut state.token_cache,
                        )
                        .await
                        {
                            if file_meta["mimeType"].as_str()
                                == Some("application/vnd.google-apps.folder")
                            {
                                tracing::info!(
                                    provided_id = id,
                                    "document_id is a folder — treating as folder_id"
                                );
                                folder_id = Some(id.to_string());
                                None
                            } else {
                                Some(id)
                            }
                        } else {
                            Some(id)
                        }
                    } else {
                        Some(id)
                    }
                } else {
                    Some(id)
                }
            } else {
                Some(id)
            }
        } else {
            Some(id)
        }
    } else {
        doc_id_arg
    };

    let folder_id = folder_id.as_deref();

    // Step A: resolve, find existing, or create the document
    let (doc_id, created_new_doc) = if let Some(id) = doc_id_arg {
        (id.to_string(), false)
    } else if title.is_some() || folder_id.is_some() {
        let doc_title = title.unwrap_or("Untitled");
        let effective_policy = policy_for_folder(folder_id, policy, meta, state).await?;
        let drive_doc = state.get_doc("drive").await.map_err(|e| {
            GwsError::Other(anyhow::anyhow!(
                "gws_docs_import_markdown: failed to load Drive API: {e}"
            ))
        })?;
        let drive_resource =
            tools::find_resource(&drive_doc.resources, "files").ok_or_else(|| {
                GwsError::Validation(
                    "gws_docs_import_markdown: files resource not found in drive API".into(),
                )
            })?;

        {
            let mut body = json!({
                "name": doc_title,
                "mimeType": "application/vnd.google-apps.document"
            });
            if let Some(fid) = folder_id {
                body["parents"] = json!([fid]);
            }
            let create_args = json!({"body": body});
            let create_method = drive_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
            let result = crate::execute::execute_tool(
                &drive_doc,
                create_method,
                "files",
                "create",
                &create_args,
                "drive",
                &effective_policy,
                meta,
                None,
                None,
                dry_run,
                &mut state.token_cache,
            )
            .await?;
            check_api_result(&result)?;
            let new_id = result["id"]
                .as_str()
                .ok_or_else(|| {
                    GwsError::Other(anyhow::anyhow!("No 'id' in drive.files.create response"))
                })?
                .to_string();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            (new_id, true)
        }
    } else {
        return Err(GwsError::Validation(
            "Either 'document_id' (existing doc) or 'title' (create new doc) is required. \
             Pass document_id to import into an existing document, or title to create a new one."
                .into(),
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
        let idx = if created_new_doc {
            1
        } else if let Some(i) = arguments.get("index").and_then(|v| v.as_i64()) {
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
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
    let batch_method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;

    if let Some(style_reqs) = template_requests {
        let style_args = json!({
            "params": { "documentId": doc_id },
            "body": { "requests": style_reqs }
        });
        let style_result = crate::execute::execute_tool(
            &docs_doc,
            batch_method,
            "documents",
            "batchUpdate",
            &style_args,
            "docs",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await?;
        check_api_result(&style_result)?;
    }

    let mut content_requests: Vec<Value> = Vec::new();
    if let Some(delete_req) = section_delete {
        content_requests.push(delete_req);
    }
    content_requests.extend(crate::format::content_to_batch_requests(
        content,
        format,
        insert_index,
    ));

    // Split at table boundaries: insertTable changes the doc's index space,
    // so subsequent inserts at pre-calculated offsets fail. Split into
    // separate batches, re-derive indexes from the doc after each table.
    let mut batches: Vec<(Vec<Value>, Option<Value>)> = Vec::new();
    let mut current_batch: Vec<Value> = Vec::new();
    for req in &content_requests {
        if let Some(mut it) = req.get("insertTable").cloned() {
            let table_data = it.as_object_mut().and_then(|m| m.remove("_tableData"));
            current_batch.push(json!({ "insertTable": it }));
            batches.push((current_batch, table_data));
            current_batch = Vec::new();
        } else {
            current_batch.push(req.clone());
        }
    }
    if !current_batch.is_empty() {
        batches.push((current_batch, None));
    }

    let mut result: Result<Value, GwsError> = Ok(json!({}));
    for (batch_idx, (batch_reqs, table_data)) in batches.iter().enumerate() {
        let final_reqs = if batch_idx > 0 && !batch_reqs.is_empty() {
            let doc_now = crate::execute::execute_tool(
                &docs_doc,
                get_method,
                "documents",
                "get",
                &json!({"params": {"documentId": &doc_id}}),
                "docs",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let end_index = doc_now
                .pointer("/body/content")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.last())
                .and_then(|el| el["endIndex"].as_i64())
                .unwrap_or(1) as i32;
            let first_idx = batch_reqs
                .iter()
                .find_map(|r| {
                    r.pointer("/insertText/location/index")
                        .and_then(|v| v.as_i64())
                        .map(|i| i as i32)
                })
                .unwrap_or(end_index);
            let shift = (end_index - 1) - first_idx;
            shift_request_indexes(batch_reqs, shift)
        } else {
            batch_reqs.clone()
        };

        if !final_reqs.is_empty() {
            let batch_args = json!({
                "params": { "documentId": doc_id },
                "body": { "requests": final_reqs }
            });
            result = crate::execute::execute_tool(
                &docs_doc,
                batch_method,
                "documents",
                "batchUpdate",
                &batch_args,
                "docs",
                policy,
                meta,
                None,
                None,
                dry_run,
                &mut state.token_cache,
            )
            .await;

            // If batchUpdate fails with too many requests, retry in smaller chunks
            let should_chunk = match &result {
                Err(_) => final_reqs.len() > 10,
                Ok(r) => check_api_result(r).is_err() && final_reqs.len() > 10,
            };
            if should_chunk {
                tracing::info!(
                    total_requests = final_reqs.len(),
                    "batchUpdate failed, retrying in chunks of 50"
                );
                result = Ok(json!({}));
                for chunk in final_reqs.chunks(50) {
                    let chunk_args = json!({
                        "params": { "documentId": doc_id },
                        "body": { "requests": chunk }
                    });
                    result = crate::execute::execute_tool(
                        &docs_doc, batch_method, "documents", "batchUpdate",
                        &chunk_args, "docs", policy, meta, None, None, dry_run,
                        &mut state.token_cache,
                    ).await;
                    match &result {
                        Ok(r) if check_api_result(r).is_err() => break,
                        Err(_) => break,
                        _ => {}
                    }
                }
            } else if let Ok(ref r) = result {
                if check_api_result(r).is_err() {
                    break;
                }
            } else {
                break;
            }
        }

        // Populate table cells after inserting the table
        if let Some(data) = table_data {
            if !dry_run {
                let doc_now = crate::execute::execute_tool(
                    &docs_doc,
                    get_method,
                    "documents",
                    "get",
                    &json!({"params": {"documentId": &doc_id}}),
                    "docs",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                let rows: Vec<Vec<String>> = data
                    .get("rows")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let is_header = data
                    .get("header")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !rows.is_empty() {
                    let (headers, data_rows) = if is_header && rows.len() > 1 {
                        (Some(rows[0].clone()), rows[1..].to_vec())
                    } else {
                        (None, rows)
                    };
                    let populate_reqs = helpers::build_table_populate_requests(
                        &doc_now,
                        headers.as_deref(),
                        &data_rows,
                    );
                    if !populate_reqs.is_empty() {
                        let _ = crate::execute::execute_tool(
                            &docs_doc, batch_method, "documents", "batchUpdate",
                            &json!({"params": {"documentId": doc_id}, "body": {"requests": populate_reqs}}),
                            "docs", policy, meta, None, None, false, &mut state.token_cache,
                        ).await;
                    }
                }
            }
        }
    }

    let failed = match &result {
        Ok(r) => check_api_result(r).is_err(),
        Err(_) => true,
    };

    if failed && created_new_doc {
        if let Ok(drive_doc) = state.get_doc("drive").await {
            if let Some(resource) = tools::find_resource(&drive_doc.resources, "files") {
                if let Some(delete_method) = resource.methods.get("delete") {
                    let args = json!({"params": {"fileId": &doc_id}});
                    let _ = crate::execute::execute_tool(
                        &drive_doc,
                        delete_method,
                        "files",
                        "delete",
                        &args,
                        "drive",
                        policy,
                        meta,
                        None,
                        None,
                        false,
                        &mut state.token_cache,
                    )
                    .await;
                    tracing::info!(doc_id = %doc_id, "Cleaned up empty doc after failed write");
                }
            }
        }
    }

    let result = match result {
        Ok(mut r) => {
            if let Err(e) = check_api_result(&r) {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!(
                        "gws_docs_write: content insertion failed: {e}. \
                         Try with simpler content or split into smaller sections."
                    )}],
                    "isError": true
                }));
            }
            r["document_id"] = json!(doc_id);
            r
        }
        Err(e) => {
            return Ok(json!({
                "content": [{ "type": "text", "text": format!(
                    "gws_docs_write: failed: {e}. \
                     Try with simpler content or split into smaller sections."
                )}],
                "isError": true
            }));
        }
    };

    let text = if created_new_doc {
        format!("Content written to new document.\ndocument_id: {doc_id}")
    } else {
        format!("Content written to document.\ndocument_id: {doc_id}")
    };

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_list_templates(
    arguments: Option<&Value>,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Value {
    let name_filter = arguments
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str());

    let mut templates: Vec<Value> = Vec::new();

    for t in policy.templates() {
        if let Some(filter) = name_filter {
            if !t.name.to_lowercase().contains(&filter.to_lowercase()) {
                continue;
            }
        }

        let mut entry = json!({
            "name": t.name,
            "id": t.id
        });
        if let Some(ref desc) = t.description {
            entry["description"] = json!(desc);
        }
        if let Ok(slides_doc) = state.get_doc("slides").await {
            if let Some(pres_resource) =
                tools::find_resource(&slides_doc.resources, "presentations")
            {
                if let Some(get_method) = pres_resource.methods.get("get") {
                    let args = json!({ "params": { "presentationId": &t.id } });
                    let mut tc = state.token_cache.take();
                    if let Ok(pres_data) = crate::execute::execute_tool(
                        &slides_doc,
                        get_method,
                        "presentations",
                        "get",
                        &args,
                        "slides",
                        policy,
                        meta,
                        None,
                        None,
                        false,
                        &mut tc,
                    )
                    .await
                    {
                        let raw_layouts = pres_data
                            .get("layouts")
                            .and_then(|l| l.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let active_master = pres_data
                            .get("slides")
                            .and_then(|s| s.as_array())
                            .and_then(|slides| slides.last())
                            .and_then(|s| s.get("slideProperties"))
                            .and_then(|sp| sp.get("masterObjectId"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("");

                        let mut seen = std::collections::HashSet::new();
                        let mut layout_details = Vec::new();
                        for layout in &raw_layouts {
                            let master = layout
                                .get("layoutProperties")
                                .and_then(|lp| lp.get("masterObjectId"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("");
                            if !active_master.is_empty() && master != active_master {
                                continue;
                            }
                            let name = layout
                                .get("layoutProperties")
                                .and_then(|lp| lp.get("displayName"))
                                .and_then(|dn| dn.as_str())
                                .unwrap_or("");
                            if name.is_empty() || !seen.insert(name.to_string()) {
                                continue;
                            }
                            layout_details.push(
                                crate::slides_helpers::extract_layout_details(layout),
                            );
                        }
                        entry["layouts"] = json!(layout_details);
                    }
                    state.token_cache = tc;
                }
            }
        }
        templates.push(entry);
    }

    json!({
        "templates": templates,
        "count": templates.len(),
        "hint": "Use the template 'name' or 'id' as the 'template' argument in gws_slides_import_marp. Use placeholder labels with gws_slides_update."
    })
}

async fn execute_sheets_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    tracing::info!(
        tool = tool_name,
        has_spreadsheet_id = arguments.get("spreadsheet_id").is_some(),
        has_title = arguments.get("title").is_some(),
        has_data = arguments.get("data").is_some(),
        has_folder_id = arguments.get("folder_id").is_some(),
        arg_keys = ?arguments.as_object().map(|m| m.keys().collect::<Vec<_>>()),
        "sheets_helper dispatch"
    );

    let sheets_doc = state.get_doc("sheets").await?;

    let raw_id = arguments
        .get("spreadsheet_id")
        .or_else(|| arguments.get("spreadsheetId"))
        .and_then(|v| v.as_str());
    let title = arguments.get("title").and_then(|v| v.as_str());
    let mut folder_id = arguments
        .get("folder_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Auto-detect: if spreadsheet_id is a folder (title also provided), treat as folder_id
    let spreadsheet_id_opt = if let (Some(id), Some(_)) = (raw_id, title) {
        if folder_id.is_none() {
            if let Ok(drive_doc) = state.get_doc("drive").await {
                if let Some(res) = tools::find_resource(&drive_doc.resources, "files") {
                    if let Some(gm) = res.methods.get("get") {
                        let args = json!({"params": {"fileId": id}, "fields": "mimeType"});
                        if let Ok(meta_result) = crate::execute::execute_tool(
                            &drive_doc, gm, "files", "get", &args, "drive",
                            policy, meta, None, None, false, &mut state.token_cache,
                        ).await {
                            if meta_result["mimeType"].as_str()
                                == Some("application/vnd.google-apps.folder")
                            {
                                tracing::info!(provided_id = id, "spreadsheet_id is a folder — treating as folder_id");
                                folder_id = Some(id.to_string());
                                None
                            } else {
                                Some(id)
                            }
                        } else {
                            Some(id)
                        }
                    } else { Some(id) }
                } else { Some(id) }
            } else { Some(id) }
        } else {
            Some(id)
        }
    } else {
        raw_id
    };

    let folder_id = folder_id.as_deref();

    if tool_name != "gws_sheets_write" {
        if spreadsheet_id_opt.is_none() {
            return Err(GwsError::Validation(
                "Missing 'spreadsheet_id'. Pass the Google Sheets spreadsheet ID \
                 (the long string from the spreadsheet URL).".into()
            ));
        }
        crate::sheets_helpers::validate_spreadsheet_id(spreadsheet_id_opt.unwrap())
            .map_err(GwsError::Validation)?;
    }

    match tool_name {
        "gws_sheets_write" if spreadsheet_id_opt.is_none() => {
            let title = title.ok_or_else(|| {
                GwsError::Validation(
                    "Missing title. To create a new spreadsheet, call with: \
                     {\"title\": \"My Sheet\", \"folder_id\": \"FOLDER_ID\", \"data\": [[\"Header1\",\"Header2\"],[\"val1\",\"val2\"]]}. \
                     To write to an existing spreadsheet, pass spreadsheet_id instead of title.".into(),
                )
            })?;
            let data = arguments
                .get("data")
                .ok_or_else(|| GwsError::Validation("Missing 'data'".into()))?;
            let range = arguments
                .get("range")
                .and_then(|v| v.as_str())
                .unwrap_or("Sheet1");
            let sheet = arguments.get("sheet").and_then(|v| v.as_str());

            let effective_policy = policy_for_folder(folder_id, policy, meta, state).await?;
            let drive_doc = state.get_doc("drive").await?;
            let files_resource = tools::find_resource(&drive_doc.resources, "files")
                .ok_or_else(|| GwsError::Validation("files resource not found in drive API".into()))?;
            let create_method = files_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
            let mut body = json!({
                "name": title,
                "mimeType": "application/vnd.google-apps.spreadsheet"
            });
            if let Some(fid) = folder_id {
                body["parents"] = json!([fid]);
            }
            let created = crate::execute::execute_tool(
                &drive_doc, create_method, "files", "create",
                &json!({"body": body}), "drive", &effective_policy, meta, None, None, false,
                &mut state.token_cache,
            ).await?;

            let new_id = created.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let url = format!("https://docs.google.com/spreadsheets/d/{new_id}/edit");

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let normalized_data = crate::sheets_helpers::normalize_data(data);

            if !new_id.is_empty() && normalized_data.is_array() {
                let values_resource =
                    tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                        .ok_or_else(|| {
                            GwsError::Validation("spreadsheets.values resource not found".into())
                        })?;
                let update_method = values_resource
                    .methods
                    .get("update")
                    .ok_or_else(|| GwsError::Validation("update method not found".into()))?;
                let full_range = crate::sheets_helpers::build_range(range, sheet);
                let write_args = json!({
                    "params": {
                        "spreadsheetId": new_id,
                        "range": full_range,
                        "valueInputOption": "USER_ENTERED"
                    },
                    "body": { "range": full_range, "values": normalized_data }
                });
                let mut write_result = crate::execute::execute_tool(
                    &sheets_doc, update_method, "spreadsheets.values", "update",
                    &write_args, "sheets", policy, meta, None, None, false,
                    &mut state.token_cache,
                ).await?;

                if write_result.get("updatedRows").is_none() {
                    tracing::info!("sheets create-on-write: first write returned no updatedRows, retrying after 1s");
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    write_result = crate::execute::execute_tool(
                        &sheets_doc, update_method, "spreadsheets.values", "update",
                        &write_args, "sheets", policy, meta, None, None, false,
                        &mut state.token_cache,
                    ).await?;
                }

                tracing::info!(
                    spreadsheet_id = new_id,
                    updated_rows = ?write_result.get("updatedRows"),
                    updated_range = ?write_result.get("updatedRange"),
                    "sheets create-on-write: data write result"
                );

                let output = json!({
                    "spreadsheetId": new_id,
                    "title": title,
                    "url": url,
                    "updatedRange": write_result.get("updatedRange"),
                    "updatedRows": write_result.get("updatedRows"),
                    "updatedColumns": write_result.get("updatedColumns"),
                });
                return Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                    "structuredContent": output,
                    "isError": false
                }));
            }

            let output = json!({ "spreadsheetId": new_id, "title": title, "url": url });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_sheets_read" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let range = arguments
                .get("range")
                .and_then(|v| v.as_str())
                .unwrap_or("Sheet1");
            let sheet = arguments.get("sheet").and_then(|v| v.as_str());
            let format = arguments.get("format").and_then(|v| v.as_str());
            let full_range = crate::sheets_helpers::build_range(range, sheet);
            let render_option = match format {
                Some("values") => "UNFORMATTED_VALUE",
                Some("formula") => "FORMULA",
                _ => "FORMATTED_VALUE",
            };

            if let Some(cached) = state.sheet_cache.get(spreadsheet_id, &full_range, render_option) {
                tracing::debug!(spreadsheet_id, range = %full_range, "sheets cache hit");
                let result = cached.clone();
                return Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| {
                    GwsError::Validation("spreadsheets.values resource not found".into())
                })?;
            let get_method = values_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
            let args = crate::sheets_helpers::build_read_args(range, sheet, format);
            let mut args_with_id = args.clone();
            args_with_id["params"]["spreadsheetId"] = json!(spreadsheet_id);
            let result = crate::execute::execute_tool(
                &sheets_doc,
                get_method,
                "spreadsheets.values",
                "get",
                &args_with_id,
                "sheets",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            state.sheet_cache.put(spreadsheet_id, &full_range, render_option, result.clone());

            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_sheets_write" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let range = arguments
                .get("range")
                .and_then(|v| v.as_str())
                .unwrap_or("Sheet1");
            let data = arguments
                .get("data")
                .ok_or_else(|| GwsError::Validation(
                    "Missing 'data'. Pass an array of rows, e.g. [[\"Name\",\"Score\"],[\"Alice\",95]]".into()
                ))?;
            let normalized_data = crate::sheets_helpers::normalize_data(data);
            let sheet = arguments.get("sheet").and_then(|v| v.as_str());
            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| {
                    GwsError::Validation("spreadsheets.values resource not found".into())
                })?;
            let update_method = values_resource
                .methods
                .get("update")
                .ok_or_else(|| GwsError::Validation("update method not found".into()))?;
            let args = crate::sheets_helpers::build_write_args(range, &normalized_data, sheet);
            let mut args_with_id = args.clone();
            args_with_id["params"]["spreadsheetId"] = json!(spreadsheet_id);
            let result = crate::execute::execute_tool(
                &sheets_doc,
                update_method,
                "spreadsheets.values",
                "update",
                &args_with_id,
                "sheets",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            state.sheet_cache.invalidate(spreadsheet_id);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_sheets_append" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let range = arguments
                .get("range")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'range'".into()))?;
            let data = arguments
                .get("data")
                .ok_or_else(|| GwsError::Validation("Missing 'data'".into()))?;
            let sheet = arguments.get("sheet").and_then(|v| v.as_str());
            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| {
                    GwsError::Validation("spreadsheets.values resource not found".into())
                })?;
            let append_method = values_resource
                .methods
                .get("append")
                .ok_or_else(|| GwsError::Validation("append method not found".into()))?;
            let args = crate::sheets_helpers::build_append_args(range, data, sheet);
            let mut args_with_id = args.clone();
            args_with_id["params"]["spreadsheetId"] = json!(spreadsheet_id);
            let result = crate::execute::execute_tool(
                &sheets_doc,
                append_method,
                "spreadsheets.values",
                "append",
                &args_with_id,
                "sheets",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            state.sheet_cache.invalidate(spreadsheet_id);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_sheets_info" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let spreadsheets_resource =
                tools::find_resource(&sheets_doc.resources, "spreadsheets").ok_or_else(|| {
                    GwsError::Validation("spreadsheets resource not found".into())
                })?;
            let get_method = spreadsheets_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
            let mut args = crate::sheets_helpers::build_info_args();
            args["params"] = json!({ "spreadsheetId": spreadsheet_id });
            let result = crate::execute::execute_tool(
                &sheets_doc,
                get_method,
                "spreadsheets",
                "get",
                &args,
                "sheets",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let formatted = crate::sheets_helpers::format_info_result(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&formatted).unwrap_or_default() }],
                "structuredContent": formatted,
                "isError": false
            }))
        }

        "gws_sheets_clear" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let range = arguments
                .get("range")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'range'".into()))?;
            let sheet = arguments.get("sheet").and_then(|v| v.as_str());
            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| {
                    GwsError::Validation("spreadsheets.values resource not found".into())
                })?;
            let clear_method = values_resource
                .methods
                .get("clear")
                .ok_or_else(|| GwsError::Validation("clear method not found".into()))?;
            let args = crate::sheets_helpers::build_clear_args(range, sheet);
            let mut args_with_id = args.clone();
            args_with_id["params"]["spreadsheetId"] = json!(spreadsheet_id);
            let result = crate::execute::execute_tool(
                &sheets_doc,
                clear_method,
                "spreadsheets.values",
                "clear",
                &args_with_id,
                "sheets",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            state.sheet_cache.invalidate(spreadsheet_id);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_sheets_manage_tabs" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let action = arguments
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'action'".into()))?;
            let title = arguments.get("title").and_then(|v| v.as_str());
            let sheet_id = arguments.get("sheet_id").and_then(|v| v.as_i64());
            let batch_args = crate::sheets_helpers::build_tab_request(action, title, sheet_id)
                .map_err(GwsError::Validation)?;
            let spreadsheets_resource =
                tools::find_resource(&sheets_doc.resources, "spreadsheets").ok_or_else(|| {
                    GwsError::Validation("spreadsheets resource not found".into())
                })?;
            let batch_method = spreadsheets_resource
                .methods
                .get("batchUpdate")
                .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;
            let mut args = batch_args;
            args["params"] = json!({ "spreadsheetId": spreadsheet_id });
            let result = crate::execute::execute_tool(
                &sheets_doc,
                batch_method,
                "spreadsheets",
                "batchUpdate",
                &args,
                "sheets",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_sheets_format" | "gws_sheets_validate" | "gws_sheets_named_range" | "gws_sheets_dimensions" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let action = arguments.get("action").and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'action'".into()))?;
            let mut sheet_id = arguments.get("sheet_id").and_then(|v| v.as_i64());
            let sheet_name = arguments.get("sheet").and_then(|v| v.as_str());

            // Resolve sheet name to sheet_id if not provided
            if sheet_id.is_none() {
                if let Some(name) = sheet_name {
                    let spreadsheets_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets")
                        .ok_or_else(|| GwsError::Validation("spreadsheets resource not found".into()))?;
                    let get_method = spreadsheets_resource.methods.get("get")
                        .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                    let meta_result = crate::execute::execute_tool(
                        &sheets_doc, get_method, "spreadsheets", "get",
                        &json!({"params": {"spreadsheetId": spreadsheet_id}, "fields": "sheets(properties(sheetId,title))"}),
                        "sheets", policy, meta, None, None, false, &mut state.token_cache,
                    ).await?;
                    sheet_id = meta_result.get("sheets")
                        .and_then(|v| v.as_array())
                        .and_then(|sheets| sheets.iter().find(|s| {
                            s.pointer("/properties/title").and_then(|t| t.as_str()) == Some(name)
                        }))
                        .and_then(|s| s.pointer("/properties/sheetId"))
                        .and_then(|v| v.as_i64());
                    if sheet_id.is_none() {
                        return Err(GwsError::Validation(format!(
                            "Tab '{name}' not found. Use gws_sheets_info to list available tabs."
                        )));
                    }
                } else if action != "list" {
                    sheet_id = Some(0);
                }
            }
            let range = arguments.get("range").and_then(|v| v.as_str());
            let rule = arguments.get("rule");
            let index = arguments.get("index").and_then(|v| v.as_i64());
            let name = arguments.get("name").and_then(|v| v.as_str());
            let named_range_id = arguments.get("named_range_id").and_then(|v| v.as_str());
            let dimension = arguments.get("dimension").and_then(|v| v.as_str());
            let start = arguments.get("start").and_then(|v| v.as_i64());
            let end = arguments.get("end").and_then(|v| v.as_i64());
            let count = arguments.get("count").and_then(|v| v.as_i64());
            let size = arguments.get("size").and_then(|v| v.as_i64());
            let destination = arguments.get("destination").and_then(|v| v.as_i64());

            // List actions: read from spreadsheet metadata
            if action == "list" {
                let spreadsheets_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets")
                    .ok_or_else(|| GwsError::Validation("spreadsheets resource not found".into()))?;
                let get_method = spreadsheets_resource.methods.get("get")
                    .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                let fields = match tool_name {
                    "gws_sheets_format" => "sheets(conditionalFormats)",
                    "gws_sheets_validate" => "sheets(data(rowData(values(dataValidation))))",
                    "gws_sheets_named_range" => "namedRanges",
                    _ => "sheets",
                };
                let args = json!({"params": {"spreadsheetId": spreadsheet_id}, "fields": fields});
                let result = crate::execute::execute_tool(
                    &sheets_doc, get_method, "spreadsheets", "get",
                    &args, "sheets", policy, meta, None, None, false,
                    &mut state.token_cache,
                ).await?;
                return Ok(json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default()}],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            // Read action for named ranges
            if tool_name == "gws_sheets_named_range" && action == "read" {
                let n = name.ok_or_else(|| GwsError::Validation("Missing 'name' for read".into()))?;
                let spreadsheets_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets")
                    .ok_or_else(|| GwsError::Validation("spreadsheets resource not found".into()))?;
                let get_method = spreadsheets_resource.methods.get("get")
                    .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                let meta_result = crate::execute::execute_tool(
                    &sheets_doc, get_method, "spreadsheets", "get",
                    &json!({"params": {"spreadsheetId": spreadsheet_id}, "fields": "namedRanges,sheets(properties(sheetId,title))"}),
                    "sheets", policy, meta, None, None, false, &mut state.token_cache,
                ).await?;
                // Find the named range and read its values
                let named_ranges = meta_result.get("namedRanges").and_then(|v| v.as_array());
                let found = named_ranges.and_then(|nrs| nrs.iter().find(|nr| nr.get("name").and_then(|v| v.as_str()) == Some(n)));
                if let Some(nr) = found {
                    let nr_range = nr.get("range");
                    let result = json!({"name": n, "namedRange": nr, "range": nr_range});
                    return Ok(json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default()}],
                        "structuredContent": result,
                        "isError": false
                    }));
                }
                return Err(GwsError::Validation(format!("Named range '{n}' not found")));
            }

            // Build batchUpdate request
            let batch_args = match tool_name {
                "gws_sheets_format" => crate::sheets_helpers::build_conditional_format_request(action, sheet_id, range, rule, index),
                "gws_sheets_validate" => crate::sheets_helpers::build_data_validation_request(action, sheet_id, range, rule),
                "gws_sheets_named_range" => crate::sheets_helpers::build_named_range_request(action, name, sheet_id, range, named_range_id),
                "gws_sheets_dimensions" => crate::sheets_helpers::build_dimension_request(action, sheet_id, dimension, start, end, count, size, destination),
                _ => unreachable!(),
            }.map_err(GwsError::Validation)?;

            let spreadsheets_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets")
                .ok_or_else(|| GwsError::Validation("spreadsheets resource not found".into()))?;
            let batch_method = spreadsheets_resource.methods.get("batchUpdate")
                .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;
            let mut args = batch_args;
            args["params"] = json!({"spreadsheetId": spreadsheet_id});
            let result = crate::execute::execute_tool(
                &sheets_doc, batch_method, "spreadsheets", "batchUpdate",
                &args, "sheets", policy, meta, None, None, false,
                &mut state.token_cache,
            ).await?;
            state.sheet_cache.invalidate(spreadsheet_id);
            Ok(json!({
                "content": [{"type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default()}],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_sheets_csv" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let action = arguments.get("action").and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'action' (export or import)".into()))?;
            let sheet = arguments.get("sheet").and_then(|v| v.as_str()).unwrap_or("Sheet1");
            let separator = arguments.get("separator").and_then(|v| v.as_str())
                .and_then(|s| s.chars().next()).unwrap_or(',');

            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| GwsError::Validation("spreadsheets.values resource not found".into()))?;

            if action == "export" {
                let get_method = values_resource.methods.get("get")
                    .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                let full_range = crate::sheets_helpers::build_range(sheet, None);
                let args = json!({"params": {"spreadsheetId": spreadsheet_id, "range": full_range, "valueRenderOption": "FORMATTED_VALUE"}});
                let result = crate::execute::execute_tool(
                    &sheets_doc, get_method, "spreadsheets.values", "get",
                    &args, "sheets", policy, meta, None, None, false,
                    &mut state.token_cache,
                ).await?;
                let values: Vec<Vec<String>> = result.get("values")
                    .and_then(|v| v.as_array())
                    .map(|rows| rows.iter().map(|row| {
                        row.as_array().map(|cells| cells.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
                    }).collect())
                    .unwrap_or_default();
                let csv = crate::sheets_helpers::values_to_csv(&values, separator);
                Ok(json!({
                    "content": [{"type": "text", "text": csv}],
                    "isError": false
                }))
            } else if action == "import" {
                let csv_data = arguments.get("data").and_then(|v| v.as_str())
                    .ok_or_else(|| GwsError::Validation("Missing 'data' (CSV string) for import".into()))?;
                let values = crate::sheets_helpers::csv_to_values(csv_data, separator);
                let update_method = values_resource.methods.get("update")
                    .ok_or_else(|| GwsError::Validation("update method not found".into()))?;
                let full_range = crate::sheets_helpers::build_range(sheet, None);
                let data_json: Vec<Value> = values.iter().map(|row| json!(row)).collect();
                let args = json!({
                    "params": {"spreadsheetId": spreadsheet_id, "range": full_range, "valueInputOption": "USER_ENTERED"},
                    "body": {"range": full_range, "values": data_json}
                });
                let result = crate::execute::execute_tool(
                    &sheets_doc, update_method, "spreadsheets.values", "update",
                    &args, "sheets", policy, meta, None, None, false,
                    &mut state.token_cache,
                ).await?;
                state.sheet_cache.invalidate(spreadsheet_id);
                Ok(json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default()}],
                    "structuredContent": result,
                    "isError": false
                }))
            } else {
                Err(GwsError::Validation("action must be 'export' or 'import'".into()))
            }
        }

        "gws_sheets_formulas" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let sheet = arguments.get("sheet").and_then(|v| v.as_str()).unwrap_or("Sheet1");
            let range = arguments.get("range").and_then(|v| v.as_str()).unwrap_or(sheet);
            let full_range = crate::sheets_helpers::build_range(range, Some(sheet));

            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| GwsError::Validation("spreadsheets.values resource not found".into()))?;
            let get_method = values_resource.methods.get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;

            // Read formulas
            let formula_args = json!({"params": {"spreadsheetId": spreadsheet_id, "range": full_range, "valueRenderOption": "FORMULA"}});
            let formula_result = crate::execute::execute_tool(
                &sheets_doc, get_method, "spreadsheets.values", "get",
                &formula_args, "sheets", policy, meta, None, None, false,
                &mut state.token_cache,
            ).await?;

            // Read headers
            let header_range = crate::sheets_helpers::build_range("1:1", Some(sheet));
            let header_args = json!({"params": {"spreadsheetId": spreadsheet_id, "range": header_range, "valueRenderOption": "FORMATTED_VALUE"}});
            let header_result = crate::execute::execute_tool(
                &sheets_doc, get_method, "spreadsheets.values", "get",
                &header_args, "sheets", policy, meta, None, None, false,
                &mut state.token_cache,
            ).await?;
            let headers: Vec<String> = header_result.pointer("/values/0")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let rows = formula_result.get("values").and_then(|v| v.as_array());
            let mut columns_with_formulas: Vec<Value> = Vec::new();
            let mut all_formulas: Vec<Value> = Vec::new();

            if let Some(rows) = rows {
                let col_count = rows.iter().map(|r| r.as_array().map(|a| a.len()).unwrap_or(0)).max().unwrap_or(0);
                for col_idx in 0..col_count {
                    let header = headers.get(col_idx).cloned().unwrap_or_else(|| {
                        let mut s = String::new();
                        let mut c = col_idx;
                        loop { s.insert(0, (b'A' + (c % 26) as u8) as char); if c < 26 { break; } c = c / 26 - 1; }
                        s
                    });
                    let mut formula_count = 0;
                    let mut samples = Vec::new();
                    for (row_idx, row) in rows.iter().enumerate() {
                        if let Some(cell) = row.as_array().and_then(|a| a.get(col_idx)).and_then(|v| v.as_str()) {
                            if cell.starts_with('=') {
                                formula_count += 1;
                                let cell_ref = format!("{}{}", {
                                    let mut s = String::new();
                                    let mut c = col_idx;
                                    loop { s.insert(0, (b'A' + (c % 26) as u8) as char); if c < 26 { break; } c = c / 26 - 1; }
                                    s
                                }, row_idx + 1);
                                all_formulas.push(json!({"cell": cell_ref, "formula": cell}));
                                if samples.len() < 3 { samples.push(cell.to_string()); }
                            }
                        }
                    }
                    if formula_count > 0 {
                        columns_with_formulas.push(json!({
                            "column": header,
                            "formula_count": formula_count,
                            "samples": samples
                        }));
                    }
                }
            }

            let output = json!({
                "columns_with_formulas": columns_with_formulas,
                "total_formulas": all_formulas.len(),
                "all_formulas": all_formulas
            });
            Ok(json!({
                "content": [{"type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default()}],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_sheets_trace" | "gws_sheets_explain" => {
            let spreadsheet_id = spreadsheet_id_opt.unwrap();
            let cell = arguments
                .get("cell")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'cell' (e.g. 'B5')".into()))?;
            let sheet = arguments
                .get("sheet")
                .and_then(|v| v.as_str())
                .unwrap_or("Sheet1");
            let values_resource = tools::find_resource(&sheets_doc.resources, "spreadsheets.values")
                .ok_or_else(|| {
                    GwsError::Validation("spreadsheets.values resource not found".into())
                })?;
            let get_method = values_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;

            // Read the cell's formula
            let cell_range = crate::sheets_helpers::build_range(cell, Some(sheet));
            let cell_args = json!({
                "params": {
                    "spreadsheetId": spreadsheet_id,
                    "range": cell_range,
                    "valueRenderOption": "FORMULA"
                }
            });
            let cell_result = crate::execute::execute_tool(
                &sheets_doc, get_method, "spreadsheets.values", "get",
                &cell_args, "sheets", policy, meta, None, None, false,
                &mut state.token_cache,
            ).await?;

            let formula = cell_result
                .pointer("/values/0/0")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_formula = formula.starts_with('=');

            if tool_name == "gws_sheets_explain" {
                // Read headers (row 1) and row labels (column A)
                let header_args = json!({
                    "params": {
                        "spreadsheetId": spreadsheet_id,
                        "range": crate::sheets_helpers::build_range("1:1", Some(sheet)),
                        "valueRenderOption": "FORMATTED_VALUE"
                    }
                });
                let header_result = crate::execute::execute_tool(
                    &sheets_doc, get_method, "spreadsheets.values", "get",
                    &header_args, "sheets", policy, meta, None, None, false,
                    &mut state.token_cache,
                ).await?;
                let headers: Vec<String> = header_result
                    .pointer("/values/0")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                let label_args = json!({
                    "params": {
                        "spreadsheetId": spreadsheet_id,
                        "range": crate::sheets_helpers::build_range("A2:A100", Some(sheet)),
                        "valueRenderOption": "FORMATTED_VALUE"
                    }
                });
                let label_result = crate::execute::execute_tool(
                    &sheets_doc, get_method, "spreadsheets.values", "get",
                    &label_args, "sheets", policy, meta, None, None, false,
                    &mut state.token_cache,
                ).await?;
                let row_labels: Vec<String> = label_result
                    .get("values")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|row| row.get(0).and_then(|v| v.as_str()).map(String::from)).collect())
                    .unwrap_or_default();

                let explanation = if is_formula {
                    crate::sheets_helpers::explain_formula(formula, &headers, &row_labels)
                } else {
                    format!("Cell {cell} contains the value: {formula}")
                };

                let refs = crate::sheets_helpers::extract_cell_references(formula);
                let referenced: Vec<Value> = refs.iter().map(|r| {
                    let name = crate::sheets_helpers::resolve_cell_name(r, &headers, &row_labels);
                    json!({
                        "ref": r,
                        "column": name.as_ref().map(|(c, _)| c.as_str()).unwrap_or(""),
                        "row_label": name.as_ref().map(|(_, r)| r.as_str()).unwrap_or(""),
                    })
                }).collect();

                let output = json!({
                    "cell": cell,
                    "formula": formula,
                    "explanation": explanation,
                    "referenced_cells": referenced,
                });
                Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                    "structuredContent": output,
                    "isError": false
                }))
            } else {
                // gws_sheets_trace — single-level dependency extraction
                fn build_trace_tree(
                    cell: &str,
                    formula: &str,
                    is_formula: bool,
                ) -> Value {
                    if !is_formula {
                        return json!({
                            "cell": cell,
                            "value": formula,
                            "type": if formula.is_empty() { "empty" } else { "value" },
                        });
                    }
                    let refs = crate::sheets_helpers::extract_cell_references(formula);
                    json!({
                        "cell": cell,
                        "formula": formula,
                        "type": "formula",
                        "references": refs,
                    })
                }

                let tree = build_trace_tree(cell, formula, is_formula);

                let output = json!({
                    "cell": cell,
                    "formula": if is_formula { formula } else { "" },
                    "type": if is_formula { "formula" } else if formula.is_empty() { "empty" } else { "value" },
                    "references": if is_formula { crate::sheets_helpers::extract_cell_references(formula) } else { vec![] },
                    "note": if is_formula {
                        format!("Formula references {} cells. Use gws_sheets_explain for a human-readable explanation.", crate::sheets_helpers::extract_cell_references(formula).len())
                    } else {
                        format!("Cell contains a plain value: {formula}")
                    },
                });
                Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                    "structuredContent": output,
                    "isError": false
                }))
            }
        }

        _ => Err(GwsError::Validation(format!(
            "Unknown sheets helper: {tool_name}"
        ))),
    }
}

async fn execute_slides_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    if tool_name == "gws_slides_import_marp" {
        return execute_slides_import_marp(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_read" {
        return execute_slides_read(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_add" {
        return execute_slides_add(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_delete" {
        return execute_slides_delete(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_reorder" {
        return execute_slides_reorder(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_duplicate" {
        return execute_slides_duplicate(arguments, policy, meta, state, dry_run).await;
    }
    if tool_name == "gws_slides_update" {
        return execute_slides_update(arguments, policy, meta, state, dry_run).await;
    }
    Err(GwsError::Validation(format!(
        "Unknown slides helper: {tool_name}"
    )))
}

async fn execute_slides_import_marp(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let marp_source = arguments
        .get("marp")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'marp' argument".into()))?;

    let presentation_id_arg = arguments.get("presentation_id").and_then(|v| v.as_str());
    let title = arguments.get("title").and_then(|v| v.as_str());
    let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());
    let template_arg = arguments
        .get("template")
        .or_else(|| arguments.get("template_id"))
        .and_then(|v| v.as_str());
    let template_id =
        template_arg.and_then(|t| policy.find_template(t).map(|e| e.id.as_str()).or(Some(t)));

    let pres = crate::marp::parse_marp(marp_source)
        .map_err(|e| GwsError::Validation(format!("Marp parse error: {e}")))?;

    // Step A: Resolve or create presentation
    let slides_doc = state.get_doc("slides").await?;
    let drive_doc = state.get_doc("drive").await?;

    let presentation_id: String;
    let mut created_new = false;

    if let Some(pid) = presentation_id_arg {
        presentation_id = pid.to_string();
    } else if let Some(tmpl_id) = template_id {
        // Copy template presentation via Drive
        let files_resource = tools::find_resource(&drive_doc.resources, "files")
            .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;
        let copy_method = files_resource
            .methods
            .get("copy")
            .ok_or_else(|| GwsError::Validation("Drive files.copy method not found".into()))?;

        let mut copy_body = json!({});
        if let Some(t) = title {
            copy_body["name"] = json!(t);
        }
        if let Some(fid) = folder_id {
            copy_body["parents"] = json!([fid]);
        }

        let copy_args = json!({
            "params": { "fileId": tmpl_id },
            "body": copy_body
        });

        let mut tc = state.token_cache.take();
        let copy_result = crate::execute::execute_tool(
            &drive_doc,
            copy_method,
            "files",
            "copy",
            &copy_args,
            "drive",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        presentation_id = copy_result
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GwsError::Validation("Drive copy did not return an ID".into()))?
            .to_string();
        created_new = true;
    } else if let Some(t) = title {
        // Search for existing presentation or create new one
        let files_resource = tools::find_resource(&drive_doc.resources, "files")
            .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;

        let mut query = format!(
            "name = '{}' and mimeType = 'application/vnd.google-apps.presentation' and trashed = false",
            t.replace('\'', "\\'")
        );
        if let Some(fid) = folder_id {
            query.push_str(&format!(" and '{}' in parents", fid.replace('\'', "\\'")));
        }

        let list_method = files_resource
            .methods
            .get("list")
            .ok_or_else(|| GwsError::Validation("Drive files.list method not found".into()))?;

        let list_args = json!({ "params": { "q": query } });
        let mut tc = state.token_cache.take();
        let list_result = crate::execute::execute_tool(
            &drive_doc,
            list_method,
            "files",
            "list",
            &list_args,
            "drive",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        let files = list_result
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if let Some(existing) = files.first() {
            presentation_id = existing
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Existing file has no ID".into()))?
                .to_string();
        } else {
            // Create new presentation
            let presentations_resource =
                tools::find_resource(&slides_doc.resources, "presentations").ok_or_else(|| {
                    GwsError::Validation("Slides presentations resource not found".into())
                })?;
            let create_method = presentations_resource
                .methods
                .get("create")
                .ok_or_else(|| {
                    GwsError::Validation("Slides presentations.create not found".into())
                })?;

            let create_args = json!({
                "body": { "title": t }
            });

            let mut tc = state.token_cache.take();
            let create_result = crate::execute::execute_tool(
                &slides_doc,
                create_method,
                "presentations",
                "create",
                &create_args,
                "slides",
                policy,
                meta,
                None,
                None,
                dry_run,
                &mut tc,
            )
            .await?;
            state.token_cache = tc;

            presentation_id = create_result
                .get("presentationId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    let keys: Vec<_> = create_result.as_object().map(|o| o.keys().collect()).unwrap_or_default();
                    GwsError::Validation(format!(
                        "Create did not return presentationId. Response keys: {:?}",
                        keys
                    ))
                })?
                .to_string();
            created_new = true;

            if let Some(fid) = folder_id {
                let drive_doc = state.get_doc("drive").await?;
                let files_resource =
                    tools::find_resource(&drive_doc.resources, "files").ok_or_else(|| {
                        GwsError::Validation("Drive files resource not found".into())
                    })?;
                let update_method = files_resource
                    .methods
                    .get("update")
                    .ok_or_else(|| GwsError::Validation("Drive files.update not found".into()))?;
                let move_args = json!({
                    "params": {
                        "fileId": &presentation_id,
                        "addParents": fid
                    }
                });
                let mut tc = state.token_cache.take();
                let _move_result = crate::execute::execute_tool(
                    &drive_doc,
                    update_method,
                    "files",
                    "update",
                    &move_args,
                    "drive",
                    policy,
                    meta,
                    None,
                    None,
                    dry_run,
                    &mut tc,
                )
                .await?;
                state.token_cache = tc;
            }
        }
    } else {
        return Err(GwsError::Validation(
            "One of 'presentation_id', 'title', or 'template_id' is required".into(),
        ));
    }

    if dry_run {
        return Ok(json!({
            "dry_run": true,
            "presentation_id": presentation_id,
            "slide_count": pres.slides.len()
        }));
    }

    // Step B: Fetch presentation, extract layouts, collect existing slide IDs
    let (template_layouts, existing_slide_ids) = {
        let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
            .ok_or_else(|| {
            GwsError::Validation("Slides presentations resource not found".into())
        })?;
        let get_method = presentations_resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;

        let get_args = json!({ "params": { "presentationId": &presentation_id } });
        let mut tc = state.token_cache.take();
        let get_result = crate::execute::execute_tool(
            &slides_doc,
            get_method,
            "presentations",
            "get",
            &get_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;
        check_api_result(&get_result)?;

        let layouts = if template_id.is_some() {
            crate::slides_helpers::extract_layouts(&get_result)
        } else {
            Vec::new()
        };

        let existing_slide_ids: Vec<String> = get_result
            .get("slides")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        s.get("objectId")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default();

        (layouts, existing_slide_ids)
    };

    // Step C: Generate slide create requests, then batch cleanup + creation
    let layouts_ref = if template_layouts.is_empty() {
        None
    } else {
        Some(template_layouts.as_slice())
    };
    let (create_reqs, mut content_reqs) =
        crate::slides_helpers::marp_to_slide_requests(&pres, None, layouts_ref);

    let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let batch_method = presentations_resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("Slides batchUpdate not found".into()))?;

    // Step C1: Delete existing slides (create temp slide first, then delete all old ones)
    if !existing_slide_ids.is_empty() {
        let mut cleanup_reqs = vec![json!({
            "createSlide": { "objectId": "temp_cleanup_slide" }
        })];
        for old_id in &existing_slide_ids {
            cleanup_reqs.push(json!({ "deleteObject": { "objectId": old_id } }));
        }
        let batch_args = json!({
            "params": { "presentationId": &presentation_id },
            "body": { "requests": cleanup_reqs }
        });
        let mut tc = state.token_cache.take();
        let cleanup_result = crate::execute::execute_tool(
            &slides_doc,
            batch_method,
            "presentations",
            "batchUpdate",
            &batch_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;
        check_api_result(&cleanup_result)?;
    }

    // Step C2: Create new slides and delete the temp cleanup slide
    if !create_reqs.is_empty() {
        let mut pass1_reqs = create_reqs;
        if !existing_slide_ids.is_empty() {
            pass1_reqs.push(json!({
                "deleteObject": { "objectId": "temp_cleanup_slide" }
            }));
        }

        let batch_args = json!({
            "params": { "presentationId": &presentation_id },
            "body": { "requests": pass1_reqs }
        });
        let mut tc = state.token_cache.take();
        let create_result = crate::execute::execute_tool(
            &slides_doc,
            batch_method,
            "presentations",
            "batchUpdate",
            &batch_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;
        check_api_result(&create_result)?;
    }

    // Step D: Fetch presentation to get speaker notes object IDs
    let has_notes = pres.slides.iter().any(|s| s.speaker_notes.is_some());
    if has_notes {
        let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
            .ok_or_else(|| {
            GwsError::Validation("Slides presentations resource not found".into())
        })?;
        let get_method = presentations_resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;

        let get_args = json!({ "params": { "presentationId": &presentation_id } });
        let mut tc = state.token_cache.take();
        let get_result = crate::execute::execute_tool(
            &slides_doc,
            get_method,
            "presentations",
            "get",
            &get_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        let slides_arr = get_result
            .get("slides")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let notes_ids: Vec<String> = slides_arr
            .iter()
            .filter_map(|s| {
                s.get("slideProperties")
                    .and_then(|sp| sp.get("notesPage"))
                    .and_then(|np| np.get("notesProperties"))
                    .and_then(|npp| npp.get("speakerNotesObjectId"))
                    .and_then(|id| id.as_str())
                    .map(String::from)
            })
            .collect();

        let (_, notes_content_reqs) =
            crate::slides_helpers::marp_to_slide_requests(&pres, Some(&notes_ids), None);
        // Only add notes-related requests (insertText targeting notes IDs)
        for req in notes_content_reqs {
            if let Some(insert) = req.get("insertText") {
                if let Some(obj_id) = insert.get("objectId").and_then(|v| v.as_str()) {
                    if notes_ids.contains(&obj_id.to_string()) {
                        content_reqs.push(req);
                    }
                }
            }
        }
    }

    // Step E: Remap placeholder IDs to actual server-assigned IDs
    // When createSlide uses placeholderIdMappings, the API may assign different IDs
    // if the layout's placeholder type/index doesn't match exactly
    if layouts_ref.is_some() && !content_reqs.is_empty() {
        let refreshed = fetch_presentation(&presentation_id, state, policy, meta).await?;
        let refreshed_slides = refreshed
            .get("slides")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        let mut content_json = serde_json::to_string(&content_reqs).unwrap_or_default();
        for (idx, slide) in refreshed_slides.iter().enumerate() {
            let elements = slide
                .get("pageElements")
                .and_then(|pe| pe.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(real_title) = crate::slides_helpers::find_title_object_id(&elements) {
                content_json = content_json.replace(
                    &format!("\"title_{idx}\""),
                    &format!("\"{}\"", real_title),
                );
            }
            if let Some(real_body) = crate::slides_helpers::find_body_object_id(&elements) {
                content_json = content_json.replace(
                    &format!("\"body_{idx}\""),
                    &format!("\"{}\"", real_body),
                );
            }
        }
        if let Ok(fixed) = serde_json::from_str::<Vec<Value>>(&content_json) {
            content_reqs = fixed;
        }
    }

    // Step F: Execute pass 2 — content, styling, backgrounds, notes
    if !content_reqs.is_empty() {
        let presentations_resource = tools::find_resource(&slides_doc.resources, "presentations")
            .ok_or_else(|| {
            GwsError::Validation("Slides presentations resource not found".into())
        })?;
        let batch_method = presentations_resource
            .methods
            .get("batchUpdate")
            .ok_or_else(|| GwsError::Validation("Slides batchUpdate not found".into()))?;

        let batch_args = json!({
            "params": { "presentationId": &presentation_id },
            "body": { "requests": content_reqs }
        });
        let mut tc = state.token_cache.take();
        let result = crate::execute::execute_tool(
            &slides_doc,
            batch_method,
            "presentations",
            "batchUpdate",
            &batch_args,
            "slides",
            policy,
            meta,
            None,
            None,
            false,
            &mut tc,
        )
        .await?;
        state.token_cache = tc;

        let mut final_result = result;
        final_result["presentation_id"] = json!(presentation_id);
        final_result["slide_count"] = json!(pres.slides.len());
        if created_new {
            final_result["created_presentation_id"] = json!(&presentation_id);
        }
        final_result["url"] = json!(format!(
            "https://docs.google.com/presentation/d/{}/edit",
            presentation_id
        ));
        return Ok(final_result);
    }

    Ok(json!({
        "presentation_id": presentation_id,
        "slide_count": pres.slides.len(),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    }))
}

async fn fetch_presentation(
    presentation_id: &str,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<Value, GwsError> {
    let slides_doc = state.get_doc("slides").await?;
    let resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;
    let args = json!({ "params": { "presentationId": presentation_id } });
    let mut tc = state.token_cache.take();
    let result = crate::execute::execute_tool(
        &slides_doc,
        get_method,
        "presentations",
        "get",
        &args,
        "slides",
        policy,
        meta,
        None,
        None,
        false,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;
    check_api_result(&result)?;
    Ok(result)
}

fn extract_slide_object_ids(presentation: &Value) -> Vec<String> {
    presentation
        .get("slides")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.get("objectId").and_then(|id| id.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

async fn slides_batch_update(
    presentation_id: &str,
    requests: Vec<Value>,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let slides_doc = state.get_doc("slides").await?;
    let resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let batch_method = resource
        .methods
        .get("batchUpdate")
        .ok_or_else(|| GwsError::Validation("Slides presentations.batchUpdate not found".into()))?;
    let batch_args = json!({
        "params": { "presentationId": presentation_id },
        "body": { "requests": requests }
    });
    let mut tc = state.token_cache.take();
    let result = crate::execute::execute_tool(
        &slides_doc,
        batch_method,
        "presentations",
        "batchUpdate",
        &batch_args,
        "slides",
        policy,
        meta,
        None,
        None,
        dry_run,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;
    check_api_result(&result)?;
    Ok(result)
}

async fn fetch_slide_summary(
    presentation_id: &str,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<Value, GwsError> {
    let slides_doc = state.get_doc("slides").await?;
    let resource = tools::find_resource(&slides_doc.resources, "presentations")
        .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
    let get_method = resource
        .methods
        .get("get")
        .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;
    let args = json!({
        "params": {
            "presentationId": presentation_id,
            "fields": "slides(objectId,pageElements(shape(placeholder,text)))"
        }
    });
    let mut tc = state.token_cache.take();
    let result = crate::execute::execute_tool(
        &slides_doc, get_method, "presentations", "get", &args, "slides",
        policy, meta, None, None, false, &mut tc,
    )
    .await?;
    state.token_cache = tc;
    check_api_result(&result)?;

    let slides = result.get("slides").and_then(|s| s.as_array()).cloned().unwrap_or_default();
    let mut summary = Vec::new();
    for (i, slide) in slides.iter().enumerate() {
        let elements = slide.get("pageElements").and_then(|pe| pe.as_array()).cloned().unwrap_or_default();
        let title = {
            let t = crate::slides_helpers::extract_slide_text(&elements, "TITLE");
            if t.is_empty() {
                let t2 = crate::slides_helpers::extract_slide_text(&elements, "CENTERED_TITLE");
                if t2.is_empty() {
                    if let Some(title_id) = crate::slides_helpers::find_title_object_id(&elements) {
                        elements.iter()
                            .find(|e| e.get("objectId").and_then(|id| id.as_str()) == Some(&title_id))
                            .map(|e| crate::slides_helpers::extract_text_from_shape(e))
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else { t2 }
            } else { t }
        };
        summary.push(json!({
            "slide_number": i + 1,
            "object_id": slide.get("objectId").and_then(|v| v.as_str()).unwrap_or(""),
            "title": title
        }));
    }
    Ok(json!({ "slide_count": slides.len(), "slides": summary }))
}

async fn execute_slides_read(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    _dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_number = arguments
        .get("slide_number")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let format = arguments
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;

    if format == "markdown" {
        let md = crate::slides_helpers::presentation_to_markdown(&pres_data);
        return Ok(json!({
            "content": [{ "type": "text", "text": md }],
            "isError": false
        }));
    }

    let structured = crate::slides_helpers::presentation_to_structured(&pres_data, slide_number)
        .map_err(|e| GwsError::Validation(e))?;

    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&structured).unwrap_or_default() }],
        "structuredContent": structured,
        "isError": false
    }))
}

async fn execute_slides_delete(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_numbers: Vec<usize> = arguments
        .get("slide_numbers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_numbers' argument".into()))?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();

    if slide_numbers.is_empty() {
        return Err(GwsError::Validation(
            "slide_numbers must contain at least one slide number".into(),
        ));
    }

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    for &num in &slide_numbers {
        if num < 1 || num > slide_count {
            return Err(GwsError::Validation(format!(
                "Slide number {num} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
            )));
        }
    }

    let mut unique_numbers: Vec<usize> = slide_numbers.clone();
    unique_numbers.sort();
    unique_numbers.dedup();

    if unique_numbers.len() >= slide_count {
        return Err(GwsError::Validation(
            "Cannot delete all slides. A presentation must have at least one slide.".into(),
        ));
    }

    let mut delete_reqs = Vec::new();
    let mut deleted_ids = Vec::new();
    for &num in &unique_numbers {
        let obj_id = &slide_ids[num - 1];
        delete_reqs.push(json!({ "deleteObject": { "objectId": obj_id } }));
        deleted_ids.push(obj_id.clone());
    }

    slides_batch_update(presentation_id, delete_reqs, state, policy, meta, dry_run).await?;

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "deleted": deleted_ids,
        "remaining_slides": summary.get("slide_count"),
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_reorder(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_numbers: Vec<usize> = arguments
        .get("slide_numbers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_numbers' argument".into()))?
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as usize))
        .collect();
    let position = arguments
        .get("position")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| GwsError::Validation("Missing 'position' argument".into()))?
        as usize;

    if slide_numbers.is_empty() {
        return Err(GwsError::Validation(
            "slide_numbers must contain at least one slide number".into(),
        ));
    }

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    for &num in &slide_numbers {
        if num < 1 || num > slide_count {
            return Err(GwsError::Validation(format!(
                "Slide number {num} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
            )));
        }
    }
    if position < 1 || position > slide_count {
        return Err(GwsError::Validation(format!(
            "Position {position} is out of range. Must be 1-{slide_count}."
        )));
    }

    let move_ids: Vec<String> = slide_numbers.iter().map(|&n| slide_ids[n - 1].clone()).collect();
    let reqs = vec![json!({
        "updateSlidesPosition": {
            "slideObjectIds": move_ids,
            "insertionIndex": position - 1
        }
    })];

    slides_batch_update(presentation_id, reqs, state, policy, meta, dry_run).await?;

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "moved": move_ids,
        "position": position,
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_add(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let marp_source = arguments
        .get("marp")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'marp' argument".into()))?;
    let position = arguments
        .get("position")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    if marp_source.contains("\n---\n") || marp_source.contains("\r\n---\r\n") {
        return Err(GwsError::Validation(
            "Only single-slide Marp is supported. Remove '---' separators or use gws_slides_import_marp for multi-slide content.".into(),
        ));
    }

    let is_blank = marp_source.trim().is_empty()
        || marp_source.trim() == "---\nmarp: true\n---"
        || marp_source.trim().chars().all(|c| c.is_whitespace());

    let pres = crate::marp::parse_marp(marp_source)
        .map_err(|e| GwsError::Validation(format!("Failed to parse Marp: {e}")))?;

    if pres.slides.len() != 1 {
        return Err(GwsError::Validation(format!(
            "Expected 1 slide, got {}. Use gws_slides_import_marp for multi-slide content.",
            pres.slides.len()
        )));
    }

    let has_content = !is_blank
        && (pres.slides[0].title.is_some() || !pres.slides[0].body_blocks.is_empty());

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    if let Some(pos) = position {
        if pos < 1 || pos > slide_count + 1 {
            return Err(GwsError::Validation(format!(
                "Position {pos} is out of range. Must be 1-{}.",
                slide_count + 1
            )));
        }
    }

    let suffix = format!("_{:08x}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0));
    let new_slide_id = format!("slide_0{suffix}");

    if has_content {
        let template_name = arguments.get("template").and_then(|v| v.as_str());
        let layouts = if template_name.is_some() {
            crate::slides_helpers::extract_layouts(&pres_data)
        } else {
            crate::slides_helpers::extract_layouts(&pres_data)
        };
        let layouts_ref = if layouts.is_empty() {
            None
        } else {
            Some(layouts.as_slice())
        };

        let (mut create_reqs, mut content_reqs) =
            crate::slides_helpers::marp_to_slide_requests(&pres, None, layouts_ref);

        let rewrite_ids = |reqs: &mut [Value]| {
            let json_str = serde_json::to_string(&reqs).unwrap_or_default();
            let replaced = json_str
                .replace("\"slide_0\"", &format!("\"slide_0{suffix}\""))
                .replace("\"title_0\"", &format!("\"title_0{suffix}\""))
                .replace("\"body_0\"", &format!("\"body_0{suffix}\""))
                .replace("\"table_0_", &format!("\"table_0{suffix}_"));
            if let Ok(parsed) = serde_json::from_str::<Vec<Value>>(&replaced) {
                for (i, v) in parsed.into_iter().enumerate() {
                    if i < reqs.len() {
                        reqs[i] = v;
                    }
                }
            }
        };
        rewrite_ids(&mut create_reqs);
        rewrite_ids(&mut content_reqs);

        if let Some(pos) = position {
            if let Some(req) = create_reqs.get_mut(0) {
                if let Some(create_slide) = req.get_mut("createSlide") {
                    create_slide["insertionIndex"] = json!(pos - 1);
                }
            }
        }

        slides_batch_update(presentation_id, create_reqs, state, policy, meta, dry_run).await?;

    if !content_reqs.is_empty() {
        // Re-fetch the slide to discover actual placeholder IDs (mapped IDs may differ
        // from server-assigned ones when placeholderIdMappings don't match exactly)
        let refreshed = fetch_presentation(presentation_id, state, policy, meta).await?;
        let refreshed_slides = refreshed
            .get("slides")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        let target_idx = position.map(|p| p - 1).unwrap_or(refreshed_slides.len().saturating_sub(1));
        if let Some(new_slide) = refreshed_slides.get(target_idx) {
            let elements = new_slide
                .get("pageElements")
                .and_then(|pe| pe.as_array())
                .cloned()
                .unwrap_or_default();
            let actual_title = crate::slides_helpers::find_title_object_id(&elements);
            let actual_body = crate::slides_helpers::find_body_object_id(&elements);

            // Rewrite content requests to use actual IDs instead of generated ones
            let content_json = serde_json::to_string(&content_reqs).unwrap_or_default();
            let mut replaced = content_json;
            if let Some(ref real_title) = actual_title {
                replaced = replaced.replace(
                    &format!("\"title_0{suffix}\""),
                    &format!("\"{}\"", real_title),
                );
            }
            if let Some(ref real_body) = actual_body {
                replaced = replaced.replace(
                    &format!("\"body_0{suffix}\""),
                    &format!("\"{}\"", real_body),
                );
            }
            if let Ok(fixed) = serde_json::from_str::<Vec<Value>>(&replaced) {
                content_reqs = fixed;
            }

            // Filter content requests to only those targeting existing elements
            let existing_ids: std::collections::HashSet<String> = elements.iter()
                .filter_map(|e| e.get("objectId").and_then(|id| id.as_str()).map(String::from))
                .collect();
            content_reqs.retain(|req| {
                let obj_id = req.as_object()
                    .and_then(|m| m.values().next())
                    .and_then(|v| v.get("objectId"))
                    .and_then(|id| id.as_str())
                    .unwrap_or("");
                obj_id.is_empty() || existing_ids.contains(obj_id)
            });
        }

        if !content_reqs.is_empty() {
            slides_batch_update(presentation_id, content_reqs, state, policy, meta, dry_run).await?;
        }
    }

        if pres.slides[0].speaker_notes.is_some() {
            let updated_pres = fetch_presentation(presentation_id, state, policy, meta).await?;
            let updated_slides = updated_pres
                .get("slides")
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default();

            let target_idx = position.map(|p| p - 1).unwrap_or(updated_slides.len().saturating_sub(1));
            if let Some(slide) = updated_slides.get(target_idx) {
                let notes_obj_id = slide
                    .get("slideProperties")
                    .and_then(|sp| sp.get("notesPage"))
                    .and_then(|np| np.get("notesProperties"))
                    .and_then(|np| np.get("speakerNotesObjectId"))
                    .and_then(|id| id.as_str());
                if let (Some(notes_id), Some(notes_text)) = (notes_obj_id, &pres.slides[0].speaker_notes) {
                    let notes_reqs = vec![json!({
                        "insertText": {
                            "objectId": notes_id,
                            "text": notes_text
                        }
                    })];
                    slides_batch_update(presentation_id, notes_reqs, state, policy, meta, dry_run).await?;
                }
            }
        }
    } else {
        // Blank slide — just create it without content
        let mut create_req = json!({ "createSlide": { "objectId": &new_slide_id } });
        if let Some(pos) = position {
            create_req["createSlide"]["insertionIndex"] = json!(pos - 1);
        }
        slides_batch_update(presentation_id, vec![create_req], state, policy, meta, dry_run).await?;
    }

    if let Some(bg) = arguments.get("background_image").and_then(|v| v.as_str()) {
        let bg_url = if bg.starts_with("http") {
            bg.to_string()
        } else {
            format!("https://drive.google.com/uc?export=download&id={bg}")
        };
        let bg_reqs = vec![json!({
            "updatePageProperties": {
                "objectId": &new_slide_id,
                "pageProperties": {
                    "pageBackgroundFill": {
                        "stretchedPictureFill": { "contentUrl": bg_url }
                    }
                },
                "fields": "pageBackgroundFill"
            }
        })];
        slides_batch_update(presentation_id, bg_reqs, state, policy, meta, dry_run).await?;
    }

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "presentation_id": presentation_id,
        "slide_object_id": new_slide_id,
        "position": position.unwrap_or(slide_count + 1),
        "slide_count": summary.get("slide_count"),
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_duplicate(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_number = arguments
        .get("slide_number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_number' argument".into()))?
        as usize;
    let position = arguments
        .get("position")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slide_ids = extract_slide_object_ids(&pres_data);
    let slide_count = slide_ids.len();

    if slide_number < 1 || slide_number > slide_count {
        return Err(GwsError::Validation(format!(
            "Slide number {slide_number} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
        )));
    }

    let source_id = &slide_ids[slide_number - 1];
    let dup_reqs = vec![json!({ "duplicateObject": { "objectId": source_id } })];
    let dup_result =
        slides_batch_update(presentation_id, dup_reqs, state, policy, meta, dry_run).await?;

    let new_slide_id = dup_result
        .get("replies")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|r| r.get("duplicateObject"))
        .and_then(|d| d.get("objectId"))
        .and_then(|id| id.as_str())
        .unwrap_or("unknown")
        .to_string();

    if let Some(pos) = position {
        if pos < 1 || pos > slide_count + 1 {
            return Err(GwsError::Validation(format!(
                "Position {pos} is out of range. Must be 1-{}.",
                slide_count + 1
            )));
        }
        let move_reqs = vec![json!({
            "updateSlidesPosition": {
                "slideObjectIds": [&new_slide_id],
                "insertionIndex": pos - 1
            }
        })];
        slides_batch_update(presentation_id, move_reqs, state, policy, meta, dry_run).await?;
    }

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "duplicated_from": source_id,
        "new_slide_id": new_slide_id,
        "slides": summary.get("slides"),
        "slide_count": summary.get("slide_count"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_slides_update(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'presentation_id' argument".into()))?;
    let slide_number = arguments
        .get("slide_number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| GwsError::Validation("Missing 'slide_number' argument".into()))?
        as usize;
    let new_title = arguments.get("title").and_then(|v| v.as_str());
    let new_body = arguments.get("body").and_then(|v| v.as_str());
    let new_notes = arguments.get("notes").and_then(|v| v.as_str());
    let placeholders_map = arguments.get("placeholders").and_then(|v| v.as_object());

    if new_title.is_none() && new_body.is_none() && new_notes.is_none() && placeholders_map.is_none() {
        return Err(GwsError::Validation(
            "At least one of 'title', 'body', 'notes', or 'placeholders' must be provided".into(),
        ));
    }

    let pres_data = fetch_presentation(presentation_id, state, policy, meta).await?;
    let slides = pres_data
        .get("slides")
        .and_then(|s| s.as_array())
        .ok_or_else(|| GwsError::Validation("Presentation has no slides".into()))?;
    let slide_count = slides.len();

    if slide_number < 1 || slide_number > slide_count {
        return Err(GwsError::Validation(format!(
            "Slide number {slide_number} is out of range. Presentation has {slide_count} slides (1-{slide_count})."
        )));
    }

    let slide = &slides[slide_number - 1];
    let page_elements = slide
        .get("pageElements")
        .and_then(|pe| pe.as_array())
        .cloned()
        .unwrap_or_default();

    let mut update_reqs: Vec<Value> = Vec::new();

    if let Some(title_text) = new_title {
        let title_id = crate::slides_helpers::find_title_object_id(&page_elements);
        if let Some(obj_id) = title_id {
            let existing_title = crate::slides_helpers::extract_slide_text(&page_elements, "TITLE");
            let existing_centered = crate::slides_helpers::extract_slide_text(&page_elements, "CENTERED_TITLE");
            let existing_fallback = if existing_title.is_empty() && existing_centered.is_empty() {
                crate::slides_helpers::extract_all_body_text(&page_elements)
            } else {
                String::new()
            };
            let has_existing = !existing_title.is_empty() || !existing_centered.is_empty() || !existing_fallback.is_empty();
            if has_existing {
                update_reqs.push(json!({
                    "deleteText": { "objectId": &obj_id, "textRange": { "type": "ALL" } }
                }));
            }
            update_reqs.push(json!({
                "insertText": { "objectId": &obj_id, "text": title_text }
            }));
        }
    }

    if let Some(body_text) = new_body {
        let body_id = crate::slides_helpers::find_body_object_id(&page_elements);
        if let Some(obj_id) = body_id {
            let existing = crate::slides_helpers::extract_all_body_text(&page_elements);
            if !existing.is_empty() {
                update_reqs.push(json!({
                    "deleteText": { "objectId": &obj_id, "textRange": { "type": "ALL" } }
                }));
            }

            let parsed = crate::marp::parse_marp(body_text)
                .map_err(|e| GwsError::Validation(format!("Failed to parse body Marp: {e}")))?;
            if let Some(slide_content) = parsed.slides.first() {
                let blocks = &slide_content.body_blocks;
                if !blocks.is_empty() {
                    let (text, styles, bullets) =
                        crate::slides_helpers::build_body_content(blocks, &obj_id);
                    if !text.is_empty() {
                        update_reqs.push(json!({
                            "insertText": { "objectId": &obj_id, "text": text }
                        }));
                        update_reqs.extend(styles);
                        update_reqs.extend(bullets);
                    }
                }
            }
        }
    }

    if let Some(notes_text) = new_notes {
        let notes_obj_id = slide
            .get("slideProperties")
            .and_then(|sp| sp.get("notesPage"))
            .and_then(|np| np.get("notesProperties"))
            .and_then(|np| np.get("speakerNotesObjectId"))
            .and_then(|id| id.as_str());
        if let Some(notes_id) = notes_obj_id {
            let existing = crate::slides_helpers::extract_notes_text(slide);
            if existing.is_some() {
                update_reqs.push(json!({
                    "deleteText": { "objectId": notes_id, "textRange": { "type": "ALL" } }
                }));
            }
            update_reqs.push(json!({
                "insertText": { "objectId": notes_id, "text": notes_text }
            }));
        }
    }

    if let Some(ph_map) = placeholders_map {
        for (label, text_val) in ph_map {
            let text = text_val.as_str().unwrap_or("");
            let obj_id = crate::slides_helpers::find_placeholder_by_label(&page_elements, label)
                .ok_or_else(|| {
                    GwsError::Validation(format!(
                        "Placeholder '{label}' not found on slide {slide_number}. Use gws_templates to see available placeholders."
                    ))
                })?;
            let has_text = page_elements.iter()
                .find(|e| e.get("objectId").and_then(|id| id.as_str()) == Some(&obj_id))
                .map(|e| !crate::slides_helpers::extract_text_from_shape(e).is_empty())
                .unwrap_or(false);
            if has_text {
                update_reqs.push(json!({
                    "deleteText": { "objectId": &obj_id, "textRange": { "type": "ALL" } }
                }));
            }
            update_reqs.push(json!({
                "insertText": { "objectId": &obj_id, "text": text }
            }));
        }
    }

    if update_reqs.is_empty() {
        return Err(GwsError::Validation(
            "No updatable elements found on this slide. The slide may not have the expected placeholder shapes.".into(),
        ));
    }

    slides_batch_update(presentation_id, update_reqs, state, policy, meta, dry_run).await?;

    let summary = fetch_slide_summary(presentation_id, state, policy, meta).await?;
    let result = json!({
        "updated_slide": slide_number,
        "slides": summary.get("slides"),
        "url": format!("https://docs.google.com/presentation/d/{}/edit", presentation_id)
    });
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "structuredContent": result,
        "isError": false
    }))
}

async fn execute_generate_image(
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    let prompt = arguments
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'prompt' argument".into()))?;

    let model = arguments
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini-2.5-flash-image");

    let aspect_ratio = arguments.get("aspect_ratio").and_then(|v| v.as_str());
    let image_size = arguments.get("image_size").and_then(|v| v.as_str());
    let document_id = arguments
        .get("document_id")
        .or_else(|| arguments.get("documentId"))
        .and_then(|v| v.as_str());
    let presentation_id = arguments
        .get("presentation_id")
        .or_else(|| arguments.get("presentationId"))
        .and_then(|v| v.as_str());
    let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());

    if dry_run {
        return Ok(json!({
            "dry_run": true,
            "prompt": prompt,
            "model": model,
            "target": if document_id.is_some() { "document" }
                     else if presentation_id.is_some() { "presentation" }
                     else { "standalone" }
        }));
    }

    let credentials_file = policy.credentials_file.as_deref();
    let mut tc = state.token_cache.take();
    let generated = crate::image_gen::generate_image(
        prompt,
        model,
        aspect_ratio,
        image_size,
        credentials_file,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;

    if let Some(doc_id) = document_id {
        let position = parse_position(arguments);
        let w = arguments.get("width_pt").and_then(|v| v.as_f64());
        let h = arguments.get("height_pt").and_then(|v| v.as_f64());

        let file_id = upload_image_to_drive(&generated, folder_id, policy, meta, state).await?;
        let (public_url, perm_id) = make_image_insertable(&file_id, policy, meta, state).await?;

        let mut reqs = vec![helpers::build_insert_image_request(
            &public_url,
            position,
            w,
            h,
        )];
        reqs.push(
            json!({ "insertText": { "text": "\n", "endOfSegmentLocation": { "segmentId": "" } } }),
        );

        let docs_doc = state.get_doc("docs").await?;
        let resource = tools::find_resource(&docs_doc.resources, "documents")
            .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
        let batch_method = resource
            .methods
            .get("batchUpdate")
            .ok_or_else(|| GwsError::Validation("batchUpdate method not found".into()))?;

        let result = crate::execute::execute_tool(
            &docs_doc,
            batch_method,
            "documents",
            "batchUpdate",
            &json!({"params": {"documentId": doc_id}, "body": {"requests": reqs}}),
            "docs",
            policy,
            meta,
            None,
            None,
            dry_run,
            &mut state.token_cache,
        )
        .await;

        if let Some(pid) = &perm_id {
            revoke_image_sharing(&file_id, pid, policy, meta, state).await;
        }

        return match result {
            Ok(ref r) if check_api_result(r).is_ok() => Ok(json!({
                "content": [{ "type": "text", "text": format!("Image generated and inserted into document {doc_id}") }],
                "isError": false
            })),
            _ => Ok(json!({
                "content": [{ "type": "text", "text": format!(
                    "Image generated and uploaded to Drive (file: {file_id}) but insertion failed. \
                     Enterprise orgs may block public sharing required by the Docs API. \
                     Insert via Docs UI: Insert > Image > Drive."
                )}],
                "isError": true
            })),
        };
    }

    if let Some(pres_id) = presentation_id {
        let file_id = upload_image_to_drive(&generated, folder_id, policy, meta, state).await?;
        return Ok(json!({
            "content": [{ "type": "text", "text": format!(
                "Image generated and uploaded to Drive (file: {file_id}). \
                 Insert into presentation {pres_id} via Slides UI."
            )}],
            "isError": false
        }));
    }

    // Standalone: upload to Drive (no public sharing needed) and return reference
    let file_id = upload_image_to_drive(&generated, folder_id, policy, meta, state).await?;
    let drive_url = format!("https://drive.google.com/file/d/{}/view", file_id);
    Ok(json!({
        "drive_file_id": file_id,
        "drive_url": drive_url,
        "mime_type": generated.mime_type,
        "prompt": prompt,
        "_mcp_content": [
            {
                "type": "image",
                "data": generated.base64_data,
                "mimeType": generated.mime_type
            },
            {
                "type": "text",
                "text": format!("Generated image for prompt: {}\nDrive: {}", prompt, drive_url)
            }
        ]
    }))
}

async fn upload_image_to_drive(
    generated: &crate::image_gen::GeneratedImage,
    folder_id: Option<&str>,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<String, GwsError> {
    let drive_doc = state.get_doc("drive").await?;
    let files_resource = tools::find_resource(&drive_doc.resources, "files")
        .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;
    let create_method = files_resource
        .methods
        .get("create")
        .ok_or_else(|| GwsError::Validation("Drive files.create not found".into()))?;

    let mut body = json!({
        "name": format!("generated_{}.png", chrono_free_timestamp()),
        "mimeType": &generated.mime_type
    });
    if let Some(fid) = folder_id {
        body["parents"] = json!([fid]);
    }

    let upload_args = json!({
        "body": body,
        "media_data": &generated.base64_data,
        "media_content_type": &generated.mime_type
    });
    let mut tc = state.token_cache.take();
    let upload_result = crate::execute::execute_tool(
        &drive_doc,
        create_method,
        "files",
        "create",
        &upload_args,
        "drive",
        policy,
        meta,
        None,
        None,
        false,
        &mut tc,
    )
    .await?;
    state.token_cache = tc;
    check_api_result(&upload_result)?;

    upload_result
        .get("id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| GwsError::Validation("Drive upload did not return file ID".into()))
}

fn chrono_free_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        if let Err(e) = policy.enforce_constraints(service, method_name, method, &mut params, &body) {
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
            None,
            false,
            &mut state.token_cache,
        )
        .await;
        let duration_ms = exec_start.elapsed().as_millis() as u64;

        match exec_result {
            Ok(result) => {
                if let Some(ref a) = audit {
                    a.log_allowed_with_tool(
                        Some("gws_batch"),
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

pub(crate) fn server_instructions() -> &'static str {
    "MCP server for Google Workspace APIs with per-project safety policies. \
     Use gws_discover to explore available services, resources, and methods. \
     Each enabled Google service is exposed as a tool."
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(range, Some((1, 49)));
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
