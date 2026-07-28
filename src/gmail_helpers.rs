use base64::Engine;
use serde_json::{Value, json};

pub fn gmail_search_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_search",
        "title": "Search Email",
        "description": "Search Gmail messages. Uses Gmail search syntax (from:, to:, subject:, etc).",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Gmail search query (e.g. 'from:alice subject:report is:unread')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum messages to return",
                    "default": 20
                }
            }
        },
        "outputSchema": {
            "type": "object",
            "properties": {
                "messages": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "threadId": { "type": "string" },
                            "subject": { "type": "string" },
                            "from": { "type": "string" },
                            "to": { "type": "string" },
                            "date": { "type": "string" },
                            "snippet": { "type": "string" }
                        },
                        "required": ["id", "threadId"]
                    }
                },
                "resultSizeEstimate": { "type": "integer" }
            },
            "required": ["messages"]
        }
    })
}

pub fn gmail_read_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_read",
        "title": "Read Email",
        "description": "Read a Gmail message. Returns decoded body, headers, and attachment list.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "Message ID from search results"
                },
                "format": {
                    "type": "string",
                    "enum": ["full", "metadata", "minimal"],
                    "description": "Response detail level (default: full)"
                }
            },
            "required": ["message_id"]
        }
    })
}

pub fn gmail_draft_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_draft",
        "title": "Create Draft",
        "description": "Create a draft (saved, NOT sent). Use this to compose without sending.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient email address"
                },
                "subject": {
                    "type": "string",
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "description": "Email body (plain text, or markdown if format=markdown)"
                },
                "format": {
                    "type": "string",
                    "enum": ["plain", "markdown"],
                    "description": "Body format: plain (default) or markdown (converted to HTML email)"
                },
                "cc": {
                    "type": "string",
                    "description": "CC recipients (comma-separated)"
                },
                "bcc": {
                    "type": "string",
                    "description": "BCC recipients (comma-separated)"
                }
            },
            "required": ["to", "subject", "body"]
        }
    })
}

pub fn gmail_send_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_send",
        "title": "Send Email",
        "description": "Send an email immediately. Provide draft_id to send a saved draft, or to/subject/body.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "draft_id": {
                    "type": "string",
                    "description": "Draft ID to send (from gws_gmail_draft result)"
                },
                "to": {
                    "type": "string",
                    "description": "Recipient email address"
                },
                "subject": {
                    "type": "string",
                    "description": "Email subject line"
                },
                "body": {
                    "type": "string",
                    "description": "Email body (plain text, or markdown if format=markdown)"
                },
                "format": {
                    "type": "string",
                    "enum": ["plain", "markdown"],
                    "description": "Body format: plain (default) or markdown (converted to HTML email)"
                },
                "cc": {
                    "type": "string",
                    "description": "CC recipients (comma-separated)"
                },
                "bcc": {
                    "type": "string",
                    "description": "BCC recipients (comma-separated)"
                }
            }
        }
    })
}

pub fn gmail_reply_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_reply",
        "title": "Reply to Email",
        "description": "Send a reply immediately in a thread. To draft without sending, use gws_gmail_draft.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "Message ID to reply to"
                },
                "body": {
                    "type": "string",
                    "description": "Reply body (plain text, or markdown if format=markdown)"
                },
                "format": {
                    "type": "string",
                    "enum": ["plain", "markdown"],
                    "description": "Body format: plain (default) or markdown (converted to HTML email)"
                },
                "to": {
                    "type": "string",
                    "description": "Override reply-to address (default: original sender)"
                },
                "cc": {
                    "type": "string",
                    "description": "CC recipients (comma-separated)"
                }
            },
            "required": ["message_id", "body"]
        }
    })
}

pub fn gmail_thread_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_thread",
        "title": "Read Thread",
        "description": "Read all messages in a thread as a chronological conversation.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "thread_id": {
                    "type": "string",
                    "description": "Thread ID (from search results or a message's threadId)"
                }
            },
            "required": ["thread_id"]
        }
    })
}

pub fn gmail_attachment_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_attachment",
        "title": "Get Attachment",
        "description": "Get attachment content, or save it to Drive with folder_id.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "Message ID containing the attachment"
                },
                "attachment_id": {
                    "type": "string",
                    "description": "Attachment ID from gws_gmail_read results"
                },
                "folder_id": {
                    "type": "string",
                    "description": "Drive folder ID — saves attachment to Drive instead of returning content"
                }
            },
            "required": ["message_id", "attachment_id"]
        }
    })
}

pub fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/xml"
        || mime == "application/ics"
        || mime == "application/csv"
        || mime == "application/javascript"
        || mime == "application/x-yaml"
        || mime == "application/yaml"
}

pub fn gmail_contacts_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_contacts",
        "title": "Find Contact",
        "description": "Find a contact's email by name. Searches Google Contacts with prefix matching.",
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Name to search for (prefix match, e.g. 'Alice' or 'Smith')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum contacts to return (default: 5, max: 30)"
                }
            },
            "required": ["query"]
        }
    })
}

pub fn gmail_forward_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_forward",
        "title": "Forward Email",
        "description": "Forward a message to another recipient with optional comment.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "Message ID to forward"
                },
                "to": {
                    "type": "string",
                    "description": "Recipient email address"
                },
                "comment": {
                    "type": "string",
                    "description": "Optional comment above the forwarded message"
                },
                "cc": {
                    "type": "string",
                    "description": "CC recipients (comma-separated)"
                }
            },
            "required": ["message_id", "to"]
        }
    })
}

pub fn gmail_labels_tool_schema() -> Value {
    json!({
        "name": "gws_gmail_labels",
        "title": "Manage Labels",
        "description": "List labels or add/remove labels on a message.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "remove"],
                    "description": "Action to perform (default: list)"
                },
                "message_id": {
                    "type": "string",
                    "description": "Message ID (required for add/remove)"
                },
                "label": {
                    "type": "string",
                    "description": "Label name (e.g. STARRED, IMPORTANT, or custom label name)"
                }
            }
        }
    })
}

pub fn build_rfc2822_message(
    to: &str,
    cc: Option<&str>,
    bcc: Option<&str>,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
    references: Option<&str>,
    html_body: Option<&str>,
) -> String {
    let mut msg = String::new();
    msg.push_str("MIME-Version: 1.0\r\n");
    msg.push_str(&format!("To: {to}\r\n"));
    if let Some(cc) = cc {
        msg.push_str(&format!("Cc: {cc}\r\n"));
    }
    if let Some(bcc) = bcc {
        msg.push_str(&format!("Bcc: {bcc}\r\n"));
    }
    msg.push_str(&format!("Subject: {subject}\r\n"));
    if let Some(irt) = in_reply_to {
        msg.push_str(&format!("In-Reply-To: {irt}\r\n"));
    }
    if let Some(refs) = references {
        msg.push_str(&format!("References: {refs}\r\n"));
    }

    if let Some(html) = html_body {
        let boundary = "----=_Part_MCP_GWS_boundary";
        msg.push_str(&format!(
            "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n"
        ));
        msg.push_str("\r\n");
        msg.push_str(&format!("--{boundary}\r\n"));
        msg.push_str("Content-Type: text/plain; charset=\"UTF-8\"\r\n\r\n");
        msg.push_str(body);
        msg.push_str(&format!("\r\n--{boundary}\r\n"));
        msg.push_str("Content-Type: text/html; charset=\"UTF-8\"\r\n\r\n");
        msg.push_str(html);
        msg.push_str(&format!("\r\n--{boundary}--\r\n"));
    } else {
        msg.push_str("Content-Type: text/plain; charset=\"UTF-8\"\r\n");
        msg.push_str("\r\n");
        msg.push_str(body);
    }

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(msg.as_bytes())
}

pub fn decode_message_body(payload: &Value) -> String {
    if let Some(body_data) = payload
        .get("body")
        .and_then(|b| b.get("data"))
        .and_then(|d| d.as_str())
    {
        if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body_data) {
            if let Ok(text) = String::from_utf8(decoded) {
                return text;
            }
        }
    }

    if let Some(parts) = payload.get("parts").and_then(|p| p.as_array()) {
        for part in parts {
            let mime = part.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if mime == "text/plain" {
                if let Some(data) = part
                    .get("body")
                    .and_then(|b| b.get("data"))
                    .and_then(|d| d.as_str())
                {
                    if let Ok(decoded) =
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
                    {
                        if let Ok(text) = String::from_utf8(decoded) {
                            return text;
                        }
                    }
                }
            }
        }
        for part in parts {
            let mime = part.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if mime == "text/html" {
                if let Some(data) = part
                    .get("body")
                    .and_then(|b| b.get("data"))
                    .and_then(|d| d.as_str())
                {
                    if let Ok(decoded) =
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
                    {
                        if let Ok(text) = String::from_utf8(decoded) {
                            return strip_html_tags(&text);
                        }
                    }
                }
            }
        }
        for part in parts {
            let nested = decode_message_body(part);
            if !nested.is_empty() {
                return nested;
            }
        }
    }

    String::new()
}

fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

pub fn extract_headers(headers: &Value, names: &[&str]) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    if let Some(arr) = headers.as_array() {
        for header in arr {
            if let (Some(name), Some(value)) = (
                header.get("name").and_then(|n| n.as_str()),
                header.get("value").and_then(|v| v.as_str()),
            ) {
                let lower = name.to_lowercase();
                for &target in names {
                    if lower == target.to_lowercase() {
                        map.insert(target.to_lowercase(), json!(value));
                    }
                }
            }
        }
    }
    map
}

pub fn resolve_label_id(label_name: &str, labels: &Value) -> Option<String> {
    let system_labels = [
        "INBOX",
        "SENT",
        "DRAFT",
        "TRASH",
        "SPAM",
        "STARRED",
        "UNREAD",
        "IMPORTANT",
        "CATEGORY_PERSONAL",
        "CATEGORY_SOCIAL",
        "CATEGORY_PROMOTIONS",
        "CATEGORY_UPDATES",
        "CATEGORY_FORUMS",
    ];
    let upper = label_name.to_uppercase();
    if system_labels.contains(&upper.as_str()) {
        return Some(upper);
    }

    if let Some(arr) = labels.get("labels").and_then(|l| l.as_array()) {
        for label in arr {
            if let (Some(name), Some(id)) = (
                label.get("name").and_then(|n| n.as_str()),
                label.get("id").and_then(|i| i.as_str()),
            ) {
                if name.eq_ignore_ascii_case(label_name) {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

pub fn list_attachments(payload: &Value) -> Vec<Value> {
    let mut attachments = Vec::new();
    collect_attachments(payload, &mut attachments);
    attachments
}

fn collect_attachments(part: &Value, out: &mut Vec<Value>) {
    if let Some(filename) = part.get("filename").and_then(|f| f.as_str()) {
        if !filename.is_empty() {
            let size = part
                .get("body")
                .and_then(|b| b.get("size"))
                .and_then(|s| s.as_u64())
                .unwrap_or(0);
            let mime = part
                .get("mimeType")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            let attachment_id = part
                .get("body")
                .and_then(|b| b.get("attachmentId"))
                .and_then(|a| a.as_str());
            let mut entry = json!({
                "filename": filename,
                "mimeType": mime,
                "size": size
            });
            if let Some(aid) = attachment_id {
                entry["attachmentId"] = json!(aid);
            }
            out.push(entry);
        }
    }
    if let Some(parts) = part.get("parts").and_then(|p| p.as_array()) {
        for p in parts {
            collect_attachments(p, out);
        }
    }
}

pub fn strip_quoted_text(body: &str) -> String {
    let mut lines = Vec::new();
    for line in body.lines() {
        if line.starts_with('>') {
            continue;
        }
        if line.starts_with("On ") && line.contains(" wrote:") {
            break;
        }
        if line.trim() == "---------- Forwarded message ---------" {
            break;
        }
        lines.push(line);
    }
    let result = lines.join("\n");
    result.trim_end().to_string()
}

pub fn markdown_to_html(md: &str) -> String {
    let mut html = String::new();
    let mut in_code_block = false;
    let mut in_list = false;
    let mut list_ordered = false;

    for line in md.lines() {
        if line.starts_with("```") {
            if in_code_block {
                html.push_str("</code></pre>\n");
                in_code_block = false;
            } else {
                close_list(&mut html, &mut in_list, list_ordered);
                html.push_str("<pre style=\"background:#f4f4f4;padding:12px;border-radius:4px;font-family:monospace;font-size:13px;\"><code>");
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            html.push_str(&escape_html(line));
            html.push('\n');
            continue;
        }

        let trimmed = line.trim();

        if trimmed.is_empty() {
            close_list(&mut html, &mut in_list, list_ordered);
            continue;
        }

        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            if !in_list || list_ordered {
                close_list(&mut html, &mut in_list, list_ordered);
                html.push_str("<ul style=\"margin:8px 0;padding-left:24px;\">\n");
                in_list = true;
                list_ordered = false;
            }
            html.push_str(&format!("<li>{}</li>\n", inline_format(rest)));
            continue;
        }

        if let Some((num_part, rest)) = trimmed.split_once(". ") {
            if num_part.chars().all(|c| c.is_ascii_digit()) && !num_part.is_empty() {
                if !in_list || !list_ordered {
                    close_list(&mut html, &mut in_list, list_ordered);
                    html.push_str("<ol style=\"margin:8px 0;padding-left:24px;\">\n");
                    in_list = true;
                    list_ordered = true;
                }
                html.push_str(&format!("<li>{}</li>\n", inline_format(rest)));
                continue;
            }
        }

        close_list(&mut html, &mut in_list, list_ordered);

        if let Some(rest) = trimmed.strip_prefix("### ") {
            html.push_str(&format!(
                "<h3 style=\"margin:16px 0 8px;font-size:16px;\">{}</h3>\n",
                inline_format(rest)
            ));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            html.push_str(&format!(
                "<h2 style=\"margin:16px 0 8px;font-size:18px;\">{}</h2>\n",
                inline_format(rest)
            ));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            html.push_str(&format!(
                "<h1 style=\"margin:16px 0 8px;font-size:22px;\">{}</h1>\n",
                inline_format(rest)
            ));
        } else if trimmed == "---" || trimmed == "***" {
            html.push_str("<hr style=\"border:none;border-top:1px solid #ddd;margin:16px 0;\">\n");
        } else {
            html.push_str(&format!(
                "<p style=\"margin:8px 0;\">{}</p>\n",
                inline_format(trimmed)
            ));
        }
    }

    close_list(&mut html, &mut in_list, list_ordered);
    if in_code_block {
        html.push_str("</code></pre>\n");
    }

    html
}

fn close_list(html: &mut String, in_list: &mut bool, ordered: bool) {
    if *in_list {
        html.push_str(if ordered { "</ol>\n" } else { "</ul>\n" });
        *in_list = false;
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn inline_format(text: &str) -> String {
    let text = escape_html(text);
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, '`') {
                let code: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!(
                    "<code style=\"background:#f4f4f4;padding:2px 4px;border-radius:3px;font-size:13px;\">{code}</code>"
                ));
                i = end + 1;
                continue;
            }
        }

        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_closing_double(&chars, i + 2, '*') {
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&format!("<strong>{inner}</strong>"));
                i = end + 2;
                continue;
            }
        }

        if chars[i] == '*' || chars[i] == '_' {
            let marker = chars[i];
            if let Some(end) = find_closing(&chars, i + 1, marker) {
                let inner: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("<em>{inner}</em>"));
                i = end + 1;
                continue;
            }
        }

        if chars[i] == '[' {
            if let Some(link) = parse_link(&chars, i) {
                result.push_str(&format!(
                    "<a href=\"{}\" style=\"color:#1a73e8;\">{}</a>",
                    link.url, link.text
                ));
                i = link.end;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

fn find_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len() {
        if chars[i] == marker {
            return Some(i);
        }
    }
    None
}

fn find_closing_double(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len().saturating_sub(1) {
        if chars[i] == marker && chars[i + 1] == marker {
            return Some(i);
        }
    }
    None
}

struct LinkParse {
    text: String,
    url: String,
    end: usize,
}

fn parse_link(chars: &[char], start: usize) -> Option<LinkParse> {
    let close_bracket = find_closing(chars, start + 1, ']')?;
    if close_bracket + 1 >= chars.len() || chars[close_bracket + 1] != '(' {
        return None;
    }
    let close_paren = find_closing(chars, close_bracket + 2, ')')?;
    let text: String = chars[start + 1..close_bracket].iter().collect();
    let url: String = chars[close_bracket + 2..close_paren].iter().collect();
    Some(LinkParse {
        text,
        url,
        end: close_paren + 1,
    })
}

pub fn build_html_email_body(html_content: &str) -> String {
    format!(
        "<div style=\"font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;\
         font-size:14px;line-height:1.6;color:#333;\">{html_content}</div>"
    )
}

pub fn build_forward_body(
    comment: Option<&str>,
    orig_from: &str,
    orig_date: &str,
    orig_subject: &str,
    orig_to: &str,
    orig_body: &str,
) -> String {
    let mut body = String::new();
    if let Some(c) = comment {
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("---------- Forwarded message ---------\n");
    body.push_str(&format!("From: {orig_from}\n"));
    body.push_str(&format!("Date: {orig_date}\n"));
    body.push_str(&format!("Subject: {orig_subject}\n"));
    body.push_str(&format!("To: {orig_to}\n\n"));
    body.push_str(orig_body);
    body
}

pub fn check_label_policy(label_ids: &[&str], allowed: &[String]) -> Result<(), String> {
    if allowed.is_empty() {
        return Ok(());
    }
    for label in label_ids {
        if allowed.iter().any(|a| a.eq_ignore_ascii_case(label)) {
            return Ok(());
        }
    }
    Err(format!(
        "Message labels {:?} do not match allowed labels {:?}. \
         Policy restricts access to messages with at least one allowed label.",
        label_ids, allowed
    ))
}

pub fn check_label_target_policy(label_name: &str, allowed: &[String]) -> Result<(), String> {
    if allowed.is_empty() {
        return Ok(());
    }
    if allowed.iter().any(|a| a.eq_ignore_ascii_case(label_name)) {
        return Ok(());
    }
    Err(format!(
        "Label '{}' is not in the allowed labels {:?}. \
         Policy restricts label modifications to allowed labels only.",
        label_name, allowed
    ))
}

pub fn inject_label_query(query: &str, allowed_labels: &[String]) -> String {
    if allowed_labels.is_empty() {
        return query.to_string();
    }
    let label_filter = if allowed_labels.len() == 1 {
        format!("label:{}", allowed_labels[0])
    } else {
        let parts: Vec<String> = allowed_labels
            .iter()
            .map(|l| format!("label:{l}"))
            .collect();
        format!("({})", parts.join(" OR "))
    };
    if query.is_empty() {
        label_filter
    } else {
        format!("{query} {label_filter}")
    }
}

pub fn extract_label_ids(msg: &Value) -> Vec<&str> {
    msg.get("labelIds")
        .and_then(|l| l.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_schemas_have_short_descriptions() {
        let schemas = vec![
            gmail_search_tool_schema(),
            gmail_read_tool_schema(),
            gmail_draft_tool_schema(),
            gmail_send_tool_schema(),
            gmail_reply_tool_schema(),
            gmail_labels_tool_schema(),
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
        assert_eq!(schemas.len(), 6);
    }

    #[test]
    fn test_all_tool_names_start_with_gws_gmail() {
        let schemas = vec![
            gmail_search_tool_schema(),
            gmail_read_tool_schema(),
            gmail_draft_tool_schema(),
            gmail_send_tool_schema(),
            gmail_reply_tool_schema(),
            gmail_labels_tool_schema(),
        ];
        for schema in &schemas {
            let name = schema["name"].as_str().unwrap();
            assert!(
                name.starts_with("gws_gmail_"),
                "Tool name '{name}' must start with gws_gmail_"
            );
        }
    }

    #[test]
    fn test_build_rfc2822_basic() {
        let raw = build_rfc2822_message(
            "alice@example.com",
            None,
            None,
            "Hello",
            "Hi there",
            None,
            None,
            None,
        );
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&raw)
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("To: alice@example.com\r\n"));
        assert!(text.contains("Subject: Hello\r\n"));
        assert!(text.contains("\r\n\r\nHi there"));
        assert!(!text.contains("Cc:"));
        assert!(!text.contains("Bcc:"));
    }

    #[test]
    fn test_build_rfc2822_with_cc_bcc() {
        let raw = build_rfc2822_message(
            "alice@example.com",
            Some("bob@example.com"),
            Some("charlie@example.com"),
            "Test",
            "Body",
            None,
            None,
            None,
        );
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&raw)
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("Cc: bob@example.com\r\n"));
        assert!(text.contains("Bcc: charlie@example.com\r\n"));
    }

    #[test]
    fn test_build_rfc2822_reply_headers() {
        let raw = build_rfc2822_message(
            "alice@example.com",
            None,
            None,
            "Re: Hello",
            "Reply body",
            Some("<msg123@example.com>"),
            Some("<msg123@example.com>"),
            None,
        );
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&raw)
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("In-Reply-To: <msg123@example.com>\r\n"));
        assert!(text.contains("References: <msg123@example.com>\r\n"));
    }

    #[test]
    fn test_decode_body_direct() {
        let body_text = "Hello world";
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body_text.as_bytes());
        let payload = json!({
            "body": { "data": encoded }
        });
        assert_eq!(decode_message_body(&payload), "Hello world");
    }

    #[test]
    fn test_decode_body_multipart() {
        let plain = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"Plain text");
        let html = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"<b>HTML</b>");
        let payload = json!({
            "mimeType": "multipart/alternative",
            "parts": [
                { "mimeType": "text/plain", "body": { "data": plain } },
                { "mimeType": "text/html", "body": { "data": html } }
            ]
        });
        assert_eq!(decode_message_body(&payload), "Plain text");
    }

    #[test]
    fn test_decode_body_html_fallback() {
        let html = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"<p>Hello</p>");
        let payload = json!({
            "mimeType": "multipart/alternative",
            "parts": [
                { "mimeType": "text/html", "body": { "data": html } }
            ]
        });
        assert_eq!(decode_message_body(&payload), "Hello");
    }

    #[test]
    fn test_decode_body_empty() {
        let payload = json!({});
        assert_eq!(decode_message_body(&payload), "");
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>Hello <b>world</b></p>"), "Hello world");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags("<br/>line<br/>break"), "linebreak");
    }

    #[test]
    fn test_extract_headers() {
        let headers = json!([
            { "name": "From", "value": "alice@example.com" },
            { "name": "To", "value": "bob@example.com" },
            { "name": "Subject", "value": "Hello" },
            { "name": "Date", "value": "Mon, 1 Jan 2024 00:00:00 +0000" },
            { "name": "X-Custom", "value": "ignored" }
        ]);
        let map = extract_headers(&headers, &["From", "To", "Subject", "Date"]);
        assert_eq!(map.get("from").unwrap(), "alice@example.com");
        assert_eq!(map.get("to").unwrap(), "bob@example.com");
        assert_eq!(map.get("subject").unwrap(), "Hello");
        assert_eq!(map.get("date").unwrap(), "Mon, 1 Jan 2024 00:00:00 +0000");
        assert!(map.get("x-custom").is_none());
    }

    #[test]
    fn test_resolve_label_system() {
        let labels = json!({ "labels": [] });
        assert_eq!(
            resolve_label_id("INBOX", &labels),
            Some("INBOX".to_string())
        );
        assert_eq!(
            resolve_label_id("starred", &labels),
            Some("STARRED".to_string())
        );
        assert_eq!(
            resolve_label_id("Unread", &labels),
            Some("UNREAD".to_string())
        );
    }

    #[test]
    fn test_resolve_label_custom() {
        let labels = json!({
            "labels": [
                { "id": "Label_1", "name": "Work", "type": "user" },
                { "id": "Label_2", "name": "Personal", "type": "user" }
            ]
        });
        assert_eq!(
            resolve_label_id("Work", &labels),
            Some("Label_1".to_string())
        );
        assert_eq!(
            resolve_label_id("work", &labels),
            Some("Label_1".to_string())
        );
        assert_eq!(resolve_label_id("Unknown", &labels), None);
    }

    #[test]
    fn test_list_attachments() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                { "mimeType": "text/plain", "body": { "size": 100 } },
                {
                    "mimeType": "application/pdf",
                    "filename": "report.pdf",
                    "body": { "size": 50000, "attachmentId": "abc123" }
                },
                {
                    "mimeType": "image/png",
                    "filename": "photo.png",
                    "body": { "size": 12000, "attachmentId": "def456" }
                }
            ]
        });
        let atts = list_attachments(&payload);
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0]["filename"], "report.pdf");
        assert_eq!(atts[0]["size"], 50000);
        assert_eq!(atts[1]["filename"], "photo.png");
    }

    #[test]
    fn test_list_attachments_nested() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                {
                    "mimeType": "multipart/alternative",
                    "parts": [
                        { "mimeType": "text/plain", "body": { "size": 50 } }
                    ]
                },
                {
                    "mimeType": "application/zip",
                    "filename": "archive.zip",
                    "body": { "size": 999 }
                }
            ]
        });
        let atts = list_attachments(&payload);
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0]["filename"], "archive.zip");
    }

    #[test]
    fn test_check_label_policy_empty_allowed() {
        assert!(check_label_policy(&["INBOX", "UNREAD"], &[]).is_ok());
    }

    #[test]
    fn test_check_label_policy_match() {
        let allowed = vec!["GWS-MCP-Test".to_string()];
        assert!(check_label_policy(&["INBOX", "GWS-MCP-Test"], &allowed).is_ok());
    }

    #[test]
    fn test_check_label_policy_no_match() {
        let allowed = vec!["GWS-MCP-Test".to_string()];
        let result = check_label_policy(&["INBOX", "UNREAD"], &allowed);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("do not match allowed labels"));
    }

    #[test]
    fn test_check_label_policy_case_insensitive() {
        let allowed = vec!["gws-mcp-test".to_string()];
        assert!(check_label_policy(&["GWS-MCP-Test"], &allowed).is_ok());
    }

    #[test]
    fn test_check_label_target_policy_allowed() {
        let allowed = vec!["GWS-MCP-Test".to_string(), "STARRED".to_string()];
        assert!(check_label_target_policy("STARRED", &allowed).is_ok());
    }

    #[test]
    fn test_check_label_target_policy_denied() {
        let allowed = vec!["GWS-MCP-Test".to_string()];
        let result = check_label_target_policy("TRASH", &allowed);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in the allowed labels"));
    }

    #[test]
    fn test_check_label_target_policy_empty_allows_all() {
        assert!(check_label_target_policy("TRASH", &[]).is_ok());
    }

    #[test]
    fn test_inject_label_query_empty_labels() {
        assert_eq!(inject_label_query("from:alice", &[]), "from:alice");
    }

    #[test]
    fn test_inject_label_query_single_label() {
        let labels = vec!["GWS-MCP-Test".to_string()];
        assert_eq!(
            inject_label_query("from:alice", &labels),
            "from:alice label:GWS-MCP-Test"
        );
    }

    #[test]
    fn test_inject_label_query_multiple_labels() {
        let labels = vec!["Test".to_string(), "Dev".to_string()];
        assert_eq!(
            inject_label_query("is:unread", &labels),
            "is:unread (label:Test OR label:Dev)"
        );
    }

    #[test]
    fn test_inject_label_query_empty_query() {
        let labels = vec!["Test".to_string()];
        assert_eq!(inject_label_query("", &labels), "label:Test");
    }

    #[test]
    fn test_strip_quoted_basic() {
        let body = "Thanks for the update.\n\nOn Mon, Jan 1, 2024 Alice wrote:\n> Original message\n> More original";
        assert_eq!(strip_quoted_text(body), "Thanks for the update.");
    }

    #[test]
    fn test_strip_quoted_gt_lines() {
        let body = "My reply.\n> quoted line 1\n> quoted line 2\nAfter quote.";
        assert_eq!(strip_quoted_text(body), "My reply.\nAfter quote.");
    }

    #[test]
    fn test_strip_quoted_no_quotes() {
        let body = "Plain message with no quotes.";
        assert_eq!(strip_quoted_text(body), "Plain message with no quotes.");
    }

    #[test]
    fn test_strip_quoted_forwarded() {
        let body = "FYI see below.\n\n---------- Forwarded message ---------\nFrom: someone";
        assert_eq!(strip_quoted_text(body), "FYI see below.");
    }

    #[test]
    fn test_build_rfc2822_html() {
        let html = "<p>Hello <strong>world</strong></p>";
        let raw = build_rfc2822_message(
            "alice@example.com",
            None,
            None,
            "Test",
            "Hello world",
            None,
            None,
            Some(html),
        );
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&raw)
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        assert!(text.contains("multipart/alternative"));
        assert!(text.contains("text/plain"));
        assert!(text.contains("text/html"));
        assert!(text.contains("Hello world"));
        assert!(text.contains("<strong>world</strong>"));
    }

    #[test]
    fn test_markdown_to_html_headings() {
        let html = markdown_to_html("# Title\n## Subtitle\n### Section");
        assert!(html.contains("<h1"));
        assert!(html.contains("Title"));
        assert!(html.contains("<h2"));
        assert!(html.contains("<h3"));
    }

    #[test]
    fn test_markdown_to_html_bold_italic() {
        let html = markdown_to_html("This is **bold** and *italic*.");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn test_markdown_to_html_list() {
        let html = markdown_to_html("- Item 1\n- Item 2\n\n1. First\n2. Second");
        assert!(html.contains("<ul"));
        assert!(html.contains("<li>Item 1</li>"));
        assert!(html.contains("<ol"));
        assert!(html.contains("<li>First</li>"));
    }

    #[test]
    fn test_markdown_to_html_link() {
        let html = markdown_to_html("Visit [Google](https://google.com) for more.");
        assert!(html.contains("<a href=\"https://google.com\""));
        assert!(html.contains(">Google</a>"));
    }

    #[test]
    fn test_markdown_to_html_code() {
        let html = markdown_to_html("Use `git status` to check.\n\n```\nlet x = 1;\n```");
        assert!(html.contains("<code"));
        assert!(html.contains("git status"));
        assert!(html.contains("<pre"));
        assert!(html.contains("let x = 1;"));
    }

    #[test]
    fn test_markdown_to_html_escapes() {
        let html = markdown_to_html("Use <script> tags & \"quotes\"");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn test_gmail_forward_tool_schema() {
        let schema = gmail_forward_tool_schema();
        assert_eq!(schema["name"], "gws_gmail_forward");
        assert!(schema["description"].as_str().unwrap().len() < 100);
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("message_id")));
        assert!(required.contains(&json!("to")));
    }

    #[test]
    fn test_build_forward_body_with_comment() {
        let body = build_forward_body(
            Some("FYI — see below."),
            "alice@example.com",
            "Mon, 1 Jan 2024",
            "Original Subject",
            "bob@example.com",
            "Original body text.",
        );
        assert!(body.starts_with("FYI — see below."));
        assert!(body.contains("---------- Forwarded message ---------"));
        assert!(body.contains("From: alice@example.com"));
        assert!(body.contains("Subject: Original Subject"));
        assert!(body.contains("Original body text."));
    }

    #[test]
    fn test_build_forward_body_no_comment() {
        let body = build_forward_body(
            None,
            "alice@example.com",
            "Mon, 1 Jan 2024",
            "Test",
            "bob@example.com",
            "Body.",
        );
        assert!(body.starts_with("---------- Forwarded message ---------"));
    }

    #[test]
    fn test_gmail_contacts_tool_schema() {
        let schema = gmail_contacts_tool_schema();
        assert_eq!(schema["name"], "gws_gmail_contacts");
        assert!(schema["description"].as_str().unwrap().len() < 100);
        assert!(
            schema["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("query"))
        );
    }

    #[test]
    fn test_gmail_attachment_tool_schema() {
        let schema = gmail_attachment_tool_schema();
        assert_eq!(schema["name"], "gws_gmail_attachment");
        assert!(schema["description"].as_str().unwrap().len() < 100);
        let required = schema["inputSchema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("message_id")));
        assert!(required.contains(&json!("attachment_id")));
    }

    #[test]
    fn test_is_text_mime() {
        assert!(is_text_mime("text/plain"));
        assert!(is_text_mime("text/html"));
        assert!(is_text_mime("text/csv"));
        assert!(is_text_mime("application/json"));
        assert!(is_text_mime("application/xml"));
        assert!(is_text_mime("application/ics"));
        assert!(!is_text_mime("application/pdf"));
        assert!(!is_text_mime("image/png"));
        assert!(!is_text_mime("application/octet-stream"));
    }

    #[test]
    fn test_list_attachments_includes_attachment_id() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [{
                "mimeType": "application/pdf",
                "filename": "doc.pdf",
                "body": { "size": 1000, "attachmentId": "abc123" }
            }]
        });
        let atts = list_attachments(&payload);
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0]["attachmentId"], "abc123");
    }

    #[test]
    fn test_list_attachments_no_attachment_id() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [{
                "mimeType": "text/plain",
                "filename": "inline.txt",
                "body": { "size": 50, "data": "aGVsbG8=" }
            }]
        });
        let atts = list_attachments(&payload);
        assert_eq!(atts.len(), 1);
        assert!(atts[0].get("attachmentId").is_none());
    }

    #[test]
    fn test_gmail_thread_tool_schema() {
        let schema = gmail_thread_tool_schema();
        assert_eq!(schema["name"], "gws_gmail_thread");
        assert!(schema["description"].as_str().unwrap().len() < 100);
        assert!(
            schema["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("thread_id"))
        );
    }

    #[test]
    fn test_extract_label_ids() {
        let msg = json!({ "labelIds": ["INBOX", "UNREAD", "Label_1"] });
        assert_eq!(extract_label_ids(&msg), vec!["INBOX", "UNREAD", "Label_1"]);
    }

    #[test]
    fn test_extract_label_ids_missing() {
        let msg = json!({ "id": "abc" });
        let ids: Vec<&str> = extract_label_ids(&msg);
        assert!(ids.is_empty());
    }
}
