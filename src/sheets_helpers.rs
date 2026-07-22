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

pub fn normalize_data(data: &Value) -> Value {
    let Some(arr) = data.as_array() else {
        return data.clone();
    };
    if arr.is_empty() {
        return data.clone();
    }
    if arr[0].is_array() {
        return data.clone();
    }
    if let Some(obj) = arr[0].as_object() {
        let headers: Vec<String> = obj.keys().cloned().collect();
        let mut rows: Vec<Value> = vec![json!(headers)];
        for item in arr {
            if let Some(o) = item.as_object() {
                let row: Vec<Value> = headers
                    .iter()
                    .map(|h| o.get(h).cloned().unwrap_or(json!("")))
                    .collect();
                rows.push(json!(row));
            }
        }
        json!(rows)
    } else {
        let rows: Vec<Value> = arr.iter().map(|v| json!([v])).collect();
        json!(rows)
    }
}

pub fn extract_cell_references(formula: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let chars: Vec<char> = formula.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Skip quoted strings
        if chars[i] == '"' {
            i += 1;
            while i < len && chars[i] != '"' {
                i += 1;
            }
            i += 1;
            continue;
        }

        // Look for cell references: optional sheet prefix + column letters + row digits
        // Patterns: A1, AB12, Sheet1!A1, 'Sheet Name'!A1, A1:B5
        if chars[i].is_ascii_uppercase() {
            let start = i;
            // Collect column letters
            while i < len && chars[i].is_ascii_uppercase() {
                i += 1;
            }
            // Must be followed by digits
            if i < len && chars[i].is_ascii_digit() {
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let cell_ref: String = chars[start..i].iter().collect();
                // Check for range notation (A1:B5)
                if i < len && chars[i] == ':' {
                    let colon = i;
                    i += 1;
                    let range_start = i;
                    while i < len && chars[i].is_ascii_uppercase() {
                        i += 1;
                    }
                    if i < len && chars[i].is_ascii_digit() {
                        while i < len && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                        let range_ref: String = chars[start..i].iter().collect();
                        refs.push(range_ref);
                    } else {
                        refs.push(cell_ref);
                        i = colon + 1;
                    }
                } else {
                    // Skip function names (all-alpha followed by open paren)
                    if !cell_ref.chars().all(|c| c.is_ascii_alphabetic()) || (i < len && chars[i] != '(') {
                        refs.push(cell_ref);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    refs
}

static FUNCTION_TRANSLATIONS: &[(&str, &str)] = &[
    ("SUM", "sum of"),
    ("AVERAGE", "average of"),
    ("COUNT", "count of"),
    ("COUNTA", "count of non-empty values in"),
    ("MAX", "maximum of"),
    ("MIN", "minimum of"),
    ("IF", "if"),
    ("VLOOKUP", "look up"),
    ("HLOOKUP", "look up horizontally"),
    ("INDEX", "value at index in"),
    ("MATCH", "position of"),
    ("CONCATENATE", "join"),
    ("LEN", "length of"),
    ("LEFT", "left characters of"),
    ("RIGHT", "right characters of"),
    ("MID", "middle characters of"),
    ("TRIM", "trimmed"),
    ("UPPER", "uppercase of"),
    ("LOWER", "lowercase of"),
    ("ROUND", "rounded"),
    ("ROUNDUP", "rounded up"),
    ("ROUNDDOWN", "rounded down"),
    ("ABS", "absolute value of"),
    ("SUMIF", "sum where"),
    ("COUNTIF", "count where"),
    ("AVERAGEIF", "average where"),
    ("IFERROR", "if error then"),
    ("ISBLANK", "is blank"),
    ("TODAY", "today's date"),
    ("NOW", "current date and time"),
    ("YEAR", "year of"),
    ("MONTH", "month of"),
    ("DAY", "day of"),
    ("DATEDIF", "difference between dates"),
    ("TEXT", "formatted text of"),
    ("VALUE", "numeric value of"),
    ("SUBSTITUTE", "substitute in"),
    ("UNIQUE", "unique values of"),
    ("SORT", "sorted"),
    ("FILTER", "filtered"),
    ("ARRAYFORMULA", "array formula"),
];

pub fn explain_formula(formula: &str, headers: &[String], row_labels: &[String]) -> String {
    let mut explanation = formula.to_string();

    // Replace cell references with human-readable names
    let refs = extract_cell_references(formula);
    for cell_ref in &refs {
        if let Some((col_name, row_name)) = resolve_cell_name(cell_ref, headers, row_labels) {
            let readable = if row_name.is_empty() {
                format!("\"{col_name}\"")
            } else {
                format!("\"{col_name}\" for \"{row_name}\"")
            };
            explanation = explanation.replace(cell_ref, &readable);
        }
    }

    // Replace function names with English
    for (func, english) in FUNCTION_TRANSLATIONS {
        let pattern = format!("{func}(");
        if explanation.contains(&pattern) {
            explanation = explanation.replace(&pattern, &format!("{english}("));
        }
    }

    // Clean up operators
    explanation = explanation.replace(">=", " is at least ");
    explanation = explanation.replace("<=", " is at most ");
    explanation = explanation.replace("<>", " is not equal to ");
    explanation = explanation.replace('>', " is greater than ");
    explanation = explanation.replace('<', " is less than ");

    explanation
}

pub fn resolve_cell_name(cell_ref: &str, headers: &[String], row_labels: &[String]) -> Option<(String, String)> {
    // Parse column letters and row number from e.g. "B5"
    let col_end = cell_ref.chars().take_while(|c| c.is_ascii_uppercase()).count();
    if col_end == 0 || col_end >= cell_ref.len() {
        return None;
    }
    let col_letters = &cell_ref[..col_end];
    let row_num: usize = cell_ref[col_end..].parse().ok()?;

    // Convert column letters to 0-based index: A=0, B=1, ..., Z=25, AA=26
    let col_idx = col_letters
        .bytes()
        .fold(0usize, |acc, b| acc * 26 + (b - b'A') as usize + 1)
        .checked_sub(1)?;

    let col_name = headers.get(col_idx).cloned().unwrap_or_else(|| col_letters.to_string());

    let row_name = if row_num >= 2 {
        row_labels
            .get(row_num - 2) // row_labels is 0-indexed from row 2
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    };

    Some((col_name, row_name))
}

pub fn sheets_trace_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_trace",
        "title": "Trace Cell Dependencies",
        "description": "Trace what feeds into a spreadsheet cell. Shows the dependency tree of formulas.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID"
                },
                "cell": {
                    "type": "string",
                    "description": "Cell reference (e.g. 'B5')"
                },
                "sheet": {
                    "type": "string",
                    "description": "Tab name (defaults to first sheet)"
                },
                "depth": {
                    "type": "integer",
                    "description": "Max recursion depth (default 5)"
                }
            },
            "required": ["spreadsheet_id", "cell"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "cell": { "type": "string" },
                "formula": { "type": "string" },
                "type": { "type": "string" },
                "deps": { "type": "array" }
            }
        }
    })
}

pub fn sheets_explain_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_explain",
        "title": "Explain Cell Formula",
        "description": "Explain a spreadsheet formula in plain English using column headers and row labels.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": {
                    "type": "string",
                    "description": "Spreadsheet ID"
                },
                "cell": {
                    "type": "string",
                    "description": "Cell reference (e.g. 'B5')"
                },
                "sheet": {
                    "type": "string",
                    "description": "Tab name (defaults to first sheet)"
                }
            },
            "required": ["spreadsheet_id", "cell"]
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "cell": { "type": "string" },
                "formula": { "type": "string" },
                "explanation": { "type": "string" },
                "referenced_cells": { "type": "array" }
            }
        }
    })
}

pub fn sheets_format_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_format",
        "title": "Conditional Formatting",
        "description": "Add, delete, or list conditional formatting rules on a spreadsheet.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": { "type": "string", "description": "Spreadsheet ID" },
                "action": { "type": "string", "enum": ["add", "delete", "list"], "description": "add, delete, or list" },
                "range": { "type": "string", "description": "A1 range (e.g. 'E2:E10')" },
                "rule": { "type": "object", "description": "Rule: {\"type\":\"NUMBER_GREATER\",\"values\":[\"90\"],\"format\":{\"backgroundColor\":{\"red\":0,\"green\":1,\"blue\":0}}}" },
                "sheet": { "type": "string", "description": "Tab name (default: first tab)" },
                "sheet_id": { "type": "integer", "description": "Tab ID (alternative to sheet name)" },
                "index": { "type": "integer", "description": "Rule index for delete (0-based)" }
            },
            "required": ["spreadsheet_id", "action"]
        }
    })
}

pub fn sheets_validate_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_validate",
        "title": "Data Validation",
        "description": "Set dropdown or validation rules on spreadsheet cells.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": { "type": "string", "description": "Spreadsheet ID" },
                "action": { "type": "string", "enum": ["set", "clear", "list"], "description": "set, clear, or list" },
                "range": { "type": "string", "description": "A1 range (e.g. 'F2:F100')" },
                "rule": { "type": "object", "description": "Rule: {\"type\":\"ONE_OF_LIST\",\"values\":[\"Yes\",\"No\"],\"strict\":true}" },
                "sheet": { "type": "string", "description": "Tab name (default: first tab)" },
                "sheet_id": { "type": "integer", "description": "Tab ID (alternative to sheet name)" }
            },
            "required": ["spreadsheet_id", "action"]
        }
    })
}

pub fn sheets_named_range_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_named_range",
        "title": "Named Ranges",
        "description": "Create, delete, or list named ranges in a spreadsheet.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": { "type": "string", "description": "Spreadsheet ID" },
                "action": { "type": "string", "enum": ["create", "delete", "list", "read"], "description": "create, delete, list, or read" },
                "name": { "type": "string", "description": "Range name (e.g. 'SalesData')" },
                "range": { "type": "string", "description": "A1 range (e.g. 'A1:F10') for create" },
                "sheet": { "type": "string", "description": "Tab name (default: first tab)" },
                "sheet_id": { "type": "integer", "description": "Tab ID (alternative to sheet name)" },
                "named_range_id": { "type": "string", "description": "ID from list (for delete)" }
            },
            "required": ["spreadsheet_id", "action"]
        }
    })
}

pub fn sheets_csv_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_csv",
        "title": "CSV Import/Export",
        "description": "Export spreadsheet data as CSV string or import CSV data into a spreadsheet.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": { "type": "string", "description": "Spreadsheet ID" },
                "action": { "type": "string", "enum": ["export", "import"], "description": "export or import" },
                "sheet": { "type": "string", "description": "Tab name (defaults to Sheet1)" },
                "data": { "type": "string", "description": "CSV string (for import)" },
                "separator": { "type": "string", "description": "Field separator (default: comma)" }
            },
            "required": ["spreadsheet_id", "action"]
        }
    })
}

pub fn sheets_dimensions_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_dimensions",
        "title": "Row/Column Management",
        "description": "Insert, append, or delete rows/columns in a spreadsheet.",
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": { "type": "string", "description": "Spreadsheet ID" },
                "action": { "type": "string", "enum": ["insert", "append", "delete", "move", "resize"], "description": "insert, append, delete, move, or resize" },
                "dimension": { "type": "string", "enum": ["ROWS", "COLUMNS"], "description": "ROWS or COLUMNS (default: ROWS)" },
                "start": { "type": "integer", "description": "Start position (0-based). Row 6 = index 5." },
                "count": { "type": "integer", "description": "Number to insert/append (default 1)" },
                "end": { "type": "integer", "description": "End position (exclusive, for delete)" },
                "sheet": { "type": "string", "description": "Tab name (default: first tab)" },
                "sheet_id": { "type": "integer", "description": "Tab ID (alternative to sheet name)" },
                "size": { "type": "integer", "description": "Pixel size (for resize)" },
                "destination": { "type": "integer", "description": "Destination index (for move)" }
            },
            "required": ["spreadsheet_id", "action"]
        }
    })
}

pub fn sheets_formulas_tool_schema() -> Value {
    json!({
        "name": "gws_sheets_formulas",
        "title": "Dump All Formulas",
        "description": "List all formulas in a spreadsheet tab. Shows which columns have formulas and their patterns.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "spreadsheet_id": { "type": "string", "description": "Spreadsheet ID" },
                "sheet": { "type": "string", "description": "Tab name (defaults to Sheet1)" },
                "range": { "type": "string", "description": "A1 range (defaults to all data)" }
            },
            "required": ["spreadsheet_id"]
        }
    })
}

pub fn a1_to_grid_range(range: &str, sheet_id: i64) -> Value {
    let parts: Vec<&str> = range.split(':').collect();
    let (start_col, start_row) = parse_cell_ref(parts[0]);
    if parts.len() == 1 {
        return json!({
            "sheetId": sheet_id,
            "startRowIndex": start_row,
            "endRowIndex": start_row + 1,
            "startColumnIndex": start_col,
            "endColumnIndex": start_col + 1
        });
    }
    let (end_col, end_row) = parse_cell_ref(parts[1]);
    json!({
        "sheetId": sheet_id,
        "startRowIndex": start_row,
        "endRowIndex": end_row + 1,
        "startColumnIndex": start_col,
        "endColumnIndex": end_col + 1
    })
}

fn parse_cell_ref(cell: &str) -> (i64, i64) {
    let col_end = cell.chars().take_while(|c| c.is_ascii_uppercase()).count();
    let col_letters = &cell[..col_end];
    let row: i64 = cell[col_end..].parse().unwrap_or(1) - 1;
    let col = col_letters
        .bytes()
        .fold(0i64, |acc, b| acc * 26 + (b - b'A') as i64 + 1)
        - 1;
    (col.max(0), row.max(0))
}

pub fn build_conditional_format_request(
    action: &str,
    sheet_id: Option<i64>,
    range: Option<&str>,
    rule: Option<&Value>,
    index: Option<i64>,
) -> Result<Value, String> {
    match action {
        "add" => {
            let sid = sheet_id.ok_or("Missing sheet_id for add")?;
            let r = range.ok_or("Missing range for add")?;
            let rl = rule.ok_or("Missing rule for add")?;
            let grid = a1_to_grid_range(r, sid);
            let condition_type = rl.get("type").and_then(|v| v.as_str()).unwrap_or("CUSTOM_FORMULA");
            let values: Vec<Value> = rl.get("values")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().map(|v| json!({"userEnteredValue": v})).collect())
                .unwrap_or_default();
            let format = rl.get("format").cloned().unwrap_or(json!({}));
            Ok(json!({
                "body": { "requests": [{
                    "addConditionalFormatRule": {
                        "rule": {
                            "ranges": [grid],
                            "booleanRule": {
                                "condition": { "type": condition_type, "values": values },
                                "format": format
                            }
                        },
                        "index": index.unwrap_or(0)
                    }
                }]}
            }))
        }
        "delete" => {
            let idx = index.ok_or("Missing index for delete")?;
            let sid = sheet_id.ok_or("Missing sheet_id for delete")?;
            Ok(json!({
                "body": { "requests": [{
                    "deleteConditionalFormatRule": { "sheetId": sid, "index": idx }
                }]}
            }))
        }
        _ => Err(format!("Use action 'add', 'delete', or 'list'. Got: {action}")),
    }
}

pub fn build_data_validation_request(
    action: &str,
    sheet_id: Option<i64>,
    range: Option<&str>,
    rule: Option<&Value>,
) -> Result<Value, String> {
    match action {
        "set" => {
            let sid = sheet_id.ok_or("Missing sheet_id for set")?;
            let r = range.ok_or("Missing range for set")?;
            let rl = rule.ok_or("Missing rule for set")?;
            let grid = a1_to_grid_range(r, sid);
            let condition_type = rl.get("type").and_then(|v| v.as_str()).unwrap_or("ONE_OF_LIST");
            let values: Vec<Value> = rl.get("values")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().map(|v| json!({"userEnteredValue": v})).collect())
                .unwrap_or_default();
            let strict = rl.get("strict").and_then(|v| v.as_bool()).unwrap_or(true);
            let message = rl.get("message").and_then(|v| v.as_str());
            let mut dv_rule = json!({
                "condition": { "type": condition_type, "values": values },
                "strict": strict,
                "showCustomUi": true
            });
            if let Some(msg) = message {
                dv_rule["inputMessage"] = json!(msg);
            }
            Ok(json!({
                "body": { "requests": [{
                    "setDataValidation": {
                        "range": grid,
                        "rule": dv_rule
                    }
                }]}
            }))
        }
        "clear" => {
            let sid = sheet_id.ok_or("Missing sheet_id for clear")?;
            let r = range.ok_or("Missing range for clear")?;
            let grid = a1_to_grid_range(r, sid);
            Ok(json!({
                "body": { "requests": [{
                    "setDataValidation": { "range": grid }
                }]}
            }))
        }
        _ => Err(format!("Use action 'set', 'clear', or 'list'. Got: {action}")),
    }
}

pub fn build_named_range_request(
    action: &str,
    name: Option<&str>,
    sheet_id: Option<i64>,
    range: Option<&str>,
    named_range_id: Option<&str>,
) -> Result<Value, String> {
    match action {
        "create" => {
            let n = name.ok_or("Missing name for create")?;
            let sid = sheet_id.ok_or("Missing sheet_id for create")?;
            let r = range.ok_or("Missing range for create")?;
            let grid = a1_to_grid_range(r, sid);
            Ok(json!({
                "body": { "requests": [{
                    "addNamedRange": {
                        "namedRange": { "name": n, "range": grid }
                    }
                }]}
            }))
        }
        "delete" => {
            let nid = named_range_id.ok_or("Missing named_range_id for delete")?;
            Ok(json!({
                "body": { "requests": [{
                    "deleteNamedRange": { "namedRangeId": nid }
                }]}
            }))
        }
        _ => Err(format!("Use action 'create', 'delete', 'list', or 'read'. Got: {action}")),
    }
}

pub fn build_dimension_request(
    action: &str,
    sheet_id: Option<i64>,
    dimension: Option<&str>,
    start: Option<i64>,
    end: Option<i64>,
    count: Option<i64>,
    size: Option<i64>,
    destination: Option<i64>,
) -> Result<Value, String> {
    let sid = sheet_id.ok_or("Missing sheet_id")?;
    let dim = dimension.unwrap_or("ROWS");
    match action {
        "insert" => {
            let s = start.ok_or("Missing start index for insert")?;
            let c = count.unwrap_or(1);
            Ok(json!({
                "body": { "requests": [{
                    "insertDimension": {
                        "range": { "sheetId": sid, "dimension": dim, "startIndex": s, "endIndex": s + c },
                        "inheritFromBefore": s > 0
                    }
                }]}
            }))
        }
        "append" => {
            let c = count.unwrap_or(1);
            Ok(json!({
                "body": { "requests": [{
                    "appendDimension": { "sheetId": sid, "dimension": dim, "length": c }
                }]}
            }))
        }
        "delete" => {
            let s = start.ok_or("Missing start index for delete")?;
            let e = end.ok_or("Missing end index for delete")?;
            Ok(json!({
                "body": { "requests": [{
                    "deleteDimension": {
                        "range": { "sheetId": sid, "dimension": dim, "startIndex": s, "endIndex": e }
                    }
                }]}
            }))
        }
        "move" => {
            let s = start.ok_or("Missing start index for move")?;
            let e = end.unwrap_or(s + 1);
            let d = destination.ok_or("Missing destination for move")?;
            Ok(json!({
                "body": { "requests": [{
                    "moveDimension": {
                        "source": { "sheetId": sid, "dimension": dim, "startIndex": s, "endIndex": e },
                        "destinationIndex": d
                    }
                }]}
            }))
        }
        "resize" => {
            let s = start.ok_or("Missing start index for resize")?;
            let e = end.unwrap_or(s + 1);
            let px = size.ok_or("Missing size (pixels) for resize")?;
            Ok(json!({
                "body": { "requests": [{
                    "updateDimensionProperties": {
                        "range": { "sheetId": sid, "dimension": dim, "startIndex": s, "endIndex": e },
                        "properties": { "pixelSize": px },
                        "fields": "pixelSize"
                    }
                }]}
            }))
        }
        _ => Err(format!("Use action 'insert', 'append', 'delete', 'move', or 'resize'. Got: {action}")),
    }
}

pub fn csv_to_values(csv: &str, separator: char) -> Vec<Vec<String>> {
    csv.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = Vec::new();
            let mut current = String::new();
            let mut in_quotes = false;
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '"' {
                    if in_quotes && chars.peek() == Some(&'"') {
                        current.push('"');
                        chars.next();
                    } else {
                        in_quotes = !in_quotes;
                    }
                } else if c == separator && !in_quotes {
                    fields.push(current.trim().to_string());
                    current = String::new();
                } else {
                    current.push(c);
                }
            }
            fields.push(current.trim().to_string());
            fields
        })
        .collect()
}

pub fn values_to_csv(values: &[Vec<String>], separator: char) -> String {
    values
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    if cell.contains(separator) || cell.contains('"') || cell.contains('\n') {
                        format!("\"{}\"", cell.replace('"', "\"\""))
                    } else {
                        cell.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(&separator.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
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

    #[test]
    fn normalize_array_of_arrays_passthrough() {
        let data = json!([["A", "B"], [1, 2]]);
        assert_eq!(normalize_data(&data), data);
    }

    #[test]
    fn normalize_array_of_objects() {
        let data = json!([
            {"Name": "Alice", "Score": 95},
            {"Name": "Bob", "Score": 78}
        ]);
        let result = normalize_data(&data);
        let rows = result.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].as_array().unwrap().contains(&json!("Name")));
        assert!(rows[0].as_array().unwrap().contains(&json!("Score")));
    }

    #[test]
    fn normalize_flat_array() {
        let data = json!(["Alice", "Bob", "Charlie"]);
        let result = normalize_data(&data);
        let rows = result.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], json!(["Alice"]));
    }

    #[test]
    fn normalize_empty_array() {
        let data = json!([]);
        assert_eq!(normalize_data(&data), data);
    }

    #[test]
    fn extract_simple_refs() {
        let refs = extract_cell_references("=A1+B2");
        assert_eq!(refs, vec!["A1", "B2"]);
    }

    #[test]
    fn extract_range_ref() {
        let refs = extract_cell_references("=SUM(A1:A10)");
        assert_eq!(refs, vec!["A1:A10"]);
    }

    #[test]
    fn extract_mixed_refs() {
        let refs = extract_cell_references("=IF(A1>0,B2,C3)");
        assert_eq!(refs, vec!["A1", "B2", "C3"]);
    }

    #[test]
    fn extract_no_function_names() {
        let refs = extract_cell_references("=SUM(A1)");
        assert!(refs.contains(&"A1".to_string()));
        assert!(!refs.contains(&"SUM".to_string()));
    }

    #[test]
    fn extract_skips_quoted_strings() {
        let refs = extract_cell_references("=IF(A1=\"B2\",C3,D4)");
        assert!(!refs.contains(&"B2".to_string()));
        assert!(refs.contains(&"A1".to_string()));
        assert!(refs.contains(&"C3".to_string()));
    }

    #[test]
    fn explain_replaces_refs() {
        let headers = vec!["Name".into(), "Score".into(), "Status".into()];
        let row_labels = vec!["Alice".into(), "Bob".into()];
        let result = explain_formula("=B2+B3", &headers, &row_labels);
        assert!(result.contains("Score"));
        assert!(result.contains("Alice"));
    }

    #[test]
    fn explain_translates_functions() {
        let result = explain_formula("=SUM(A1:A10)", &[], &[]);
        assert!(result.contains("sum of"));
    }

    #[test]
    fn resolve_cell_name_basic() {
        let headers = vec!["Name".into(), "Score".into()];
        let row_labels = vec!["Alice".into()];
        let (col, row) = resolve_cell_name("B2", &headers, &row_labels).unwrap();
        assert_eq!(col, "Score");
        assert_eq!(row, "Alice");
    }

    #[test]
    fn csv_roundtrip() {
        let values = vec![
            vec!["Name".into(), "Score".into()],
            vec!["Alice".into(), "95".into()],
        ];
        let csv = values_to_csv(&values, ',');
        assert_eq!(csv, "Name,Score\nAlice,95");
        let parsed = csv_to_values(&csv, ',');
        assert_eq!(parsed, values);
    }

    #[test]
    fn csv_with_quotes() {
        let values = vec![vec!["Hello, World".into(), "normal".into()]];
        let csv = values_to_csv(&values, ',');
        assert!(csv.contains("\"Hello, World\""));
        let parsed = csv_to_values(&csv, ',');
        assert_eq!(parsed[0][0], "Hello, World");
    }

    #[test]
    fn a1_to_grid_single_cell() {
        let grid = a1_to_grid_range("B2", 0);
        assert_eq!(grid["startColumnIndex"], 1);
        assert_eq!(grid["startRowIndex"], 1);
        assert_eq!(grid["endColumnIndex"], 2);
        assert_eq!(grid["endRowIndex"], 2);
    }

    #[test]
    fn a1_to_grid_range_multi() {
        let grid = a1_to_grid_range("A1:C5", 42);
        assert_eq!(grid["sheetId"], 42);
        assert_eq!(grid["startColumnIndex"], 0);
        assert_eq!(grid["startRowIndex"], 0);
        assert_eq!(grid["endColumnIndex"], 3);
        assert_eq!(grid["endRowIndex"], 5);
    }

    #[test]
    fn build_dimension_insert() {
        let req = build_dimension_request("insert", Some(0), Some("ROWS"), Some(5), None, Some(3), None, None).unwrap();
        let insert = &req["body"]["requests"][0]["insertDimension"];
        assert_eq!(insert["range"]["startIndex"], 5);
        assert_eq!(insert["range"]["endIndex"], 8);
        assert_eq!(insert["range"]["dimension"], "ROWS");
    }

    #[test]
    fn build_cond_format_add() {
        let rule = json!({"type": "NUMBER_GREATER", "values": ["90"], "format": {"backgroundColor": {"red": 0, "green": 1, "blue": 0}}});
        let req = build_conditional_format_request("add", Some(0), Some("A1:A10"), Some(&rule), None).unwrap();
        assert!(req["body"]["requests"][0]["addConditionalFormatRule"].is_object());
    }

    #[test]
    fn build_data_validation_set() {
        let rule = json!({"type": "ONE_OF_LIST", "values": ["Yes", "No"], "strict": true});
        let req = build_data_validation_request("set", Some(0), Some("B2:B100"), Some(&rule)).unwrap();
        assert!(req["body"]["requests"][0]["setDataValidation"].is_object());
    }

    #[test]
    fn resolve_cell_name_header_row() {
        let headers = vec!["Name".into()];
        let (col, row) = resolve_cell_name("A1", &headers, &[]).unwrap();
        assert_eq!(col, "Name");
        assert_eq!(row, "");
    }
}
