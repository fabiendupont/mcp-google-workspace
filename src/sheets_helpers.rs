use serde_json::{Value, json};

pub fn validate_spreadsheet_id(id: &str) -> Result<(), String> {
    if id.len() < 20 {
        return Err(format!(
            "Invalid spreadsheet ID '{}' (too short — Google Sheets IDs are typically 44 characters). \
             Check the ID from the previous tool call response.",
            id
        ));
    }
    Ok(())
}

pub fn sheets_read_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_read",
        "title": "Read Sheet Range",
        "description": "Read spreadsheet data. Returns rows as JSON arrays.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Google Sheets spreadsheet ID"
                },
                "range": {
                    "type": "string",
                    "description": "A1 range (e.g. 'A1:D10', 'Sheet1!A:C')"
                },
                "sheet": {
                    "type": "string",
                    "description": "Tab name if not included in range"
                },
                "format": {
                    "type": "string",
                    "enum": ["values", "formatted", "formula"],
                    "description": "values=raw, formatted=display text, formula=formulas"
                }
            },
            "required": ["spreadsheet_id"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "range": { "type": "string" },
                "values": {
                    "type": "array",
                    "items": { "type": "array", "items": {} }
                }
            },
            "required": ["values"]
        }
    })
}

pub fn sheets_write_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_write",
        "title": "Write Sheet Data",
        "description": "Write to a spreadsheet. Creates a new spreadsheet if title is provided instead of spreadsheet_id.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Spreadsheet title. Required when creating new."
                },
                "data": {
                    "type": "array",
                    "description": "Rows of data. [[\"Name\",\"Score\"],[\"Alice\",95]]",
                    "items": { "type": "array", "items": {} }
                },
                "folder_id": {
                    "type": "string",
                    "description": "Drive folder ID. Required when creating new."
                },
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Existing spreadsheet ID. Only for writing to an existing spreadsheet."
                },
                "range": {
                    "type": "string",
                    "description": "A1 range (e.g. 'A1:C3'). Defaults to 'Sheet1'."
                },
                "sheet": {
                    "type": "string",
                    "description": "Tab name if not included in range"
                }
            },
            "required": ["data"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "spreadsheetId": { "type": "string" },
                "title": { "type": "string" },
                "url": { "type": "string" },
                "updatedRange": { "type": "string" },
                "updatedRows": { "type": "integer" },
                "updatedColumns": { "type": "integer" }
            },
            "required": ["spreadsheetId"]
        }
    })
}

pub fn sheets_append_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_append",
        "title": "Append Sheet Rows",
        "description": "Append rows to a spreadsheet after existing data. Does not overwrite.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID"
                },
                "range": {
                    "type": "string",
                    "description": "A1 range defining the table area to append to"
                },
                "data": {
                    "type": "array",
                    "description": "Rows to append. [[\"Alice\",95],[\"Bob\",78]]",
                    "items": { "type": "array", "items": {} }
                },
                "sheet": {
                    "type": "string",
                    "description": "Tab name if not included in range"
                }
            },
            "required": ["spreadsheet_id", "range", "data"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "updatedRange": { "type": "string" },
                "updatedRows": { "type": "integer" },
                "updatedColumns": { "type": "integer" },
                "updatedCells": { "type": "integer" }
            }
        }
    })
}

pub fn sheets_info_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_info",
        "title": "Spreadsheet Info",
        "description": "Get spreadsheet info: title, tab names, row/column counts.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID"
                }
            },
            "required": ["spreadsheet_id"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "spreadsheetId": { "type": "string" },
                "sheets": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sheetId": { "type": "integer" },
                            "title": { "type": "string" },
                            "rowCount": { "type": "integer" },
                            "columnCount": { "type": "integer" }
                        }
                    }
                }
            }
        }
    })
}

pub fn sheets_clear_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_clear",
        "title": "Clear Sheet Range",
        "description": "Clear spreadsheet cell contents in a range. Keeps formatting.",
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID"
                },
                "range": {
                    "type": "string",
                    "description": "A1 range (e.g. 'A1:C10')"
                },
                "sheet": {
                    "type": "string",
                    "description": "Tab name if not included in range"
                }
            },
            "required": ["spreadsheet_id", "range"]
        }
    })
}

pub fn sheets_manage_tabs_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_manage_tabs",
        "title": "Manage Sheet Tabs",
        "description": "Create, rename, or delete spreadsheet tabs.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID"
                },
                "action": {
                    "type": "string",
                    "enum": ["create", "rename", "delete"],
                    "description": "create, rename, or delete"
                },
                "title": {
                    "type": "string",
                    "description": "Tab title (for create/rename)"
                },
                "sheet_id": {
                    "type": "integer",
                    "description": "Tab ID from gws_sheets_info (for rename/delete)"
                }
            },
            "required": ["spreadsheet_id", "action"]
        }
    })
}

pub fn build_range(range: &str, sheet: Option<&str>) -> String {
    if range.contains('!') {
        return range.to_string();
    }
    match sheet {
        Some(name) => format!("'{}'!{}", name.replace('\'', "''"), range),
        None => range.to_string(),
    }
}

fn render_option(format: &str) -> &str {
    match format {
        "values" => "UNFORMATTED_VALUE",
        "formula" => "FORMULA",
        _ => "FORMATTED_VALUE",
    }
}

pub fn build_read_args(range: &str, sheet: Option<&str>, format: Option<&str>) -> Value {
    let full_range = build_range(range, sheet);
    let render = render_option(format.unwrap_or("formatted"));
    json!({
        "params": {
            "range": full_range,
            "valueRenderOption": render
        }
    })
}

pub fn build_write_args(range: &str, data: &Value, sheet: Option<&str>) -> Value {
    let full_range = build_range(range, sheet);
    json!({
        "params": {
            "range": full_range,
            "valueInputOption": "USER_ENTERED"
        },
        "body": {
            "range": full_range,
            "values": data
        }
    })
}

pub fn build_append_args(range: &str, data: &Value, sheet: Option<&str>) -> Value {
    let full_range = build_range(range, sheet);
    json!({
        "params": {
            "range": full_range,
            "valueInputOption": "USER_ENTERED",
            "insertDataOption": "INSERT_ROWS"
        },
        "body": {
            "range": full_range,
            "values": data
        }
    })
}

pub fn build_clear_args(range: &str, sheet: Option<&str>) -> Value {
    let full_range = build_range(range, sheet);
    json!({
        "params": { "range": full_range }
    })
}

pub fn build_info_args() -> Value {
    json!({
        "fields": "spreadsheetId,properties(title),sheets(properties(sheetId,title,index,gridProperties(rowCount,columnCount)))"
    })
}

pub fn format_info_result(raw: &Value) -> Value {
    let title = raw
        .pointer("/properties/title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled");
    let spreadsheet_id = raw
        .get("spreadsheetId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let sheets: Vec<Value> = raw
        .get("sheets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let props = s.get("properties")?;
                    Some(json!({
                        "sheetId": props.get("sheetId"),
                        "title": props.get("title"),
                        "index": props.get("index"),
                        "rowCount": props.pointer("/gridProperties/rowCount"),
                        "columnCount": props.pointer("/gridProperties/columnCount"),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "title": title,
        "spreadsheetId": spreadsheet_id,
        "sheets": sheets
    })
}

pub fn build_tab_request(action: &str, title: Option<&str>, sheet_id: Option<i64>) -> Result<Value, String> {
    match action {
        "create" => {
            let name = title.ok_or("Missing 'title' for create action")?;
            Ok(json!({
                "body": {
                    "requests": [{
                        "addSheet": {
                            "properties": { "title": name }
                        }
                    }]
                }
            }))
        }
        "rename" => {
            let sid = sheet_id.ok_or("Missing 'sheet_id' for rename action")?;
            let name = title.ok_or("Missing 'title' for rename action")?;
            Ok(json!({
                "body": {
                    "requests": [{
                        "updateSheetProperties": {
                            "properties": {
                                "sheetId": sid,
                                "title": name
                            },
                            "fields": "title"
                        }
                    }]
                }
            }))
        }
        "delete" => {
            let sid = sheet_id.ok_or("Missing 'sheet_id' for delete action")?;
            Ok(json!({
                "body": {
                    "requests": [{
                        "deleteSheet": {
                            "sheetId": sid
                        }
                    }]
                }
            }))
        }
        _ => Err(format!("Unknown action '{}'. Use: create, rename, delete", action)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_range_with_sheet() {
        assert_eq!(build_range("A1:D10", Some("Data")), "'Data'!A1:D10");
    }

    #[test]
    fn build_range_without_sheet() {
        assert_eq!(build_range("A1:D10", None), "A1:D10");
    }

    #[test]
    fn build_range_already_qualified() {
        assert_eq!(build_range("Sheet1!A1:D10", Some("Other")), "Sheet1!A1:D10");
    }

    #[test]
    fn build_range_escapes_quotes() {
        assert_eq!(build_range("A1:B2", Some("Mike's")), "'Mike''s'!A1:B2");
    }

    #[test]
    fn render_option_values() {
        assert_eq!(render_option("values"), "UNFORMATTED_VALUE");
        assert_eq!(render_option("formula"), "FORMULA");
        assert_eq!(render_option("formatted"), "FORMATTED_VALUE");
        assert_eq!(render_option("other"), "FORMATTED_VALUE");
    }

    #[test]
    fn build_read_args_default_format() {
        let args = build_read_args("A1:B2", None, None);
        assert_eq!(
            args["params"]["valueRenderOption"],
            "FORMATTED_VALUE"
        );
    }

    #[test]
    fn build_write_args_structure() {
        let data = json!([["a", "b"], [1, 2]]);
        let args = build_write_args("A1:B2", &data, Some("Sheet1"));
        assert_eq!(args["params"]["range"], "'Sheet1'!A1:B2");
        assert_eq!(args["params"]["valueInputOption"], "USER_ENTERED");
        assert_eq!(args["body"]["values"], data);
    }

    #[test]
    fn build_append_args_has_insert_rows() {
        let data = json!([["x"]]);
        let args = build_append_args("A:A", &data, None);
        assert_eq!(args["params"]["insertDataOption"], "INSERT_ROWS");
    }

    #[test]
    fn build_info_args_has_fields() {
        let args = build_info_args();
        let fields = args["fields"].as_str().unwrap();
        assert!(fields.contains("sheets"));
        assert!(fields.contains("gridProperties"));
    }

    #[test]
    fn format_info_result_extracts_sheets() {
        let raw = json!({
            "spreadsheetId": "abc123",
            "properties": { "title": "My Sheet" },
            "sheets": [{
                "properties": {
                    "sheetId": 0,
                    "title": "Sheet1",
                    "index": 0,
                    "gridProperties": { "rowCount": 1000, "columnCount": 26 }
                }
            }]
        });
        let result = format_info_result(&raw);
        assert_eq!(result["title"], "My Sheet");
        assert_eq!(result["sheets"][0]["title"], "Sheet1");
        assert_eq!(result["sheets"][0]["rowCount"], 1000);
    }

    #[test]
    fn build_tab_create() {
        let req = build_tab_request("create", Some("New Tab"), None).unwrap();
        assert!(req["body"]["requests"][0]["addSheet"].is_object());
    }

    #[test]
    fn build_tab_rename() {
        let req = build_tab_request("rename", Some("Renamed"), Some(0)).unwrap();
        assert!(req["body"]["requests"][0]["updateSheetProperties"].is_object());
    }

    #[test]
    fn build_tab_delete() {
        let req = build_tab_request("delete", None, Some(1)).unwrap();
        assert!(req["body"]["requests"][0]["deleteSheet"].is_object());
    }

    #[test]
    fn build_tab_create_missing_title() {
        assert!(build_tab_request("create", None, None).is_err());
    }

    #[test]
    fn validate_spreadsheet_id_valid() {
        assert!(validate_spreadsheet_id("1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgVE2upms").is_ok());
    }

    #[test]
    fn validate_spreadsheet_id_too_short() {
        assert!(validate_spreadsheet_id("short").is_err());
    }
}
