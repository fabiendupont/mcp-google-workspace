use std::collections::HashSet;

use base64::Engine;
use serde_json::{Map, Value, json};

use google_workspace::client;
use google_workspace::discovery::{RestDescription, RestMethod};
use google_workspace::error::GwsError;
use google_workspace::validate;

use crate::meta::RequestMeta;
use crate::policy::Policy;

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(input: &str) -> Result<Vec<u8>, GwsError> {
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|_| GwsError::Validation("Invalid base64 data".to_string()))
}

fn gws_err(msg: impl std::fmt::Display) -> GwsError {
    GwsError::Other(anyhow::anyhow!("{msg}"))
}

fn apply_common_headers(
    mut request: reqwest::RequestBuilder,
    meta: &RequestMeta,
) -> reqwest::RequestBuilder {
    if let Some(quota_project) = crate::auth::get_quota_project() {
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

pub type NotifySender = tokio::sync::mpsc::UnboundedSender<Value>;

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const DOWNLOAD_CHUNK_SIZE: usize = 10 * 1024 * 1024;

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(doc, method, arguments, policy, meta, notify_tx), fields(service, resource = resource_path, method_name))]
pub async fn execute_tool(
    doc: &RestDescription,
    method: &RestMethod,
    resource_path: &str,
    method_name: &str,
    arguments: &Value,
    service: &str,
    policy: &Policy,
    meta: &RequestMeta,
    notify_tx: Option<&NotifySender>,
    dry_run: bool,
) -> Result<Value, GwsError> {
    policy.check_method(service, resource_path, method_name, method)?;

    let mut params: Map<String, Value> = arguments
        .get("params")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    // Drop empty body objects — LLMs commonly send "body": {} on GET methods.
    let body: Option<Value> = arguments
        .get("body")
        .filter(|v| !v.as_object().is_some_and(|m| m.is_empty()))
        .cloned();

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

    if service == "drive" {
        policy.enforce_drive_folder_list(service, &mut params);
        if method.http_method != "GET" {
            policy.enforce_drive_folder_write(service, &body)?;
            policy.enforce_drive_folder_params(service, &params)?;
        }
    }

    if service == "calendar" {
        policy.enforce_calendar(service, method, &params)?;
    }

    let scopes: Vec<&str> = select_scope(&method.scopes).into_iter().collect();

    let is_upload = media_data.is_some() && method.supports_media_upload;
    let is_download = method.supports_media_download
        && params
            .get("alt")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "media");

    let url = if is_upload {
        build_upload_url(doc, method, &params)?
    } else {
        let (u, _) = build_url(doc, method, &params)?;
        u
    };
    let (_, query_params) = build_url(doc, method, &params)?;

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
                return Err(GwsError::Validation(format!(
                    "Media upload too large: {} bytes (max {})",
                    raw_bytes.len(),
                    MAX_UPLOAD_BYTES
                )));
            }
            let (multipart_body, content_type) =
                build_multipart_body(&body, &raw_bytes, media_content_type)?;
            dry["upload_content_type"] = json!(content_type);
            dry["upload_body_size"] = json!(multipart_body.len());
        }
        if let Some(ref b) = body {
            dry["body"] = b.clone();
        }
        return Ok(dry);
    }

    let token = crate::auth::get_token(&scopes, policy.credentials_file.as_deref())
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
        request = apply_common_headers(request, meta);

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
                    return Err(GwsError::Validation(format!(
                        "Media upload too large: {} bytes (max {})",
                        raw_bytes.len(),
                        MAX_UPLOAD_BYTES
                    )));
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
            let bytes = response
                .bytes()
                .await
                .map_err(|e| gws_err(format!("Failed to read binary response: {e}")))?;
            let total = bytes.len();
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
            return Err(parse_api_error(status.as_u16(), &body_text));
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
                if let Some(tx) = notify_tx {
                    let _ = tx.send(json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": {
                            "progress": all_results.len(),
                            "total": page_limit,
                            "message": format!("Fetched page {}", all_results.len())
                        }
                    }));
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
    GwsError::Api {
        code: status,
        message: body.to_string(),
        reason: String::new(),
        enable_url: None,
    }
}

pub(crate) async fn initiate_resumable_upload(
    doc: &RestDescription,
    method: &RestMethod,
    arguments: &Value,
    service: &str,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<Value, GwsError> {
    let params: Map<String, Value> = arguments
        .get("params")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let body: Option<Value> = arguments
        .get("body")
        .filter(|v| !v.as_object().is_some_and(|m| m.is_empty()))
        .cloned();

    let resource_path = arguments
        .get("resource")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let method_name = arguments
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    policy.check_method(service, resource_path, method_name, method)?;

    if service == "drive" && method.http_method != "GET" {
        policy.enforce_drive_folder_write(service, &body)?;
    }

    let content_type = arguments
        .get("media_content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream");
    let total_size = arguments
        .get("media_total_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let scopes: Vec<&str> = select_scope(&method.scopes).into_iter().collect();
    let token = crate::auth::get_token(&scopes, policy.credentials_file.as_deref())
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

    request = apply_common_headers(request, meta);

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
                assert_eq!(message, "Internal Server Error");
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, true,
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
            let toml = r#"
[server]
read_only = true
[[services]]
name = "drive"
"#;
            let file: crate::policy::PolicyFile = toml::from_str(toml).unwrap();
            crate::policy::Policy::from_policy_file(file)
        };
        let meta = RequestMeta::default();
        let args = json!({
            "resource": "files",
            "method": "create",
            "params": { "fileId": "abc" }
        });

        let err = execute_tool(
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, true,
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
            "media_data": "SGVsbG8=",
            "media_content_type": "text/plain"
        });

        let err = execute_tool(
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, true,
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
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, true,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, true,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, true,
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
            &doc, &method, "files", "get", &args, "drive", &policy, &meta, None, true,
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
            "media_data": big_data,
            "media_content_type": "application/octet-stream"
        });

        let err = execute_tool(
            &doc, &method, "files", "create", &args, "drive", &policy, &meta, None, true,
        )
        .await;

        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("too large"));
    }
}
