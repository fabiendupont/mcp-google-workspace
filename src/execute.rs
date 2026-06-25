use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use base64::Engine;
use serde_json::{Map, Value, json};

use google_workspace::client;
use google_workspace::discovery::{RestDescription, RestMethod};
use google_workspace::error::GwsError;
use google_workspace::validate;

use crate::meta::RequestMeta;
use crate::policy::Policy;

static FIELD_DEFAULTS: LazyLock<HashMap<(&str, &str, &str), &str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // Drive
    m.insert(("drive", "files", "list"), "files(id,name,mimeType,modifiedTime,size,parents),nextPageToken");
    m.insert(("drive", "files", "get"), "id,name,mimeType,modifiedTime,size,parents,webViewLink");
    m.insert(("drive", "files", "create"), "id,name,mimeType,parents,webViewLink");
    m.insert(("drive", "files", "copy"), "id,name,mimeType,parents,webViewLink");
    m.insert(("drive", "permissions", "list"), "permissions(id,role,type,emailAddress)");
    m.insert(("drive", "permissions", "create"), "id,role,type,emailAddress");
    // Docs
    m.insert(("docs", "documents", "get"), "documentId,title,body");
    m.insert(("docs", "documents", "create"), "documentId,title");
    // Slides
    m.insert(("slides", "presentations", "get"), "presentationId,title,slides(objectId,pageElements(objectId,size,transform,shape))");
    m.insert(("slides", "presentations", "create"), "presentationId,title");
    // Gmail
    m.insert(("gmail", "messages", "list"), "messages(id,threadId),nextPageToken,resultSizeEstimate");
    m.insert(("gmail", "messages", "get"), "id,threadId,labelIds,snippet,payload(headers(name,value),mimeType,body),sizeEstimate,internalDate");
    m.insert(("gmail", "threads", "list"), "threads(id,snippet),nextPageToken");
    m.insert(("gmail", "drafts", "list"), "drafts(id,message(id,snippet)),nextPageToken");
    m.insert(("gmail", "drafts", "create"), "id,message(id,threadId)");
    m.insert(("gmail", "labels", "list"), "labels(id,name,type)");
    // Calendar
    m.insert(("calendar", "events", "list"), "items(id,summary,start,end,status,organizer,attendees(email,responseStatus)),nextPageToken");
    m.insert(("calendar", "events", "get"), "id,summary,description,start,end,status,location,organizer,attendees,conferenceData,htmlLink");
    // Sheets
    m.insert(("sheets", "spreadsheets", "get"), "spreadsheetId,properties(title),sheets(properties(sheetId,title,index),data(rowData(values(formattedValue))))");
    m
});

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(input: &str) -> Result<Vec<u8>, GwsError> {
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|_| GwsError::Validation("Invalid base64 data".to_string()))
}

const STRIP_KEYS: &[&str] = &[
    "kind", "etag", "selfLink", "iconLink", "thumbnailLink", "hasThumbnail",
    "exportLinks", "capabilities", "permissionIds", "spaces", "shared",
    "ownedByMe", "isAppAuthorized", "linkShareMetadata", "labelInfo",
    "sha256Checksum", "md5Checksum", "originalFilename", "fullFileExtension",
    "fileExtension", "headRevisionId", "imageMediaMetadata", "videoMediaMetadata",
    "shortcutDetails", "resourceKey", "driveId", "teamDriveId",
    "copyRequiresWriterPermission", "writersCanShare", "viewersCanCopyContent",
];

pub(crate) fn strip_google_metadata(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in STRIP_KEYS {
                map.remove(*key);
            }
            if let Some(Value::Bool(false)) = map.get("trashed") {
                map.remove("trashed");
            }
            for v in map.values_mut() {
                strip_google_metadata(v);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                strip_google_metadata(item);
            }
        }
        _ => {}
    }
}

fn gws_err(msg: impl std::fmt::Display) -> GwsError {
    GwsError::Other(anyhow::anyhow!("{msg}"))
}

fn apply_common_headers(
    mut request: reqwest::RequestBuilder,
    meta: &RequestMeta,
    policy: &Policy,
) -> reqwest::RequestBuilder {
    if let Some(quota_project) = crate::auth::get_quota_project(policy.project_id.as_deref()) {
        request = request.header("x-goog-user-project", quota_project);
    }
    if let Some(ref tp) = meta.trace_parent {
        request = request.header("traceparent", tp.as_str());
    }
    if let Some(ref ts) = meta.trace_state {
        request = request.header("tracestate", ts.as_str());
    }
    if let Some(ref bg) = meta.baggage {
        request = request.header("baggage", bg.as_str());
    }
    request
}

struct ParsedArgs {
    params: Map<String, Value>,
    body: Option<Value>,
}

fn parse_args(
    arguments: &Value,
    service: &str,
    resource_path: &str,
    method_name: &str,
    method: &RestMethod,
    policy: &Policy,
) -> Result<ParsedArgs, GwsError> {
    policy.check_method(service, resource_path, method_name, method)?;

    let mut params: Map<String, Value> = arguments
        .get("params")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let body: Option<Value> = arguments
        .get("body")
        .filter(|v| !v.as_object().is_some_and(|m| m.is_empty()))
        .cloned();

    policy.enforce_constraints(service, method, &mut params, &body)?;

    Ok(ParsedArgs { params, body })
}

fn validate_params(method: &RestMethod, params: &Value) -> Result<(), Value> {
    let param_map = params.as_object();
    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();

    for (name, schema) in &method.parameters {
        let value = param_map.and_then(|m| m.get(name));

        if schema.required && value.is_none() {
            errors.push(json!({
                "param": name,
                "issue": "missing_required",
                "hint": format!("Required parameter '{}' is missing", name),
                "type": schema.param_type.as_deref().unwrap_or("string"),
                "location": schema.location.as_deref().unwrap_or("query"),
                "description": schema.description.as_deref().unwrap_or("")
            }));
            continue;
        }

        let Some(val) = value else { continue };

        if let Some(ref param_type) = schema.param_type {
            let type_ok = match param_type.as_str() {
                "string" => val.is_string(),
                "integer" => val.is_i64() || val.is_u64(),
                "boolean" => val.is_boolean(),
                "number" => val.is_number(),
                _ => true,
            };
            if !type_ok {
                errors.push(json!({
                    "param": name,
                    "issue": "wrong_type",
                    "expected": param_type,
                    "provided_type": value_type_name(val),
                    "hint": format!("Parameter '{}' must be of type {}", name, param_type)
                }));
                continue;
            }
        }

        if let Some(ref allowed) = schema.enum_values
            && let Some(s) = val.as_str()
            && !allowed.contains(&s.to_string())
        {
            errors.push(json!({
                "param": name,
                "issue": "invalid_enum_value",
                "provided": s,
                "allowed": allowed,
                "hint": "Use one of the allowed values"
            }));
        }
    }

    if let Some(map) = param_map {
        for key in map.keys() {
            if !method.parameters.contains_key(key) {
                warnings.push(json!({
                    "param": key,
                    "issue": "unknown_parameter",
                    "hint": format!("Parameter '{}' is not defined in the API schema", key)
                }));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        let mut result = json!({
            "validation_error": true,
            "errors": errors,
        });
        if !warnings.is_empty() {
            result["warnings"] = json!(warnings);
        }
        Err(result)
    }
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const DOWNLOAD_CHUNK_SIZE: usize = 10 * 1024 * 1024;
const MAX_DOWNLOAD_SIZE: usize = 100 * 1024 * 1024;

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(doc, method, arguments, policy, meta, peer), fields(service, resource = resource_path, method_name))]
pub async fn execute_tool(
    doc: &RestDescription,
    method: &RestMethod,
    resource_path: &str,
    method_name: &str,
    arguments: &Value,
    service: &str,
    policy: &Policy,
    meta: &RequestMeta,
    peer: Option<&rmcp::Peer<rmcp::RoleServer>>,
    progress_token: Option<&rmcp::model::ProgressToken>,
    dry_run: bool,
    token_cache: &mut Option<crate::auth::TokenCache>,
) -> Result<Value, GwsError> {
    let ParsedArgs { mut params, body } = parse_args(
        arguments,
        service,
        resource_path,
        method_name,
        method,
        policy,
    )?;

    if let Err(validation_error) = validate_params(method, &Value::Object(params.clone())) {
        return Ok(validation_error);
    }

    if !params.contains_key("fields")
        && let Some(default) = FIELD_DEFAULTS.get(&(service, resource_path, method_name))
    {
        params.insert("fields".to_string(), Value::String(default.to_string()));
    }

    let page_all = arguments
        .get("page_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let media_data = arguments.get("media_data").and_then(|v| v.as_str());
    let media_content_type = arguments
        .get("media_content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream");

    if media_data.is_some() && !method.supports_media_upload {
        return Err(GwsError::Validation(format!(
            "Method '{method_name}' does not support media upload"
        )));
    }

    let scopes: Vec<&str> = select_scope(&method.scopes).into_iter().collect();

    let is_upload = media_data.is_some() && method.supports_media_upload;
    let is_download = method.supports_media_download
        && params
            .get("alt")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "media");

    let query_params = build_query_params(method, &params);
    let url = if is_upload {
        build_upload_url(doc, method, &params)?
    } else {
        let (u, _) = build_url(doc, method, &params)?;
        u
    };

    if dry_run {
        let mut dry = json!({
            "dry_run": true,
            "http_method": method.http_method,
            "url": url,
            "query_params": query_params.iter().map(|(k, v)| json!({k: v})).collect::<Vec<_>>(),
            "scopes": scopes,
            "is_upload": is_upload,
            "is_download": is_download,
        });
        if is_upload && let Some(b64_data) = media_data {
            let raw_bytes = b64_decode(b64_data)?;
            if raw_bytes.len() > MAX_UPLOAD_BYTES {
                dry["auto_resumable"] = json!(true);
                dry["upload_total_size"] = json!(raw_bytes.len());
            } else {
                let (multipart_body, content_type) =
                    build_multipart_body(&body, &raw_bytes, media_content_type)?;
                dry["upload_content_type"] = json!(content_type);
                dry["upload_body_size"] = json!(multipart_body.len());
            }
        }
        if let Some(ref b) = body {
            dry["body"] = b.clone();
        }
        return Ok(dry);
    }

    let token = crate::auth::get_token(
        &scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
    )
    .await
    .map_err(|e| GwsError::Auth(format!("Authentication failed: {e}")))?;

    let http_client = client::shared_client()?;
    let mut all_results: Vec<Value> = Vec::new();
    let mut page_token: Option<String> = None;
    let page_limit: u32 = 100;

    loop {
        let mut request = match method.http_method.as_str() {
            "GET" => http_client.get(&url),
            "POST" => http_client.post(&url),
            "PUT" => http_client.put(&url),
            "PATCH" => http_client.patch(&url),
            "DELETE" => http_client.delete(&url),
            other => {
                return Err(gws_err(format!("Unsupported HTTP method: {other}")));
            }
        };

        request = request.bearer_auth(&token);
        request = apply_common_headers(request, meta, policy);

        let mut qp = query_params.clone();
        if let Some(ref pt) = page_token {
            qp.push(("pageToken".to_string(), pt.clone()));
        }
        if is_upload {
            qp.push(("uploadType".to_string(), "multipart".to_string()));
        }
        if !qp.is_empty() {
            request = request.query(&qp);
        }

        if is_upload {
            if let Some(b64_data) = media_data {
                let raw_bytes = b64_decode(b64_data)?;
                if raw_bytes.len() > MAX_UPLOAD_BYTES {
                    let result = resumable_upload_all(
                        doc, method, arguments, service, policy, meta, token_cache,
                        &raw_bytes, media_content_type, peer, progress_token,
                    ).await?;
                    return Ok(result);
                }
                let (multipart_body, content_type) =
                    build_multipart_body(&body, &raw_bytes, media_content_type)?;
                let content_length = multipart_body.len();
                request = request
                    .header("Content-Type", content_type)
                    .header("Content-Length", content_length.to_string())
                    .body(multipart_body);
            }
        } else if let Some(ref body_val) = body {
            request = request
                .header("Content-Type", "application/json")
                .json(body_val);
        } else if matches!(method.http_method.as_str(), "POST" | "PUT" | "PATCH") {
            request = request.header("Content-Length", "0");
        }

        let response =
            client::send_with_retry(|| request.try_clone().expect("request must be clonable"))
                .await
                .map_err(|e| gws_err(format!("HTTP request failed: {e}")))?;

        let status = response.status();
        let response_content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if is_download && !is_json_content_type(&response_content_type) && status.is_success() {
            if let Some(cl) = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<usize>().ok())
                && cl > MAX_DOWNLOAD_SIZE
            {
                return Err(gws_err(format!(
                    "Download too large ({cl} bytes, limit {MAX_DOWNLOAD_SIZE})"
                )));
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|e| gws_err(format!("Failed to read binary response: {e}")))?;
            let total = bytes.len();
            if total > MAX_DOWNLOAD_SIZE {
                return Err(gws_err(format!(
                    "Download too large ({total} bytes, limit {MAX_DOWNLOAD_SIZE})"
                )));
            }
            if total <= DOWNLOAD_CHUNK_SIZE {
                let b64 = b64_encode(&bytes);
                return Ok(json!({
                    "_mcp_content": build_mcp_binary_content(&b64, &response_content_type, total)
                }));
            }
            let b64_full = b64_encode(&bytes);
            return Ok(json!({
                "_mcp_large_download": {
                    "b64_data": b64_full,
                    "content_type": response_content_type,
                    "total_size": total
                }
            }));
        }

        let body_text = response
            .text()
            .await
            .map_err(|e| gws_err(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            return Ok(build_error_with_recovery(
                status.as_u16(),
                &body_text,
                service,
                method_name,
            ));
        }

        if body_text.is_empty() {
            return Ok(json!({ "status": "ok" }));
        }

        let json_val: Value = serde_json::from_str(&body_text)
            .map_err(|e| gws_err(format!("Invalid JSON response: {e}")))?;

        if !page_all {
            return Ok(json_val);
        }

        let next_token = json_val
            .get("nextPageToken")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        all_results.push(json_val);

        match next_token {
            Some(nt) if all_results.len() < page_limit as usize => {
                page_token = Some(nt);
                if let (Some(p), Some(pt)) = (peer, progress_token) {
                    let _ = p.notify_progress(rmcp::model::ProgressNotificationParam::new(
                        pt.clone(),
                        all_results.len() as f64,
                    ).with_total(page_limit as f64)
                     .with_message(format!("Fetched page {}", all_results.len()))
                    ).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            _ => break,
        }
    }

    if all_results.len() == 1 {
        Ok(all_results.into_iter().next().unwrap())
    } else {
        Ok(json!({ "pages": all_results }))
    }
}

fn select_scope(scopes: &[String]) -> Option<&str> {
    scopes
        .iter()
        .find(|s| s.ends_with(".readonly"))
        .or_else(|| scopes.iter().min_by_key(|s| s.len()))
        .map(|s| s.as_str())
}

fn build_upload_url(
    doc: &RestDescription,
    method: &RestMethod,
    params: &Map<String, Value>,
) -> Result<String, GwsError> {
    let upload_path = method
        .media_upload
        .as_ref()
        .and_then(|mu| mu.protocols.as_ref())
        .and_then(|p| p.simple.as_ref())
        .map(|s| s.path.as_str())
        .ok_or_else(|| {
            GwsError::Validation(
                "Method supports media upload but no upload path in Discovery Document".to_string(),
            )
        })?;

    let rendered = render_path_template(upload_path, params)?;
    Ok(format!(
        "{}{}",
        doc.root_url.trim_end_matches('/'),
        rendered
    ))
}

fn build_multipart_body(
    metadata: &Option<Value>,
    data: &[u8],
    content_type: &str,
) -> Result<(Vec<u8>, String), GwsError> {
    if content_type.contains('\r') || content_type.contains('\n') {
        return Err(GwsError::Validation(
            "Content type must not contain CR or LF characters".to_string(),
        ));
    }
    let boundary = format!("mcp_gws_{:016x}", simple_hash(data));

    let metadata_json = match metadata {
        Some(m) => serde_json::to_string(m).map_err(|e| {
            GwsError::Validation(format!("Failed to serialize upload metadata: {e}"))
        })?,
        None => "{}".to_string(),
    };

    let preamble = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata_json}\r\n\
         --{boundary}\r\nContent-Type: {content_type}\r\n\r\n"
    );
    let postamble = format!("\r\n--{boundary}--\r\n");

    let mut body = Vec::with_capacity(preamble.len() + data.len() + postamble.len());
    body.extend_from_slice(preamble.as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(postamble.as_bytes());

    let header = format!("multipart/related; boundary={boundary}");
    Ok((body, header))
}

pub(crate) fn simple_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn is_json_content_type(ct: &str) -> bool {
    ct.contains("application/json") || ct.contains("text/json")
}

fn build_mcp_binary_content(b64_data: &str, mime_type: &str, size: usize) -> Vec<Value> {
    let mut content = Vec::new();

    if mime_type.starts_with("image/") {
        content.push(json!({
            "type": "image",
            "data": b64_data,
            "mimeType": mime_type
        }));
    }

    content.push(json!({
        "type": "text",
        "text": format!("Downloaded {} bytes of {}", size, mime_type)
    }));

    content
}

fn build_query_params(method: &RestMethod, params: &Map<String, Value>) -> Vec<(String, String)> {
    let path_template = method.flat_path.as_deref().unwrap_or(method.path.as_str());
    let path_params = extract_path_params(path_template);
    let mut query_params: Vec<(String, String)> = Vec::new();

    for (key, value) in params {
        if path_params.contains(key.as_str()) {
            continue;
        }
        let is_repeated = method
            .parameters
            .get(key)
            .map(|p| p.repeated)
            .unwrap_or(false);
        if is_repeated && let Value::Array(arr) = value {
            for item in arr {
                let val_str = match item {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                query_params.push((key.clone(), val_str));
            }
            continue;
        }
        let val_str = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        query_params.push((key.clone(), val_str));
    }

    query_params
}

fn build_url(
    doc: &RestDescription,
    method: &RestMethod,
    params: &Map<String, Value>,
) -> Result<(String, Vec<(String, String)>), GwsError> {
    let base_url = if let Some(b) = &doc.base_url {
        b.clone()
    } else {
        format!("{}{}", doc.root_url, doc.service_path)
    };

    let path_template = match method.flat_path.as_deref() {
        Some(fp) => {
            let all_match = method
                .parameters
                .iter()
                .filter(|(_, p)| p.location.as_deref() == Some("path"))
                .all(|(name, _)| {
                    let plain = format!("{{{name}}}");
                    let plus = format!("{{+{name}}}");
                    fp.contains(&plain) || fp.contains(&plus)
                });
            if all_match { fp } else { method.path.as_str() }
        }
        None => method.path.as_str(),
    };

    let query_params = build_query_params(method, params);
    let url_path = render_path_template(path_template, params)?;
    let full_url = format!("{base_url}{url_path}");
    Ok((full_url, query_params))
}

fn extract_path_params(template: &str) -> HashSet<&str> {
    let mut found = HashSet::new();
    let mut cursor = 0;
    while let Some(open) = template[cursor..].find('{') {
        let start = cursor + open;
        let Some(close) = template[start..].find('}') else {
            break;
        };
        let end = start + close;
        let token = &template[start + 1..end];
        if let Some(key) = token.strip_prefix('+') {
            found.insert(key);
        } else {
            found.insert(token);
        }
        cursor = end + 1;
    }
    found
}

fn render_path_template(template: &str, params: &Map<String, Value>) -> Result<String, GwsError> {
    let mut rendered = String::with_capacity(template.len());
    let mut cursor = 0;

    while let Some(open) = template[cursor..].find('{') {
        let start = cursor + open;
        rendered.push_str(&template[cursor..start]);
        let Some(close) = template[start..].find('}') else {
            rendered.push_str(&template[start..]);
            return Ok(rendered);
        };
        let end = start + close;
        let token = &template[start + 1..end];
        let (is_plus, key) = if let Some(key) = token.strip_prefix('+') {
            (true, key)
        } else {
            (false, token)
        };

        if let Some(value) = params.get(key) {
            let val_str = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let encoded = if is_plus {
                let validated = validate::validate_resource_name(&val_str)?;
                validate::encode_path_preserving_slashes(validated)
            } else {
                validate::encode_path_segment(&val_str)
            };
            rendered.push_str(&encoded);
        } else {
            rendered.push_str(&template[start..=end]);
        }
        cursor = end + 1;
    }
    rendered.push_str(&template[cursor..]);
    Ok(rendered)
}

fn recovery_hints(
    status: u16,
    reason: &str,
    _message: &str,
    _service: &str,
    _method_name: &str,
) -> Option<Value> {
    match (status, reason) {
        (403, "insufficientPermissions") => Some(json!({
            "hint": "Check that the OAuth scope covers this operation. Required scope may be broader than the current token's scope.",
            "action": "scope_check",
            "retryable": false
        })),
        (403, r)
            if r == "rateLimitExceeded" || r == "usageLimits" || r == "userRateLimitExceeded" =>
        {
            Some(json!({
                "hint": "API quota exceeded. Wait before retrying. Consider using batch operations for bulk work.",
                "action": "wait_and_retry",
                "retryable": true
            }))
        }
        (404, "notFound") => Some(json!({
            "hint": "The resource may not exist or you may lack access. Try listing resources first to confirm the ID.",
            "action": "verify_resource",
            "retryable": false
        })),
        (400, r) if r == "invalidArgument" || r == "invalid" || r == "badRequest" => Some(json!({
            "hint": "Check parameter values against the API schema. Use dry_run: true to inspect the request before sending.",
            "action": "check_params",
            "retryable": false
        })),
        (401, _) => Some(json!({
            "hint": "Credential expired or invalid. Run --check-auth to diagnose.",
            "action": "reauth",
            "retryable": false
        })),
        (409, r) if r == "conflict" || r == "alreadyExists" || r == "duplicate" => Some(json!({
            "hint": "Resource already exists or was modified concurrently. Fetch the latest version before retrying.",
            "action": "fetch_latest",
            "retryable": true
        })),
        (429, _) => Some(json!({
            "hint": "Rate limited. Wait 30-60 seconds before retrying.",
            "action": "backoff",
            "retryable": true
        })),
        (s, _) if s >= 500 => Some(json!({
            "hint": "Google server error. Retry with exponential backoff.",
            "action": "retry",
            "retryable": true
        })),
        _ => None,
    }
}

fn parse_api_error(status: u16, body: &str) -> GwsError {
    if let Ok(json) = serde_json::from_str::<Value>(body)
        && let Some(err) = json.get("error")
    {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error")
            .to_string();
        let reason = err
            .get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        return GwsError::Api {
            code: status,
            message,
            reason,
            enable_url: None,
        };
    }
    let truncated: String = body.chars().take(200).collect();
    GwsError::Api {
        code: status,
        message: format!("Non-JSON error response: {truncated}"),
        reason: String::new(),
        enable_url: None,
    }
}

fn build_error_with_recovery(status: u16, body: &str, service: &str, method_name: &str) -> Value {
    let (message, reason) = if let Ok(json) = serde_json::from_str::<Value>(body)
        && let Some(err) = json.get("error")
    {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error")
            .to_string();
        let rsn = err
            .get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        (msg, rsn)
    } else {
        let truncated: String = body.chars().take(200).collect();
        (
            format!("Non-JSON error response: {truncated}"),
            String::new(),
        )
    };

    let error_label = if reason.is_empty() {
        message.clone()
    } else {
        format!("{reason}: {message}")
    };

    let mut result = json!({
        "error": error_label,
        "status": status,
    });

    if let Some(recovery) = recovery_hints(status, &reason, &message, service, method_name) {
        result["recovery"] = recovery;
    }

    result
}

pub(crate) async fn initiate_resumable_upload(
    doc: &RestDescription,
    method: &RestMethod,
    arguments: &Value,
    service: &str,
    policy: &Policy,
    meta: &RequestMeta,
    token_cache: &mut Option<crate::auth::TokenCache>,
) -> Result<Value, GwsError> {
    let resource_path = arguments
        .get("resource")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let method_name = arguments
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let ParsedArgs { params, body } = parse_args(
        arguments,
        service,
        resource_path,
        method_name,
        method,
        policy,
    )?;

    let content_type = arguments
        .get("media_content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream");
    let total_size = arguments
        .get("media_total_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if content_type.contains('\r') || content_type.contains('\n') {
        return Err(GwsError::Validation(
            "Content type must not contain CR or LF characters".to_string(),
        ));
    }

    let scopes: Vec<&str> = select_scope(&method.scopes).into_iter().collect();
    let token = crate::auth::get_token(
        &scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
    )
    .await
    .map_err(|e| GwsError::Auth(format!("Authentication failed: {e}")))?;

    let upload_url = build_upload_url(doc, method, &params)?;

    let http_client = client::shared_client()?;
    let mut request = http_client
        .post(&upload_url)
        .bearer_auth(&token)
        .query(&[("uploadType", "resumable")])
        .header("X-Upload-Content-Type", content_type);

    if total_size > 0 {
        request = request.header("X-Upload-Content-Length", total_size.to_string());
    }

    request = apply_common_headers(request, meta, policy);

    if let Some(ref body_val) = body {
        request = request
            .header("Content-Type", "application/json")
            .json(body_val);
    } else {
        request = request.header("Content-Length", "0");
    }

    let response =
        client::send_with_retry(|| request.try_clone().expect("request must be clonable"))
            .await
            .map_err(|e| gws_err(format!("Resumable upload init failed: {e}")))?;

    let status = response.status();
    let session_uri = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(parse_api_error(status.as_u16(), &body_text));
    }

    let uri = session_uri
        .ok_or_else(|| gws_err("Resumable upload init succeeded but no Location header"))?;

    Ok(json!({ "sessionUri": uri }))
}

pub(crate) async fn upload_chunk(
    session_uri: &str,
    chunk: &[u8],
    offset: u64,
    total_size: u64,
    content_type: &str,
) -> Result<Value, GwsError> {
    if chunk.is_empty() {
        return Err(GwsError::Validation("Empty chunk data".into()));
    }
    let http_client = client::shared_client()?;
    let end = offset + chunk.len() as u64 - 1;

    let total_str = if total_size > 0 {
        total_size.to_string()
    } else {
        "*".to_string()
    };

    let content_range = format!("bytes {offset}-{end}/{total_str}");

    let request = http_client
        .put(session_uri)
        .header("Content-Range", &content_range)
        .header("Content-Type", content_type)
        .header("Content-Length", chunk.len().to_string())
        .body(chunk.to_vec());

    let response =
        client::send_with_retry(|| request.try_clone().expect("request must be clonable"))
            .await
            .map_err(|e| gws_err(format!("Chunk upload failed: {e}")))?;

    let status = response.status();

    if status.as_u16() == 308 {
        let range = response
            .headers()
            .get("range")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        return Ok(json!({
            "complete": false,
            "range": range,
            "bytes_uploaded": end + 1
        }));
    }

    if status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let result: Value = serde_json::from_str(&body_text).unwrap_or(json!({"status": "ok"}));
        return Ok(json!({
            "complete": true,
            "result": result
        }));
    }

    let body_text = response.text().await.unwrap_or_default();
    Err(parse_api_error(status.as_u16(), &body_text))
}

const RESUMABLE_CHUNK_SIZE: usize = 5 * 1024 * 1024;

#[allow(clippy::too_many_arguments)]
async fn resumable_upload_all(
    doc: &RestDescription,
    method: &RestMethod,
    arguments: &Value,
    service: &str,
    policy: &Policy,
    meta: &RequestMeta,
    token_cache: &mut Option<crate::auth::TokenCache>,
    data: &[u8],
    content_type: &str,
    peer: Option<&rmcp::Peer<rmcp::RoleServer>>,
    progress_token: Option<&rmcp::model::ProgressToken>,
) -> Result<Value, GwsError> {
    let init_result = initiate_resumable_upload(
        doc, method, arguments, service, policy, meta, token_cache,
    ).await?;

    let session_uri = init_result["sessionUri"]
        .as_str()
        .ok_or_else(|| gws_err("No session URI in upload init response"))?;

    let total_size = data.len() as u64;
    let mut offset: u64 = 0;

    loop {
        let end = ((offset as usize) + RESUMABLE_CHUNK_SIZE).min(data.len());
        let chunk = &data[offset as usize..end];

        let chunk_result = upload_chunk(
            session_uri, chunk, offset, total_size, content_type,
        ).await?;

        let is_complete = chunk_result
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        offset = end as u64;

        if let (Some(p), Some(pt)) = (peer, progress_token) {
            let _ = p.notify_progress(rmcp::model::ProgressNotificationParam::new(
                pt.clone(), offset as f64,
            ).with_total(total_size as f64)
             .with_message(format!("Uploaded {} of {} bytes", offset, total_size))
            ).await;
        }

        if is_complete {
            return Ok(chunk_result.get("result").cloned().unwrap_or(json!({"status": "ok"})));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_workspace::discovery::{
        MediaUpload, MediaUploadProtocol, MediaUploadProtocols, MethodParameter,
    };
    use std::collections::HashMap;

    #[test]
    fn test_select_scope_prefers_readonly() {
        let scopes = vec![
            "https://www.googleapis.com/auth/drive".to_string(),
            "https://www.googleapis.com/auth/drive.readonly".to_string(),
        ];
        assert_eq!(
            select_scope(&scopes),
            Some("https://www.googleapis.com/auth/drive.readonly")
        );
    }

    #[test]
    fn test_select_scope_falls_back_to_shortest() {
        let scopes = vec![
            "https://www.googleapis.com/auth/drive".to_string(),
            "https://www.googleapis.com/auth/drive.file".to_string(),
        ];
        assert_eq!(
            select_scope(&scopes),
            Some("https://www.googleapis.com/auth/drive")
        );
    }

    #[test]
    fn test_select_scope_empty() {
        let scopes: Vec<String> = vec![];
        assert_eq!(select_scope(&scopes), None);
    }

    #[test]
    fn test_extract_path_params_simple() {
        let params = extract_path_params("files/{fileId}/revisions/{revisionId}");
        assert!(params.contains("fileId"));
        assert!(params.contains("revisionId"));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_extract_path_params_plus_notation() {
        let params = extract_path_params("v1/{+name}");
        assert!(params.contains("name"));
    }

    #[test]
    fn test_extract_path_params_empty() {
        let params = extract_path_params("files");
        assert!(params.is_empty());
    }

    #[test]
    fn test_render_path_simple() {
        let mut params = Map::new();
        params.insert("fileId".to_string(), json!("abc123"));
        let result = render_path_template("files/{fileId}", &params).unwrap();
        assert_eq!(result, "files/abc123");
    }

    #[test]
    fn test_render_path_missing_param_left_as_template() {
        let params = Map::new();
        let result = render_path_template("files/{fileId}", &params).unwrap();
        assert_eq!(result, "files/{fileId}");
    }

    #[test]
    fn test_render_path_encodes_special_chars() {
        let mut params = Map::new();
        params.insert("fileId".to_string(), json!("a b/c"));
        let result = render_path_template("files/{fileId}", &params).unwrap();
        assert!(result.contains("a%20b"));
        assert!(result.contains("%2Fc") || result.contains("%2fc"));
    }

    #[test]
    fn test_build_url_separates_path_and_query_params() {
        let doc = RestDescription {
            root_url: "https://www.googleapis.com/".to_string(),
            service_path: "drive/v3/".to_string(),
            base_url: None,
            ..Default::default()
        };
        let mut method_params = HashMap::new();
        method_params.insert(
            "fileId".to_string(),
            MethodParameter {
                location: Some("path".to_string()),
                ..Default::default()
            },
        );
        let method = RestMethod {
            path: "files/{fileId}".to_string(),
            parameters: method_params,
            ..Default::default()
        };
        let mut params = Map::new();
        params.insert("fileId".to_string(), json!("abc"));
        params.insert("fields".to_string(), json!("id,name"));

        let (url, qp) = build_url(&doc, &method, &params).unwrap();
        assert_eq!(url, "https://www.googleapis.com/drive/v3/files/abc");
        assert_eq!(qp.len(), 1);
        assert_eq!(qp[0], ("fields".to_string(), "id,name".to_string()));
    }

    #[test]
    fn test_build_url_uses_base_url_when_present() {
        let doc = RestDescription {
            root_url: "https://root.example.com/".to_string(),
            service_path: "ignored/".to_string(),
            base_url: Some("https://override.example.com/v1/".to_string()),
            ..Default::default()
        };
        let method = RestMethod {
            path: "things".to_string(),
            ..Default::default()
        };
        let params = Map::new();
        let (url, _) = build_url(&doc, &method, &params).unwrap();
        assert_eq!(url, "https://override.example.com/v1/things");
    }

    #[test]
    fn test_parse_api_error_structured() {
        let body = r#"{"error":{"message":"Not found","errors":[{"reason":"notFound"}]}}"#;
        let err = parse_api_error(404, body);
        match err {
            GwsError::Api {
                code,
                message,
                reason,
                ..
            } => {
                assert_eq!(code, 404);
                assert_eq!(message, "Not found");
                assert_eq!(reason, "notFound");
            }
            _ => panic!("expected Api error"),
        }
    }

    #[test]
    fn test_parse_api_error_plain_text() {
        let err = parse_api_error(500, "Internal Server Error");
        match err {
            GwsError::Api { code, message, .. } => {
                assert_eq!(code, 500);
                assert_eq!(message, "Non-JSON error response: Internal Server Error");
            }
            _ => panic!("expected Api error"),
        }
    }

    #[test]
    fn test_parse_api_error_truncates_long_body() {
        let long_body = "x".repeat(500);
        let err = parse_api_error(500, &long_body);
        match err {
            GwsError::Api { message, .. } => {
                assert!(message.len() < 250);
                assert!(message.starts_with("Non-JSON error response: "));
            }
            _ => panic!("expected Api error"),
        }
    }

    // -- Media upload tests --

    #[test]
    fn test_build_multipart_body_structure() {
        let metadata = Some(json!({"name": "test.txt"}));
        let data = b"hello world";
        let (body, ct) = build_multipart_body(&metadata, data, "text/plain").unwrap();

        assert!(ct.starts_with("multipart/related; boundary="));
        let body_str = String::from_utf8(body).unwrap();
        assert!(body_str.contains("Content-Type: application/json; charset=UTF-8"));
        assert!(body_str.contains(r#""name":"test.txt""#));
        assert!(body_str.contains("Content-Type: text/plain"));
        assert!(body_str.contains("hello world"));
    }

    #[test]
    fn test_build_multipart_body_no_metadata() {
        let (body, _) = build_multipart_body(&None, b"data", "application/octet-stream").unwrap();
        let body_str = String::from_utf8(body).unwrap();
        assert!(body_str.contains("{}"));
    }

    #[test]
    fn test_build_upload_url() {
        let doc = RestDescription {
            root_url: "https://www.googleapis.com/".to_string(),
            ..Default::default()
        };
        let method = RestMethod {
            media_upload: Some(MediaUpload {
                protocols: Some(MediaUploadProtocols {
                    simple: Some(MediaUploadProtocol {
                        path: "/upload/drive/v3/files".to_string(),
                        multipart: Some(true),
                    }),
                }),
                accept: None,
            }),
            ..Default::default()
        };
        let params = Map::new();
        let url = build_upload_url(&doc, &method, &params).unwrap();
        assert_eq!(url, "https://www.googleapis.com/upload/drive/v3/files");
    }

    #[test]
    fn test_build_upload_url_no_upload_path() {
        let doc = RestDescription {
            root_url: "https://www.googleapis.com/".to_string(),
            ..Default::default()
        };
        let method = RestMethod::default();
        let params = Map::new();
        assert!(build_upload_url(&doc, &method, &params).is_err());
    }

    // -- Media download tests --

    #[test]
    fn test_is_json_content_type() {
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("application/json; charset=utf-8"));
        assert!(!is_json_content_type("image/png"));
        assert!(!is_json_content_type("application/octet-stream"));
    }

    #[test]
    fn test_build_mcp_binary_content_image() {
        let content = build_mcp_binary_content("abc123", "image/png", 1024);
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["data"], "abc123");
        assert_eq!(content[0]["mimeType"], "image/png");
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn test_build_mcp_binary_content_non_image() {
        let content = build_mcp_binary_content("abc123", "application/pdf", 2048);
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains("2048"));
    }

    fn test_doc() -> RestDescription {
        RestDescription {
            root_url: "https://www.googleapis.com/".to_string(),
            service_path: "drive/v3/".to_string(),
            base_url: None,
            ..Default::default()
        }
    }

    fn test_method() -> RestMethod {
        let mut params = HashMap::new();
        params.insert(
            "fileId".to_string(),
            MethodParameter {
                location: Some("path".to_string()),
                required: true,
                ..Default::default()
            },
        );
        RestMethod {
            http_method: "GET".to_string(),
            path: "files/{fileId}".to_string(),
            parameters: params,
            scopes: vec![
                "https://www.googleapis.com/auth/drive".to_string(),
                "https://www.googleapis.com/auth/drive.readonly".to_string(),
            ],
            ..Default::default()
        }
    }

    fn test_policy() -> crate::policy::Policy {
        crate::policy::Policy::from_services(&["drive".to_string()])
    }

    #[tokio::test]
    async fn test_dry_run_basic_get() {
        let doc = test_doc();
        let method = test_method();
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc123", "fields": "id,name" }
        });

        let result = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        assert_eq!(result["dry_run"], true);
        assert_eq!(result["http_method"], "GET");
        assert!(result["url"].as_str().unwrap().contains("files/abc123"));
        assert_eq!(result["is_upload"], false);
        assert_eq!(result["is_download"], false);
        let scopes = result["scopes"].as_array().unwrap();
        assert!(
            scopes
                .iter()
                .any(|s| s.as_str().unwrap().contains("readonly"))
        );
    }

    #[tokio::test]
    async fn test_dry_run_policy_blocks_read_only_write() {
        let doc = test_doc();
        let mut method = test_method();
        method.http_method = "POST".to_string();
        let policy = {
            let json_str = r#"{
                "server": { "read_only": true },
                "services": [{ "name": "drive" }]
            }"#;
            let file: crate::policy::PolicyFile = serde_json::from_str(json_str).unwrap();
            crate::policy::Policy::from_policy_file(file)
        };
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "create",
            "params": { "fileId": "abc" }
        });

        let err = execute_tool(
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await;

        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("read-only"));
    }

    #[tokio::test]
    async fn test_dry_run_media_upload_rejected_on_non_upload_method() {
        let doc = test_doc();
        let method = test_method();
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc" },
            "media_data": "SGVsbG8=",
            "media_content_type": "text/plain"
        });

        let err = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await;

        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("does not support media upload")
        );
    }

    #[tokio::test]
    async fn test_dry_run_upload_with_multipart_body() {
        let doc = test_doc();
        let mut method = test_method();
        method.http_method = "POST".to_string();
        method.path = "files".to_string();
        method.parameters.clear();
        method.supports_media_upload = true;
        method.media_upload = Some(MediaUpload {
            protocols: Some(MediaUploadProtocols {
                simple: Some(MediaUploadProtocol {
                    path: "/upload/drive/v3/files".to_string(),
                    multipart: Some(true),
                }),
            }),
            accept: None,
        });
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "create",
            "body": { "name": "test.txt" },
            "media_data": "SGVsbG8gV29ybGQ=",
            "media_content_type": "text/plain"
        });

        let result = execute_tool(
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        assert_eq!(result["dry_run"], true);
        assert_eq!(result["is_upload"], true);
        assert!(
            result["url"]
                .as_str()
                .unwrap()
                .contains("/upload/drive/v3/files")
        );
        assert!(
            result["upload_content_type"]
                .as_str()
                .unwrap()
                .starts_with("multipart/related")
        );
        assert!(result["upload_body_size"].as_u64().unwrap() > 0);
        assert_eq!(result["body"]["name"], "test.txt");
    }

    #[tokio::test]
    async fn test_dry_run_download_detection() {
        let doc = test_doc();
        let mut method = test_method();
        method.supports_media_download = true;
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc", "alt": "media" }
        });

        let result = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        assert_eq!(result["is_download"], true);
    }

    #[tokio::test]
    async fn test_dry_run_empty_body_dropped() {
        let doc = test_doc();
        let method = test_method();
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc" },
            "body": {}
        });

        let result = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        assert!(result.get("body").is_none());
    }

    #[tokio::test]
    async fn test_dry_run_service_not_allowed() {
        let doc = test_doc();
        let method = test_method();
        let policy = crate::policy::Policy::from_services(&["gmail".to_string()]);
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc" }
        });

        let err = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await;

        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not allowed"));
    }

    #[tokio::test]
    async fn test_dry_run_upload_too_large() {
        let doc = test_doc();
        let mut method = test_method();
        method.http_method = "POST".to_string();
        method.supports_media_upload = true;
        method.media_upload = Some(MediaUpload {
            protocols: Some(MediaUploadProtocols {
                simple: Some(MediaUploadProtocol {
                    path: "/upload/drive/v3/files".to_string(),
                    multipart: Some(true),
                }),
            }),
            accept: None,
        });
        let policy = test_policy();
        let meta = RequestMeta::default();
        let big_data = b64_encode(&vec![0u8; MAX_UPLOAD_BYTES + 1]);
        let args = json!({
            "resource": "files",
            "method": "create",
            "params": { "fileId": "abc" },
            "media_data": big_data,
            "media_content_type": "application/octet-stream"
        });

        let result = execute_tool(
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        assert_eq!(result["auto_resumable"], true);
        assert!(result["upload_total_size"].as_u64().unwrap() > MAX_UPLOAD_BYTES as u64);
    }

    #[tokio::test]
    async fn test_smart_fields_injected() {
        let doc = test_doc();
        let mut method = test_method();
        method.path = "files".to_string();
        method.parameters.clear();
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "list",
            "params": {}
        });

        let result = execute_tool(
            &doc, &method, "files", "list", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        let qp = result["query_params"].as_array().unwrap();
        let has_fields = qp
            .iter()
            .any(|entry| entry.as_object().unwrap().contains_key("fields"));
        assert!(
            has_fields,
            "smart fields should be injected when caller omits fields"
        );
    }

    #[tokio::test]
    async fn test_smart_fields_not_overridden() {
        let doc = test_doc();
        let method = test_method();
        let policy = test_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc123", "fields": "id,name" }
        });

        let result = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true, &mut None,
        )
        .await
        .unwrap();

        let qp = result["query_params"].as_array().unwrap();
        let fields_entry = qp
            .iter()
            .find(|entry| entry.as_object().unwrap().contains_key("fields"))
            .unwrap();
        assert_eq!(
            fields_entry.as_object().unwrap()["fields"],
            "id,name",
            "explicit fields should not be overridden by smart defaults"
        );
    }

    // -- Recovery hints tests --

    #[test]
    fn test_recovery_hints_403_insufficient_permissions() {
        let hints = recovery_hints(403, "insufficientPermissions", "", "drive", "files.list");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "scope_check");
        assert_eq!(h["retryable"], false);
        assert!(h["hint"].as_str().unwrap().contains("OAuth scope"));
    }

    #[test]
    fn test_recovery_hints_403_rate_limit() {
        let hints = recovery_hints(403, "rateLimitExceeded", "", "drive", "files.list");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "wait_and_retry");
        assert_eq!(h["retryable"], true);
    }

    #[test]
    fn test_recovery_hints_404_not_found() {
        let hints = recovery_hints(404, "notFound", "", "drive", "files.get");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "verify_resource");
        assert_eq!(h["retryable"], false);
    }

    #[test]
    fn test_recovery_hints_400_invalid() {
        for reason in &["invalidArgument", "invalid", "badRequest"] {
            let hints = recovery_hints(400, reason, "", "gmail", "messages.send");
            assert!(hints.is_some(), "expected hints for 400/{reason}");
            assert_eq!(hints.unwrap()["action"], "check_params");
        }
    }

    #[test]
    fn test_recovery_hints_401() {
        let hints = recovery_hints(401, "unauthorized", "", "calendar", "events.list");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "reauth");
        assert_eq!(h["retryable"], false);
    }

    #[test]
    fn test_recovery_hints_409_conflict() {
        let hints = recovery_hints(409, "conflict", "", "drive", "files.update");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "fetch_latest");
        assert_eq!(h["retryable"], true);
    }

    #[test]
    fn test_recovery_hints_429() {
        let hints = recovery_hints(429, "tooManyRequests", "", "drive", "files.list");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "backoff");
        assert_eq!(h["retryable"], true);
    }

    #[test]
    fn test_recovery_hints_500() {
        let hints = recovery_hints(500, "", "", "drive", "files.list");
        assert!(hints.is_some());
        let h = hints.unwrap();
        assert_eq!(h["action"], "retry");
        assert_eq!(h["retryable"], true);
    }

    #[test]
    fn test_recovery_hints_503() {
        let hints = recovery_hints(503, "backendError", "", "gmail", "messages.get");
        assert!(hints.is_some());
        assert_eq!(hints.unwrap()["action"], "retry");
    }

    #[test]
    fn test_recovery_hints_unknown_returns_none() {
        let hints = recovery_hints(418, "teapot", "", "drive", "files.list");
        assert!(hints.is_none());
    }

    #[test]
    fn test_build_error_with_recovery_structured_json() {
        let body = r#"{"error":{"message":"Insufficient Permission","errors":[{"reason":"insufficientPermissions"}]}}"#;
        let result = build_error_with_recovery(403, body, "drive", "files.list");
        assert_eq!(result["status"], 403);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("insufficientPermissions")
        );
        assert!(result.get("recovery").is_some());
        assert_eq!(result["recovery"]["action"], "scope_check");
    }

    #[test]
    fn test_build_error_with_recovery_plain_text() {
        let result = build_error_with_recovery(500, "Internal Server Error", "drive", "files.list");
        assert_eq!(result["status"], 500);
        assert!(result["error"].as_str().unwrap().contains("Non-JSON error"));
        assert!(result.get("recovery").is_some());
        assert_eq!(result["recovery"]["action"], "retry");
    }

    #[test]
    fn test_build_error_with_recovery_no_hints() {
        let body = r#"{"error":{"message":"I'm a teapot","errors":[{"reason":"teapot"}]}}"#;
        let result = build_error_with_recovery(418, body, "drive", "files.list");
        assert_eq!(result["status"], 418);
        assert!(result.get("recovery").is_none());
    }

    #[test]
    fn test_build_error_with_recovery_404_structured() {
        let body =
            r#"{"error":{"message":"File not found: abc123","errors":[{"reason":"notFound"}]}}"#;
        let result = build_error_with_recovery(404, body, "drive", "files.get");
        assert_eq!(result["status"], 404);
        assert_eq!(result["error"], "notFound: File not found: abc123");
        let recovery = &result["recovery"];
        assert_eq!(recovery["action"], "verify_resource");
        assert_eq!(recovery["retryable"], false);
        assert!(
            recovery["hint"]
                .as_str()
                .unwrap()
                .contains("listing resources")
        );
    }

    // -- Parameter validation tests --

    #[test]
    fn test_validate_params_missing_required() {
        let mut params = HashMap::new();
        params.insert(
            "fileId".to_string(),
            MethodParameter {
                required: true,
                param_type: Some("string".to_string()),
                location: Some("path".to_string()),
                ..Default::default()
            },
        );
        params.insert(
            "fields".to_string(),
            MethodParameter {
                required: false,
                param_type: Some("string".to_string()),
                ..Default::default()
            },
        );
        let method = RestMethod {
            parameters: params,
            ..Default::default()
        };
        let result = validate_params(&method, &json!({}));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err["validation_error"], true);
        let errors = err["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["param"], "fileId");
        assert_eq!(errors[0]["issue"], "missing_required");
    }

    #[test]
    fn test_validate_params_invalid_enum() {
        let mut params = HashMap::new();
        params.insert(
            "orderBy".to_string(),
            MethodParameter {
                param_type: Some("string".to_string()),
                enum_values: Some(vec![
                    "createdTime".to_string(),
                    "modifiedTime".to_string(),
                    "name".to_string(),
                ]),
                ..Default::default()
            },
        );
        let method = RestMethod {
            parameters: params,
            ..Default::default()
        };
        let result = validate_params(&method, &json!({"orderBy": "date"}));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err["validation_error"], true);
        let errors = err["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["param"], "orderBy");
        assert_eq!(errors[0]["issue"], "invalid_enum_value");
        assert_eq!(errors[0]["provided"], "date");
        let allowed = errors[0]["allowed"].as_array().unwrap();
        assert!(allowed.contains(&json!("createdTime")));
        assert!(allowed.contains(&json!("modifiedTime")));
        assert!(allowed.contains(&json!("name")));
    }

    #[test]
    fn test_validate_params_passes() {
        let mut params = HashMap::new();
        params.insert(
            "fileId".to_string(),
            MethodParameter {
                required: true,
                param_type: Some("string".to_string()),
                location: Some("path".to_string()),
                ..Default::default()
            },
        );
        params.insert(
            "orderBy".to_string(),
            MethodParameter {
                param_type: Some("string".to_string()),
                enum_values: Some(vec!["name".to_string(), "modifiedTime".to_string()]),
                ..Default::default()
            },
        );
        let method = RestMethod {
            parameters: params,
            ..Default::default()
        };
        let result = validate_params(&method, &json!({"fileId": "abc123", "orderBy": "name"}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_params_wrong_type() {
        let mut params = HashMap::new();
        params.insert(
            "maxResults".to_string(),
            MethodParameter {
                param_type: Some("integer".to_string()),
                ..Default::default()
            },
        );
        let method = RestMethod {
            parameters: params,
            ..Default::default()
        };
        let result = validate_params(&method, &json!({"maxResults": "ten"}));
        assert!(result.is_err());
        let err = result.unwrap_err();
        let errors = err["errors"].as_array().unwrap();
        assert_eq!(errors[0]["issue"], "wrong_type");
        assert_eq!(errors[0]["expected"], "integer");
    }

    #[test]
    fn test_validate_params_unknown_warns() {
        let method = RestMethod {
            parameters: HashMap::new(),
            ..Default::default()
        };
        let result = validate_params(&method, &json!({"bogus": "value"}));
        assert!(result.is_ok());
    }
}
