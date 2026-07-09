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
}
