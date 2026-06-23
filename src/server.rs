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

    if tool_name.starts_with("gws_docs_") {
        let mut st = state.lock().await;
        let result =
            execute_docs_helper(tool_name, arguments, policy, meta, &mut st, dry_run).await?;
        return Ok(result);
    }

    if tool_name == "gws_templates" {
        let mut st = state.lock().await;
        let result = execute_list_templates(policy, meta, &mut st).await;
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
        let result =
            execute_generate_image(arguments, policy, meta, &mut st, dry_run).await?;
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
    if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
        return Err(GwsError::Validation(err.to_string()));
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
            } else if drive_file_id.is_some() || image_data.is_some() {
                return Err(GwsError::Validation(
                    "Google Docs insertInlineImage requires a publicly accessible URL \
                     (2KB URI limit prevents data URIs). Use 'image_url' with a public \
                     URL, or insert images via the Google Docs UI from Drive."
                        .into(),
                ));
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
    .await?;
    check_api_result(&result)?;
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

    // Step A: resolve, find existing, or create the document
    let (doc_id, created_new_doc) = if let Some(id) = doc_id_arg {
        (id.to_string(), false)
    } else if title.is_some() || folder_id.is_some() {
        let doc_title = title.unwrap_or("Untitled");
        let drive_doc = state.get_doc("drive").await?;
        let drive_resource = tools::find_resource(&drive_doc.resources, "files")
            .ok_or_else(|| GwsError::Validation("files resource not found in drive API".into()))?;

        // Check if a doc with this title already exists in the folder
        let existing_id = if let Some(fid) = folder_id {
            let q = format!(
                "name='{}' and '{}' in parents and mimeType='application/vnd.google-apps.document' and trashed=false",
                doc_title.replace('\'', "\\'"),
                fid
            );
            let list_method = drive_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("list method not found".into()))?;
            let list_args = json!({"params": {"q": q, "fields": "files(id)", "pageSize": 1}});
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
                false,
                &mut state.token_cache,
            )
            .await?;
            list_result["files"]
                .as_array()
                .and_then(|f| f.first())
                .and_then(|f| f["id"].as_str())
                .map(|s| s.to_string())
        } else {
            None
        };

        if let Some(existing) = existing_id {
            // Reuse existing doc — clear its content
            let docs_doc = state.get_doc("docs").await?;
            let resource = tools::find_resource(&docs_doc.resources, "documents")
                .ok_or_else(|| GwsError::Validation("documents resource not found".into()))?;
            let get_method = resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
            let get_args = json!({"params": {"documentId": &existing}});
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
            let end_idx = doc_content["body"]["content"]
                .as_array()
                .and_then(|arr| arr.last())
                .and_then(|el| el["endIndex"].as_i64())
                .unwrap_or(2) as i32;
            if end_idx > 2 {
                let batch_method = resource
                    .methods
                    .get("batchUpdate")
                    .ok_or_else(|| GwsError::Validation("batchUpdate not found".into()))?;
                let clear_args = json!({
                    "params": {"documentId": &existing},
                    "body": {"requests": [{"deleteContentRange": {"range": {"startIndex": 1, "endIndex": end_idx - 1}}}]}
                });
                let clear_result = crate::execute::execute_tool(
                    &docs_doc,
                    batch_method,
                    "documents",
                    "batchUpdate",
                    &clear_args,
                    "docs",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                check_api_result(&clear_result)?;
            }
            tracing::info!(doc_id = %existing, title = doc_title, "Reusing existing document");
            (existing, false)
        } else {
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
                policy,
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
            (new_id, true)
        }
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
        None,
        dry_run,
        &mut state.token_cache,
    )
    .await?;
    check_api_result(&result)?;

    // Step E: include doc ID in result (especially useful when a new doc was created)
    if created_new_doc {
        result["created_document_id"] = json!(doc_id);
    }
    result["document_id"] = json!(doc_id);

    Ok(result)
}

async fn execute_list_templates(
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Value {
    let mut templates: Vec<Value> = Vec::new();

    for t in policy.templates() {
        let mut entry = json!({
            "name": t.name,
            "id": t.id
        });
        if let Some(ref desc) = t.description {
            entry["description"] = json!(desc);
        }
        if let Ok(slides_doc) = state.get_doc("slides").await {
            if let Some(pres_resource) = tools::find_resource(&slides_doc.resources, "presentations") {
                if let Some(get_method) = pres_resource.methods.get("get") {
                    let args = json!({ "params": { "presentationId": &t.id } });
                    let mut tc = state.token_cache.take();
                    if let Ok(pres_data) = crate::execute::execute_tool(
                        &slides_doc, get_method, "presentations", "get", &args,
                        "slides", policy, meta, None, None, false, &mut tc,
                    ).await {
                        let layouts = crate::slides_helpers::extract_layouts(&pres_data);
                        let layout_names: Vec<&str> = layouts.iter()
                            .map(|l| l.display_name.as_str())
                            .collect();
                        entry["layouts"] = json!(layout_names);
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
        "hint": "Use the template 'name' or 'id' as the 'template' argument in gws_slides_import_marp"
    })
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
    let template_id = template_arg.and_then(|t| {
        policy.find_template(t).map(|e| e.id.as_str()).or(Some(t))
    });

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
        let files_resource =
            tools::find_resource(&drive_doc.resources, "files")
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
            &drive_doc, copy_method, "files", "copy", &copy_args,
            "drive", policy, meta, None, None, dry_run, &mut tc,
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
        let files_resource =
            tools::find_resource(&drive_doc.resources, "files")
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
            &drive_doc, list_method, "files", "list", &list_args,
            "drive", policy, meta, None, None, dry_run, &mut tc,
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
                tools::find_resource(&slides_doc.resources, "presentations")
                    .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
            let create_method = presentations_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("Slides presentations.create not found".into()))?;

            let create_args = json!({
                "body": { "title": t }
            });

            let mut tc = state.token_cache.take();
            let create_result = crate::execute::execute_tool(
                &slides_doc, create_method, "presentations", "create", &create_args,
                "slides", policy, meta, None, None, dry_run, &mut tc,
            )
            .await?;
            state.token_cache = tc;

            presentation_id = create_result
                .get("presentationId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Create did not return presentationId".into()))?
                .to_string();
            created_new = true;
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
        let presentations_resource =
            tools::find_resource(&slides_doc.resources, "presentations")
                .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
        let get_method = presentations_resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;

        let get_args = json!({ "params": { "presentationId": &presentation_id } });
        let mut tc = state.token_cache.take();
        let get_result = crate::execute::execute_tool(
            &slides_doc, get_method, "presentations", "get", &get_args,
            "slides", policy, meta, None, None, false, &mut tc,
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
                    .filter_map(|s| s.get("objectId").and_then(|id| id.as_str()).map(String::from))
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

    let presentations_resource =
        tools::find_resource(&slides_doc.resources, "presentations")
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
            &slides_doc, batch_method, "presentations", "batchUpdate", &batch_args,
            "slides", policy, meta, None, None, false, &mut tc,
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
            &slides_doc, batch_method, "presentations", "batchUpdate", &batch_args,
            "slides", policy, meta, None, None, false, &mut tc,
        )
        .await?;
        state.token_cache = tc;
        check_api_result(&create_result)?;
    }

    // Step D: Fetch presentation to get speaker notes object IDs
    let has_notes = pres.slides.iter().any(|s| s.speaker_notes.is_some());
    if has_notes {
        let presentations_resource =
            tools::find_resource(&slides_doc.resources, "presentations")
                .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
        let get_method = presentations_resource
            .methods
            .get("get")
            .ok_or_else(|| GwsError::Validation("Slides presentations.get not found".into()))?;

        let get_args = json!({ "params": { "presentationId": &presentation_id } });
        let mut tc = state.token_cache.take();
        let get_result = crate::execute::execute_tool(
            &slides_doc, get_method, "presentations", "get", &get_args,
            "slides", policy, meta, None, None, false, &mut tc,
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

    // Step E: Execute pass 2 — content, styling, backgrounds, notes
    if !content_reqs.is_empty() {
        let presentations_resource =
            tools::find_resource(&slides_doc.resources, "presentations")
                .ok_or_else(|| GwsError::Validation("Slides presentations resource not found".into()))?;
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
            &slides_doc, batch_method, "presentations", "batchUpdate", &batch_args,
            "slides", policy, meta, None, None, false, &mut tc,
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
    let document_id = arguments.get("document_id").and_then(|v| v.as_str());
    let presentation_id = arguments.get("presentation_id").and_then(|v| v.as_str());
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

    // If targeting a document, upload to Drive and return the file ID.
    // Google Docs insertInlineImage requires a publicly accessible URL, which
    // enterprise orgs may block. The image is uploaded to Drive for the user
    // to insert via the Docs UI or gws_docs_insert_image with image_url.
    if let Some(doc_id) = document_id {
        let file_id = upload_image_to_drive(&generated, folder_id, policy, meta, state).await?;
        return Ok(json!({
            "document_id": doc_id,
            "drive_file_id": file_id,
            "drive_url": format!("https://drive.google.com/file/d/{}/view", file_id),
            "mime_type": generated.mime_type,
            "note": "Image generated and uploaded to Drive. Google Docs API requires \
                    a publicly accessible URL for inline image insertion. Insert the \
                    image from Drive using the Google Docs UI (Insert > Image > By URL \
                    or Drive), or use gws_docs_insert_image with a public image_url.",
            "_mcp_content": [{
                "type": "image",
                "data": generated.base64_data,
                "mimeType": generated.mime_type
            }, {
                "type": "text",
                "text": format!("Generated image uploaded to Drive: https://drive.google.com/file/d/{}/view", file_id)
            }]
        }));
    }

    // Same limitation applies to Slides — createImage requires a publicly accessible URL
    if let Some(pres_id) = presentation_id {
        let file_id = upload_image_to_drive(&generated, folder_id, policy, meta, state).await?;
        return Ok(json!({
            "presentation_id": pres_id,
            "drive_file_id": file_id,
            "drive_url": format!("https://drive.google.com/file/d/{}/view", file_id),
            "mime_type": generated.mime_type,
            "note": "Image generated and uploaded to Drive. Google Slides API requires \
                    a publicly accessible URL for image insertion. Insert the image from \
                    Drive using the Slides UI (Insert > Image > By URL or Drive).",
            "_mcp_content": [{
                "type": "image",
                "data": generated.base64_data,
                "mimeType": generated.mime_type
            }, {
                "type": "text",
                "text": format!("Generated image uploaded to Drive: https://drive.google.com/file/d/{}/view", file_id)
            }]
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
    let files_resource =
        tools::find_resource(&drive_doc.resources, "files")
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
        &drive_doc, create_method, "files", "create", &upload_args,
        "drive", policy, meta, None, None, false, &mut tc,
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
