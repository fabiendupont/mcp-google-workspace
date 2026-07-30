use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use google_workspace::discovery::RestDescription;
use google_workspace::error::GwsError;

use crate::helpers;
use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::shared::{
    ServerState, check_api_result, make_image_insertable, parse_position, policy_for_folder,
    revoke_image_sharing,
};
use crate::tasks;
use crate::tools;

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
        if let Some(svc) = arguments.get("service").and_then(|v| v.as_str())
            && !st.eager_tools
            && st.activated_services.insert(svc.to_string())
        {
            st.tools = None; // Force rebuild on next list_tools
            tracing::info!(service = svc, "Lazy discovery: service activated");
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
        let result =
            crate::drive_helpers::execute_drive_helper(tool_name, arguments, policy, meta, &mut st)
                .await?;
        return Ok(result);
    }

    if tool_name.starts_with("gws_docs_") {
        let mut st = state.lock().await;
        let result = crate::helpers::execute_docs_helper(
            tool_name, arguments, policy, meta, &mut st, dry_run,
        )
        .await?;
        return Ok(result);
    }

    if tool_name.starts_with("gws_sheets_") {
        let mut st = state.lock().await;
        let result = crate::sheets_helpers::execute_sheets_helper(
            tool_name, arguments, policy, meta, &mut st,
        )
        .await?;
        return Ok(result);
    }

    if tool_name.starts_with("gws_gmail_") {
        let mut st = state.lock().await;
        let result =
            crate::gmail_helpers::execute_gmail_helper(tool_name, arguments, policy, meta, &mut st)
                .await?;
        return Ok(result);
    }

    if tool_name.starts_with("gws_calendar_") {
        let mut st = state.lock().await;
        let result = crate::calendar_helpers::execute_calendar_helper(
            tool_name, arguments, policy, meta, &mut st,
        )
        .await?;
        return Ok(result);
    }

    if tool_name == "gws_templates" {
        let mut st = state.lock().await;
        let result =
            crate::slides_helpers::execute_list_templates(Some(arguments), policy, meta, &mut st)
                .await;
        return Ok(result);
    }

    if tool_name.starts_with("gws_slides_") {
        let mut st = state.lock().await;
        let result = crate::slides_helpers::execute_slides_helper(
            tool_name, arguments, policy, meta, &mut st, dry_run,
        )
        .await?;
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

    const HELPER_ONLY_SERVICES: &[&str] =
        &["drive", "docs", "sheets", "slides", "gmail", "calendar"];
    if HELPER_ONLY_SERVICES.contains(&svc_alias) {
        tracing::warn!(
            service = svc_alias,
            "Generic tool blocked — use helper tools instead"
        );
        return Err(GwsError::Validation(format!(
            "Service '{svc_alias}' uses helper tools (gws_{svc_alias}_*). \
             Use gws_discover to see available tools."
        )));
    }

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
    _doc: &RestDescription,
    _policy: &Policy,
    _meta: &RequestMeta,
    _tc: &mut Option<crate::auth::TokenCache>,
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

    let (_effective_policy, resolved_folder) =
        policy_for_folder(folder_id, policy, meta, state).await?;
    let folder_id = resolved_folder.as_deref().or(folder_id);

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
        if let Err(e) = policy.enforce_constraints(service, method_name, method, &mut params, &body)
        {
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
