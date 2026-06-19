use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use google_workspace::discovery::{RestDescription, RestResource};
use google_workspace::error::GwsError;
use google_workspace::services;

use crate::policy::Policy;

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
) -> Result<Vec<Value>, GwsError> {
    let mut tools = Vec::new();

    for svc_name in policy.allowed_services() {
        let doc = match get_or_fetch_doc(docs, svc_name).await {
            Ok(doc) => doc,
            Err(e) => {
                tracing::warn!(service = svc_name, error = %e, "Failed to load discovery doc");
                continue;
            }
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

        let description = if resource_names.is_empty() {
            format!("{desc}{read_only_note}")
        } else {
            format!(
                "{desc}{read_only_note}. Resources: {}",
                resource_names.join(", ")
            )
        };

        tools.push(json!({
            "name": svc_name,
            "title": title,
            "description": description,
            "annotations": {
                "readOnlyHint": is_read_only,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true
            },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "resource": {
                        "type": "string",
                        "description": "Resource name (e.g., files, permissions)"
                    },
                    "method": {
                        "type": "string",
                        "description": "Method name (e.g., list, get, create)"
                    },
                    "params": {
                        "type": "object",
                        "description": "Query or path parameters"
                    },
                    "body": {
                        "type": "object",
                        "description": "Request body"
                    },
                    "page_all": {
                        "type": "boolean",
                        "description": "Auto-paginate, returning all pages"
                    },
                    "media_data": {
                        "type": "string",
                        "description": "Base64-encoded file content for media upload"
                    },
                    "media_content_type": {
                        "type": "string",
                        "description": "MIME type of the media content (e.g., application/pdf, image/png)"
                    },
                    "media_upload_init": {
                        "type": "boolean",
                        "description": "Start a resumable upload session for large files (>10MB)"
                    },
                    "media_total_size": {
                        "type": "integer",
                        "description": "Total file size in bytes (for resumable uploads)"
                    },
                    "upload_handle": {
                        "type": "string",
                        "description": "Handle from a previous media_upload_init call"
                    },
                    "media_chunk": {
                        "type": "string",
                        "description": "Base64-encoded chunk data (for resumable uploads)"
                    },
                    "media_chunk_offset": {
                        "type": "integer",
                        "description": "Byte offset of this chunk (0-based, for resumable uploads)"
                    },
                    "download_handle": {
                        "type": "string",
                        "description": "Handle from a large file download to retrieve chunks"
                    },
                    "download_chunk_offset": {
                        "type": "integer",
                        "description": "Base64 char offset for the next download chunk (0-based)"
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "If true, returns the HTTP request that would be sent without executing it. Shows URL, method, scopes, and body."
                    }
                },
                "required": ["resource", "method"]
            }
        }));
    }

    tools.push(json!({
        "name": "gws_discover",
        "title": "API Schema Discovery",
        "description": "Query available resources, methods, and parameter schemas for any enabled service. Call with service only to list resources; add resource to list methods; add method to get full parameter schema.",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "service": {
                    "type": "string",
                    "description": "Service name (e.g., drive, gmail)"
                },
                "resource": {
                    "type": "string",
                    "description": "Resource name to list methods for"
                },
                "method": {
                    "type": "string",
                    "description": "Method name to get full parameter schema"
                }
            },
            "required": ["service"]
        }
    }));

    tools.push(json!({
        "name": "gws_batch",
        "title": "Batch API Calls",
        "description": "Execute multiple Google API calls in a single request. All sub-requests are validated against policy before any are executed. Max 100 requests per batch.",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true
        },
        "inputSchema": {
            "type": "object",
            "properties": {
                "service": {
                    "type": "string",
                    "description": "Service name (e.g., drive, gmail)"
                },
                "requests": {
                    "type": "array",
                    "description": "Array of sub-requests to execute",
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "resource": {
                                "type": "string",
                                "description": "Resource name (e.g., files, permissions)"
                            },
                            "method": {
                                "type": "string",
                                "description": "Method name (e.g., list, get, create)"
                            },
                            "params": {
                                "type": "object",
                                "description": "Query or path parameters"
                            },
                            "body": {
                                "type": "object",
                                "description": "Request body"
                            }
                        },
                        "required": ["resource", "method"]
                    }
                }
            },
            "required": ["service", "requests"]
        }
    }));

    tools.extend(crate::helpers::helper_tool_schemas());
    tools.push(crate::helpers::markdown_tool_schema());

    Ok(tools)
}

pub async fn handle_discover(
    arguments: &Value,
    policy: &Policy,
    docs: &mut HashMap<String, Arc<RestDescription>>,
) -> Result<Value, GwsError> {
    let service = arguments
        .get("service")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation("Missing 'service' in gws_discover".to_string()))?;

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
}
