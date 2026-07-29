use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::server::ServerState;
use crate::tools;
use base64::Engine;
use google_workspace::error::GwsError;
use serde_json::{Value, json};
use std::sync::Arc;

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
        && let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body_data)
        && let Ok(text) = String::from_utf8(decoded)
    {
        return text;
    }

    if let Some(parts) = payload.get("parts").and_then(|p| p.as_array()) {
        for part in parts {
            let mime = part.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if mime == "text/plain"
                && let Some(data) = part
                    .get("body")
                    .and_then(|b| b.get("data"))
                    .and_then(|d| d.as_str())
                && let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
                && let Ok(text) = String::from_utf8(decoded)
            {
                return text;
            }
        }
        for part in parts {
            let mime = part.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if mime == "text/html"
                && let Some(data) = part
                    .get("body")
                    .and_then(|b| b.get("data"))
                    .and_then(|d| d.as_str())
                && let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
                && let Ok(text) = String::from_utf8(decoded)
            {
                return strip_html_tags(&text);
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
            ) && name.eq_ignore_ascii_case(label_name)
            {
                return Some(id.to_string());
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
    if let Some(filename) = part.get("filename").and_then(|f| f.as_str())
        && !filename.is_empty()
    {
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

        if let Some((num_part, rest)) = trimmed.split_once(". ")
            && num_part.chars().all(|c| c.is_ascii_digit())
            && !num_part.is_empty()
        {
            if !in_list || !list_ordered {
                close_list(&mut html, &mut in_list, list_ordered);
                html.push_str("<ol style=\"margin:8px 0;padding-left:24px;\">\n");
                in_list = true;
                list_ordered = true;
            }
            html.push_str(&format!("<li>{}</li>\n", inline_format(rest)));
            continue;
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
        if chars[i] == '`'
            && let Some(end) = find_closing(&chars, i + 1, '`')
        {
            let code: String = chars[i + 1..end].iter().collect();
            result.push_str(&format!(
                    "<code style=\"background:#f4f4f4;padding:2px 4px;border-radius:3px;font-size:13px;\">{code}</code>"
                ));
            i = end + 1;
            continue;
        }

        if i + 1 < len
            && chars[i] == '*'
            && chars[i + 1] == '*'
            && let Some(end) = find_closing_double(&chars, i + 2, '*')
        {
            let inner: String = chars[i + 2..end].iter().collect();
            result.push_str(&format!("<strong>{inner}</strong>"));
            i = end + 2;
            continue;
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

        if chars[i] == '['
            && let Some(link) = parse_link(&chars, i)
        {
            result.push_str(&format!(
                "<a href=\"{}\" style=\"color:#1a73e8;\">{}</a>",
                link.url, link.text
            ));
            i = link.end;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

fn find_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    chars[start..]
        .iter()
        .position(|&c| c == marker)
        .map(|p| start + p)
}

fn find_closing_double(chars: &[char], start: usize, marker: char) -> Option<usize> {
    chars[start..]
        .windows(2)
        .position(|w| w[0] == marker && w[1] == marker)
        .map(|p| start + p)
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

async fn resolve_allowed_label_ids(
    allowed_labels: &[String],
    gmail_doc: &Arc<google_workspace::discovery::RestDescription>,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Vec<String>, GwsError> {
    if allowed_labels.is_empty() {
        return Ok(vec![]);
    }
    let labels_resource = tools::find_resource(&gmail_doc.resources, "users.labels")
        .ok_or_else(|| GwsError::Validation("users.labels resource not found".into()))?;
    let list_method = labels_resource
        .methods
        .get("list")
        .ok_or_else(|| GwsError::Validation("labels.list method not found".into()))?;
    let args = json!({ "params": { "userId": "me" } });
    let labels_result = crate::execute::execute_tool(
        gmail_doc,
        list_method,
        "users.labels",
        "list",
        &args,
        "gmail",
        policy,
        meta,
        None,
        None,
        false,
        &mut state.token_cache,
    )
    .await?;

    let mut ids = Vec::new();
    for name in allowed_labels {
        if let Some(id) = resolve_label_id(name, &labels_result) {
            ids.push(id);
        } else {
            tracing::warn!(label = %name, "Allowed label not found in Gmail — skipped");
        }
    }
    Ok(ids)
}

pub(crate) async fn execute_gmail_helper(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<Value, GwsError> {
    let gmail_doc = state.get_doc("gmail").await?;

    let allowed_labels = policy.allowed_labels("gmail");
    let allowed_label_ids =
        resolve_allowed_label_ids(allowed_labels, &gmail_doc, policy, meta, state).await?;

    match tool_name {
        "gws_gmail_search" => {
            let raw_query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let query_string = inject_label_query(raw_query, allowed_labels);
            let query = &query_string;
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(20);

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| {
                    GwsError::Validation("users.messages resource not found in gmail API".into())
                })?;
            let list_method = messages_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("list method not found".into()))?;

            let args = json!({
                "params": { "userId": "me", "q": query, "maxResults": max_results },
                "fields": "messages(id,threadId),resultSizeEstimate,nextPageToken"
            });
            let list_result = crate::execute::execute_tool(
                &gmail_doc,
                list_method,
                "users.messages",
                "list",
                &args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let empty_vec = vec![];
            let message_ids = list_result
                .get("messages")
                .and_then(|m| m.as_array())
                .unwrap_or(&empty_vec);
            let result_size = list_result
                .get("resultSizeEstimate")
                .and_then(|r| r.as_u64())
                .unwrap_or(0);

            if message_ids.is_empty() {
                return Ok(json!({
                    "content": [{ "type": "text", "text": "No messages found" }],
                    "structuredContent": { "messages": [], "resultSizeEstimate": 0 },
                    "isError": false
                }));
            }

            let get_method = messages_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;

            let mut enriched = Vec::new();
            for msg_ref in message_ids.iter().take(max_results as usize) {
                let msg_id = msg_ref.get("id").and_then(|i| i.as_str()).unwrap_or("");
                let thread_id = msg_ref
                    .get("threadId")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let get_args = json!({
                    "params": { "userId": "me", "id": msg_id, "format": "metadata" },
                    "fields": "id,threadId,snippet,labelIds,payload(headers)"
                });
                match crate::execute::execute_tool(
                    &gmail_doc,
                    get_method,
                    "users.messages",
                    "get",
                    &get_args,
                    "gmail",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await
                {
                    Ok(msg) => {
                        let empty_headers = json!([]);
                        let headers = msg
                            .get("payload")
                            .and_then(|p| p.get("headers"))
                            .unwrap_or(&empty_headers);
                        let hdr_map = extract_headers(headers, &["From", "To", "Subject", "Date"]);
                        enriched.push(json!({
                            "id": msg.get("id").unwrap_or(&json!(msg_id)),
                            "threadId": msg.get("threadId").unwrap_or(&json!(thread_id)),
                            "subject": hdr_map.get("subject").unwrap_or(&json!("")),
                            "from": hdr_map.get("from").unwrap_or(&json!("")),
                            "to": hdr_map.get("to").unwrap_or(&json!("")),
                            "date": hdr_map.get("date").unwrap_or(&json!("")),
                            "snippet": msg.get("snippet").unwrap_or(&json!("")),
                            "labelIds": msg.get("labelIds").unwrap_or(&json!([]))
                        }));
                    }
                    Err(e) => {
                        tracing::warn!(message_id = msg_id, error = %e, "Failed to fetch message metadata");
                    }
                }
            }

            let result = json!({ "messages": enriched, "resultSizeEstimate": result_size });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_gmail_read" => {
            let message_id = arguments
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'message_id'".into()))?;
            let format = arguments
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("full");

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| GwsError::Validation("users.messages resource not found".into()))?;
            let get_method = messages_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;

            let args = json!({
                "params": { "userId": "me", "id": message_id, "format": format }
            });
            let msg = crate::execute::execute_tool(
                &gmail_doc,
                get_method,
                "users.messages",
                "get",
                &args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            if !allowed_label_ids.is_empty() {
                let msg_labels = extract_label_ids(&msg);
                check_label_policy(&msg_labels, &allowed_label_ids)
                    .map_err(GwsError::Validation)?;
            }

            let empty_headers = json!([]);
            let empty_payload = json!({});
            let headers = msg
                .get("payload")
                .and_then(|p| p.get("headers"))
                .unwrap_or(&empty_headers);
            let hdr_map = extract_headers(
                headers,
                &["From", "To", "Cc", "Subject", "Date", "Message-ID"],
            );

            let body_text = if format == "full" {
                let payload = msg.get("payload").unwrap_or(&empty_payload);
                decode_message_body(payload)
            } else {
                String::new()
            };

            let attachments = if format == "full" {
                let payload = msg.get("payload").unwrap_or(&empty_payload);
                list_attachments(payload)
            } else {
                vec![]
            };

            let result = json!({
                "id": msg.get("id"),
                "threadId": msg.get("threadId"),
                "labelIds": msg.get("labelIds"),
                "snippet": msg.get("snippet"),
                "headers": hdr_map,
                "body": body_text,
                "attachments": attachments
            });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_gmail_draft" => {
            let to = arguments
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'to'".into()))?;
            let subject = arguments
                .get("subject")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'subject'".into()))?;
            let body = arguments
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'body'".into()))?;
            let cc = arguments.get("cc").and_then(|v| v.as_str());
            let bcc = arguments.get("bcc").and_then(|v| v.as_str());
            let is_markdown = arguments.get("format").and_then(|v| v.as_str()) == Some("markdown");

            let html_body = if is_markdown {
                let html = markdown_to_html(body);
                Some(build_html_email_body(&html))
            } else {
                None
            };
            let raw =
                build_rfc2822_message(to, cc, bcc, subject, body, None, None, html_body.as_deref());

            let drafts_resource = tools::find_resource(&gmail_doc.resources, "users.drafts")
                .ok_or_else(|| GwsError::Validation("users.drafts resource not found".into()))?;
            let create_method = drafts_resource
                .methods
                .get("create")
                .ok_or_else(|| GwsError::Validation("create method not found".into()))?;

            let args = json!({
                "params": { "userId": "me" },
                "body": { "message": { "raw": raw } }
            });
            let result = crate::execute::execute_tool(
                &gmail_doc,
                create_method,
                "users.drafts",
                "create",
                &args,
                "gmail",
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

        "gws_gmail_send" => {
            let draft_id = arguments.get("draft_id").and_then(|v| v.as_str());

            if let Some(did) = draft_id {
                let drafts_resource = tools::find_resource(&gmail_doc.resources, "users.drafts")
                    .ok_or_else(|| {
                        GwsError::Validation("users.drafts resource not found".into())
                    })?;
                let send_method = drafts_resource
                    .methods
                    .get("send")
                    .ok_or_else(|| GwsError::Validation("drafts.send method not found".into()))?;
                let args = json!({
                    "params": { "userId": "me" },
                    "body": { "id": did }
                });
                let result = crate::execute::execute_tool(
                    &gmail_doc,
                    send_method,
                    "users.drafts",
                    "send",
                    &args,
                    "gmail",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                return Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            let to = arguments
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GwsError::Validation(
                        "Missing 'to' (or provide 'draft_id' to send an existing draft)".into(),
                    )
                })?;
            let subject = arguments
                .get("subject")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'subject'".into()))?;
            let body = arguments
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'body'".into()))?;
            let cc = arguments.get("cc").and_then(|v| v.as_str());
            let bcc = arguments.get("bcc").and_then(|v| v.as_str());
            let is_markdown = arguments.get("format").and_then(|v| v.as_str()) == Some("markdown");

            let html_body = if is_markdown {
                let html = markdown_to_html(body);
                Some(build_html_email_body(&html))
            } else {
                None
            };
            let raw =
                build_rfc2822_message(to, cc, bcc, subject, body, None, None, html_body.as_deref());

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| GwsError::Validation("users.messages resource not found".into()))?;
            let send_method = messages_resource
                .methods
                .get("send")
                .ok_or_else(|| GwsError::Validation("messages.send method not found".into()))?;

            let args = json!({
                "params": { "userId": "me" },
                "body": { "raw": raw }
            });
            let result = crate::execute::execute_tool(
                &gmail_doc,
                send_method,
                "users.messages",
                "send",
                &args,
                "gmail",
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

        "gws_gmail_reply" => {
            let message_id = arguments
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'message_id'".into()))?;
            let reply_body = arguments
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'body'".into()))?;
            let override_to = arguments.get("to").and_then(|v| v.as_str());
            let cc = arguments.get("cc").and_then(|v| v.as_str());

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| GwsError::Validation("users.messages resource not found".into()))?;
            let get_method = messages_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;

            let get_args = json!({
                "params": { "userId": "me", "id": message_id, "format": "metadata" },
                "fields": "id,threadId,labelIds,payload(headers)"
            });
            let original = crate::execute::execute_tool(
                &gmail_doc,
                get_method,
                "users.messages",
                "get",
                &get_args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            if !allowed_label_ids.is_empty() {
                let msg_labels = extract_label_ids(&original);
                check_label_policy(&msg_labels, &allowed_label_ids)
                    .map_err(GwsError::Validation)?;
            }

            let empty_headers = json!([]);
            let orig_headers = original
                .get("payload")
                .and_then(|p| p.get("headers"))
                .unwrap_or(&empty_headers);
            let hdr_map = extract_headers(orig_headers, &["From", "Subject", "Message-ID"]);

            let to = override_to
                .unwrap_or_else(|| hdr_map.get("from").and_then(|v| v.as_str()).unwrap_or(""));
            let orig_subject = hdr_map
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let subject = if orig_subject.to_lowercase().starts_with("re:") {
                orig_subject.to_string()
            } else {
                format!("Re: {orig_subject}")
            };
            let message_id_header = hdr_map.get("message-id").and_then(|v| v.as_str());
            let thread_id = original
                .get("threadId")
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let is_markdown = arguments.get("format").and_then(|v| v.as_str()) == Some("markdown");
            let html_body = if is_markdown {
                let html = markdown_to_html(reply_body);
                Some(build_html_email_body(&html))
            } else {
                None
            };
            let raw = build_rfc2822_message(
                to,
                cc,
                None,
                &subject,
                reply_body,
                message_id_header,
                message_id_header,
                html_body.as_deref(),
            );

            let send_method = messages_resource
                .methods
                .get("send")
                .ok_or_else(|| GwsError::Validation("messages.send method not found".into()))?;

            let args = json!({
                "params": { "userId": "me" },
                "body": { "raw": raw, "threadId": thread_id }
            });
            let result = crate::execute::execute_tool(
                &gmail_doc,
                send_method,
                "users.messages",
                "send",
                &args,
                "gmail",
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

        "gws_gmail_thread" => {
            let thread_id = arguments
                .get("thread_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'thread_id'".into()))?;

            let threads_resource = tools::find_resource(&gmail_doc.resources, "users.threads")
                .ok_or_else(|| GwsError::Validation("users.threads resource not found".into()))?;
            let get_method = threads_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("threads.get method not found".into()))?;

            let args = json!({
                "params": { "userId": "me", "id": thread_id, "format": "full" }
            });
            let thread = crate::execute::execute_tool(
                &gmail_doc,
                get_method,
                "users.threads",
                "get",
                &args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            if !allowed_label_ids.is_empty() {
                let empty_msg = json!({});
                let first_msg = thread
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .and_then(|a| a.first())
                    .unwrap_or(&empty_msg);
                let msg_labels = extract_label_ids(first_msg);
                check_label_policy(&msg_labels, &allowed_label_ids)
                    .map_err(GwsError::Validation)?;
            }

            let empty_arr = vec![];
            let messages = thread
                .get("messages")
                .and_then(|m| m.as_array())
                .unwrap_or(&empty_arr);

            let mut conversation = Vec::new();
            for msg in messages {
                let empty_headers = json!([]);
                let headers = msg
                    .get("payload")
                    .and_then(|p| p.get("headers"))
                    .unwrap_or(&empty_headers);
                let hdr_map = extract_headers(headers, &["From", "To", "Cc", "Subject", "Date"]);
                let empty_payload = json!({});
                let payload = msg.get("payload").unwrap_or(&empty_payload);
                let raw_body = decode_message_body(payload);
                let body = strip_quoted_text(&raw_body);
                let attachments = list_attachments(payload);

                conversation.push(json!({
                    "id": msg.get("id"),
                    "from": hdr_map.get("from"),
                    "to": hdr_map.get("to"),
                    "cc": hdr_map.get("cc"),
                    "subject": hdr_map.get("subject"),
                    "date": hdr_map.get("date"),
                    "body": body,
                    "attachments": attachments
                }));
            }

            let result = json!({
                "threadId": thread.get("id"),
                "messageCount": messages.len(),
                "messages": conversation
            });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_gmail_attachment" => {
            let message_id = arguments
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'message_id'".into()))?;
            let attachment_id = arguments
                .get("attachment_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'attachment_id'".into()))?;

            if !allowed_label_ids.is_empty() {
                let messages_resource =
                    tools::find_resource(&gmail_doc.resources, "users.messages").ok_or_else(
                        || GwsError::Validation("users.messages resource not found".into()),
                    )?;
                let get_method = messages_resource
                    .methods
                    .get("get")
                    .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                let get_args = json!({
                    "params": { "userId": "me", "id": message_id, "format": "minimal" },
                    "fields": "id,labelIds"
                });
                let msg = crate::execute::execute_tool(
                    &gmail_doc,
                    get_method,
                    "users.messages",
                    "get",
                    &get_args,
                    "gmail",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                let msg_labels = extract_label_ids(&msg);
                check_label_policy(&msg_labels, &allowed_label_ids)
                    .map_err(GwsError::Validation)?;
            }

            let attachments_resource =
                tools::find_resource(&gmail_doc.resources, "users.messages.attachments")
                    .ok_or_else(|| {
                        GwsError::Validation("users.messages.attachments resource not found".into())
                    })?;
            let get_method = attachments_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("attachments.get method not found".into()))?;

            let args = json!({
                "params": { "userId": "me", "messageId": message_id, "id": attachment_id }
            });
            let att_result = crate::execute::execute_tool(
                &gmail_doc,
                get_method,
                "users.messages.attachments",
                "get",
                &args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let data = att_result
                .get("data")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let size = att_result.get("size").and_then(|s| s.as_u64()).unwrap_or(0);

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| GwsError::Validation("users.messages resource not found".into()))?;
            let msg_get = messages_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
            let msg_args = json!({
                "params": { "userId": "me", "id": message_id, "format": "full" },
                "fields": "payload"
            });
            let msg = crate::execute::execute_tool(
                &gmail_doc,
                msg_get,
                "users.messages",
                "get",
                &msg_args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;
            let empty_payload = json!({});
            let payload = msg.get("payload").unwrap_or(&empty_payload);
            let attachments = list_attachments(payload);
            let mime = attachments
                .iter()
                .find(|a| a.get("attachmentId").and_then(|i| i.as_str()) == Some(attachment_id))
                .and_then(|a| a.get("mimeType").and_then(|m| m.as_str()))
                .unwrap_or("application/octet-stream");
            let filename = attachments
                .iter()
                .find(|a| a.get("attachmentId").and_then(|i| i.as_str()) == Some(attachment_id))
                .and_then(|a| a.get("filename").and_then(|f| f.as_str()))
                .unwrap_or("attachment");

            let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());

            if let Some(fid) = folder_id {
                let raw_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(data)
                    .map_err(|_| GwsError::Validation("Failed to decode attachment data".into()))?;
                let standard_b64 = base64::engine::general_purpose::STANDARD.encode(&raw_bytes);

                let drive_doc = state.get_doc("drive").await?;
                let files_resource = tools::find_resource(&drive_doc.resources, "files")
                    .ok_or_else(|| GwsError::Validation("Drive files resource not found".into()))?;
                let create_method = files_resource
                    .methods
                    .get("create")
                    .ok_or_else(|| GwsError::Validation("Drive files.create not found".into()))?;

                let upload_args = json!({
                    "body": { "name": filename, "parents": [fid] },
                    "media_data": standard_b64,
                    "media_content_type": mime
                });
                let upload_result = crate::execute::execute_tool(
                    &drive_doc,
                    create_method,
                    "files",
                    "create",
                    &upload_args,
                    "drive",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;

                let file_id = upload_result
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let result = json!({
                    "filename": filename,
                    "mimeType": mime,
                    "size": size,
                    "savedToDrive": true,
                    "driveFileId": file_id,
                    "folderId": fid
                });
                return Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            if is_text_mime(mime)
                && let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
                && let Ok(text) = String::from_utf8(decoded)
            {
                let result = json!({
                    "filename": filename,
                    "mimeType": mime,
                    "size": size,
                    "content": text
                });
                return Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            let result = json!({
                "filename": filename,
                "mimeType": mime,
                "size": size,
                "encoding": "base64url",
                "data": data
            });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_gmail_contacts" => {
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'query'".into()))?;
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(5)
                .min(30);

            let people_doc = state.get_doc("people").await?;
            let people_resource = tools::find_resource(&people_doc.resources, "people.connections")
                .ok_or_else(|| {
                    GwsError::Validation(
                        "people.connections resource not found in People API".into(),
                    )
                })?;
            let list_method = people_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("connections.list method not found".into()))?;

            let query_lower = query.to_lowercase();
            let mut contacts = Vec::new();
            let mut page_token: Option<String> = None;

            loop {
                let mut params = json!({
                    "resourceName": "people/me",
                    "personFields": "names,emailAddresses,organizations",
                    "pageSize": 100
                });
                if let Some(ref token) = page_token {
                    params["pageToken"] = json!(token);
                }
                let args = json!({ "params": params });
                let page = crate::execute::execute_tool(
                    &people_doc,
                    list_method,
                    "people.connections",
                    "list",
                    &args,
                    "people",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;

                let empty_arr = vec![];
                let connections = page
                    .get("connections")
                    .and_then(|c| c.as_array())
                    .unwrap_or(&empty_arr);

                for person in connections {
                    let name = person
                        .get("names")
                        .and_then(|n| n.as_array())
                        .and_then(|a| a.first())
                        .and_then(|n| n.get("displayName"))
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let emails: Vec<&str> = person
                        .get("emailAddresses")
                        .and_then(|e| e.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| e.get("value").and_then(|v| v.as_str()))
                                .collect()
                        })
                        .unwrap_or_default();

                    if emails.is_empty() {
                        continue;
                    }

                    let name_match = name.to_lowercase().contains(&query_lower);
                    let email_match = emails
                        .iter()
                        .any(|e| e.to_lowercase().contains(&query_lower));

                    if name_match || email_match {
                        let org = person
                            .get("organizations")
                            .and_then(|o| o.as_array())
                            .and_then(|a| a.first())
                            .and_then(|o| o.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        contacts.push(json!({
                            "name": name,
                            "emails": emails,
                            "organization": org
                        }));
                        if contacts.len() >= max_results as usize {
                            break;
                        }
                    }
                }

                if contacts.len() >= max_results as usize {
                    break;
                }
                page_token = page
                    .get("nextPageToken")
                    .and_then(|t| t.as_str())
                    .map(String::from);
                if page_token.is_none() {
                    break;
                }
            }

            let result = json!({ "contacts": contacts, "count": contacts.len() });
            Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                "structuredContent": result,
                "isError": false
            }))
        }

        "gws_gmail_forward" => {
            let message_id = arguments
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'message_id'".into()))?;
            let to = arguments
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| GwsError::Validation("Missing 'to'".into()))?;
            let comment = arguments.get("comment").and_then(|v| v.as_str());
            let cc = arguments.get("cc").and_then(|v| v.as_str());

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| GwsError::Validation("users.messages resource not found".into()))?;
            let get_method = messages_resource
                .methods
                .get("get")
                .ok_or_else(|| GwsError::Validation("get method not found".into()))?;

            let get_args = json!({
                "params": { "userId": "me", "id": message_id, "format": "full" }
            });
            let original = crate::execute::execute_tool(
                &gmail_doc,
                get_method,
                "users.messages",
                "get",
                &get_args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            if !allowed_label_ids.is_empty() {
                let msg_labels = extract_label_ids(&original);
                check_label_policy(&msg_labels, &allowed_label_ids)
                    .map_err(GwsError::Validation)?;
            }

            let empty_headers = json!([]);
            let empty_payload = json!({});
            let headers = original
                .get("payload")
                .and_then(|p| p.get("headers"))
                .unwrap_or(&empty_headers);
            let hdr_map = extract_headers(headers, &["From", "To", "Subject", "Date"]);
            let orig_from = hdr_map.get("from").and_then(|v| v.as_str()).unwrap_or("");
            let orig_to = hdr_map.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let orig_subject = hdr_map
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let orig_date = hdr_map.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let payload = original.get("payload").unwrap_or(&empty_payload);
            let orig_body = decode_message_body(payload);

            let fwd_subject = if orig_subject.to_lowercase().starts_with("fwd:") {
                orig_subject.to_string()
            } else {
                format!("Fwd: {orig_subject}")
            };

            let fwd_body = build_forward_body(
                comment,
                orig_from,
                orig_date,
                orig_subject,
                orig_to,
                &orig_body,
            );

            let raw =
                build_rfc2822_message(to, cc, None, &fwd_subject, &fwd_body, None, None, None);

            let send_method = messages_resource
                .methods
                .get("send")
                .ok_or_else(|| GwsError::Validation("messages.send method not found".into()))?;

            let args = json!({
                "params": { "userId": "me" },
                "body": { "raw": raw }
            });
            let result = crate::execute::execute_tool(
                &gmail_doc,
                send_method,
                "users.messages",
                "send",
                &args,
                "gmail",
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

        "gws_gmail_labels" => {
            let action = arguments
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list");

            let labels_resource = tools::find_resource(&gmail_doc.resources, "users.labels")
                .ok_or_else(|| GwsError::Validation("users.labels resource not found".into()))?;

            if action == "list" {
                let list_method = labels_resource
                    .methods
                    .get("list")
                    .ok_or_else(|| GwsError::Validation("labels.list method not found".into()))?;
                let args = json!({ "params": { "userId": "me" } });
                let result = crate::execute::execute_tool(
                    &gmail_doc,
                    list_method,
                    "users.labels",
                    "list",
                    &args,
                    "gmail",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                return Ok(json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
                    "structuredContent": result,
                    "isError": false
                }));
            }

            let msg_id = arguments
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GwsError::Validation("Missing 'message_id' for label add/remove".into())
                })?;
            let label_name = arguments
                .get("label")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GwsError::Validation("Missing 'label' for label add/remove".into())
                })?;

            if !allowed_label_ids.is_empty() {
                check_label_target_policy(label_name, allowed_labels)
                    .map_err(GwsError::Validation)?;

                let messages_resource =
                    tools::find_resource(&gmail_doc.resources, "users.messages").ok_or_else(
                        || GwsError::Validation("users.messages resource not found".into()),
                    )?;
                let get_method = messages_resource
                    .methods
                    .get("get")
                    .ok_or_else(|| GwsError::Validation("get method not found".into()))?;
                let get_args = json!({
                    "params": { "userId": "me", "id": msg_id, "format": "minimal" },
                    "fields": "id,labelIds"
                });
                let msg = crate::execute::execute_tool(
                    &gmail_doc,
                    get_method,
                    "users.messages",
                    "get",
                    &get_args,
                    "gmail",
                    policy,
                    meta,
                    None,
                    None,
                    false,
                    &mut state.token_cache,
                )
                .await?;
                let msg_labels = extract_label_ids(&msg);
                check_label_policy(&msg_labels, &allowed_label_ids)
                    .map_err(GwsError::Validation)?;
            }

            let list_method = labels_resource
                .methods
                .get("list")
                .ok_or_else(|| GwsError::Validation("labels.list method not found".into()))?;
            let labels_args = json!({ "params": { "userId": "me" } });
            let labels_result = crate::execute::execute_tool(
                &gmail_doc,
                list_method,
                "users.labels",
                "list",
                &labels_args,
                "gmail",
                policy,
                meta,
                None,
                None,
                false,
                &mut state.token_cache,
            )
            .await?;

            let label_id = resolve_label_id(label_name, &labels_result)
                .ok_or_else(|| GwsError::Validation(format!("Label '{label_name}' not found")))?;

            let messages_resource = tools::find_resource(&gmail_doc.resources, "users.messages")
                .ok_or_else(|| GwsError::Validation("users.messages resource not found".into()))?;
            let modify_method = messages_resource
                .methods
                .get("modify")
                .ok_or_else(|| GwsError::Validation("messages.modify method not found".into()))?;

            let body = match action {
                "add" => json!({ "addLabelIds": [label_id] }),
                "remove" => json!({ "removeLabelIds": [label_id] }),
                _ => {
                    return Err(GwsError::Validation(format!(
                        "Invalid action '{action}'. Use: list, add, remove"
                    )));
                }
            };

            let args = json!({
                "params": { "userId": "me", "id": msg_id },
                "body": body
            });
            let result = crate::execute::execute_tool(
                &gmail_doc,
                modify_method,
                "users.messages",
                "modify",
                &args,
                "gmail",
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
            "Unknown Gmail tool '{tool_name}'. Available: gws_gmail_search, gws_gmail_read, \
             gws_gmail_thread, gws_gmail_attachment, gws_gmail_contacts, gws_gmail_forward, \
             gws_gmail_draft, gws_gmail_send, gws_gmail_reply, gws_gmail_labels"
        ))),
    }
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
