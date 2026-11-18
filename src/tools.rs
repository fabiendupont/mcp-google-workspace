use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Value, json};

use google_workspace::discovery::{RestDescription, RestResource};
use google_workspace::error::GwsError;
use google_workspace::services;

use crate::policy::Policy;

/// Tools that are not bound to a single Google service and are always visible.
const META_TOOLS: &[&str] = &["gws_discover", "gws_batch"];

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
        "tasks" => Some(
            "helper tools: gws_tasks_list, gws_tasks_create, gws_tasks_complete (tasklist_id optional, defaults to first list)",
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

/// Every meta and helper tool this server can expose, independent of which
/// services the policy allows.
///
/// Kept separate from [`build_tools_list`] so the approval registry
/// (`tool_full_schema`) can be checked against the full helper surface
/// without loading discovery documents or hitting the network.
fn helper_tools(policy: &Policy) -> Vec<Tool> {
    let mut tools = Vec::new();

    tools.push(make_tool(
        "gws_discover",
        "API & Tool Discovery",
        "Explore APIs and activate services. Use service= to enable tools, tool= for docs.",
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
        "Run multiple API calls in one request. Max 100 per batch.",
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
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_trace_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_explain_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_format_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_validate_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_named_range_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_csv_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_dimensions_tool_schema(),
        ));
        tools.push(tool_from_json(
            crate::sheets_helpers::sheets_formulas_tool_schema(),
        ));
    }

    // Slides & image tools
    tools.push(tool_from_json(crate::slides_helpers::marp_tool_schema()));
    tools.push(tool_from_json(
        crate::slides_helpers::templates_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::slides_helpers::slides_read_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::slides_helpers::slides_add_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::slides_helpers::slides_delete_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::slides_helpers::slides_reorder_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::slides_helpers::slides_duplicate_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::slides_helpers::slides_update_tool_schema(),
    ));
    tools.push(tool_from_json(crate::image_gen::image_gen_tool_schema()));

    // Gmail helper tools
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_search_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_read_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_draft_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_send_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_reply_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_thread_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_attachment_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_contacts_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_forward_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::gmail_helpers::gmail_labels_tool_schema(),
    ));

    // Calendar helper tools
    tools.push(tool_from_json(
        crate::calendar_helpers::calendar_list_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::calendar_helpers::calendar_get_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::calendar_helpers::calendar_create_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::calendar_helpers::calendar_update_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::calendar_helpers::calendar_delete_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::calendar_helpers::calendar_freebusy_tool_schema(),
    ));

    // Tasks helper tools
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_lists_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_list_tool_schema(),
    ));
    tools.push(tool_from_json(crate::tasks_helpers::tasks_get_tool_schema()));
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_create_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_update_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_complete_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_delete_tool_schema(),
    ));
    tools.push(tool_from_json(
        crate::tasks_helpers::tasks_clear_tool_schema(),
    ));
    tools
}

pub async fn build_tools_list(
    policy: &Policy,
    docs: &mut HashMap<String, Arc<RestDescription>>,
    activated: Option<&std::collections::HashSet<String>>,
    eager: bool,
) -> Result<Vec<Tool>, GwsError> {
    let mut tools = Vec::new();

    const SERVICES_WITH_HELPERS: &[&str] = &[
        "drive", "docs", "sheets", "slides", "gmail", "calendar", "tasks",
    ];

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
        collect_resource_paths(
            &doc.resources,
            "",
            policy.allowed_resources(svc_name),
            &mut resource_names,
        );
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

        if policy.compact_schemas
            && let Some(hints) = method_hints(svc_name)
        {
            description.push_str(&format!(". Common: {hints}"));
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

    tools.extend(helper_tools(policy));

    Ok(filter_tools(policy, tools, activated, eager))
}

/// Map a helper tool name to the service that owns it.
///
/// Every helper advertised by [`helper_tools`] must resolve here, otherwise it
/// would bypass the policy filter in [`filter_tools`]; the
/// `test_every_helper_maps_to_a_service` guard enforces that.
fn tool_service(name: &str) -> Option<&'static str> {
    if name.starts_with("gws_drive_") {
        Some("drive")
    } else if name.starts_with("gws_docs_") {
        Some("docs")
    } else if name.starts_with("gws_sheets_") {
        Some("sheets")
    } else if name.starts_with("gws_slides_")
        || name == "gws_templates"
        || name == "gws_generate_image"
    {
        Some("slides")
    } else if name.starts_with("gws_gmail_") {
        Some("gmail")
    } else if name.starts_with("gws_calendar_") {
        Some("calendar")
    } else if name.starts_with("gws_tasks_") {
        Some("tasks")
    } else {
        None
    }
}

/// Apply the policy allowlist to a tool list.
///
/// The service allowlist is authoritative in *both* modes: a tool whose
/// service is not configured is never advertised, matching
/// `policy-reference.md` ("Only services listed here are exposed as MCP
/// tools"). Lazy mode additionally narrows helpers down to the services the
/// client activated via `gws_discover`, so eager mode is a superset of lazy
/// mode for the same policy — never a way to exceed it.
fn filter_tools(
    policy: &Policy,
    tools: Vec<Tool>,
    activated: Option<&std::collections::HashSet<String>>,
    eager: bool,
) -> Vec<Tool> {
    let allowed: std::collections::HashSet<&str> = policy.allowed_services().into_iter().collect();

    tools
        .into_iter()
        .filter(|t| {
            let name: &str = &t.name;
            if META_TOOLS.contains(&name) {
                return true;
            }
            match tool_service(name) {
                Some(svc) => {
                    // Helpers must satisfy both the policy allowlist (always)
                    // and, in lazy mode, client activation.
                    allowed.contains(svc) && (eager || activated.is_some_and(|a| a.contains(svc)))
                }
                // Generic per-service tools (tool name == service alias) are
                // only ever built from `policy.allowed_services()`.
                None => true,
            }
        })
        .collect()
}

pub(crate) fn tool_full_schema(name: &str) -> Option<Value> {
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
        "gws_sheets_trace" => crate::sheets_helpers::sheets_trace_tool_schema(),
        "gws_sheets_explain" => crate::sheets_helpers::sheets_explain_tool_schema(),
        "gws_sheets_format" => crate::sheets_helpers::sheets_format_tool_schema(),
        "gws_sheets_validate" => crate::sheets_helpers::sheets_validate_tool_schema(),
        "gws_sheets_named_range" => crate::sheets_helpers::sheets_named_range_tool_schema(),
        "gws_sheets_csv" => crate::sheets_helpers::sheets_csv_tool_schema(),
        "gws_sheets_dimensions" => crate::sheets_helpers::sheets_dimensions_tool_schema(),
        "gws_sheets_formulas" => crate::sheets_helpers::sheets_formulas_tool_schema(),
        "gws_slides_import_marp" => crate::slides_helpers::marp_tool_schema(),
        "gws_templates" => crate::slides_helpers::templates_tool_schema(),
        "gws_slides_read" => crate::slides_helpers::slides_read_tool_schema(),
        "gws_slides_add" => crate::slides_helpers::slides_add_tool_schema(),
        "gws_slides_delete" => crate::slides_helpers::slides_delete_tool_schema(),
        "gws_slides_reorder" => crate::slides_helpers::slides_reorder_tool_schema(),
        "gws_slides_duplicate" => crate::slides_helpers::slides_duplicate_tool_schema(),
        "gws_slides_update" => crate::slides_helpers::slides_update_tool_schema(),
        "gws_generate_image" => crate::image_gen::image_gen_tool_schema(),
        "gws_gmail_search" => crate::gmail_helpers::gmail_search_tool_schema(),
        "gws_gmail_read" => crate::gmail_helpers::gmail_read_tool_schema(),
        "gws_gmail_draft" => crate::gmail_helpers::gmail_draft_tool_schema(),
        "gws_gmail_send" => crate::gmail_helpers::gmail_send_tool_schema(),
        "gws_gmail_reply" => crate::gmail_helpers::gmail_reply_tool_schema(),
        "gws_gmail_thread" => crate::gmail_helpers::gmail_thread_tool_schema(),
        "gws_gmail_attachment" => crate::gmail_helpers::gmail_attachment_tool_schema(),
        "gws_gmail_contacts" => crate::gmail_helpers::gmail_contacts_tool_schema(),
        "gws_gmail_forward" => crate::gmail_helpers::gmail_forward_tool_schema(),
        "gws_gmail_labels" => crate::gmail_helpers::gmail_labels_tool_schema(),
        "gws_calendar_list" => crate::calendar_helpers::calendar_list_tool_schema(),
        "gws_calendar_get" => crate::calendar_helpers::calendar_get_tool_schema(),
        "gws_calendar_create" => crate::calendar_helpers::calendar_create_tool_schema(),
        "gws_calendar_update" => crate::calendar_helpers::calendar_update_tool_schema(),
        "gws_calendar_delete" => crate::calendar_helpers::calendar_delete_tool_schema(),
        "gws_calendar_freebusy" => crate::calendar_helpers::calendar_freebusy_tool_schema(),
        "gws_tasks_lists" => crate::tasks_helpers::tasks_lists_tool_schema(),
        "gws_tasks_list" => crate::tasks_helpers::tasks_list_tool_schema(),
        "gws_tasks_get" => crate::tasks_helpers::tasks_get_tool_schema(),
        "gws_tasks_create" => crate::tasks_helpers::tasks_create_tool_schema(),
        "gws_tasks_update" => crate::tasks_helpers::tasks_update_tool_schema(),
        "gws_tasks_complete" => crate::tasks_helpers::tasks_complete_tool_schema(),
        "gws_tasks_delete" => crate::tasks_helpers::tasks_delete_tool_schema(),
        "gws_tasks_clear" => crate::tasks_helpers::tasks_clear_tool_schema(),
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
            "For Drive images, use drive_file_id: the file is temporarily shared as \
            anyone-with-link for the Docs fetcher and the permission is revoked after \
            insertion. For generated images, call gws_generate_image first.",
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

    let allowed = policy.allowed_resources(service);

    let result = match (resource_name, method_name) {
        (None, _) => {
            let mut entries = Vec::new();
            collect_resource_entries(&doc.resources, "", allowed, &mut entries);
            json!({ "service": service, "resources": entries })
        }
        (Some(res), None) => {
            crate::execute::ensure_resource_allowed(service, res, policy)?;
            let resource = find_resource(&doc.resources, res).ok_or_else(|| {
                let mut all = Vec::new();
                collect_resource_paths(&doc.resources, "", allowed, &mut all);
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
            let sub: Vec<&str> = resource
                .resources
                .keys()
                .filter(|name| {
                    crate::resources::is_resource_allowed(allowed, &format!("{res}.{name}"))
                })
                .map(String::as_str)
                .collect();
            let mut r = json!({ "service": service, "resource": res, "methods": methods });
            if !sub.is_empty() {
                r["subResources"] = json!(sub);
            }
            r
        }
        (Some(res), Some(meth)) => {
            crate::execute::ensure_resource_allowed(service, res, policy)?;
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

/// Collect dotted resource paths, skipping any excluded by `allowed_resources`.
///
/// Nested resources are still visited when their parent is filtered out so that
/// an explicitly allowed deep path (for example `spreadsheets.values`) remains
/// discoverable. An empty allowlist advertises everything.
fn collect_resource_paths(
    resources: &HashMap<String, RestResource>,
    prefix: &str,
    allowed: &[String],
    out: &mut Vec<String>,
) {
    for (name, res) in resources {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if crate::resources::is_resource_allowed(allowed, &path) {
            out.push(path.clone());
        }
        if !res.resources.is_empty() {
            collect_resource_paths(&res.resources, &path, allowed, out);
        }
    }
}

fn collect_resource_entries(
    resources: &HashMap<String, RestResource>,
    prefix: &str,
    allowed: &[String],
    out: &mut Vec<Value>,
) {
    for (name, res) in resources {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let methods: Vec<&str> = res.methods.keys().map(|s| s.as_str()).collect();
        if !methods.is_empty() && crate::resources::is_resource_allowed(allowed, &path) {
            out.push(json!({ "name": path.clone(), "methods": methods }));
        }
        if !res.resources.is_empty() {
            collect_resource_entries(&res.resources, &path, allowed, out);
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
        collect_resource_paths(&resources, "", &[], &mut paths);
        paths.sort();
        assert!(paths.contains(&"files".to_string()));
        assert!(paths.contains(&"files.revisions".to_string()));
    }

    #[test]
    fn test_collect_resource_entries_has_methods() {
        let resources = make_resources();
        let mut entries = Vec::new();
        collect_resource_entries(&resources, "", &[], &mut entries);
        assert!(!entries.is_empty());
        let files_entry = entries.iter().find(|e| e["name"] == "files").unwrap();
        let methods = files_entry["methods"].as_array().unwrap();
        assert!(methods.iter().any(|m| m == "get"));
    }

    #[test]
    fn test_collect_resource_paths_filters_by_allowed_resources() {
        let resources = make_resources();
        // Deeply allowed resources stay discoverable even when their parent is
        // excluded from the advertised surface.
        let allowed = vec!["files.revisions".to_string()];

        let mut paths = Vec::new();
        collect_resource_paths(&resources, "", &allowed, &mut paths);
        assert_eq!(paths, vec!["files.revisions".to_string()]);

        let mut entries = Vec::new();
        collect_resource_entries(&resources, "", &allowed, &mut entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "files.revisions");

        // An empty allowlist advertises the full surface.
        let mut all = Vec::new();
        collect_resource_entries(&resources, "", &[], &mut all);
        assert!(all.iter().any(|e| e["name"] == "files"));
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

    fn all_helper_services_policy() -> Policy {
        Policy::from_services(
            &[
                "drive", "docs", "sheets", "slides", "gmail", "calendar", "tasks",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        )
    }

    fn tool_names(tools: &[Tool]) -> Vec<String> {
        tools.iter().map(|t| t.name.to_string()).collect()
    }

    /// `tool_full_schema` is the ONLY schema source for the `confirm_writes`
    /// approval path (`server::helper_write_summary` -> `tool_full_schema`), so
    /// a mutating helper missing from it silently skips user approval. This is
    /// the regression guard for `gws_generate_image`, which reached an
    /// outbound Gemini call without eliciting.
    #[test]
    fn test_every_mutating_helper_has_approval_schema() {
        let policy = all_helper_services_policy();
        let tools = helper_tools(&policy);
        assert!(!tools.is_empty(), "helper registry must not be empty");

        for tool in &tools {
            let name: &str = &tool.name;
            // `gws_batch` is not approvable: with `confirm_writes` enabled,
            // server.rs rejects it outright instead of eliciting per
            // sub-request.
            if name == "gws_batch" {
                continue;
            }
            let read_only = tool
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                .unwrap_or(false);
            if read_only {
                continue;
            }
            let info = tool_full_schema(name).unwrap_or_else(|| {
                panic!("mutating tool '{name}' has no schema in tool_full_schema")
            });
            assert_eq!(
                info["annotations"]["readOnlyHint"],
                json!(false),
                "approval schema for '{name}' must keep readOnlyHint=false"
            );
            assert!(
                info["inputSchema"].is_object(),
                "approval schema for '{name}' must expose an inputSchema"
            );
        }
    }

    /// Every helper must map to a service, otherwise `filter_tools` cannot
    /// deny it under a restrictive policy (a `None` mapping always passes).
    #[test]
    fn test_every_helper_maps_to_a_service() {
        let policy = all_helper_services_policy();
        for tool in helper_tools(&policy) {
            let name: &str = &tool.name;
            if META_TOOLS.contains(&name) {
                continue;
            }
            assert!(
                tool_service(name).is_some(),
                "helper '{name}' does not map to a service; add it to tool_service"
            );
        }
    }

    /// Eager mode must not advertise tools for services the policy forbids:
    /// `--services drive --eager-tools` previously exposed every helper.
    #[test]
    fn test_eager_tools_respect_policy_services() {
        let policy = Policy::from_services(&["drive".to_string()]);
        let eager_tools = filter_tools(
            &policy,
            helper_tools(&policy),
            Some(&std::collections::HashSet::new()),
            true,
        );
        let names = tool_names(&eager_tools);

        for name in &names {
            assert!(
                META_TOOLS.contains(&name.as_str()) || name.starts_with("gws_drive_"),
                "'{name}' advertised outside a drive-only policy: {names:?}"
            );
        }
        for forbidden in [
            "gws_gmail_send",
            "gws_sheets_write",
            "gws_calendar_create",
            "gws_tasks_create",
            "gws_slides_add",
            "gws_generate_image",
        ] {
            assert!(
                !names.iter().any(|n| n == forbidden),
                "'{forbidden}' leaked past the drive-only policy"
            );
        }
        assert!(
            names.iter().any(|n| n == "gws_drive_list"),
            "drive helpers must stay visible for a drive-only policy"
        );
    }

    /// Lazy mode keeps the policy gate and additionally requires activation.
    #[test]
    fn test_lazy_tools_respect_policy_and_activation() {
        let policy = Policy::from_services(&["drive".to_string()]);

        // Nothing activated: only meta tools.
        let idle = filter_tools(
            &policy,
            helper_tools(&policy),
            Some(&std::collections::HashSet::new()),
            false,
        );
        assert_eq!(tool_names(&idle), vec!["gws_discover", "gws_batch"]);

        // Activating a service the policy denies must not expose it.
        let denied: std::collections::HashSet<String> = ["gmail".to_string()].into_iter().collect();
        let gmail = filter_tools(&policy, helper_tools(&policy), Some(&denied), false);
        assert!(
            !tool_names(&gmail)
                .iter()
                .any(|n| n.starts_with("gws_gmail_")),
            "activation must not override the policy allowlist"
        );

        // Activating an allowed service exposes its helpers.
        let activated: std::collections::HashSet<String> =
            ["drive".to_string()].into_iter().collect();
        let drive = tool_names(&filter_tools(
            &policy,
            helper_tools(&policy),
            Some(&activated),
            false,
        ));
        assert!(drive.iter().any(|n| n == "gws_drive_list"));
    }

    #[test]
    fn test_tool_full_schema_generate_image() {
        let info = tool_full_schema("gws_generate_image")
            .expect("gws_generate_image must resolve via gws_discover");
        assert_eq!(info["tool"], "gws_generate_image");
        assert_eq!(info["annotations"]["readOnlyHint"], json!(false));
        assert!(info["inputSchema"]["properties"]["prompt"].is_object());
    }
}
