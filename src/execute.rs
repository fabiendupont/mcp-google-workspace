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
    m.insert(
        ("drive", "files", "list"),
        "files(id,name,mimeType,modifiedTime,size,parents),nextPageToken",
    );
    m.insert(
        ("drive", "files", "get"),
        "id,name,mimeType,modifiedTime,size,parents,webViewLink",
    );
    m.insert(
        ("drive", "files", "create"),
        "id,name,mimeType,parents,webViewLink",
    );
    m.insert(
        ("drive", "files", "copy"),
        "id,name,mimeType,parents,webViewLink",
    );
    m.insert(
        ("drive", "permissions", "list"),
        "permissions(id,role,type,emailAddress)",
    );
    m.insert(
        ("drive", "permissions", "create"),
        "id,role,type,emailAddress",
    );
    // Docs
    m.insert(("docs", "documents", "get"), "documentId,title,body");
    m.insert(("docs", "documents", "create"), "documentId,title");
    // Slides
    m.insert(
        ("slides", "presentations", "get"),
        "presentationId,title,masters(objectId),layouts(objectId,layoutProperties,pageElements(objectId,size,transform,shape(placeholder,text,shapeType))),slides(objectId,slideProperties(layoutObjectId,notesPage(notesProperties,pageElements(objectId,shape(text)))),pageElements(objectId,size,transform,shape,table))",
    );
    m.insert(
        ("slides", "presentations", "create"),
        "presentationId,title",
    );
    // Gmail
    m.insert(
        ("gmail", "messages", "list"),
        "messages(id,threadId),nextPageToken,resultSizeEstimate",
    );
    m.insert(("gmail", "messages", "get"), "id,threadId,labelIds,snippet,payload(headers(name,value),mimeType,body),sizeEstimate,internalDate");
    m.insert(
        ("gmail", "threads", "list"),
        "threads(id,snippet),nextPageToken",
    );
    m.insert(
        ("gmail", "drafts", "list"),
        "drafts(id,message(id,snippet)),nextPageToken",
    );
    m.insert(("gmail", "drafts", "create"), "id,message(id,threadId)");
    m.insert(("gmail", "labels", "list"), "labels(id,name,type)");
    // Calendar
    m.insert(("calendar", "events", "list"), "items(id,summary,start,end,status,organizer,attendees(email,responseStatus)),nextPageToken");
    m.insert(("calendar", "events", "get"), "id,summary,description,start,end,status,location,organizer,attendees,conferenceData,htmlLink");
    // Sheets
    m.insert(("sheets", "spreadsheets", "get"), "spreadsheetId,properties(title),sheets(properties(sheetId,title,index),data(rowData(values(formattedValue))))");
    m.insert(
        ("sheets", "spreadsheets.values", "get"),
        "range,values,majorDimension",
    );
    m.insert(
        ("sheets", "spreadsheets.values", "update"),
        "updatedRange,updatedRows,updatedColumns,updatedCells",
    );
    m.insert(
        ("sheets", "spreadsheets.values", "append"),
        "updates(updatedRange,updatedRows,updatedColumns,updatedCells)",
    );
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
    "kind",
    "etag",
    "selfLink",
    "iconLink",
    "thumbnailLink",
    "hasThumbnail",
    "exportLinks",
    "capabilities",
    "permissionIds",
    "spaces",
    "shared",
    "ownedByMe",
    "isAppAuthorized",
    "linkShareMetadata",
    "labelInfo",
    "sha256Checksum",
    "md5Checksum",
    "originalFilename",
    "fullFileExtension",
    "fileExtension",
    "headRevisionId",
    "imageMediaMetadata",
    "videoMediaMetadata",
    "shortcutDetails",
    "resourceKey",
    "driveId",
    "teamDriveId",
    "copyRequiresWriterPermission",
    "writersCanShare",
    "viewersCanCopyContent",
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

/// Error for a prepared request whose headers reqwest could not convert.
///
/// `RequestBuilder::header` stores a header-conversion error instead of
/// failing, so a client-controlled value (a `_meta` trace header, a media
/// content type) only surfaces from `try_clone()` or `build()`. Release builds
/// use `panic = "abort"`, so this must be an error path, never an `expect`.
fn invalid_request_header_error() -> GwsError {
    GwsError::Validation(
        "invalid _meta trace header value: the request headers could not be converted. \
         Fix: send traceparent/tracestate/baggage containing only printable ASCII and tabs"
            .to_string(),
    )
}

/// Prepare a request for [`client::send_with_retry`], rejecting values reqwest
/// cannot convert into headers.
///
/// The returned builder is a clone of `request`; the retry loop clones it again
/// per attempt. Cloning a builder that cloned successfully is stable — the
/// stored error and the replayable bodies (empty, JSON bytes, multipart bytes)
/// are unchanged — so the in-loop clone cannot fail.
fn prepare_request(request: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, GwsError> {
    request.try_clone().ok_or_else(invalid_request_header_error)
}

struct ParsedArgs {
    params: Map<String, Value>,
    body: Option<Value>,
}

/// Enforce the per-service `allowed_resources` allowlist.
///
/// An empty list keeps its historical meaning of "every resource". This is the
/// execution chokepoint shared by generic dispatch, `gws_batch` sub-requests,
/// helper tools and resumable uploads, so it must stay in sync with
/// [`crate::resources::is_resource_allowed`], which enforces the same allowlist
/// for `gws://` resource reads.
pub(crate) fn ensure_resource_allowed(
    service: &str,
    resource_path: &str,
    policy: &Policy,
) -> Result<(), GwsError> {
    let allowed = policy.allowed_resources(service);
    if crate::resources::is_resource_allowed(allowed, resource_path) {
        return Ok(());
    }

    Err(GwsError::Validation(format!(
        "Resource '{resource_path}' is not in allowed_resources for service '{service}'. \
         Allowed resources: {}. \
         Fix: add \"{resource_path}\" to the \"allowed_resources\" array in the \"{service}\" \
         service of your policy file",
        allowed.join(", ")
    )))
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
    ensure_resource_allowed(service, resource_path, policy)?;

    let mut params: Map<String, Value> = arguments
        .get("params")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let body: Option<Value> = arguments
        .get("body")
        .filter(|v| !v.as_object().is_some_and(|m| m.is_empty()))
        .cloned();

    // Pass the real resource path: the policy uses it to distinguish
    // `files.create` (body `parents` constraint applies) from by-ID writes such
    // as `permissions.create` / `files.update` (which validate the target id
    // through `execute::write_scope_id_param` instead).
    policy.enforce_constraints_for(
        service,
        resource_path,
        method_name,
        method,
        &mut params,
        &body,
    )?;

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

    let subject = policy.resolve_delegation_subject(meta.user_email.as_deref())?;
    let token = crate::auth::get_token_as(
        &scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
        subject.as_deref(),
    )
    .await
    .map_err(|e| GwsError::Auth(format!("Authentication failed: {e}")))?;

    enforce_read_scope(
        doc,
        service,
        resource_path,
        method_name,
        &params,
        policy,
        meta,
        subject.as_deref(),
        false,
    )
    .await?;

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
                        doc,
                        method,
                        arguments,
                        service,
                        policy,
                        meta,
                        token_cache,
                        &raw_bytes,
                        media_content_type,
                        peer,
                        progress_token,
                    )
                    .await?;
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

        crate::rate_limit::acquire_global(service).await;

        let request = prepare_request(request)?;
        let response = client::send_with_retry(|| {
            // `prepare_request` proved this builder clonable; cloning a builder
            // that cloned once cannot fail for the bodies used here.
            request
                .try_clone()
                .expect("request validated as clonable by prepare_request")
        })
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

        let mut json_val: Value = serde_json::from_str(&body_text)
            .map_err(|e| gws_err(format!("Invalid JSON response: {e}")))?;

        // Defence in depth for `drive.files.list`: the policy's injected `q` is
        // the primary containment, but a precedence surprise must not leak the
        // whole Drive. See `enforce_list_containment`.
        if service == "drive" && resource_path == "files" && method_name == "list" {
            enforce_list_containment(&mut json_val, doc, &token, meta, policy).await;
        }

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
                    let _ = p
                        .notify_progress(
                            rmcp::model::ProgressNotificationParam::new(
                                pt.clone(),
                                all_results.len() as f64,
                            )
                            .with_total(page_limit as f64)
                            .with_message(format!("Fetched page {}", all_results.len())),
                        )
                        .await;
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

// -- Read-by-ID folder scoping (Item 4) --

/// Base URL of the Drive v3 `files` collection.
///
/// Requests from Docs, Sheets and Slides arrive with their own discovery
/// document, which has no `files` resource. Their ids are Drive file ids, so
/// the read-scope metadata lookup falls back to this endpoint.
const DRIVE_FILES_V3_URL: &str = "https://www.googleapis.com/drive/v3/files";

/// Candidate scopes for the internal Drive metadata lookup.
///
/// These are the two Drive scopes that grant folder visibility. They are fed
/// through [`select_scope`] — the same helper [`execute_tool`] uses — so the
/// lookup asks for `drive.readonly` rather than the full write scope.
const DRIVE_METADATA_SCOPE_CANDIDATES: &[&str] = &[
    "https://www.googleapis.com/auth/drive",
    "https://www.googleapis.com/auth/drive.readonly",
];

/// Scope list requested for the read-scope metadata lookup.
///
/// The Drive `files.get` discovery scope list also contains unrelated
/// `drive.*.readonly` scopes (meet/photos/metadata), which [`select_scope`]
/// would pick first. Selecting from [`DRIVE_METADATA_SCOPE_CANDIDATES`] keeps
/// the lookup on `drive.readonly`, the scope that can read folder metadata.
fn read_scope_scopes() -> Vec<String> {
    let candidates: Vec<String> = DRIVE_METADATA_SCOPE_CANDIDATES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect();
    select_scope(&candidates)
        .map(str::to_string)
        .into_iter()
        .collect()
}

/// Acquire the Drive-scoped token used by the internal metadata lookup.
///
/// The request token is scoped to the target API — for `documents.get`,
/// `select_scope` picks `documents.readonly`, which cannot read Drive
/// metadata. This asks for `drive.readonly` instead, with the same credential
/// and delegation subject as the request.
///
/// The shared `token_cache` is deliberately bypassed (`None`): it is keyed by
/// validity only, so it would hand back the request token that [`execute_tool`]
/// just cached — and replacing it with a Drive-only token would break later
/// calls that need a different scope.
async fn read_scope_token(policy: &Policy, subject: Option<&str>) -> Result<String, GwsError> {
    let scopes = read_scope_scopes();
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();

    crate::auth::get_token_as(
        &scope_refs,
        policy.credentials_file.as_deref(),
        None,
        subject,
    )
    .await
    .map_err(|e| {
        GwsError::Auth(format!(
            "Read denied: cannot enforce folder scoping because a Drive metadata token could not \
             be acquired ({e}). The policy needs Drive metadata access to enforce folder scoping. \
             Fix: grant the Drive read-only scope \
             (https://www.googleapis.com/auth/drive.readonly) to the credential, or remove the \
             recursive \"parents\" constraint"
        ))
    })
}

/// Parameter that carries the Drive file id which must be inside the roots.
///
/// Google Docs, Sheets and Slides use the Drive file id as their document /
/// spreadsheet / presentation id, so the same containment check covers their
/// read-by-id methods. Every other request (list, create, batchUpdate, ...) is
/// covered by the `parents` constraints or has no file id to check, and maps to
/// `None`. By-ID writes are listed separately by [`write_scope_id_param`].
///
/// The `(service, resource_path, method)` triples are the exact values the
/// helper call sites pass to [`execute_tool`].
fn read_scope_id_param(service: &str, resource: &str, method_name: &str) -> Option<&'static str> {
    match (service, resource, method_name) {
        ("drive", "files", "get") => Some("fileId"),
        ("drive", "files", "export") => Some("fileId"),
        // `files.copy` reads the source file; the destination folder is
        // validated by the `parents` body constraint.
        ("drive", "files", "copy") => Some("fileId"),
        ("docs", "documents", "get") => Some("documentId"),
        ("sheets", "spreadsheets", "get") => Some("spreadsheetId"),
        ("sheets", "spreadsheets.values", "get") => Some("spreadsheetId"),
        ("slides", "presentations", "get") => Some("presentationId"),
        _ => None,
    }
}

/// Parameter that carries the Drive file id for a by-ID *write*.
///
/// `parents` constraints only validate folder ids supplied in a request body,
/// so without this table a folder-scoped policy could still trash, rename,
/// share or overwrite an out-of-tree file by id. The same containment check as
/// [`read_scope_id_param`] applies: the file id must be a recursive root or its
/// ancestry must reach one.
///
/// `permissions.create|update|delete` use the parent file id in `fileId`; Docs,
/// Sheets and Slides use the Drive id under their own parameter name. `None`
/// means no by-ID scope check: either the method has no file id (`files.create`)
/// or its containment comes from a body constraint.
fn write_scope_id_param(service: &str, resource: &str, method_name: &str) -> Option<&'static str> {
    match (service, resource, method_name) {
        ("drive", "permissions", "create") => Some("fileId"),
        ("drive", "permissions", "update") => Some("fileId"),
        ("drive", "permissions", "delete") => Some("fileId"),
        ("drive", "files", "update") => Some("fileId"),
        ("drive", "files", "delete") => Some("fileId"),
        ("docs", "documents", "batchUpdate") => Some("documentId"),
        ("sheets", "spreadsheets.values", "update") => Some("spreadsheetId"),
        ("sheets", "spreadsheets.values", "append") => Some("spreadsheetId"),
        ("sheets", "spreadsheets.values", "clear") => Some("spreadsheetId"),
        ("sheets", "spreadsheets", "batchUpdate") => Some("spreadsheetId"),
        ("slides", "presentations", "batchUpdate") => Some("presentationId"),
        _ => None,
    }
}

/// Whether the read-by-ID scope check applies to this request.
///
/// Covers Drive `files.get`, `files.export` and `files.copy`, plus the
/// Docs/Sheets/Slides read methods listed by [`read_scope_id_param`]. The check
/// only runs when the policy configures at least one recursive `parents` root
/// for the `drive` service: without such roots there is no folder boundary to
/// enforce, so the request proceeds unchecked. [`write_scope_applies`] is the
/// sibling predicate for by-ID writes.
fn read_scope_applies(policy: &Policy, service: &str, resource: &str, method_name: &str) -> bool {
    read_scope_id_param(service, resource, method_name).is_some()
        && !policy.recursive_parent_values("drive").is_empty()
}

/// Whether the by-ID write scope check applies to this request.
///
/// Mirrors [`read_scope_applies`] for the [`write_scope_id_param`] table, and is
/// gated on the same recursive `parents` roots: no roots means no boundary to
/// enforce.
fn write_scope_applies(policy: &Policy, service: &str, resource: &str, method_name: &str) -> bool {
    write_scope_id_param(service, resource, method_name).is_some()
        && !policy.recursive_parent_values("drive").is_empty()
}

/// Pure decision for read-by-ID scoping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadScopeDecision {
    /// The file is the configured root or sits directly inside one.
    Allowed,
    /// No parent metadata: fail closed unless the file is an allowed root.
    MissingParents,
    /// Parents are known but none is a root; ancestry must be walked.
    CheckAncestors,
}

fn read_scope_decision(file_id: &str, parents: &[String], roots: &[&str]) -> ReadScopeDecision {
    if roots.is_empty() || roots.contains(&file_id) {
        return ReadScopeDecision::Allowed;
    }
    if parents.is_empty() {
        return ReadScopeDecision::MissingParents;
    }
    if parents.iter().any(|p| roots.contains(&p.as_str())) {
        ReadScopeDecision::Allowed
    } else {
        ReadScopeDecision::CheckAncestors
    }
}

fn read_scope_denied(file_id: &str, roots: &[&str]) -> GwsError {
    GwsError::Validation(format!(
        "Read denied: file {file_id} is outside the folders allowed by policy. \
         Allowed roots: {}. \
         Fix: move the file into one of those folders or discover it with files.list",
        roots.join(", ")
    ))
}

/// Resolved outcome of the initial parent check.
///
/// Mirrors [`ReadScopeDecision`] but carries the actionable error for the
/// fail-closed case, so callers — and tests — can assert on denial without any
/// network access.
#[derive(Debug)]
enum ReadScopeVerdict {
    /// The file is the configured root or sits directly inside one.
    Allowed,
    /// Parents are known but none is a root; ancestry must be walked.
    WalkAncestors,
    /// No parent metadata: fail closed unless the file is an allowed root.
    Denied(GwsError),
}

fn read_scope_verdict(file_id: &str, parents: &[String], roots: &[&str]) -> ReadScopeVerdict {
    match read_scope_decision(file_id, parents, roots) {
        ReadScopeDecision::Allowed => ReadScopeVerdict::Allowed,
        ReadScopeDecision::CheckAncestors => ReadScopeVerdict::WalkAncestors,
        ReadScopeDecision::MissingParents => {
            ReadScopeVerdict::Denied(GwsError::Validation(format!(
                "Read denied: file {file_id} has no parent folders visible to the credential, so \
                 it cannot be verified to be inside the folders allowed by policy. \
                 Allowed roots: {}. Fix: move the file into one of those folders",
                roots.join(", ")
            )))
        }
    }
}

/// URL for the internal `files.get?fields=parents` metadata lookup.
///
/// Drive requests carry the Drive discovery document, so the canonical URL is
/// derived from it exactly like a real request. Docs/Sheets/Slides requests
/// carry their own discovery document, which has no `files` resource, so they
/// fall back to [`DRIVE_FILES_V3_URL`].
fn drive_files_get_url(doc: &RestDescription, file_id: &str) -> String {
    if let Some(files_resource) = crate::tools::find_resource(&doc.resources, "files")
        && let Some(get_method) = files_resource.methods.get("get")
    {
        let mut params = Map::new();
        params.insert("fileId".to_string(), Value::String(file_id.to_string()));
        params.insert("fields".to_string(), Value::String("parents".to_string()));
        if let Ok((url, _)) = build_url(doc, get_method, &params) {
            return url;
        }
    }
    format!(
        "{DRIVE_FILES_V3_URL}/{}",
        validate::encode_path_segment(file_id)
    )
}

/// Fetch a Drive file's `parents` with a direct HTTP call.
///
/// `doc` is the discovery document of the service that received the request;
/// [`drive_files_get_url`] turns it into the Drive metadata URL. Docs, Sheets
/// and Slides ids are Drive file ids, so the same lookup verifies them.
///
/// This deliberately bypasses policy evaluation: it is the internal metadata
/// lookup used by [`enforce_read_scope`] and its ancestry walk. Because it never
/// calls [`execute_tool`] it cannot recurse into the read-scope check; the
/// `internal` flag documents and enforces that contract.
async fn fetch_file_parents(
    doc: &RestDescription,
    file_id: &str,
    token: &str,
    meta: &RequestMeta,
    policy: &Policy,
    internal: bool,
) -> Result<Vec<String>, GwsError> {
    if !internal {
        return Err(gws_err(
            "internal metadata lookup must bypass policy evaluation",
        ));
    }

    let url = drive_files_get_url(doc, file_id);

    // Meter the internal lookup against the same Drive budget as real requests:
    // a scoped read can issue one metadata call plus up to ten ancestry walks,
    // and those must not be invisible to the rate limiter.
    crate::rate_limit::acquire_global("drive").await;

    let http_client = client::shared_client()?;
    let mut request = http_client
        .get(&url)
        .bearer_auth(token)
        .query(&[("fields", "parents")]);
    request = apply_common_headers(request, meta, policy);

    let request = prepare_request(request)?;
    let response = client::send_with_retry(|| {
        // `prepare_request` proved this builder clonable.
        request
            .try_clone()
            .expect("request validated as clonable by prepare_request")
    })
    .await
    .map_err(|e| {
        GwsError::Validation(format!(
            "Read denied: could not verify that file {file_id} is inside the folders \
             allowed by policy (metadata lookup failed: {e}). \
             Fix: retry, or grant the credential access to the file"
        ))
    })?;

    let status = response.status();
    let body_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(GwsError::Validation(format!(
            "Read denied: could not verify that file {file_id} is inside the folders allowed \
             by policy (metadata lookup returned HTTP {}). \
             Fix: grant the credential access to the file, or move it into an allowed folder",
            status.as_u16()
        )));
    }

    let value: Value = serde_json::from_str(&body_text).map_err(|e| {
        GwsError::Validation(format!(
            "Read denied: could not verify that file {file_id} is inside the folders allowed \
             by policy (invalid metadata response: {e}). Fix: retry the request"
        ))
    })?;

    Ok(value
        .get("parents")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// Walk `start`'s ancestry looking for one of `roots`.
///
/// Fails closed: any metadata error aborts the walk as "not inside".
async fn lineage_reaches_roots(
    doc: &RestDescription,
    start: &[String],
    roots: &[&str],
    token: &str,
    meta: &RequestMeta,
    policy: &Policy,
) -> bool {
    let mut frontier: Vec<String> = start.to_vec();
    let mut seen: HashSet<String> = HashSet::new();

    // Same depth bound used by shared::is_descendant_of.
    for _ in 0..10 {
        let mut next = Vec::new();
        for id in frontier {
            if roots.contains(&id.as_str()) {
                return true;
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            match fetch_file_parents(doc, &id, token, meta, policy, true).await {
                Ok(parents) => next.extend(parents),
                Err(_) => return false,
            }
        }
        if next.is_empty() {
            return false;
        }
        frontier = next;
    }
    false
}

/// Enforce folder scope for by-ID reads *and* writes.
///
/// Covered read requests are those mapped by [`read_scope_id_param`]:
///
/// - Drive `files.get`, `files.export` and `files.copy` (the copy *source*);
///   the copy destination folder is validated by the `parents` body constraint.
/// - Docs `documents.get`, Sheets `spreadsheets.get` and
///   `spreadsheets.values.get`, Slides `presentations.get` — their ids are
///   Drive file ids, so the same parent lookup applies.
///
/// Covered write requests are those mapped by [`write_scope_id_param`]: Drive
/// `permissions.create|update|delete`, `files.update` and `files.delete`, Docs
/// `documents.batchUpdate`, Sheets `spreadsheets.values.update|append|clear`
/// and `spreadsheets.batchUpdate`, and Slides `presentations.batchUpdate`.
/// `parents` body constraints cannot see the target of a by-ID write, so
/// without this check a folder-scoped policy could trash, rename, share or
/// overwrite any file id the credential can access.
///
/// Requests without a file id — and policies with no recursive `parents` roots —
/// are not checked. Metadata lookups pass `internal = true` and skip the check.
/// Failures to read metadata deny the request. Denials keep the historical
/// "Read denied" / "outside the folders allowed by policy" phrasing for both
/// reads and writes: `server::is_audit_denial` classifies on it.
///
/// The metadata lookup runs with its own `drive.readonly` token from
/// [`read_scope_token`], because the request token is scoped to the target API
/// (Docs reads select `documents.readonly`, which cannot read Drive metadata).
#[allow(clippy::too_many_arguments)]
async fn enforce_read_scope(
    doc: &RestDescription,
    service: &str,
    resource: &str,
    method_name: &str,
    params: &Map<String, Value>,
    policy: &Policy,
    meta: &RequestMeta,
    subject: Option<&str>,
    internal: bool,
) -> Result<(), GwsError> {
    if internal {
        return Ok(());
    }

    let id_param = if read_scope_applies(policy, service, resource, method_name) {
        // `read_scope_applies` guarantees the row, so this cannot be `None`.
        read_scope_id_param(service, resource, method_name)
    } else if write_scope_applies(policy, service, resource, method_name) {
        // `write_scope_applies` guarantees the row, so this cannot be `None`.
        write_scope_id_param(service, resource, method_name)
    } else {
        None
    };
    let Some(id_param) = id_param else {
        return Ok(());
    };
    // Both `*_scope_applies` predicates require at least one root.
    let roots = policy.recursive_parent_values("drive");

    let Some(file_id) = params.get(id_param).and_then(Value::as_str) else {
        return Err(GwsError::Validation(format!(
            "Read denied: '{id_param}' is required to verify folder scope. \
             Fix: include \"{id_param}\" in the request params"
        )));
    };

    // Fail closed if the Drive metadata token cannot be acquired.
    let metadata_token = read_scope_token(policy, subject).await?;

    let parents = fetch_file_parents(doc, file_id, &metadata_token, meta, policy, true).await?;

    match read_scope_verdict(file_id, &parents, &roots) {
        ReadScopeVerdict::Allowed => Ok(()),
        ReadScopeVerdict::Denied(err) => Err(err),
        ReadScopeVerdict::WalkAncestors => {
            if lineage_reaches_roots(doc, &parents, &roots, &metadata_token, meta, policy).await {
                Ok(())
            } else {
                Err(read_scope_denied(file_id, &roots))
            }
        }
    }
}

/// Maximum number of `files.list` entries whose ancestry is verified per page.
///
/// The post-filter performs one metadata lookup (plus one per ancestor level)
/// per file that is not already decided by its response `parents`. Capping the
/// work keeps a hostile or very large page from turning one list call into
/// thousands of Drive requests; entries past the cap are dropped, never
/// released unchecked.
const MAX_LIST_CONTAINMENT_FILES: usize = 200;

/// Post-filter a `drive.files.list` page to the policy's recursive roots.
///
/// The policy injects a `q` clause for list calls (`src/policy.rs`), which is
/// the primary containment. This is defence in depth: a `q`-injection or a
/// parser-precedence surprise must not turn one list call into a full-scope
/// enumeration. Only runs when the policy configures recursive `parents` roots.
///
/// Each returned file is kept only when its id is a root or its ancestry
/// reaches one. Ancestry comes from the response's `parents` when present and
/// from [`fetch_file_parents`] / [`lineage_reaches_roots`] otherwise. Anything
/// unverifiable is dropped (fail closed), and at most
/// [`MAX_LIST_CONTAINMENT_FILES`] entries are checked per page — the remainder
/// is dropped with a warning. `nextPageToken` is left untouched so paging still
/// works; a dropped file is simply absent from this page.
async fn enforce_list_containment(
    value: &mut Value,
    doc: &RestDescription,
    token: &str,
    meta: &RequestMeta,
    policy: &Policy,
) {
    let roots = policy.recursive_parent_values("drive");
    if roots.is_empty() {
        return;
    }

    let Some(files) = value.get_mut("files").and_then(Value::as_array_mut) else {
        return;
    };

    if files.len() > MAX_LIST_CONTAINMENT_FILES {
        tracing::warn!(
            returned = files.len(),
            checked = MAX_LIST_CONTAINMENT_FILES,
            dropped = files.len() - MAX_LIST_CONTAINMENT_FILES,
            "files.list page exceeds the containment check cap; dropping the remainder"
        );
        files.truncate(MAX_LIST_CONTAINMENT_FILES);
    }

    let mut kept: Vec<Value> = Vec::with_capacity(files.len());
    let checked = files.len();
    for file in files.drain(..) {
        let Some(file_id) = file.get("id").and_then(Value::as_str) else {
            tracing::warn!("dropping files.list entry without an id");
            continue;
        };
        let file_id = file_id.to_string();

        let parents = match listed_file_parents(&file) {
            Some(parents) => parents,
            None => match fetch_file_parents(doc, &file_id, token, meta, policy, true).await {
                Ok(parents) => parents,
                Err(e) => {
                    tracing::warn!(
                        file_id,
                        error = %e,
                        "dropping files.list entry whose parents could not be fetched"
                    );
                    continue;
                }
            },
        };

        let keep = match read_scope_decision(&file_id, &parents, &roots) {
            ReadScopeDecision::Allowed => true,
            // No visible parents and not a root: fail closed.
            ReadScopeDecision::MissingParents => false,
            ReadScopeDecision::CheckAncestors => {
                lineage_reaches_roots(doc, &parents, &roots, token, meta, policy).await
            }
        };
        if keep {
            kept.push(file);
        }
    }

    if kept.len() != checked {
        tracing::debug!(
            kept = kept.len(),
            checked,
            "files.list page filtered to the folders allowed by policy"
        );
    }
    *files = kept;
}

/// Parents carried by a `files.list` entry, when the response included them.
///
/// `None` means the caller-supplied `fields` mask omitted `parents` (or the
/// value was not an array), so the caller must fetch them.
fn listed_file_parents(file: &Value) -> Option<Vec<String>> {
    file.get("parents")
        .and_then(Value::as_array)
        .map(|parents| {
            parents
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
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

/// Start a resumable upload session.
///
/// **This performs a real network request**: it POSTs to the upload endpoint
/// with the caller's credential and returns the session URI that subsequent
/// [`upload_chunk`] calls write to. It has no dry-run branch of its own.
///
/// The caller MUST have obtained user approval before calling this (see the
/// approval gate in `server.rs`); it must not be reachable while `dry_run` is
/// set. The approval flag is computed by the caller, so the invariant is
/// documented here, at the site that actually spends the credential.
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

    // A resumable session is a by-ID write: `files.update` (and the other
    // `write_scope_id_param` rows) would otherwise reach the upload endpoint
    // without the folder-scope containment that `execute_tool` applies. Run it
    // here, before any network I/O and with the same arguments as the direct
    // path.
    let subject = policy.resolve_delegation_subject(meta.user_email.as_deref())?;
    enforce_read_scope(
        doc,
        service,
        resource_path,
        method_name,
        &params,
        policy,
        meta,
        subject.as_deref(),
        false,
    )
    .await?;

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
    let token = crate::auth::get_token_as(
        &scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
        subject.as_deref(),
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

    crate::rate_limit::acquire_global(service).await;

    let request = prepare_request(request)?;
    let response = client::send_with_retry(|| {
        // `prepare_request` proved this builder clonable.
        request
            .try_clone()
            .expect("request validated as clonable by prepare_request")
    })
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

    let request = prepare_request(request)?;
    let response = client::send_with_retry(|| {
        // `prepare_request` proved this builder clonable.
        request
            .try_clone()
            .expect("request validated as clonable by prepare_request")
    })
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

/// Upload a whole payload through a resumable session.
///
/// **This performs real network requests**: it calls
/// [`initiate_resumable_upload`] (session init) and then [`upload_chunk`] for
/// every 5 MiB chunk, writing the caller's bytes to Google Drive. It has no
/// dry-run branch of its own.
///
/// Entered from [`execute_tool`] after its `dry_run` check, so it must never be
/// reachable with `dry_run` set. The caller MUST have obtained user approval
/// before execution (see the approval gate in `server.rs`). The approval flag
/// is computed by the caller, so the invariant is documented here, at the site
/// that actually writes the data.
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
    let init_result =
        initiate_resumable_upload(doc, method, arguments, service, policy, meta, token_cache)
            .await?;

    let session_uri = init_result["sessionUri"]
        .as_str()
        .ok_or_else(|| gws_err("No session URI in upload init response"))?;

    let total_size = data.len() as u64;
    let mut offset: u64 = 0;

    loop {
        let end = ((offset as usize) + RESUMABLE_CHUNK_SIZE).min(data.len());
        let chunk = &data[offset as usize..end];

        let chunk_result =
            upload_chunk(session_uri, chunk, offset, total_size, content_type).await?;

        let is_complete = chunk_result
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        offset = end as u64;

        if let (Some(p), Some(pt)) = (peer, progress_token) {
            let _ = p
                .notify_progress(
                    rmcp::model::ProgressNotificationParam::new(pt.clone(), offset as f64)
                        .with_total(total_size as f64)
                        .with_message(format!("Uploaded {} of {} bytes", offset, total_size)),
                )
                .await;
        }

        if is_complete {
            return Ok(chunk_result
                .get("result")
                .cloned()
                .unwrap_or(json!({"status": "ok"})));
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

    /// Drive-shaped discovery document that carries the `files` resource, so
    /// [`drive_files_get_url`] takes the discovery-document path.
    fn test_doc_with_files_resource() -> RestDescription {
        use google_workspace::discovery::RestResource;

        let mut method_params = HashMap::new();
        method_params.insert(
            "fileId".to_string(),
            MethodParameter {
                location: Some("path".to_string()),
                required: true,
                ..Default::default()
            },
        );
        let mut files = RestResource::default();
        files.methods.insert(
            "get".to_string(),
            RestMethod {
                http_method: "GET".to_string(),
                path: "files/{fileId}".to_string(),
                parameters: method_params,
                ..Default::default()
            },
        );
        let mut resources = HashMap::new();
        resources.insert("files".to_string(), files);

        RestDescription {
            root_url: "https://www.googleapis.com/".to_string(),
            service_path: "drive/v3/".to_string(),
            base_url: None,
            resources,
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

    #[test]
    fn test_poisoned_trace_header_is_an_error_not_a_panic() {
        let http_client = client::shared_client().expect("shared HTTP client");
        // reqwest stores the header-conversion error in the builder; only
        // `try_clone` / `build` surface it.
        let request = http_client
            .get("https://www.googleapis.com/drive/v3/files")
            .header("traceparent", "00-abc\r\ndef-01");

        let err = prepare_request(request).expect_err("poisoned header must not be clonable");
        assert!(
            err.to_string().contains("invalid _meta trace header value"),
            "{err}"
        );
    }

    #[test]
    fn test_prepare_request_accepts_valid_headers() {
        let http_client = client::shared_client().expect("shared HTTP client");
        let request = http_client
            .get("https://www.googleapis.com/drive/v3/files")
            .header("traceparent", "00-abc-def-01");
        assert!(prepare_request(request).is_ok());
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "list", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
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

    // -- Read-by-ID folder scoping tests --

    fn recursive_policy() -> crate::policy::Policy {
        let file: crate::policy::PolicyFile = serde_json::from_value(serde_json::json!({
            "services": [{
                "name": "drive",
                "constraints": [{
                    "param": "parents",
                    "values": ["root-folder"],
                    "access": "read-write",
                    "location": "body-write-only",
                    "mode": "restrict",
                    "recursive": true
                }]
            }]
        }))
        .unwrap();
        crate::policy::Policy::from_policy_file(file)
    }

    /// Same folder scoping as [`recursive_policy`], with the Drive sibling
    /// services enabled so their read tools can reach the execution chokepoint.
    fn sibling_recursive_policy() -> crate::policy::Policy {
        let file: crate::policy::PolicyFile = serde_json::from_value(serde_json::json!({
            "services": [
                {
                    "name": "drive",
                    "constraints": [{
                        "param": "parents",
                        "values": ["root-folder"],
                        "access": "read-write",
                        "location": "body-write-only",
                        "mode": "restrict",
                        "recursive": true
                    }]
                },
                { "name": "docs" },
                { "name": "sheets" },
                { "name": "slides" }
            ]
        }))
        .unwrap();
        crate::policy::Policy::from_policy_file(file)
    }

    /// `(service, resource_path, method, id param)` — every row of the
    /// read-by-ID containment table.
    const READ_SCOPE_ROWS: &[(&str, &str, &str, &str)] = &[
        ("drive", "files", "get", "fileId"),
        ("drive", "files", "export", "fileId"),
        ("drive", "files", "copy", "fileId"),
        ("docs", "documents", "get", "documentId"),
        ("sheets", "spreadsheets", "get", "spreadsheetId"),
        ("sheets", "spreadsheets.values", "get", "spreadsheetId"),
        ("slides", "presentations", "get", "presentationId"),
    ];

    /// `(service, resource_path, method, id param)` — every row of the
    /// by-ID *write* containment table.
    const WRITE_SCOPE_ROWS: &[(&str, &str, &str, &str)] = &[
        ("drive", "permissions", "create", "fileId"),
        ("drive", "permissions", "update", "fileId"),
        ("drive", "permissions", "delete", "fileId"),
        ("drive", "files", "update", "fileId"),
        ("drive", "files", "delete", "fileId"),
        ("docs", "documents", "batchUpdate", "documentId"),
        ("sheets", "spreadsheets.values", "update", "spreadsheetId"),
        ("sheets", "spreadsheets.values", "append", "spreadsheetId"),
        ("sheets", "spreadsheets.values", "clear", "spreadsheetId"),
        ("sheets", "spreadsheets", "batchUpdate", "spreadsheetId"),
        ("slides", "presentations", "batchUpdate", "presentationId"),
    ];

    #[test]
    fn test_read_scope_id_param_covers_every_row() {
        for (service, resource, method, expected) in READ_SCOPE_ROWS {
            assert_eq!(
                read_scope_id_param(service, resource, method),
                Some(*expected),
                "{service}/{resource}.{method} must map to its id parameter"
            );
        }
    }

    #[test]
    fn test_read_scope_id_param_ignores_other_requests() {
        // Lists, creates and sub-resources have no single file id to check;
        // by-ID writes are covered by `write_scope_id_param`.
        assert_eq!(read_scope_id_param("drive", "files", "list"), None);
        assert_eq!(read_scope_id_param("drive", "files", "create"), None);
        assert_eq!(read_scope_id_param("drive", "files", "update"), None);
        assert_eq!(read_scope_id_param("drive", "permissions", "get"), None);
        assert_eq!(
            read_scope_id_param("docs", "documents", "batchUpdate"),
            None
        );
        assert_eq!(
            read_scope_id_param("sheets", "spreadsheets.values", "update"),
            None
        );
        assert_eq!(
            read_scope_id_param("slides", "presentations", "batchUpdate"),
            None
        );
        // Services that are not Drive siblings never match, even when the
        // resource and method names coincide.
        assert_eq!(read_scope_id_param("gmail", "files", "get"), None);
        assert_eq!(read_scope_id_param("gmail", "documents", "get"), None);
        assert_eq!(
            read_scope_id_param("calendar", "presentations", "get"),
            None
        );
    }

    #[test]
    fn test_write_scope_id_param_covers_every_row() {
        for (service, resource, method, expected) in WRITE_SCOPE_ROWS {
            assert_eq!(
                write_scope_id_param(service, resource, method),
                Some(*expected),
                "{service}/{resource}.{method} must map to its id parameter"
            );
        }
    }

    #[test]
    fn test_write_scope_id_param_ignores_other_requests() {
        // Lists, reads and creates have no by-ID write target.
        assert_eq!(write_scope_id_param("drive", "files", "list"), None);
        assert_eq!(write_scope_id_param("drive", "files", "create"), None);
        assert_eq!(write_scope_id_param("drive", "files", "get"), None);
        assert_eq!(write_scope_id_param("drive", "files", "copy"), None);
        assert_eq!(write_scope_id_param("drive", "permissions", "list"), None);
        assert_eq!(write_scope_id_param("drive", "permissions", "get"), None);
        assert_eq!(write_scope_id_param("docs", "documents", "get"), None);
        assert_eq!(write_scope_id_param("docs", "documents", "create"), None);
        assert_eq!(write_scope_id_param("sheets", "spreadsheets", "get"), None);
        assert_eq!(
            write_scope_id_param("sheets", "spreadsheets", "create"),
            None
        );
        assert_eq!(
            write_scope_id_param("sheets", "spreadsheets.values", "get"),
            None
        );
        assert_eq!(write_scope_id_param("slides", "presentations", "get"), None);
        // Services outside the Drive family never match.
        assert_eq!(
            write_scope_id_param("gmail", "documents", "batchUpdate"),
            None
        );
        assert_eq!(write_scope_id_param("gmail", "files", "delete"), None);
        assert_eq!(
            write_scope_id_param("calendar", "presentations", "batchUpdate"),
            None
        );
    }

    #[test]
    fn test_write_scope_applies_only_with_recursive_roots() {
        let scoped = sibling_recursive_policy();
        for (service, resource, method, id_param) in WRITE_SCOPE_ROWS {
            assert!(
                write_scope_applies(&scoped, service, resource, method),
                "{service}/{resource}.{method} ('{id_param}') must be scoped"
            );
        }
        assert!(!write_scope_applies(&scoped, "drive", "files", "create"));
        assert!(!write_scope_applies(&scoped, "gmail", "files", "delete"));

        // No recursive roots => no by-ID containment check.
        let unscoped = test_policy();
        for (service, resource, method, _) in WRITE_SCOPE_ROWS {
            assert!(
                !write_scope_applies(&unscoped, service, resource, method),
                "{service}/{resource}.{method} must not be checked without roots"
            );
        }
    }

    #[tokio::test]
    async fn test_enforce_write_scope_without_roots_makes_no_request() {
        // Every write row must short-circuit before the metadata token is
        // acquired / the metadata lookup runs when there are no roots.
        let policy = test_policy();
        let meta = RequestMeta::default();
        let doc = RestDescription::default();
        for (service, resource, method, id_param) in WRITE_SCOPE_ROWS {
            let mut params = Map::new();
            params.insert(id_param.to_string(), json!("outside-id"));
            enforce_read_scope(
                &doc, service, resource, method, &params, &policy, &meta, None, false,
            )
            .await
            .unwrap_or_else(|e| panic!("{service}/{resource}.{method} was checked: {e}"));
        }
    }

    #[tokio::test]
    async fn test_enforce_write_scope_requires_the_row_id_param() {
        let policy = sibling_recursive_policy();
        let meta = RequestMeta::default();
        let cases = [
            ("drive", "permissions", "create", "fileId"),
            ("drive", "permissions", "delete", "fileId"),
            ("drive", "files", "update", "fileId"),
            ("docs", "documents", "batchUpdate", "documentId"),
            ("sheets", "spreadsheets.values", "update", "spreadsheetId"),
            ("slides", "presentations", "batchUpdate", "presentationId"),
        ];
        for (service, resource, method, id_param) in cases {
            // Denial happens while extracting the id, before any metadata
            // lookup, so this needs no network access.
            let err = enforce_read_scope(
                &RestDescription::default(),
                service,
                resource,
                method,
                &Map::new(),
                &policy,
                &meta,
                None,
                false,
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("'{id_param}' is required")),
                "{service}/{resource}.{method}: {err}"
            );
        }
    }

    #[test]
    fn test_write_scope_denies_out_of_tree_document() {
        let policy = sibling_recursive_policy();
        let roots = policy.recursive_parent_values("drive");
        assert!(write_scope_applies(
            &policy,
            "docs",
            "documents",
            "batchUpdate"
        ));
        let err = match read_scope_verdict("outside-doc", &[], &roots) {
            ReadScopeVerdict::Denied(err) => err,
            other => panic!("expected write denial, got {other:?}"),
        };
        let msg = err.to_string();
        // Same phrasing as reads so `server::is_audit_denial` still matches.
        assert!(
            msg.contains("inside the folders allowed by policy"),
            "{msg}"
        );
        assert!(msg.contains("Allowed roots: root-folder"), "{msg}");
    }

    // -- files.list result-side containment tests --

    #[tokio::test]
    async fn test_list_containment_drops_out_of_root_entries() {
        let policy = recursive_policy();
        let meta = RequestMeta::default();
        let mut value = json!({
            "files": [
                { "id": "root-folder", "name": "root", "parents": [] },
                { "id": "inside-file", "name": "in", "parents": ["root-folder"] },
                { "id": "outside-file", "name": "out", "parents": [] }
            ],
            "nextPageToken": "page-2"
        });

        enforce_list_containment(
            &mut value,
            &RestDescription::default(),
            // Never used: every entry is decided by its response `parents`.
            "unused-token",
            &meta,
            &policy,
        )
        .await;

        let ids: Vec<&str> = value["files"]
            .as_array()
            .expect("files array")
            .iter()
            .map(|f| f["id"].as_str().expect("id"))
            .collect();
        assert_eq!(ids, vec!["root-folder", "inside-file"]);
        // Paging still works: the token is not part of the filtered slice.
        assert_eq!(value["nextPageToken"], "page-2");
    }

    #[tokio::test]
    async fn test_list_containment_without_roots_leaves_page_untouched() {
        let meta = RequestMeta::default();
        let mut value = json!({ "files": [{ "id": "a" }, { "id": "b" }] });

        enforce_list_containment(
            &mut value,
            &RestDescription::default(),
            "unused-token",
            &meta,
            &test_policy(),
        )
        .await;

        assert_eq!(value["files"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_list_containment_caps_checked_entries() {
        // More than the cap: the excess is dropped without any metadata call.
        let policy = recursive_policy();
        let meta = RequestMeta::default();
        let files: Vec<Value> = (0..MAX_LIST_CONTAINMENT_FILES + 5)
            .map(|i| json!({ "id": format!("file-{i}"), "parents": ["root-folder"] }))
            .collect();
        let mut value = json!({ "files": files });

        enforce_list_containment(
            &mut value,
            &RestDescription::default(),
            "unused-token",
            &meta,
            &policy,
        )
        .await;

        assert_eq!(
            value["files"].as_array().unwrap().len(),
            MAX_LIST_CONTAINMENT_FILES
        );
    }

    #[test]
    fn test_read_scope_scopes_request_drive_metadata_access() {
        // The metadata lookup must not reuse the request token: for
        // `documents.get` that token carries `documents.readonly`, which the
        // Drive API rejects.
        let scopes = read_scope_scopes();
        assert_eq!(
            scopes,
            vec!["https://www.googleapis.com/auth/drive.readonly".to_string()],
            "the read-scope lookup must ask for the read-only Drive scope"
        );
        assert!(scopes.iter().any(|s| s.ends_with("/drive.readonly")));
        assert!(
            !scopes.iter().any(|s| s.contains("documents")),
            "a Docs scope cannot read Drive metadata: {scopes:?}"
        );
        // Least privilege: never escalate to the full write scope.
        assert!(
            !scopes.iter().any(|s| s.ends_with("/auth/drive")),
            "{scopes:?}"
        );
    }

    #[tokio::test]
    async fn test_read_scope_token_fails_closed_with_actionable_message() {
        // An explicit, missing credentials_file short-circuits credential
        // loading before any network access.
        if std::env::var("GOOGLE_WORKSPACE_CLI_TOKEN").is_ok() {
            return; // env override bypasses credentials_file entirely
        }
        let file: crate::policy::PolicyFile = serde_json::from_value(json!({
            "server": { "credentials_file": "/nonexistent/read-scope-token-test.json" },
            "services": [{ "name": "drive" }]
        }))
        .unwrap();
        let policy = crate::policy::Policy::from_policy_file(file);

        let err = read_scope_token(&policy, None).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot enforce folder scoping"), "{msg}");
        assert!(msg.contains("Drive metadata access"), "{msg}");
        assert!(msg.contains("drive.readonly"), "{msg}");
    }

    #[test]
    fn test_read_scope_applies_only_to_scoped_reads() {
        let scoped = recursive_policy();
        assert!(read_scope_applies(&scoped, "drive", "files", "get"));
        assert!(read_scope_applies(&scoped, "drive", "files", "export"));
        // list already gets a q filter; other methods are writes or unrelated.
        assert!(!read_scope_applies(&scoped, "drive", "files", "list"));
        assert!(!read_scope_applies(&scoped, "drive", "files", "create"));
        assert!(!read_scope_applies(&scoped, "gmail", "files", "get"));

        // No recursive parent constraints => no read-by-ID check.
        let unscoped = test_policy();
        assert!(!read_scope_applies(&unscoped, "drive", "files", "get"));
    }

    #[test]
    fn test_read_scope_applies_to_docs_sheets_slides_and_copy() {
        let scoped = sibling_recursive_policy();
        for (service, resource, method, id_param) in READ_SCOPE_ROWS {
            assert!(
                read_scope_applies(&scoped, service, resource, method),
                "{service}/{resource}.{method} ('{id_param}') must be scoped"
            );
        }
    }

    #[test]
    fn test_read_scope_without_recursive_roots_skips_every_row() {
        let unscoped = test_policy();
        for (service, resource, method, _) in READ_SCOPE_ROWS {
            assert!(
                !read_scope_applies(&unscoped, service, resource, method),
                "{service}/{resource}.{method} must not be checked without roots"
            );
        }
        // The pure decision keeps allowing when there are no roots at all.
        assert_eq!(
            read_scope_decision("any-id", &[], &[]),
            ReadScopeDecision::Allowed
        );
    }

    fn assert_read_scope_denied(file_id: &str, parents: &[String], roots: &[&str]) {
        match read_scope_verdict(file_id, parents, roots) {
            ReadScopeVerdict::Denied(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains(&format!("Read denied: file {file_id}")),
                    "{msg}"
                );
                assert!(msg.contains("Allowed roots: root-folder"), "{msg}");
            }
            other => panic!("expected read denial for {file_id}, got {other:?}"),
        }
    }

    #[test]
    fn test_read_scope_denies_docs_read_outside_roots() {
        let policy = recursive_policy();
        let roots = policy.recursive_parent_values("drive");
        assert!(read_scope_applies(&policy, "docs", "documents", "get"));
        // An outside document either has no visible parents (My Drive root) or
        // parents that cannot reach a root, which the ancestor walk then denies.
        assert_read_scope_denied("outside-doc", &[], &roots);
        assert!(matches!(
            read_scope_verdict("outside-doc", &["other-folder".to_string()], &roots),
            ReadScopeVerdict::WalkAncestors
        ));
    }

    #[test]
    fn test_read_scope_denies_sheets_values_read_outside_roots() {
        let policy = recursive_policy();
        let roots = policy.recursive_parent_values("drive");
        assert!(read_scope_applies(
            &policy,
            "sheets",
            "spreadsheets.values",
            "get"
        ));
        assert_read_scope_denied("outside-sheet", &[], &roots);
    }

    #[test]
    fn test_read_scope_denies_slides_read_outside_roots() {
        let policy = recursive_policy();
        let roots = policy.recursive_parent_values("drive");
        assert!(read_scope_applies(
            &policy,
            "slides",
            "presentations",
            "get"
        ));
        assert_read_scope_denied("outside-deck", &[], &roots);
    }

    #[test]
    fn test_read_scope_denies_drive_copy_from_outside_roots() {
        let policy = recursive_policy();
        let roots = policy.recursive_parent_values("drive");
        assert!(read_scope_applies(&policy, "drive", "files", "copy"));
        assert_read_scope_denied("outside-file", &[], &roots);
    }

    #[test]
    fn test_read_scope_allows_drive_copy_from_inside_roots() {
        let policy = recursive_policy();
        let roots = policy.recursive_parent_values("drive");
        assert!(read_scope_applies(&policy, "drive", "files", "copy"));
        // The root itself and a direct child are allowed without a walk.
        assert!(matches!(
            read_scope_verdict("root-folder", &[], &roots),
            ReadScopeVerdict::Allowed
        ));
        assert!(matches!(
            read_scope_verdict("inside-file", &["root-folder".to_string()], &roots),
            ReadScopeVerdict::Allowed
        ));
    }

    #[tokio::test]
    async fn test_enforce_read_scope_no_roots_skips_sibling_services() {
        // Without recursive roots no metadata lookup may happen for any row;
        // returning Ok here proves no network access was attempted.
        let policy = test_policy();
        let meta = RequestMeta::default();
        let doc = RestDescription::default();
        for (service, resource, method, id_param) in READ_SCOPE_ROWS {
            let mut params = Map::new();
            params.insert(id_param.to_string(), json!("outside-id"));
            enforce_read_scope(
                &doc, service, resource, method, &params, &policy, &meta, None, false,
            )
            .await
            .unwrap_or_else(|e| panic!("{service}/{resource}.{method} was checked: {e}"));
        }
    }

    #[tokio::test]
    async fn test_enforce_read_scope_requires_the_row_id_param() {
        let policy = sibling_recursive_policy();
        let meta = RequestMeta::default();
        // Denial happens while extracting the id, before any metadata lookup.
        let err = enforce_read_scope(
            &RestDescription::default(),
            "docs",
            "documents",
            "get",
            &Map::new(),
            &policy,
            &meta,
            None,
            false,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("'documentId' is required"),
            "{err}"
        );

        let err = enforce_read_scope(
            &RestDescription::default(),
            "slides",
            "presentations",
            "get",
            &Map::new(),
            &policy,
            &meta,
            None,
            false,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("'presentationId' is required"),
            "{err}"
        );
    }

    #[test]
    fn test_drive_files_get_url_uses_drive_discovery_doc() {
        let doc = test_doc_with_files_resource();
        assert_eq!(
            drive_files_get_url(&doc, "abc123"),
            "https://www.googleapis.com/drive/v3/files/abc123"
        );
    }

    #[test]
    fn test_drive_files_get_url_falls_back_for_sibling_services() {
        // Docs/Sheets/Slides discovery documents have no `files` resource.
        let docs_doc = RestDescription {
            root_url: "https://docs.googleapis.com/".to_string(),
            service_path: "v1/".to_string(),
            ..Default::default()
        };
        assert_eq!(
            drive_files_get_url(&docs_doc, "doc1"),
            "https://www.googleapis.com/drive/v3/files/doc1"
        );
        // Ids are encoded as a single path segment with the same encoder as
        // `build_url`, so a `/` in the id cannot escape the collection.
        assert_eq!(
            drive_files_get_url(&docs_doc, "a b/c"),
            "https://www.googleapis.com/drive/v3/files/a%20b%2Fc"
        );
    }

    #[test]
    fn test_read_scope_decision_root_match_allowed() {
        let roots = vec!["root-folder"];
        assert_eq!(
            read_scope_decision("root-folder", &[], &roots),
            ReadScopeDecision::Allowed
        );
    }

    #[test]
    fn test_read_scope_decision_direct_child_allowed() {
        let roots = vec!["root-folder"];
        let parents = vec!["root-folder".to_string()];
        assert_eq!(
            read_scope_decision("file-1", &parents, &roots),
            ReadScopeDecision::Allowed
        );
    }

    #[test]
    fn test_read_scope_decision_descendant_needs_ancestor_walk() {
        let roots = vec!["root-folder"];
        let parents = vec!["sub-folder".to_string()];
        assert_eq!(
            read_scope_decision("file-1", &parents, &roots),
            ReadScopeDecision::CheckAncestors
        );
    }

    #[test]
    fn test_read_scope_decision_outside_denied_via_missing_parents() {
        let roots = vec!["root-folder"];
        // Root of My Drive has no parents at all.
        assert_eq!(
            read_scope_decision("some-file", &[], &roots),
            ReadScopeDecision::MissingParents
        );
    }

    #[test]
    fn test_read_scope_decision_without_roots_performs_no_check() {
        assert_eq!(
            read_scope_decision("any-file", &[], &[]),
            ReadScopeDecision::Allowed
        );
    }

    #[tokio::test]
    async fn test_dry_run_skips_read_scope_network_check() {
        // A recursive-parent policy would trigger a metadata lookup on a live
        // request; dry_run must return before any network access.
        let doc = test_doc();
        let method = test_method();
        let policy = recursive_policy();
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "outside-file" }
        });

        let result = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
        )
        .await
        .unwrap();
        assert_eq!(result["dry_run"], true);
    }

    /// Minimal GET method declaring only `id_param` as a required path param.
    fn get_method_for(id_param: &str) -> RestMethod {
        let mut params = HashMap::new();
        params.insert(
            id_param.to_string(),
            MethodParameter {
                location: Some("path".to_string()),
                required: true,
                ..Default::default()
            },
        );
        RestMethod {
            http_method: "GET".to_string(),
            path: format!("things/{{{id_param}}}"),
            parameters: params,
            scopes: vec!["https://www.googleapis.com/auth/drive.readonly".to_string()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_dry_run_skips_read_scope_for_docs_sheets_slides() {
        // Same short-circuit for the Drive siblings: dry_run must not touch the
        // Drive metadata endpoint even when a read-scope row matches.
        let policy = sibling_recursive_policy();
        let meta = RequestMeta::default();
        let cases = [
            (
                "docs",
                "documents",
                "documentId",
                "https://docs.googleapis.com/",
            ),
            (
                "sheets",
                "spreadsheets.values",
                "spreadsheetId",
                "https://sheets.googleapis.com/",
            ),
            (
                "slides",
                "presentations",
                "presentationId",
                "https://slides.googleapis.com/",
            ),
        ];

        for (service, resource, id_param, root_url) in cases {
            let doc = RestDescription {
                root_url: root_url.to_string(),
                ..Default::default()
            };
            let method = get_method_for(id_param);
            let mut params = Map::new();
            params.insert(id_param.to_string(), json!("outside-id"));
            let args = json!({ "params": params });

            let result = execute_tool(
                &doc, &method, resource, "get", &args, service, &policy, &meta, None, None, true,
                &mut None,
            )
            .await
            .unwrap_or_else(|e| panic!("{service} dry_run should not be scoped: {e}"));

            assert_eq!(result["dry_run"], true, "{service} dry_run");
        }
    }

    // -- allowed_resources enforcement --

    fn policy_with_allowed(service: &str, allowed: &[&str]) -> crate::policy::Policy {
        let file: crate::policy::PolicyFile = serde_json::from_value(json!({
            "services": [{ "name": service, "allowed_resources": allowed }]
        }))
        .unwrap();
        crate::policy::Policy::from_policy_file(file)
    }

    #[test]
    fn test_ensure_resource_allowed_empty_list_allows_all() {
        let policy = test_policy();
        assert!(policy.allowed_resources("drive").is_empty());
        assert!(ensure_resource_allowed("drive", "files", &policy).is_ok());
        assert!(ensure_resource_allowed("drive", "permissions", &policy).is_ok());
    }

    #[test]
    fn test_ensure_resource_allowed_listed_resource_allowed() {
        let policy = policy_with_allowed("drive", &["files"]);
        assert!(ensure_resource_allowed("drive", "files", &policy).is_ok());
    }

    #[test]
    fn test_ensure_resource_allowed_unlisted_resource_denied() {
        let policy = policy_with_allowed("gmail", &["users.labels"]);
        let err = ensure_resource_allowed("gmail", "users.messages", &policy).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("users.messages"), "{msg}");
        assert!(msg.contains("gmail"), "{msg}");
        assert!(msg.contains("users.labels"), "{msg}");
        assert!(msg.contains("allowed_resources"), "{msg}");
    }

    #[test]
    fn test_ensure_resource_allowed_matches_is_resource_allowed() {
        use crate::resources::is_resource_allowed;

        let cases: &[(&[&str], &str)] = &[
            (&[], "users.messages"),
            (&["users.messages"], "users.messages"),
            (&["users.messages"], "users.messages.attachments"),
            (
                &["users.messages", "users.messages.attachments"],
                "users.messages.attachments",
            ),
            (&["users.messages"], "users.labels"),
            (&["spreadsheets"], "spreadsheets.values"),
            (
                &["spreadsheets", "spreadsheets.values"],
                "spreadsheets.values",
            ),
        ];

        for (allowed, resource) in cases {
            let policy = policy_with_allowed("gmail", allowed);
            let allowed_owned: Vec<String> = allowed.iter().map(|s| s.to_string()).collect();
            let expected = is_resource_allowed(&allowed_owned, resource);
            assert_eq!(
                ensure_resource_allowed("gmail", resource, &policy).is_ok(),
                expected,
                "enforcement diverged from is_resource_allowed for {allowed:?} / {resource}"
            );
        }
    }

    #[tokio::test]
    async fn test_execute_tool_denies_unlisted_resource() {
        let doc = test_doc();
        let method = test_method();
        let policy = policy_with_allowed("gmail", &["users.messages"]);
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "users.labels",
            "method": "list",
            "params": {}
        });

        let err = execute_tool(
            &doc,
            &method,
            "users.labels",
            "list",
            &args,
            "gmail",
            &policy,
            &meta,
            None,
            None,
            true,
            &mut None,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("users.labels"), "{err}");
        assert!(err.to_string().contains("users.messages"), "{err}");
    }

    #[tokio::test]
    async fn test_execute_tool_allows_listed_resource() {
        let doc = test_doc();
        let method = test_method();
        let policy = policy_with_allowed("drive", &["files"]);
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc123" }
        });

        let result = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, true,
            &mut None,
        )
        .await
        .unwrap();

        assert_eq!(result["dry_run"], true);
    }

    #[tokio::test]
    async fn test_batch_style_subrequest_denied() {
        // gws_batch executes every sub-request through execute_tool, so a
        // denied sub-request must fail at the same chokepoint.
        let doc = test_doc();
        let method = test_method();
        let policy = policy_with_allowed("gmail", &["users.messages"]);
        let meta = RequestMeta::default();
        let sub_args = json!({
            "resource": "users.labels",
            "method": "list",
            "params": {},
            "body": {}
        });

        let err = execute_tool(
            &doc,
            &method,
            "users.labels",
            "list",
            &sub_args,
            "gmail",
            &policy,
            &meta,
            None,
            None,
            true,
            &mut None,
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("not in allowed_resources"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_resumable_upload_init_denies_unlisted_resource() {
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
        let policy = policy_with_allowed("drive", &["files"]);
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "permissions",
            "method": "create",
            "media_content_type": "text/plain",
            "media_total_size": 10
        });

        // Denial happens during parse_args, before any auth or network access.
        let err =
            initiate_resumable_upload(&doc, &method, &args, "drive", &policy, &meta, &mut None)
                .await
                .unwrap_err();

        assert!(err.to_string().contains("permissions"), "{err}");
    }

    #[tokio::test]
    async fn test_resumable_upload_init_applies_write_scope_check() {
        // A resumable `files.update` must be ancestry-checked before the upload
        // session is created. A missing credentials file makes the internal
        // Drive metadata token fail closed, which proves the check ran without
        // any live network call.
        if std::env::var("GOOGLE_WORKSPACE_CLI_TOKEN").is_ok() {
            return; // env override bypasses credentials_file entirely
        }
        let doc = test_doc();
        let mut method = test_method();
        method.http_method = "PUT".to_string();
        let file: crate::policy::PolicyFile = serde_json::from_value(json!({
            "server": { "credentials_file": "/nonexistent/resumable-scope-test.json" },
            "services": [{
                "name": "drive",
                "constraints": [{
                    "param": "parents",
                    "values": ["root-folder"],
                    "access": "read-write",
                    "location": "body-write-only",
                    "mode": "restrict",
                    "recursive": true
                }]
            }]
        }))
        .unwrap();
        let policy = crate::policy::Policy::from_policy_file(file);
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "update",
            "params": { "fileId": "outside-file" },
            "media_content_type": "text/plain",
            "media_total_size": 10
        });

        let err =
            initiate_resumable_upload(&doc, &method, &args, "drive", &policy, &meta, &mut None)
                .await
                .unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("cannot enforce folder scoping"), "{msg}");
        assert!(msg.contains("drive.readonly"), "{msg}");
        // The failure is the scope check, not the upload endpoint.
        assert!(!msg.contains("Resumable upload init failed"), "{msg}");
    }

    #[tokio::test]
    async fn test_execute_tool_rejects_unlisted_client_subject() {
        // Subject resolution happens before any HTTP call, so this verifies the
        // execute chokepoint without network access.
        let doc = test_doc();
        let method = test_method();
        let policy = test_policy();
        let meta = RequestMeta {
            user_email: Some("attacker@example.com".to_string()),
            ..Default::default()
        };
        let args = json!({
            "resource": "files",
            "method": "get",
            "params": { "fileId": "abc123" }
        });

        let err = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, None, false,
            &mut None,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("not in server.allowed_subjects"),
            "unexpected error: {err}"
        );
    }
}
