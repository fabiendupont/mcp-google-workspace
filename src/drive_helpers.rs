use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::server::ServerState;
use crate::tools;
use google_workspace::error::GwsError;
use serde_json::{Value, json};

pub fn validate_file_id(id: &str) -> Result<(), String> {
    if id.len() < 20 {
        return Err(format!(
            "Invalid file ID '{}' (too short — Google Drive IDs are typically 33-44 characters). \
             Check the ID from the previous tool call response.",
            id
        ));
    }
    Ok(())
}

pub fn drive_list_tool_schema() -> Value {
    json!({
        "name": "gws_drive_list",
        "title": "List Drive Files",
        "description": "List or search files in Drive. Filter by folder, name, or type.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "folder_id": {
                    "type": "string",
                    "description": "Folder ID to list contents of"
                },
                "query": {
                    "type": "string",
                    "description": "Search by name (partial match)"
                },
                "type": {
                    "type": "string",
                    "enum": ["folder", "document", "spreadsheet", "presentation", "pdf", "image"],
                    "description": "Filter by file type"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum files to return",
                    "default": 20
                }
            }
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "mimeType": { "type": "string" },
                            "modifiedTime": { "type": "string" },
                            "size": { "type": "string" },
                            "parents": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["id", "name", "mimeType"]
                    }
                }
            },
            "required": ["files"]
        }
    })
}

pub fn drive_info_tool_schema() -> Value {
    json!({
        "name": "gws_drive_info",
        "title": "File Info",
        "description": "Get file metadata: name, type, size, sharing, last modified.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "Drive file or folder ID"
                }
            },
            "required": ["file_id"]
        }
    })
}

pub fn drive_create_folder_tool_schema() -> Value {
    json!({
        "name": "gws_drive_create_folder",
        "title": "Create Folder",
        "description": "Create a folder in Google Drive.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Folder name"
                },
                "parent_id": {
                    "type": "string",
                    "description": "Parent folder ID (omit for root)"
                }
            },
            "required": ["name"]
        }
    })
}

pub fn drive_move_tool_schema() -> Value {
    json!({
        "name": "gws_drive_move",
        "title": "Move File",
        "description": "Move a file or folder to a different Drive folder.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "File or folder to move"
                },
                "to_folder_id": {
                    "type": "string",
                    "description": "Destination folder ID"
                }
            },
            "required": ["file_id", "to_folder_id"]
        }
    })
}

pub fn drive_share_tool_schema() -> Value {
    json!({
        "name": "gws_drive_share",
        "title": "Share File",
        "description": "Share a file or folder with a user, group, or domain.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "File or folder to share"
                },
                "email": {
                    "type": "string",
                    "description": "Email of user or group to share with"
                },
                "domain": {
                    "type": "string",
                    "description": "Domain to share with (e.g. company.com)"
                },
                "role": {
                    "type": "string",
                    "enum": ["reader", "commenter", "writer"],
                    "default": "reader"
                }
            },
            "required": ["file_id"]
        }
    })
}

pub fn drive_find_folder_tool_schema() -> Value {
    json!({
        "name": "gws_drive_find_folder",
        "title": "Find Folder",
        "description": "Find a Drive folder by name and return its ID.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Folder name to search for (exact match)"
                },
                "parent_id": {
                    "type": "string",
                    "description": "Search within this parent folder only"
                }
            },
            "required": ["name"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "parents": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["id", "name"]
                    }
                }
            },
            "required": ["files"]
        }
    })
}

pub fn drive_delete_tool_schema() -> Value {
    json!({
        "name": "gws_drive_trash",
        "title": "Trash File",
        "description": "Move a file or folder to the trash (recoverable).",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "File or folder to delete"
                }
            },
            "required": ["file_id"]
        }
    })
}

pub fn drive_copy_tool_schema() -> Value {
    json!({
        "name": "gws_drive_copy",
        "title": "Copy File",
        "description": "Copy a file in Drive, optionally to a different folder.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "File to copy"
                },
                "name": {
                    "type": "string",
                    "description": "Name for the copy (default: 'Copy of <original>')"
                },
                "folder_id": {
                    "type": "string",
                    "description": "Destination folder for the copy"
                }
            },
            "required": ["file_id"]
        }
    })
}

pub fn drive_rename_tool_schema() -> Value {
    json!({
        "name": "gws_drive_rename",
        "title": "Rename File",
        "description": "Rename a file or folder in Drive.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "File or folder to rename"
                },
                "name": {
                    "type": "string",
                    "description": "New name"
                }
            },
            "required": ["file_id", "name"]
        }
    })
}

fn mime_type_for_type_filter(t: &str) -> Option<&'static str> {
    match t {
        "folder" => Some("application/vnd.google-apps.folder"),
        "document" | "doc" => Some("application/vnd.google-apps.document"),
        "spreadsheet" | "sheet" => Some("application/vnd.google-apps.spreadsheet"),
        "presentation" | "slides" => Some("application/vnd.google-apps.presentation"),
        "pdf" => Some("application/pdf"),
        "image" => None, // handled separately with contains
        _ => None,
    }
}

pub fn build_drive_query(
    folder_id: Option<&str>,
    query: Option<&str>,
    file_type: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    parts.push("trashed = false".to_string());

    if let Some(fid) = folder_id {
        parts.push(format!("'{}' in parents", fid));
    }

    if let Some(q) = query {
        parts.push(format!("name contains '{}'", q.replace('\'', "\\'")));
    }

    if let Some(t) = file_type {
        if t == "image" {
            parts.push("mimeType contains 'image/'".to_string());
        } else if let Some(mime) = mime_type_for_type_filter(t) {
            parts.push(format!("mimeType = '{}'", mime));
        }
    }

    parts.join(" and ")
}

fn extract_file_id<'a>(arguments: &'a Value, param: &str) -> Result<&'a str, GwsError> {
    let id = arguments
        .get(param)
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation(format!("Missing '{param}'")))?;
    validate_file_id(id).map_err(GwsError::Validation)?;
    Ok(id)
}

pub(crate) async fn execute_drive_helper(
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
            let q = build_drive_query(folder_id, query, file_type);
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
            let effective_policy =
                crate::server::policy_for_folder(parent_id, policy, meta, state).await?;
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
            let effective_policy =
                crate::server::policy_for_folder(Some(to_folder), policy, meta, state).await?;
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
            let effective_policy =
                crate::server::policy_for_folder(folder_id, policy, meta, state).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_schemas_have_short_descriptions() {
        let schemas = vec![
            drive_list_tool_schema(),
            drive_info_tool_schema(),
            drive_create_folder_tool_schema(),
            drive_move_tool_schema(),
            drive_share_tool_schema(),
        ];
        for schema in &schemas {
            let name = schema["name"].as_str().unwrap();
            let desc = schema["description"].as_str().unwrap();
            assert!(
                desc.len() < 100,
                "Tool {name} description too long ({} chars)",
                desc.len()
            );
        }
        assert_eq!(schemas.len(), 5);
    }

    #[test]
    fn test_build_query_folder_only() {
        let q = build_drive_query(Some("abc123"), None, None);
        assert!(q.contains("'abc123' in parents"));
        assert!(q.contains("trashed = false"));
    }

    #[test]
    fn test_build_query_name_search() {
        let q = build_drive_query(None, Some("report"), None);
        assert!(q.contains("name contains 'report'"));
    }

    #[test]
    fn test_build_query_type_filter() {
        let q = build_drive_query(None, None, Some("document"));
        assert!(q.contains("mimeType = 'application/vnd.google-apps.document'"));
    }

    #[test]
    fn test_build_query_image_filter() {
        let q = build_drive_query(None, None, Some("image"));
        assert!(q.contains("mimeType contains 'image/'"));
    }

    #[test]
    fn test_build_query_combined() {
        let q = build_drive_query(Some("folder1"), Some("test"), Some("pdf"));
        assert!(q.contains("'folder1' in parents"));
        assert!(q.contains("name contains 'test'"));
        assert!(q.contains("mimeType = 'application/pdf'"));
    }

    #[test]
    fn test_output_schema_on_drive_list_tool() {
        let schema = drive_list_tool_schema();
        let os = &schema["outputSchema"];
        assert_eq!(os["type"], "object");
        assert!(os["properties"]["files"].is_object());
        let items = &os["properties"]["files"]["items"]["properties"];
        assert!(items["id"].is_object());
        assert!(items["name"].is_object());
        assert!(items["mimeType"].is_object());
    }

    #[test]
    fn test_output_schema_on_drive_find_folder_tool() {
        let schema = drive_find_folder_tool_schema();
        let os = &schema["outputSchema"];
        assert_eq!(os["type"], "object");
        assert!(os["properties"]["files"].is_object());
        let items = &os["properties"]["files"]["items"]["properties"];
        assert!(items["id"].is_object());
        assert!(items["name"].is_object());
    }
}
