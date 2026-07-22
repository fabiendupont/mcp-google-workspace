---
name: work-with-spreadsheet
description: Full workflow for creating, reading, and formatting a Google Sheets spreadsheet
arguments:
  - name: title
    description: Spreadsheet title
    required: false
  - name: folder_id
    description: Drive folder ID to create the spreadsheet in
    required: false
---

## Creating a spreadsheet with data

Create a new spreadsheet and populate it in a single call:

```json
{
  "name": "gws_sheets_write",
  "arguments": {
    "title": "{{title|My Spreadsheet}}",
    "folder_id": "{{folder_id}}",
    "data": [
      ["Name", "Score", "Status"],
      ["Alice", 95, "Pass"],
      ["Bob", 78, "Pass"]
    ]
  }
}
```

The response returns `spreadsheetId` — use it for all subsequent calls.

## Reading data

```json
{
  "name": "gws_sheets_read",
  "arguments": { "spreadsheet_id": "SPREADSHEET_ID", "range": "A1:C10" }
}
```

To read formulas instead of values, add `"format": "formula"`.

## Writing formulas

Formulas are written as strings starting with `=`:

```json
{
  "name": "gws_sheets_write",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "range": "D1:D3",
    "data": [["Total"], ["=B2+C2"], ["=B3+C3"]]
  }
}
```

## Conditional formatting

Highlight cells based on their value. No `sheet_id` needed — defaults to the first tab.

```json
{
  "name": "gws_sheets_format",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "add",
    "range": "B2:B10",
    "rule": {
      "type": "NUMBER_GREATER",
      "values": ["90"],
      "format": { "backgroundColor": { "red": 0.8, "green": 1, "blue": 0.8 } }
    }
  }
}
```

Common rule types: `NUMBER_GREATER`, `NUMBER_LESS`, `TEXT_CONTAINS`, `CUSTOM_FORMULA`, `BLANK`, `NOT_BLANK`.

To list existing rules: `"action": "list"`.

## Data validation (dropdowns)

Restrict cells to a set of allowed values:

```json
{
  "name": "gws_sheets_validate",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "set",
    "range": "C2:C100",
    "rule": {
      "type": "ONE_OF_LIST",
      "values": ["Pass", "Fail", "Pending"],
      "strict": true
    }
  }
}
```

Common types: `ONE_OF_LIST` (dropdown), `NUMBER_BETWEEN`, `DATE_AFTER`, `CUSTOM_FORMULA`.

## Named ranges

Give a name to a cell range for easier reference:

```json
{
  "name": "gws_sheets_named_range",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "create",
    "name": "ScoreData",
    "range": "A1:C10"
  }
}
```

To list: `"action": "list"`. To delete: `"action": "delete", "named_range_id": "ID_FROM_LIST"`.

## Row and column management

Insert rows at a specific position (0-based index):

```json
{
  "name": "gws_sheets_dimensions",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "insert",
    "start": 5,
    "count": 2
  }
}
```

This inserts 2 rows at position 5 (after row 5). Defaults to ROWS on the first tab.

Other actions: `"append"` (add at end), `"delete"` (requires `start` and `end`), `"resize"` (requires `size` in pixels).

## CSV export and import

Export all data as a CSV string:

```json
{
  "name": "gws_sheets_csv",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "export"
  }
}
```

Import CSV data:

```json
{
  "name": "gws_sheets_csv",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "import",
    "data": "Name,Score\nAlice,95\nBob,78"
  }
}
```

## Formula analysis

List all formulas in a spreadsheet:

```json
{
  "name": "gws_sheets_formulas",
  "arguments": { "spreadsheet_id": "SPREADSHEET_ID" }
}
```

Explain a specific formula in plain English:

```json
{
  "name": "gws_sheets_explain",
  "arguments": { "spreadsheet_id": "SPREADSHEET_ID", "cell": "D2" }
}
```

Trace what cells feed into a formula:

```json
{
  "name": "gws_sheets_trace",
  "arguments": { "spreadsheet_id": "SPREADSHEET_ID", "cell": "D2" }
}
```

## Tab management

Get spreadsheet info (tab names, row/column counts):

```json
{
  "name": "gws_sheets_info",
  "arguments": { "spreadsheet_id": "SPREADSHEET_ID" }
}
```

Create, rename, or delete tabs:

```json
{
  "name": "gws_sheets_manage_tabs",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "action": "create",
    "title": "Summary"
  }
}
```
