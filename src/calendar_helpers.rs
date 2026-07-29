use google_workspace::error::GwsError;
use serde_json::{Value, json};

use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::server::ServerState;
use crate::tools;

pub fn calendar_list_tool_schema() -> Value {
    json!({
        "name": "gws_calendar_list",
        "title": "List Events",
        "description": "List upcoming calendar events. Defaults to primary calendar, next 7 days.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (default: primary)"
                },
                "time_min": {
                    "type": "string",
                    "description": "Start of range (RFC3339, e.g. 2026-07-29T00:00:00Z). Default: now"
                },
                "time_max": {
                    "type": "string",
                    "description": "End of range (RFC3339). Default: 7 days from now"
                },
                "query": {
                    "type": "string",
                    "description": "Free text search in event titles and descriptions"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum events to return (default: 20)"
                }
            }
        }
    })
}

pub fn calendar_get_tool_schema() -> Value {
    json!({
        "name": "gws_calendar_get",
        "title": "Get Event",
        "description": "Get full details of a calendar event.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "event_id": {
                    "type": "string",
                    "description": "Event ID from list results"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (default: primary)"
                }
            },
            "required": ["event_id"]
        }
    })
}

pub fn calendar_create_tool_schema() -> Value {
    json!({
        "name": "gws_calendar_create",
        "title": "Create Event",
        "description": "Create a calendar event with title, time, and optional attendees.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Event title"
                },
                "start": {
                    "type": "string",
                    "description": "Start time (RFC3339, e.g. 2026-07-30T10:00:00+02:00) or date (2026-07-30 for all-day)"
                },
                "end": {
                    "type": "string",
                    "description": "End time (RFC3339) or date. Default: 1 hour after start"
                },
                "description": {
                    "type": "string",
                    "description": "Event description (plain text or HTML)"
                },
                "location": {
                    "type": "string",
                    "description": "Event location"
                },
                "attendees": {
                    "type": "string",
                    "description": "Comma-separated email addresses of attendees"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (default: primary)"
                }
            },
            "required": ["summary", "start"]
        }
    })
}

pub fn calendar_update_tool_schema() -> Value {
    json!({
        "name": "gws_calendar_update",
        "title": "Update Event",
        "description": "Update an existing calendar event. Only provided fields are changed.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "event_id": {
                    "type": "string",
                    "description": "Event ID to update"
                },
                "summary": {
                    "type": "string",
                    "description": "New event title"
                },
                "start": {
                    "type": "string",
                    "description": "New start time (RFC3339) or date"
                },
                "end": {
                    "type": "string",
                    "description": "New end time (RFC3339) or date"
                },
                "description": {
                    "type": "string",
                    "description": "New description"
                },
                "location": {
                    "type": "string",
                    "description": "New location"
                },
                "attendees": {
                    "type": "string",
                    "description": "New attendee list (comma-separated emails, replaces existing)"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (default: primary)"
                }
            },
            "required": ["event_id"]
        }
    })
}

pub fn calendar_delete_tool_schema() -> Value {
    json!({
        "name": "gws_calendar_delete",
        "title": "Delete Event",
        "description": "Delete (cancel) a calendar event.",
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "event_id": {
                    "type": "string",
                    "description": "Event ID to delete"
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID (default: primary)"
                }
            },
            "required": ["event_id"]
        }
    })
}

pub fn calendar_freebusy_tool_schema() -> Value {
    json!({
        "name": "gws_calendar_freebusy",
        "title": "Find Free Time",
        "description": "Check free/busy times for one or more calendars.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "time_min": {
                    "type": "string",
                    "description": "Start of range (RFC3339)"
                },
                "time_max": {
                    "type": "string",
                    "description": "End of range (RFC3339)"
                },
                "calendars": {
                    "type": "string",
                    "description": "Comma-separated calendar IDs (default: primary)"
                }
            },
            "required": ["time_min", "time_max"]
        }
    })
}

fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    // Civil date from days since epoch (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if mo <= 2 { y + 1 } else { y };
    format!("{yr:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn is_date_only(s: &str) -> bool {
    s.len() == 10 && s.chars().nth(4) == Some('-') && s.chars().nth(7) == Some('-')
}

fn build_time_object(s: &str) -> Value {
    if is_date_only(s) {
        json!({ "date": s })
    } else {
        json!({ "dateTime": s })
    }
}

fn format_event(event: &Value) -> Value {
    let start = event
        .get("start")
        .and_then(|s| s.get("dateTime").or(s.get("date")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let end = event
        .get("end")
        .and_then(|s| s.get("dateTime").or(s.get("date")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut my_status = "";
    let attendees: Vec<Value> = event
        .get("attendees")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    let email = a.get("email").and_then(|e| e.as_str()).unwrap_or("");
                    let status = a
                        .get("responseStatus")
                        .and_then(|s| s.as_str())
                        .unwrap_or("needsAction");
                    if a.get("self").and_then(|s| s.as_bool()).unwrap_or(false) {
                        my_status = match status {
                            "accepted" => "accepted",
                            "declined" => "declined",
                            "tentative" => "tentative",
                            _ => "needsAction",
                        };
                    }
                    json!({ "email": email, "responseStatus": status })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "id": event.get("id"),
        "summary": event.get("summary"),
        "start": start,
        "end": end,
        "location": event.get("location"),
        "description": event.get("description"),
        "status": event.get("status"),
        "myStatus": my_status,
        "htmlLink": event.get("htmlLink"),
        "attendees": attendees,
        "organizer": event.get("organizer").and_then(|o| o.get("email"))
    })
}

pub(crate) async fn execute_calendar_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let cal_doc = state.get_doc("calendar").await?;

    match tool_name {
        "gws_calendar_list" => {
            let calendar_id = arguments
                .get("calendar_id")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");
            let time_min = arguments.get("time_min").and_then(|v| v.as_str());
            let time_max = arguments.get("time_max").and_then(|v| v.as_str());
            let query = arguments.get("query").and_then(|v| v.as_str());
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(20);

            let events_resource = tools::find_resource(&cal_doc.resources, "events")
                .ok_or_else(|| GwsError::Validation("events resource not found".into()))?;
            let list_method = events_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("events.list method not found".into()))?;

            let mut params = json!({
                "calendarId": calendar_id,
                "maxResults": max_results,
                "singleEvents": true,
                "orderBy": "startTime"
            });
            if let Some(t) = time_min {
                params["timeMin"] = json!(t);
            } else {
                params["timeMin"] = json!(rfc3339_now());
            }
            if let Some(t) = time_max {
                params["timeMax"] = json!(t);
            }
            if let Some(q) = query {
                params["q"] = json!(q);
            }

            let args = json!({
                "params": params,
                "fields": "items(id,summary,start,end,location,description,status,htmlLink,attendees(email,responseStatus,self),organizer(email)),nextPageToken"
            });
            let result = crate::execute::execute_tool(
                &cal_doc,
                list_method,
                "events",
                "list",
                &args,
                "calendar",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let empty = vec![];
            let items = result
                .get("items")
                .and_then(|i| i.as_array())
                .unwrap_or(&empty);
            let events: Vec<Value> = items.iter().map(format_event).collect();

            let output = json!({ "events": events, "count": events.len() });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_calendar_get" => {
            let event_id = arguments
                .get("event_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'event_id'".into()))?;
            let calendar_id = arguments
                .get("calendar_id")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");

            let events_resource = tools::find_resource(&cal_doc.resources, "events")
                .ok_or_else(|| GwsError::Validation("events resource not found".into()))?;
            let get_method = events_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("events.get method not found".into()))?;

            let args = json!({
                "params": { "calendarId": calendar_id, "eventId": event_id }
            });
            let result = crate::execute::execute_tool(
                &cal_doc,
                get_method,
                "events",
                "get",
                &args,
                "calendar",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let event = format_event(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&event).unwrap_or_default() }],
                "structuredContent": event,
                "isError": false
            }))
        }

        "gws_calendar_create" => {
            let summary = arguments
                .get("summary")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'summary'".into()))?;
            let start_str = arguments
                .get("start")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'start'".into()))?;
            let calendar_id = arguments
                .get("calendar_id")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");

            let mut body = json!({
                "summary": summary,
                "start": build_time_object(start_str)
            });

            if let Some(end_str) = arguments.get("end").and_then(|v| v.as_str()) {
                body["end"] = build_time_object(end_str);
            } else {
                body["end"] = build_time_object(start_str);
            }

            if let Some(desc) = arguments.get("description").and_then(|v| v.as_str()) {
                body["description"] = json!(desc);
            }
            if let Some(loc) = arguments.get("location").and_then(|v| v.as_str()) {
                body["location"] = json!(loc);
            }
            if let Some(att) = arguments.get("attendees").and_then(|v| v.as_str()) {
                let attendees: Vec<Value> = att
                    .split(',')
                    .map(|e| json!({ "email": e.trim() }))
                    .collect();
                body["attendees"] = json!(attendees);
            }

            let events_resource = tools::find_resource(&cal_doc.resources, "events")
                .ok_or_else(|| GwsError::Validation("events resource not found".into()))?;
            let insert_method = events_resource
                .methods
                .get("insert")
                .ok_or_else(|| GwsError::Validation("events.insert method not found".into()))?;

            let args = json!({
                "params": { "calendarId": calendar_id },
                "body": body
            });
            let result = crate::execute::execute_tool(
                &cal_doc,
                insert_method,
                "events",
                "insert",
                &args,
                "calendar",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let event = format_event(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&event).unwrap_or_default() }],
                "structuredContent": event,
                "isError": false
            }))
        }

        "gws_calendar_update" => {
            let event_id = arguments
                .get("event_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'event_id'".into()))?;
            let calendar_id = arguments
                .get("calendar_id")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");

            let mut body = json!({});
            if let Some(s) = arguments.get("summary").and_then(|v| v.as_str()) {
                body["summary"] = json!(s);
            }
            if let Some(s) = arguments.get("start").and_then(|v| v.as_str()) {
                body["start"] = build_time_object(s);
            }
            if let Some(s) = arguments.get("end").and_then(|v| v.as_str()) {
                body["end"] = build_time_object(s);
            }
            if let Some(s) = arguments.get("description").and_then(|v| v.as_str()) {
                body["description"] = json!(s);
            }
            if let Some(s) = arguments.get("location").and_then(|v| v.as_str()) {
                body["location"] = json!(s);
            }
            if let Some(att) = arguments.get("attendees").and_then(|v| v.as_str()) {
                let attendees: Vec<Value> = att
                    .split(',')
                    .map(|e| json!({ "email": e.trim() }))
                    .collect();
                body["attendees"] = json!(attendees);
            }

            let events_resource = tools::find_resource(&cal_doc.resources, "events")
                .ok_or_else(|| GwsError::Validation("events resource not found".into()))?;
            let patch_method = events_resource
                .methods
                .get("patch")
                .ok_or_else(|| GwsError::Validation("events.patch method not found".into()))?;

            let args = json!({
                "params": { "calendarId": calendar_id, "eventId": event_id },
                "body": body
            });
            let result = crate::execute::execute_tool(
                &cal_doc,
                patch_method,
                "events",
                "patch",
                &args,
                "calendar",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let event = format_event(&result);
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&event).unwrap_or_default() }],
                "structuredContent": event,
                "isError": false
            }))
        }

        "gws_calendar_delete" => {
            let event_id = arguments
                .get("event_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'event_id'".into()))?;
            let calendar_id = arguments
                .get("calendar_id")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");

            let events_resource = tools::find_resource(&cal_doc.resources, "events")
                .ok_or_else(|| GwsError::Validation("events resource not found".into()))?;
            let delete_method = events_resource
                .methods
                .get("delete")
                .ok_or_else(|| GwsError::Validation("events.delete method not found".into()))?;

            let args = json!({
                "params": { "calendarId": calendar_id, "eventId": event_id }
            });
            crate::execute::execute_tool(
                &cal_doc,
                delete_method,
                "events",
                "delete",
                &args,
                "calendar",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            Ok(json!({
                "content": [{ "type": "text", "text": format!("Event {event_id} deleted") }],
                "isError": false
            }))
        }

        "gws_calendar_freebusy" => {
            let time_min = arguments
                .get("time_min")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'time_min'".into()))?;
            let time_max = arguments
                .get("time_max")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'time_max'".into()))?;
            let calendars_str = arguments
                .get("calendars")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");

            let cal_items: Vec<Value> = calendars_str
                .split(',')
                .map(|c| json!({ "id": c.trim() }))
                .collect();

            let freebusy_resource = tools::find_resource(&cal_doc.resources, "freebusy")
                .ok_or_else(|| GwsError::Validation("freebusy resource not found".into()))?;
            let query_method = freebusy_resource
                .methods
                .get("query")
                .ok_or_else(|| GwsError::Validation("freebusy.query method not found".into()))?;

            let args = json!({
                "body": {
                    "timeMin": time_min,
                    "timeMax": time_max,
                    "items": cal_items
                }
            });
            let result = crate::execute::execute_tool(
                &cal_doc,
                query_method,
                "freebusy",
                "query",
                &args,
                "calendar",
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

        _ => Err(GwsError::Validation(format!(
            "Unknown Calendar tool '{tool_name}'. Available: gws_calendar_list, gws_calendar_get, \
             gws_calendar_create, gws_calendar_update, gws_calendar_delete, gws_calendar_freebusy"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_schemas_have_short_descriptions() {
        let schemas = vec![
            calendar_list_tool_schema(),
            calendar_get_tool_schema(),
            calendar_create_tool_schema(),
            calendar_update_tool_schema(),
            calendar_delete_tool_schema(),
            calendar_freebusy_tool_schema(),
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
    }

    #[test]
    fn test_all_tool_names_start_with_gws_calendar() {
        let schemas = vec![
            calendar_list_tool_schema(),
            calendar_get_tool_schema(),
            calendar_create_tool_schema(),
            calendar_update_tool_schema(),
            calendar_delete_tool_schema(),
            calendar_freebusy_tool_schema(),
        ];
        for schema in &schemas {
            let name = schema["name"].as_str().unwrap();
            assert!(
                name.starts_with("gws_calendar_"),
                "Tool name '{name}' must start with gws_calendar_"
            );
        }
    }

    #[test]
    fn test_is_date_only() {
        assert!(is_date_only("2026-07-29"));
        assert!(!is_date_only("2026-07-29T10:00:00+02:00"));
        assert!(!is_date_only("2026-07-29T10:00:00Z"));
    }

    #[test]
    fn test_build_time_object_date() {
        let t = build_time_object("2026-07-29");
        assert_eq!(t, json!({ "date": "2026-07-29" }));
    }

    #[test]
    fn test_build_time_object_datetime() {
        let t = build_time_object("2026-07-29T10:00:00+02:00");
        assert_eq!(t, json!({ "dateTime": "2026-07-29T10:00:00+02:00" }));
    }

    #[test]
    fn test_format_event() {
        let event = json!({
            "id": "abc123",
            "summary": "Team standup",
            "start": { "dateTime": "2026-07-29T10:00:00+02:00" },
            "end": { "dateTime": "2026-07-29T10:30:00+02:00" },
            "location": "Room A",
            "status": "confirmed",
            "attendees": [
                { "email": "alice@example.com", "responseStatus": "accepted", "self": true },
                { "email": "bob@example.com", "responseStatus": "declined" }
            ],
            "organizer": { "email": "alice@example.com" }
        });
        let formatted = format_event(&event);
        assert_eq!(formatted["id"], "abc123");
        assert_eq!(formatted["start"], "2026-07-29T10:00:00+02:00");
        assert_eq!(formatted["attendees"].as_array().unwrap().len(), 2);
        assert_eq!(formatted["myStatus"], "accepted");
        assert_eq!(formatted["attendees"][1]["responseStatus"], "declined");
    }
}
