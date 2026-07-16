use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Value, json};

use google_workspace::discovery::{RestDescription, RestResource};
use google_workspace::error::GwsError;
use google_workspace::services;

use crate::policy::Policy;

fn method_hints(service: &str) -> Option<&'static str> {
    match service {
        "drive" => Some(
            "resource=files method=list, resource=files method=get params={fileId: ID}, resource=files method=create body={name: NAME, mimeType: TYPE, parents: [FOLDER_ID]}",
        ),
        "docs" => Some(
            "resource=documents method=get params={documentId: ID}, resource=documents method=create body={title: TITLE}",
        ),
        "gmail" => Some(
            "resource=messages method=list, resource=messages method=get params={userId: me, id: MSG_ID}",
        ),
        "calendar" => Some(
            "resource=events method=list params={calendarId: primary}, resource=events method=get params={calendarId: primary, eventId: ID}",
        ),
        "sheets" => Some("resource=spreadsheets method=get params={spreadsheetId: ID}"),
        "slides" => Some(
            "resource=presentations method=get params={presentationId: ID}, resource=presentations method=create body={title: TITLE}",
        ),
        _ => None,
    }
}

fn tool_from_json(schema: Value) -> Tool {
    serde_json::from_value(schema).expect("tool schema must be valid")
}

fn make_tool(
    name: impl Into<String>,
    title: impl Into<String>,
    description: impl Into<String>,
    annotations: ToolAnnotations,
    input_schema: Value,
) -> Tool {
    let schema: rmcp::model::JsonObject =
        serde_json::from_value(input_schema).expect("input schema must be an object");
    Tool::new(name.into(), description.into(), Arc::new(schema))
        .with_title(title.into())
        .with_annotations(annotations)
}

pub(crate) async fn get_or_fetch_doc(
    docs: &mut HashMap<String, Arc<RestDescription>>,
    svc_alias: &str,
) -> Result<Arc<RestDescription>, GwsError> {
    if !docs.contains_key(svc_alias) {
        let (api_name, version) = services::resolve_service(svc_alias)?;
        let cache_dir = dirs_next::cache_dir().map(|d| d.join("mcp-gws").join("discovery"));
        let doc = google_workspace::discovery::fetch_discovery_document(
            &api_name,
            &version,
            cache_dir.as_deref(),
        )
        .await?;
        docs.insert(svc_alias.to_string(), Arc::new(doc));
    }
    Ok(Arc::clone(docs.get(svc_alias).unwrap()))
}

pub async fn build_tools_list(
    policy: &Policy,
    docs: &mut HashMap<String, Arc<RestDescription>>,
) -> Result<Vec<Tool>, GwsError> {
    let mut tools = Vec::new();

    const SERVICES_WITH_HELPERS: &[&str] = &["drive", "docs", "sheets", "slides"];

    // Pre-load discovery docs for all services (helpers need them)
    for svc_name in policy.allowed_services() {
        if let Err(e) = get_or_fetch_doc(docs, svc_name).await {
            tracing::warn!(service = svc_name, error = %e, "Failed to load discovery doc");
        }
    }

    // Only register generic tools for services without helpers
    for svc_name in policy.allowed_services() {
        if SERVICES_WITH_HELPERS.contains(&svc_name) {
            continue;
        }

        let doc = match docs.get(svc_name) {
            Some(doc) => Arc::clone(doc),
            None => continue,
        };

        let mut resource_names = Vec::new();
        collect_resource_paths(&doc.resources, "", &mut resource_names);
        resource_names.sort();

        let svc_entry = services::SERVICES
            .iter()
            .find(|e| e.aliases.contains(&svc_name));
        let desc = svc_entry.map(|e| e.description).unwrap_or("Google API");
        let title = svc_entry
            .map(|e| {
                let base = e.description.split('.').next().unwrap_or(e.description);
                format!("Google {base}")
            })
            .unwrap_or_else(|| format!("Google {svc_name}"));

        let is_read_only = policy.is_read_only(svc_name);
        let read_only_note = if is_read_only { " [READ-ONLY]" } else { "" };

        let mut description = if resource_names.is_empty() {
            format!("{desc}{read_only_note}")
        } else {
            format!(
                "{desc}{read_only_note}. Resources: {}",
                resource_names.join(", ")
            )
        };

        if policy.compact_schemas {
            if let Some(hints) = method_hints(svc_name) {
                description.push_str(&format!(". Common: {hints}"));
            }
        }

        let annotations = ToolAnnotations::new()
            .read_only(is_read_only)
            .destructive(false)
            .idempotent(false)
            .open_world(true);

        let schema = if policy.compact_schemas {
            json!({
                "type": "object",
                "properties": {
                    "resource": { "type": "string", "description": "Resource name" },
                    "method": { "type": "string", "description": "Method name" },
                    "params": { "type": "object", "description": "Query/path parameters" },
                    "body": { "type": "object", "description": "Request body" },
                    "fields": { "type": "string", "description": "Response field mask" },
                    "page_all": { "type": "boolean", "description": "Auto-paginate" }
                },
                "required": ["resource", "method"]
            })
        } else {
            json!({
                "type": "object",
                "properties": {
                    "resource": { "type": "string", "description": "Resource name (e.g., files, permissions)" },
                    "method": { "type": "string", "description": "Method name (e.g., list, get, create)" },
                    "params": { "type": "object", "description": "Query or path parameters" },
                    "body": { "type": "object", "description": "Request body" },
                    "fields": { "type": "string", "description": "Response field mask (e.g., id,name,mimeType)" },
                    "page_all": { "type": "boolean", "description": "Auto-paginate, returning all pages" },
                    "media_data": { "type": "string", "description": "Base64-encoded file content for media upload" },
                    "media_content_type": { "type": "string", "description": "MIME type of the media content" },
                    "media_upload_init": { "type": "boolean", "description": "Start a resumable upload session for large files (>10MB)" },
                    "media_total_size": { "type": "integer", "description": "Total file size in bytes (for resumable uploads)" },
                    "upload_handle": { "type": "string", "description": "Handle from a previous media_upload_init call" },
                    "media_chunk": { "type": "string", "description": "Base64-encoded chunk data (for resumable uploads)" },
                    "media_chunk_offset": { "type": "integer", "description": "Byte offset of this chunk (0-based)" },
                    "download_handle": { "type": "string", "description": "Handle from a large file download" },
                    "download_chunk_offset": { "type": "integer", "description": "Base64 char offset for next download chunk" },
                    "dry_run": { "type": "boolean", "description": "Preview the HTTP request without executing" }
                },
                "required": ["resource", "method"]
            })
        };

        tools.push(make_tool(
            svc_name,
            &title,
            &description,
            annotations,
            schema,
        ));
    }

    tools.push(make_tool(
        "gws_discover",
        "API & Tool Discovery",
        "Discover API schemas or get full tool documentation. Use tool= for helper tool details, or service= for API resources/methods.",
        ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false),
        json!({
            "type": "object",
            "properties": {
                "tool": { "type": "string", "description": "Helper tool name for full schema and usage (e.g., gws_docs_write)" },
                "service": { "type": "string", "description": "Service name for API discovery (e.g., drive, gmail)" },
                "resource": { "type": "string", "description": "Resource name to list methods" },
                "method": { "type": "string", "description": "Method name for full parameter schema" }
            }
        }),
    ));

    tools.push(make_tool(
        "gws_batch",
        "Batch API Calls",
        "Execute multiple Google API calls in a single request. All sub-requests are validated against policy before any are executed. Max 100 requests per batch.",
        ToolAnnotations::new().read_only(false).destructive(false).idempotent(false).open_world(true),
        json!({
            "type": "object",
            "properties": {
                "service": { "type": "string", "description": "Service name (e.g., drive, gmail)" },
                "requests": {
                    "type": "array",
                    "description": "Array of sub-requests to execute",
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "resource": { "type": "string", "description": "Resource name" },
                            "method": { "type": "string", "description": "Method name" },
                            "params": { "type": "object", "description": "Query or path parameters" },
                            "body": { "type": "object", "description": "Request body" }
                        },
                        "required": ["resource", "method"]
                    }
                }
            },
            "required": ["service", "requests"]
        }),
    ));

    // Drive helper tools
    tools.push(tool_from_json(
        crate::drive_helpers::drive_list_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_find_folder_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_info_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_create_folder_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_copy_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_rename_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_move_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_share_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::drive_helpers::drive_delete_tool_schema(),
    ));

    // Docs helper tools
    tools.push(tool_from_json(crate::helpers::docs_write_tool_schema()));
    tools.push(tool_from_json(crate::helpers::docs_read_tool_schema()));
    tools.push(tool_from_json(
        crate::helpers::docs_replace_section_tool_schema(),
    ));
    tools.push(tool_from_json(crate::helpers::outline_tool_schema()));
    tools.push(tool_from_json(crate::helpers::find_tool_schema()));
    tools.push(tool_from_json(crate::helpers::insert_table_tool_schema()));
    tools.push(tool_from_json(crate::helpers::insert_image_tool_schema()));
    if !policy.compact_schemas {
        tools.push(tool_from_json(crate::helpers::read_table_tool_schema()));
        tools.push(tool_from_json(crate::helpers::format_tool_schema()));
    }

    // Sheets helper tools
    tools.push(tool_from_json(
        crate::sheets_helpers::sheets_read_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::sheets_helpers::sheets_write_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::sheets_helpers::sheets_info_tool_schema(),
    ));
    if !policy.compact_schemas {
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_append_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_clear_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_manage_tabs_tool_schema(),
        ));
    }

    // Slides & image tools
    tools.push(tool_from_json(crate::slides_helpers::marp_tool_schema()));
    tools.push(tool_from_json(
        crate::slides_helpers::templates_tool_schema(),
    ));
    tools.push(tool_from_json(crate::image_gen::image_gen_tool_schema()));

    Ok(tools)
}

fn tool_full_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "gws_drive_list" => crate::drive_helpers::drive_list_tool_schema(),
        "gws_drive_find_folder" => crate::drive_helpers::drive_find_folder_tool_schema(),
        "gws_drive_info" => crate::drive_helpers::drive_info_tool_schema(),
        "gws_drive_create_folder" => crate::drive_helpers::drive_create_folder_tool_schema(),
        "gws_drive_move" => crate::drive_helpers::drive_move_tool_schema(),
        "gws_drive_share" => crate::drive_helpers::drive_share_tool_schema(),
        "gws_drive_trash" => crate::drive_helpers::drive_delete_tool_schema(),
        "gws_drive_copy" => crate::drive_helpers::drive_copy_tool_schema(),
        "gws_drive_rename" => crate::drive_helpers::drive_rename_tool_schema(),
        "gws_docs_write" => crate::helpers::docs_write_tool_schema(),
        "gws_docs_replace_section" => crate::helpers::docs_replace_section_tool_schema(),
        "gws_docs_read" => crate::helpers::docs_read_tool_schema(),
        "gws_docs_outline" => crate::helpers::outline_tool_schema(),
        "gws_docs_find" => crate::helpers::find_tool_schema(),
        "gws_docs_insert_table" => crate::helpers::insert_table_tool_schema(),
        "gws_docs_read_table" => crate::helpers::read_table_tool_schema(),
        "gws_docs_insert_image" => crate::helpers::insert_image_tool_schema(),
        "gws_docs_format" => crate::helpers::format_tool_schema(),
        "gws_sheets_read" => crate::sheets_helpers::sheets_read_tool_schema(),
        "gws_sheets_write" => crate::sheets_helpers::sheets_write_tool_schema(),
        "gws_sheets_append" => crate::sheets_helpers::sheets_append_tool_schema(),
        "gws_sheets_info" => crate::sheets_helpers::sheets_info_tool_schema(),
        "gws_sheets_clear" => crate::sheets_helpers::sheets_clear_tool_schema(),
        "gws_sheets_manage_tabs" => crate::sheets_helpers::sheets_manage_tabs_tool_schema(),
        _ => return None,
    };
    let mut info = json!({
        "tool": name,
        "title": schema.get("title"),
        "description": schema.get("description"),
        "inputSchema": schema.get("inputSchema"),
        "annotations": schema.get("annotations"),
    });
    if let Some(extras) = tool_extra_info(name) {
        info["usage"] = json!(extras);
    }
    Some(info)
}

fn tool_extra_info(name: &str) -> Option<&'static str> {
    match name {
        "gws_docs_write" => Some(
            "Also accepts: template_id (doc ID to copy named styles from), \
            index (character position, overrides position).",
        ),
        "gws_docs_read" => {
            Some("format=plain returns raw text. Use gws_docs_outline for structure with indexes.")
        }
        "gws_docs_format" => Some(
            "Target by text (auto-finds position) or start_index/end_index. \
            Use gws_docs_find to get indexes first.",
        ),
        "gws_docs_insert_image" => Some(
            "For Drive images, use drive_file_id (downloaded and embedded, no sharing needed). \
            For generated images, call gws_generate_image first.",
        ),
        _ => None,
    }
}

pub async fn handle_discover(
    arguments: &Value,
    policy: &Policy,
    docs: &mut HashMap<String, Arc<RestDescription>>,
) -> Result<Value, GwsError> {
    if let Some(tool_name) = arguments.get("tool").and_then(|v| v.as_str()) {
        let result = tool_full_schema(tool_name).ok_or_else(|| {
            GwsError::Validation(format!(
                "Unknown tool '{tool_name}'. Helper tools: gws_docs_write, gws_docs_read, \
                gws_docs_outline, gws_docs_find, gws_docs_insert_table, gws_docs_read_table, \
                gws_docs_insert_image, gws_docs_format."
            ))
        })?;
        return Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
            "isError": false
        }));
    }

    let service = arguments
        .get("service")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            GwsError::Validation("Missing 'service' or 'tool' in gws_discover".to_string())
        })?;

    if !policy.is_service_allowed(service) {
        return Err(GwsError::Validation(format!(
            "Service '{service}' is not enabled. Enabled: {}",
            policy.allowed_services().join(", ")
        )));
    }

    let doc = get_or_fetch_doc(docs, service).await?;

    let resource_name = arguments.get("resource").and_then(|v| v.as_str());
    let method_name = arguments.get("method").and_then(|v| v.as_str());

    let result = match (resource_name, method_name) {
        (None, _) => {
            let mut entries = Vec::new();
            collect_resource_entries(&doc.resources, "", &mut entries);
            json!({ "service": service, "resources": entries })
        }
        (Some(res), None) => {
            let resource = find_resource(&doc.resources, res).ok_or_else(|| {
                let mut all = Vec::new();
                collect_resource_paths(&doc.resources, "", &mut all);
                GwsError::Validation(format!(
                    "Resource '{res}' not found in {service}. Available: {}",
                    all.join(", ")
                ))
            })?;
            let methods: Vec<Value> = resource
                .methods
                .iter()
                .map(|(name, m)| {
                    json!({
                        "name": name,
                        "httpMethod": m.http_method,
                        "description": m.description.as_deref().unwrap_or("")
                    })
                })
                .collect();
            let sub: Vec<&str> = resource.resources.keys().map(|s| s.as_str()).collect();
            let mut r = json!({ "service": service, "resource": res, "methods": methods });
            if !sub.is_empty() {
                r["subResources"] = json!(sub);
            }
            r
        }
        (Some(res), Some(meth)) => {
            let resource = find_resource(&doc.resources, res).ok_or_else(|| {
                GwsError::Validation(format!("Resource '{res}' not found in {service}"))
            })?;
            let method = resource.methods.get(meth).ok_or_else(|| {
                GwsError::Validation(format!(
                    "Method '{meth}' not found in {service}.{res}. Available: {}",
                    resource
                        .methods
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            let params: Vec<Value> = method
                .parameters
                .iter()
                .map(|(name, p)| {
                    json!({
                        "name": name,
                        "type": p.param_type.as_deref().unwrap_or("string"),
                        "required": p.required,
                        "location": p.location.as_deref().unwrap_or("query"),
                        "description": p.description.as_deref().unwrap_or("")
                    })
                })
                .collect();
            let mut result = json!({
                "service": service,
                "resource": res,
                "method": meth,
                "httpMethod": method.http_method,
                "description": method.description.as_deref().unwrap_or(""),
                "parameters": params,
                "supportsMediaUpload": method.supports_media_upload,
                "supportsMediaDownload": method.supports_media_download
            });
            if let Some(ref mu) = method.media_upload
                && let Some(ref accept) = mu.accept
            {
                result["mediaUploadAccept"] = json!(accept);
            }
            result
        }
    };

    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
        "isError": false
    }))
}

fn collect_resource_paths(
    resources: &HashMap<String, RestResource>,
    prefix: &str,
    out: &mut Vec<String>,
) {
    for (name, res) in resources {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        out.push(path.clone());
        if !res.resources.is_empty() {
            collect_resource_paths(&res.resources, &path, out);
        }
    }
}

fn collect_resource_entries(
    resources: &HashMap<String, RestResource>,
    prefix: &str,
    out: &mut Vec<Value>,
) {
    for (name, res) in resources {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let methods: Vec<&str> = res.methods.keys().map(|s| s.as_str()).collect();
        if !methods.is_empty() {
            out.push(json!({ "name": path.clone(), "methods": methods }));
        }
        if !res.resources.is_empty() {
            collect_resource_entries(&res.resources, &path, out);
        }
    }
}

pub fn find_resource<'a>(
    resources: &'a HashMap<String, RestResource>,
    path: &str,
) -> Option<&'a RestResource> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut current = resources.get(first)?;
    for segment in segments {
        current = current.resources.get(segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_workspace::discovery::RestMethod;

    fn make_resources() -> HashMap<String, RestResource> {
        let mut revisions = RestResource {
            methods: HashMap::new(),
            resources: HashMap::new(),
        };
        revisions.methods.insert(
            "list".to_string(),
            RestMethod {
                http_method: "GET".to_string(),
                ..Default::default()
            },
        );

        let mut files = RestResource {
            methods: HashMap::new(),
            resources: HashMap::new(),
        };
        files.methods.insert(
            "get".to_string(),
            RestMethod {
                http_method: "GET".to_string(),
                ..Default::default()
            },
        );
        files.resources.insert("revisions".to_string(), revisions);

        let mut resources = HashMap::new();
        resources.insert("files".to_string(), files);
        resources
    }

    #[test]
    fn test_find_resource_top_level() {
        let resources = make_resources();
        let found = find_resource(&resources, "files");
        assert!(found.is_some());
        assert!(found.unwrap().methods.contains_key("get"));
    }

    #[test]
    fn test_find_resource_nested() {
        let resources = make_resources();
        let found = find_resource(&resources, "files.revisions");
        assert!(found.is_some());
        assert!(found.unwrap().methods.contains_key("list"));
    }

    #[test]
    fn test_find_resource_not_found() {
        let resources = make_resources();
        assert!(find_resource(&resources, "nonexistent").is_none());
        assert!(find_resource(&resources, "files.nonexistent").is_none());
    }

    #[test]
    fn test_collect_resource_paths_includes_nested() {
        let resources = make_resources();
        let mut paths = Vec::new();
        collect_resource_paths(&resources, "", &mut paths);
        paths.sort();
        assert!(paths.contains(&"files".to_string()));
        assert!(paths.contains(&"files.revisions".to_string()));
    }

    #[test]
    fn test_collect_resource_entries_has_methods() {
        let resources = make_resources();
        let mut entries = Vec::new();
        collect_resource_entries(&resources, "", &mut entries);
        assert!(!entries.is_empty());
        let files_entry = entries.iter().find(|e| e["name"] == "files").unwrap();
        let methods = files_entry["methods"].as_array().unwrap();
        assert!(methods.iter().any(|m| m == "get"));
    }

    #[test]
    fn test_find_resource_deeply_nested_miss() {
        let resources = make_resources();
        assert!(find_resource(&resources, "files.revisions.nonexistent").is_none());
    }

    #[test]
    fn test_find_resource_empty_path() {
        let resources = make_resources();
        assert!(find_resource(&resources, "").is_none());
    }

    #[test]
    fn test_tool_full_schema_known() {
        let result = tool_full_schema("gws_docs_write");
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info["tool"], "gws_docs_write");
        assert!(info["inputSchema"].is_object());
        assert!(info["usage"].is_string());
    }

    #[test]
    fn test_tool_full_schema_unknown() {
        assert!(tool_full_schema("nonexistent_tool").is_none());
    }

    #[test]
    fn test_tool_full_schema_all_tools() {
        let names = [
            "gws_docs_write",
            "gws_docs_read",
            "gws_docs_outline",
            "gws_docs_find",
            "gws_docs_insert_table",
            "gws_docs_read_table",
            "gws_docs_insert_image",
            "gws_docs_format",
        ];
        for name in &names {
            assert!(
                tool_full_schema(name).is_some(),
                "Missing schema for {name}"
            );
        }
    }
}
