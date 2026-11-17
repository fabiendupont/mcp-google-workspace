use google_workspace::error::GwsError;
use serde_json::{Value, json};

use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::shared::ServerState;
use crate::tools;

pub fn tasks_lists_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_lists",
        "title": "List Task Lists",
        "description": "List all Google Task lists (e.g. 'My Tasks', work lists).",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "max_results": {
                    "type": "integer",
                    "description": "Maximum task lists to return (default: 20)"
                }
            }
        }
    })
}

pub fn tasks_list_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_list",
        "title": "List Tasks",
        "description": "List tasks in a task list. Defaults to the first task list.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID from gws_tasks_lists (default: first list)"
                },
                "show_completed": {
                    "type": "boolean",
                    "description": "Include completed tasks (default: false)"
                },
                "show_hidden": {
                    "type": "boolean",
                    "description": "Include hidden tasks (default: false)"
                },
                "updated_min": {
                    "type": "string",
                    "description": "Only tasks updated after this RFC3339 timestamp"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum tasks to return (default: 100)"
                }
            }
        }
    })
}

pub fn tasks_get_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_get",
        "title": "Get Task",
        "description": "Get full details of a single task.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID from list results"
                },
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID (default: first list)"
                }
            },
            "required": ["task_id"]
        }
    })
}

pub fn tasks_create_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_create",
        "title": "Create Task",
        "description": "Create a task with title, optional notes and due date.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Task title"
                },
                "notes": {
                    "type": "string",
                    "description": "Task notes / details"
                },
                "due": {
                    "type": "string",
                    "description": "Due date (RFC3339, e.g. 2026-07-30T00:00:00Z)"
                },
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID (default: first list)"
                },
                "parent_task_id": {
                    "type": "string",
                    "description": "Parent task ID to create a subtask"
                },
                "previous_task_id": {
                    "type": "string",
                    "description": "Sibling task ID to insert after (positioning)"
                }
            },
            "required": ["title"]
        }
    })
}

pub fn tasks_update_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_update",
        "title": "Update Task",
        "description": "Update a task. Only provided fields are changed.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID to update"
                },
                "title": {
                    "type": "string",
                    "description": "New task title"
                },
                "notes": {
                    "type": "string",
                    "description": "New notes (replaces existing)"
                },
                "due": {
                    "type": "string",
                    "description": "New due date (RFC3339), or empty string to clear"
                },
                "status": {
                    "type": "string",
                    "description": "Set to 'needsAction' to reopen a completed task",
                    "enum": ["needsAction"]
                },
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID (default: first list)"
                }
            },
            "required": ["task_id"]
        }
    })
}

pub fn tasks_complete_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_complete",
        "title": "Complete Task",
        "description": "Mark a task as completed.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID to complete"
                },
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID (default: first list)"
                }
            },
            "required": ["task_id"]
        }
    })
}

pub fn tasks_delete_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_delete",
        "title": "Delete Task",
        "description": "Permanently delete a task. This cannot be undone.",
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID to delete"
                },
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID (default: first list)"
                }
            },
            "required": ["task_id"]
        }
    })
}

pub fn tasks_clear_tool_schema() -> Value {
    json!({
        "name": "gws_tasks_clear",
        "title": "Clear Completed Tasks",
        "description": "Clear (delete) all completed tasks from a task list. This cannot be undone.",
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "tasklist_id": {
                    "type": "string",
                    "description": "Task list ID (default: first list)"
                }
            }
        }
    })
}

fn format_task(task: &Value) -> Value {
    let mut out = json!({
        "id": task.get("id"),
        "title": task.get("title"),
        "status": task.get("status"),
        "due": task.get("due"),
        "notes": task.get("notes"),
        "updated": task.get("updated"),
    });
    if let Some(parent) = task.get("parent").and_then(|p| p.as_str()) {
        out["parent"] = json!(parent);
    }
    out
}

async fn resolve_tasklist_id(
    arguments: &Value,
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<String, GwsError> {
    if let Some(id) = arguments
        .get("tasklist_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(id.to_string());
    }
    let result = call_tasks_api(
        state,
        "tasklists",
        "list",
        json!({ "maxResults": 1 }),
        None,
        policy,
        meta,
    )
    .await?;
    result
        .get("items")
        .and_then(|i| i.as_array())
        .and_then(|a| a.first())
        .and_then(|l| l.get("id"))
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            GwsError::Validation(
                "No task lists found. Create a task list in Google Tasks first.".into(),
            )
        })
}

#[allow(clippy::too_many_arguments)]
async fn call_tasks_api(
    state: &mut ServerState,
    resource_path: &str,
    method_name: &str,
    params: Value,
    body: Option<Value>,
    policy: &Policy,
    meta: &RequestMeta,
) -> Result<Value, GwsError> {
    let doc = state.get_doc("tasks").await?;
    let resource = tools::find_resource(&doc.resources, resource_path)
        .ok_or_else(|| GwsError::Validation(format!("{resource_path} resource not found")))?;
    let method = resource.methods.get(method_name).ok_or_else(|| {
        GwsError::Validation(format!("{resource_path}.{method_name} method not found"))
    })?;

    let mut args = json!({ "params": params });
    if let Some(b) = body {
        args["body"] = b;
    }

    crate::execute::execute_tool(
        &doc,
        method,
        resource_path,
        method_name,
        &args,
        "tasks",
        policy,
        meta,
        None,
        None,
        false,
        &mut state.token_cache,
    )
    .await
}

pub(crate) async fn execute_tasks_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    if !policy.is_service_allowed("tasks") {
        return Err(GwsError::Validation(
            "Operation not allowed by policy".to_string(),
        ));
    }

    match tool_name {
        "gws_tasks_lists" => {
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(20);
            let result = call_tasks_api(
                state,
                "tasklists",
                "list",
                json!({ "maxResults": max_results }),
                None,
                policy,
                meta,
            )
            .await?;
            let empty = vec![];
            let items = result
                .get("items")
                .and_then(|i| i.as_array())
                .unwrap_or(&empty);
            let lists: Vec<Value> = items
                .iter()
                .map(|l| {
                    json!({
                        "id": l.get("id"),
                        "title": l.get("title"),
                        "updated": l.get("updated"),
                    })
                })
                .collect();
            let output = json!({ "taskLists": lists, "count": lists.len() });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_tasks_list" => {
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;
            let mut params = json!({
                "tasklist": tasklist_id,
                "maxResults": arguments.get("max_results").and_then(|v| v.as_u64()).unwrap_or(100),
                "showCompleted": arguments.get("show_completed").and_then(|v| v.as_bool()).unwrap_or(false),
                "showHidden": arguments.get("show_hidden").and_then(|v| v.as_bool()).unwrap_or(false),
            });
            if let Some(t) = arguments.get("updated_min").and_then(|v| v.as_str()) {
                params["updatedMin"] = json!(t);
            }
            let result = call_tasks_api(state, "tasks", "list", params, None, policy, meta).await?;
            let empty = vec![];
            let items = result
                .get("items")
                .and_then(|i| i.as_array())
                .unwrap_or(&empty);
            let tasks: Vec<Value> = items.iter().map(format_task).collect();
            let output = json!({ "tasks": tasks, "count": tasks.len() });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_tasks_get" => {
            let task_id = arguments
                .get("task_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'task_id'".into()))?;
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;
            let result = call_tasks_api(
                state,
                "tasks",
                "get",
                json!({ "tasklist": tasklist_id, "task": task_id }),
                None,
                policy,
                meta,
            )
            .await?;
            let task = format_task(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&task).unwrap_or_default() }],
                "structuredContent": task,
                "isError": false
            }))
        }

        "gws_tasks_create" => {
            let title = arguments
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'title'".into()))?;
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;

            let mut body = json!({ "title": title });
            if let Some(n) = arguments.get("notes").and_then(|v| v.as_str()) {
                body["notes"] = json!(n);
            }
            if let Some(d) = arguments.get("due").and_then(|v| v.as_str()) {
                body["due"] = json!(d);
            }

            let mut params = json!({ "tasklist": tasklist_id });
            if let Some(p) = arguments.get("parent_task_id").and_then(|v| v.as_str()) {
                params["parent"] = json!(p);
            }
            if let Some(p) = arguments.get("previous_task_id").and_then(|v| v.as_str()) {
                params["previous"] = json!(p);
            }

            let result =
                call_tasks_api(state, "tasks", "insert", params, Some(body), policy, meta).await?;
            let task = format_task(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&task).unwrap_or_default() }],
                "structuredContent": task,
                "isError": false
            }))
        }

        "gws_tasks_update" => {
            let task_id = arguments
                .get("task_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'task_id'".into()))?;
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;

            let mut body = json!({});
            let mut has_changes = false;
            if let Some(t) = arguments.get("title").and_then(|v| v.as_str()) {
                body["title"] = json!(t);
                has_changes = true;
            }
            if let Some(n) = arguments.get("notes").and_then(|v| v.as_str()) {
                body["notes"] = json!(n);
                has_changes = true;
            }
            if let Some(d) = arguments.get("due").and_then(|v| v.as_str()) {
                if d.trim().is_empty() {
                    body["due"] = Value::Null;
                } else {
                    body["due"] = json!(d);
                }
                has_changes = true;
            }
            if let Some(s) = arguments.get("status").and_then(|v| v.as_str()) {
                body["status"] = json!(s);
                has_changes = true;
            }
            if !has_changes {
                return Err(GwsError::Validation(
                    "No fields to update. Provide title, notes, due, or status.".into(),
                ));
            }

            let result = call_tasks_api(
                state,
                "tasks",
                "patch",
                json!({ "tasklist": tasklist_id, "task": task_id }),
                Some(body),
                policy,
                meta,
            )
            .await?;
            let task = format_task(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&task).unwrap_or_default() }],
                "structuredContent": task,
                "isError": false
            }))
        }

        "gws_tasks_complete" => {
            let task_id = arguments
                .get("task_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'task_id'".into()))?;
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;

            let result = call_tasks_api(
                state,
                "tasks",
                "patch",
                json!({ "tasklist": tasklist_id, "task": task_id }),
                Some(json!({ "status": "completed" })),
                policy,
                meta,
            )
            .await?;
            let task = format_task(&result);
            let output = json!({ "completed": task.get("status") == Some(&json!("completed")), "task": task });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_tasks_delete" => {
            let task_id = arguments
                .get("task_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'task_id'".into()))?;
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;

            call_tasks_api(
                state,
                "tasks",
                "delete",
                json!({ "tasklist": tasklist_id, "task": task_id }),
                None,
                policy,
                meta,
            )
            .await?;

            Ok(json!({
                "content": [{ "type": "text", "text": format!("Task {task_id} deleted") }],
                "isError": false
            }))
        }

        "gws_tasks_clear" => {
            let tasklist_id = resolve_tasklist_id(arguments, state, policy, meta).await?;
            call_tasks_api(
                state,
                "tasks",
                "clear",
                json!({ "tasklist": tasklist_id }),
                None,
                policy,
                meta,
            )
            .await?;
            Ok(json!({
                "content": [{ "type": "text", "text": format!("Completed tasks cleared from list {tasklist_id}") }],
                "isError": false
            }))
        }

        _ => Err(GwsError::Validation(format!(
            "Unknown Tasks tool '{tool_name}'. Available: gws_tasks_lists, gws_tasks_list, \
             gws_tasks_get, gws_tasks_create, gws_tasks_update, gws_tasks_complete, \
             gws_tasks_delete, gws_tasks_clear"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_schemas() -> Vec<Value> {
        vec![
            tasks_lists_tool_schema(),
            tasks_list_tool_schema(),
            tasks_get_tool_schema(),
            tasks_create_tool_schema(),
            tasks_update_tool_schema(),
            tasks_complete_tool_schema(),
            tasks_delete_tool_schema(),
            tasks_clear_tool_schema(),
        ]
    }

    #[test]
    fn test_tool_schemas_have_short_descriptions() {
        for schema in all_schemas() {
            let name = schema["name"].as_str().unwrap();
            let desc = schema["description"].as_str().unwrap();
            assert!(
                desc.len() < 100,
                "Tool {name} description too long ({} chars)",
                desc.len()
            );
        }
    }

    #[test]
    fn test_all_tool_names_start_with_gws_tasks() {
        for schema in all_schemas() {
            let name = schema["name"].as_str().unwrap();
            assert!(
                name.starts_with("gws_tasks_"),
                "Tool name '{name}' must start with gws_tasks_"
            );
        }
    }

    #[test]
    fn test_read_write_annotations() {
        for schema in all_schemas() {
            let name = schema["name"].as_str().unwrap();
            let read_only = schema["annotations"]["readOnlyHint"].as_bool().unwrap();
            match name {
                "gws_tasks_lists" | "gws_tasks_list" | "gws_tasks_get" => {
                    assert!(read_only, "{name} should be readOnlyHint: true");
                }
                _ => {
                    assert!(!read_only, "{name} should be readOnlyHint: false");
                }
            }
        }
    }

    #[test]
    fn test_required_fields() {
        for (name, field) in [
            ("gws_tasks_get", "task_id"),
            ("gws_tasks_create", "title"),
            ("gws_tasks_update", "task_id"),
            ("gws_tasks_complete", "task_id"),
            ("gws_tasks_delete", "task_id"),
        ] {
            let schema = all_schemas()
                .into_iter()
                .find(|s| s["name"] == name)
                .unwrap();
            let required = schema["inputSchema"]["required"].as_array().unwrap();
            assert!(
                required.iter().any(|r| r == field),
                "{name} must require {field}"
            );
        }
    }

    #[test]
    fn test_format_task() {
        let task = json!({
            "id": "abc123",
            "title": "Write report",
            "status": "needsAction",
            "due": "2026-07-30T00:00:00.000Z",
            "notes": "Q2 numbers",
            "updated": "2026-07-28T10:00:00.000Z",
            "parent": "parent1"
        });
        let formatted = format_task(&task);
        assert_eq!(formatted["id"], "abc123");
        assert_eq!(formatted["title"], "Write report");
        assert_eq!(formatted["parent"], "parent1");
    }

    #[test]
    fn test_format_task_no_parent() {
        let formatted = format_task(&json!({ "id": "x", "title": "t" }));
        assert!(formatted.get("parent").is_none());
    }
}
