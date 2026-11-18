use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::shared::ServerState;
use crate::tools;
use google_workspace::error::GwsError;
use serde_json::{Value, json};

/// The Drive API's alias for the user's My Drive root folder.
const ROOT_ALIAS: &str = "root";

/// The documented alphabet of Google Drive (and Sheets) IDs.
///
/// IDs are opaque, but in practice they are URL-safe base64: ASCII letters,
/// digits, `-` and `_`.
///
/// Drive IDs are interpolated verbatim into the Drive `q` search grammar
/// (`'<id>' in parents`), which has no escape sequence for an ID position.
/// Restricting the charset up front — rather than trying to escape — is the
/// injection control for those fields.
fn is_valid_id_charset(id: &str) -> bool {
    id.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn validate_google_id(id: &str, label: &str, typical_len: &str) -> Result<(), String> {
    if id.len() < 20 {
        return Err(format!(
            "Invalid {label} '{id}' (too short — {typical_len}). \
             Check the ID from the previous tool call response."
        ));
    }
    if !is_valid_id_charset(id) {
        return Err(format!(
            "Invalid {label} '{id}' (only ASCII letters, digits, '-' and '_' are allowed — \
             {typical_len}). IDs are pasted into Drive query strings, so a value that needs \
             escaping is rejected instead. Check the ID from the previous tool call response."
        ));
    }
    Ok(())
}

/// Validate a Drive file ID before it reaches a request path or query.
///
/// Enforces the length heuristic *and* the ID charset: the ID is interpolated
/// into the Drive query language, so charset validation is the control here,
/// not escaping. The literal `root` alias the API accepts is allowed too.
pub fn validate_file_id(id: &str) -> Result<(), String> {
    if id == ROOT_ALIAS {
        return Ok(());
    }
    validate_google_id(
        id,
        "file ID",
        "Google Drive IDs are typically 33-44 characters",
    )
}

/// Validate a Drive folder ID, which is subject to the same interpolation risk
/// as a file ID (`'<folder id>' in parents`) and accepts the same `root` alias.
pub fn validate_folder_id(id: &str) -> Result<(), String> {
    if id == ROOT_ALIAS {
        return Ok(());
    }
    validate_google_id(
        id,
        "folder ID",
        "Google Drive folder IDs are typically 33-44 characters",
    )
}

/// Escape a free-text value for a single-quoted literal in the Drive `q`
/// grammar.
///
/// Google documents escaping the backslash first and the quote second; doing it
/// in the other order would leave a doubled backslash in front of the quote.
/// Control characters have no representation in the grammar, so they are
/// dropped instead of passed through.
fn escape_query_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            c if c.is_control() => {}
            c => escaped.push(c),
        }
    }
    escaped
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
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true },
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

/// Build the Drive `q` filter used by the list helpers.
///
/// Every ID is validated against the ID charset before interpolation: the `q`
/// grammar has no escape sequence for an ID position, so validation is the only
/// injection control there. Free-text values are escaped with
/// [`escape_query_literal`].
pub fn build_drive_query(
    folder_id: Option<&str>,
    query: Option<&str>,
    file_type: Option<&str>,
) -> Result<String, GwsError> {
    let mut parts = Vec::new();
    parts.push("trashed = false".to_string());

    if let Some(fid) = folder_id {
        validate_folder_id(fid).map_err(GwsError::Validation)?;
        parts.push(format!("'{fid}' in parents"));
    }

    if let Some(q) = query {
        parts.push(format!("name contains '{}'", escape_query_literal(q)));
    }

    if let Some(t) = file_type {
        if t == "image" {
            parts.push("mimeType contains 'image/'".to_string());
        } else if let Some(mime) = mime_type_for_type_filter(t) {
            parts.push(format!("mimeType = '{}'", mime));
        }
    }

    Ok(parts.join(" and "))
}

fn extract_file_id<'a>(arguments: &'a Value, param: &str) -> Result<&'a str, GwsError> {
    let id = arguments
        .get(param)
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation(format!("Missing '{param}'")))?;
    validate_file_id(id).map_err(GwsError::Validation)?;
    Ok(id)
}

/// Read an optional ID argument and validate it when present.
///
/// Absent and non-string values stay "not provided"; an ID that is present but
/// fails validation is an error rather than a silently ignored filter.
fn extract_optional_folder_id<'a>(
    arguments: &'a Value,
    param: &str,
) -> Result<Option<&'a str>, GwsError> {
    match arguments.get(param).and_then(|v| v.as_str()) {
        Some(id) => {
            validate_folder_id(id).map_err(GwsError::Validation)?;
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

/// Roles `gws_drive_share` may grant.
///
/// `owner` and `organizer` are deliberately excluded: this helper shares
/// content, it never transfers ownership.
const SHARE_ROLES: [&str; 3] = ["reader", "commenter", "writer"];

/// Reject any sharing role outside [`SHARE_ROLES`] before the API call, so a
/// typo produces a validation error instead of an opaque 400.
fn validate_share_role(role: &str) -> Result<(), GwsError> {
    if SHARE_ROLES.contains(&role) {
        return Ok(());
    }
    Err(GwsError::Validation(format!(
        "Invalid sharing role '{role}'. Allowed roles: {}.",
        SHARE_ROLES.join(", ")
    )))
}

/// Refuse a trash when the operator denied any member of the Drive delete
/// family.
///
/// `gws_drive_trash` is implemented as `files.update {trashed: true}`, and
/// [`crate::policy::method_aliases`] only relates `files.delete` with
/// `files.emptyTrash`, so `denied_methods: ["files.delete"]` would otherwise
/// let trashing through. Aliasing `files.update` to delete globally is not an
/// option — that would block every ordinary update — so the helper checks the
/// family explicitly.
///
/// The refusal is built with [`crate::audit::policy_denial_verbatim`]: it is a
/// policy decision (so it must be audited `action:"denied"` with status 403 and
/// must never be reported as an `error`), but the remediation is only useful to
/// the caller, so the message reaches the client instead of the generic
/// "Operation not allowed by policy" replacement.
fn check_trash_allowed(policy: &Policy) -> Result<(), GwsError> {
    for method in ["trash", "delete", "emptyTrash"] {
        if policy.is_method_denied("drive", "files", method) {
            return Err(crate::audit::policy_denial_verbatim(format!(
                "Trash blocked: gws_drive_trash moves the file to the trash, \
                 which is the same destructive action as 'files.delete' \
                 (matched 'files.{method}' in the drive denylist). \
                 Fix: remove \"files.delete\" (and \"files.trash\") from \
                 \"denied_methods\" in the \"drive\" service to allow trashing."
            )));
        }
    }
    Ok(())
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
            let folder_id = extract_optional_folder_id(arguments, "folder_id")?;
            let query = arguments.get("query").and_then(|v| v.as_str());
            let file_type = arguments.get("type").and_then(|v| v.as_str());
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(20);
            let q = build_drive_query(folder_id, query, file_type)?;
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
            let parent_id = extract_optional_folder_id(arguments, "parent_id")?;
            let list_method = files_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("list method not found".into()))?;
            // `name` is free text, so it is escaped (backslash first) rather
            // than validated; `parent_id` went through the ID charset check
            // above because it is interpolated into the query grammar.
            let mut q = format!(
                "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
                escape_query_literal(name)
            );
            if let Some(pid) = parent_id {
                q.push_str(&format!(" and '{pid}' in parents"));
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
            let parent_id = extract_optional_folder_id(arguments, "parent_id")?;
            let (effective_policy, resolved_folder) =
                crate::shared::policy_for_folder(parent_id, policy, meta, state).await?;
            let parent_id = resolved_folder.as_deref().or(parent_id);
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
            let to_folder = extract_optional_folder_id(arguments, "to_folder_id")?
                .ok_or_else(|| GwsError::Validation("Missing 'to_folder_id'".into()))?;
            let (effective_policy, _) =
                crate::shared::policy_for_folder(Some(to_folder), policy, meta, state).await?;
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
            validate_share_role(role)?;
            // `type` is derived from the target below, never taken from the
            // caller: publishing to `anyone` is intentionally unreachable here.
            //
            // A `domain` grant is still file-scoped: `permissions.create` is
            // covered by `execute::write_scope_id_param`, so when the policy has
            // recursive `parents` roots the ancestry check runs centrally before
            // the request leaves the process. No second walk is needed here.
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
            // Trashing is `files.update {trashed: true}`, so a denial written
            // for the delete family never matches this method on its own.
            check_trash_allowed(policy)?;
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
            let folder_id = extract_optional_folder_id(arguments, "folder_id")?;
            let (effective_policy, resolved_folder) =
                crate::shared::policy_for_folder(folder_id, policy, meta, state).await?;
            let folder_id = resolved_folder.as_deref().or(folder_id);
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
    use google_workspace::discovery::{RestDescription, RestMethod, RestResource};
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Realistic Drive IDs: inside the documented 33-44 char range.
    const FILE_ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz0123456";
    const FOLDER_ID: &str = "1ZzYyXxWwVvUuTtSsRrQqPpOoNnMmLlKk";

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
    fn test_trash_is_annotated_destructive() {
        let schema = drive_delete_tool_schema();
        let annotations = &schema["annotations"];
        assert_eq!(annotations["destructiveHint"].as_bool(), Some(true));
    }

    #[test]
    fn test_build_query_folder_only() {
        let q = build_drive_query(Some(FOLDER_ID), None, None).unwrap();
        assert!(q.contains(&format!("'{FOLDER_ID}' in parents")));
        assert!(q.contains("trashed = false"));
    }

    #[test]
    fn test_build_query_name_search() {
        let q = build_drive_query(None, Some("report"), None).unwrap();
        assert!(q.contains("name contains 'report'"));
    }

    #[test]
    fn test_build_query_type_filter() {
        let q = build_drive_query(None, None, Some("document")).unwrap();
        assert!(q.contains("mimeType = 'application/vnd.google-apps.document'"));
    }

    #[test]
    fn test_build_query_image_filter() {
        let q = build_drive_query(None, None, Some("image")).unwrap();
        assert!(q.contains("mimeType contains 'image/'"));
    }

    #[test]
    fn test_build_query_combined() {
        let q = build_drive_query(Some(FOLDER_ID), Some("test"), Some("pdf")).unwrap();
        assert!(q.contains(&format!("'{FOLDER_ID}' in parents")));
        assert!(q.contains("name contains 'test'"));
        assert!(q.contains("mimeType = 'application/pdf'"));
    }

    #[test]
    fn test_build_query_rejects_id_injection() {
        // 42 chars: passes the old length-only heuristic, breaks out of the
        // `'<id>' in parents` literal and appends its own predicate.
        let payload = "x' in parents) or (trashed = false) or ('y";
        assert_eq!(payload.len(), 42);
        let err = build_drive_query(Some(payload), None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("only ASCII letters"), "{msg}");
        assert!(
            msg.contains("file ID") || msg.contains("folder ID"),
            "{msg}"
        );
    }

    #[test]
    fn test_build_query_escapes_free_text_quote_and_backslash() {
        let q = build_drive_query(None, Some("re'p"), None).unwrap();
        assert!(q.contains(r"name contains 're\'p'"), "{q}");

        // Backslash escaped first, then the quote: a doubled backslash must
        // not swallow the quote escape.
        let q = build_drive_query(None, Some(r"a\b\'c"), None).unwrap();
        assert!(q.contains(r"name contains 'a\\b\\\'c'"), "{q}");
    }

    #[test]
    fn test_build_query_strips_control_characters() {
        let q = build_drive_query(None, Some("re\nport\t"), None).unwrap();
        assert!(q.contains("name contains 'report'"), "{q}");
        assert!(!q.contains('\n'));
    }

    #[test]
    fn test_validate_file_id_charset() {
        // Normal IDs are accepted.
        assert!(validate_file_id(FILE_ID).is_ok());
        assert!(validate_folder_id(FOLDER_ID).is_ok());
        // The API's root alias is accepted.
        assert!(validate_file_id("root").is_ok());
        assert!(validate_folder_id("root").is_ok());
        // Breakout payloads and other non-ID characters are rejected even at a
        // plausible length.
        assert!(validate_file_id("x' in parents) or (trashed = false) or ('y").is_err());
        assert!(validate_file_id("1AbCdEfGhIjKlMnOpQrStUvWxYz01234/5").is_err());
        assert!(validate_file_id("1AbCdEfGhIjKlMnOpQrStUvWxYz01234 5").is_err());
        // Short values still fail for the original reason.
        assert!(validate_file_id("short").is_err());
    }

    #[test]
    fn test_validate_share_role() {
        for role in ["reader", "commenter", "writer"] {
            assert!(validate_share_role(role).is_ok(), "{role}");
        }
        for role in ["owner", "organizer", "Reader", ""] {
            let err = validate_share_role(role).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("Allowed roles: reader, commenter, writer"),
                "{msg}"
            );
        }
    }

    /// Minimal drive discovery doc: just enough for the trash helper to resolve
    /// `files.update` without fetching anything over the network.
    fn drive_doc_with_files_update() -> RestDescription {
        let update = RestMethod {
            http_method: "PATCH".to_string(),
            path: "files/{fileId}".to_string(),
            ..Default::default()
        };
        let mut methods = HashMap::new();
        methods.insert("update".to_string(), update);
        let mut resources = HashMap::new();
        resources.insert(
            "files".to_string(),
            RestResource {
                methods,
                resources: HashMap::new(),
            },
        );
        RestDescription {
            name: "drive".to_string(),
            version: "v3".to_string(),
            root_url: "https://www.googleapis.com/".to_string(),
            service_path: "drive/v3/".to_string(),
            resources,
            ..Default::default()
        }
    }

    fn policy_with_drive_denials(denied: &[&str]) -> Policy {
        Policy::from_policy_file(crate::policy::PolicyFile {
            services: vec![crate::policy::ServicePolicy {
                name: "drive".to_string(),
                denied_methods: denied.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    /// `gws_drive_trash` is `files.update {trashed: true}`; a `files.delete`
    /// denial must still stop it, before any request is issued.
    #[tokio::test]
    async fn test_trash_denied_by_files_delete_before_any_request() {
        let mut state = ServerState::new();
        let doc = Arc::new(drive_doc_with_files_update());
        state.docs.insert("drive".to_string(), doc);
        let policy = policy_with_drive_denials(&["files.delete"]);
        let meta = RequestMeta::default();
        let err = execute_drive_helper(
            "gws_drive_trash",
            &json!({"file_id": FILE_ID}),
            &policy,
            &meta,
            &mut state,
        )
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("Trash blocked"), "{msg}");
        assert!(msg.contains("files.delete"), "{msg}");
        assert!(msg.contains("denied_methods"), "{msg}");
        // It is a policy decision: audited `denied`/403, not `error`/500.
        assert!(
            crate::audit::classify_denial(&msg).is_denial(),
            "trash refusal must classify as a policy denial: {msg}"
        );
    }

    /// A `files.trash` (or `emptyTrash`) denial is the same family.
    #[test]
    fn test_trash_check_covers_whole_delete_family() {
        for denied in ["files.trash", "files.delete", "files.emptyTrash", "trash"] {
            let policy = policy_with_drive_denials(&[denied]);
            assert!(
                check_trash_allowed(&policy).is_err(),
                "{denied} should block trashing"
            );
        }
        // An update denial is not a delete denial: the gate must not turn
        // ordinary renames/moves into blocked operations.
        assert!(check_trash_allowed(&policy_with_drive_denials(&["files.update"])).is_ok());
        assert!(check_trash_allowed(&policy_with_drive_denials(&[])).is_ok());
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
