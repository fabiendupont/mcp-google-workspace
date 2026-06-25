use serde_json::{Value, json};

use crate::helpers;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContentFormat {
    Markdown,
    Plain,
}

pub fn parse_format(s: Option<&str>) -> ContentFormat {
    match s {
        Some("plain") | Some("text") => ContentFormat::Plain,
        _ => ContentFormat::Markdown,
    }
}

pub fn content_to_batch_requests(
    content: &str,
    format: ContentFormat,
    start_index: i32,
) -> Vec<Value> {
    match format {
        ContentFormat::Markdown => helpers::markdown_to_batch_requests(content, start_index),
        ContentFormat::Plain => plain_to_batch_requests(content, start_index),
    }
}

pub fn doc_to_format(doc: &Value, format: ContentFormat) -> String {
    match format {
        ContentFormat::Markdown => doc_to_markdown(doc),
        ContentFormat::Plain => doc_to_plain(doc),
    }
}

fn plain_to_batch_requests(content: &str, start_index: i32) -> Vec<Value> {
    let mut requests = Vec::new();
    let mut offset = start_index;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            let text = "\n";
            requests.push(json!({
                "insertText": {
                    "text": text,
                    "location": { "index": offset }
                }
            }));
            offset += 1;
            continue;
        }

        let (clean_text, style) = strip_plain_prefix(trimmed);
        let text = format!("{clean_text}\n");
        let text_len = text.len() as i32;
        requests.push(json!({
            "insertText": {
                "text": text,
                "location": { "index": offset }
            }
        }));

        if let Some(named_style) = style {
            requests.push(json!({
                "updateParagraphStyle": {
                    "paragraphStyle": { "namedStyleType": named_style },
                    "fields": "namedStyleType",
                    "range": { "startIndex": offset, "endIndex": offset + text_len }
                }
            }));
        }

        offset += text_len;
    }

    requests
}

fn strip_plain_prefix(line: &str) -> (&str, Option<&'static str>) {
    if let Some(rest) = line.strip_prefix("#### ") {
        if !rest.is_empty() {
            return (rest, Some("HEADING_3"));
        }
    }
    if let Some(rest) = line.strip_prefix("### ") {
        if !rest.is_empty() {
            return (rest, Some("HEADING_2"));
        }
    }
    if let Some(rest) = line.strip_prefix("## ") {
        if !rest.is_empty() {
            return (rest, Some("HEADING_1"));
        }
    }
    if let Some(rest) = line.strip_prefix("# ") {
        if !rest.is_empty() {
            return (rest, Some("TITLE"));
        }
    }
    (line, None)
}

pub fn doc_to_markdown(doc: &Value) -> String {
    let mut output = String::new();

    let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) else {
        return output;
    };

    for elem in content {
        if let Some(paragraph) = elem.get("paragraph") {
            let style = paragraph
                .pointer("/paragraphStyle/namedStyleType")
                .and_then(|v| v.as_str())
                .unwrap_or("NORMAL_TEXT");

            let is_bullet = paragraph.get("bullet").is_some();
            let nesting = paragraph
                .pointer("/bullet/nestingLevel")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as usize;

            let mut text = String::new();
            if let Some(elements) = paragraph.get("elements").and_then(|v| v.as_array()) {
                for pe in elements {
                    if let Some(tr) = pe.get("textRun") {
                        let content = tr.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let raw = content.trim_end_matches('\n');
                        if raw.is_empty() {
                            continue;
                        }

                        let bold = tr
                            .pointer("/textStyle/bold")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let italic = tr
                            .pointer("/textStyle/italic")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let mut chunk = raw.to_string();
                        if bold && italic {
                            chunk = format!("***{chunk}***");
                        } else if bold {
                            chunk = format!("**{chunk}**");
                        } else if italic {
                            chunk = format!("*{chunk}*");
                        }
                        text.push_str(&chunk);
                    }
                }
            }

            if text.is_empty() {
                output.push('\n');
                continue;
            }

            match style {
                "TITLE" => output.push_str(&format!("# {text}\n\n")),
                "SUBTITLE" => output.push_str(&format!("*{text}*\n\n")),
                "HEADING_1" => output.push_str(&format!("## {text}\n\n")),
                "HEADING_2" => output.push_str(&format!("### {text}\n\n")),
                "HEADING_3" => output.push_str(&format!("#### {text}\n\n")),
                "HEADING_4" => output.push_str(&format!("##### {text}\n\n")),
                "HEADING_5" => output.push_str(&format!("###### {text}\n\n")),
                _ if is_bullet => {
                    let indent = "  ".repeat(nesting);
                    output.push_str(&format!("{indent}- {text}\n"));
                }
                _ => output.push_str(&format!("{text}\n\n")),
            }
        } else if let Some(table) = elem.get("table") {
            if let Some(rows) = table.get("tableRows").and_then(|v| v.as_array()) {
                for (i, row) in rows.iter().enumerate() {
                    if let Some(cells) = row.get("tableCells").and_then(|v| v.as_array()) {
                        let cell_texts: Vec<String> = cells
                            .iter()
                            .map(|cell| {
                                cell.get("content")
                                    .and_then(|v| v.as_array())
                                    .map(|paras| {
                                        paras
                                            .iter()
                                            .filter_map(|p| {
                                                p.pointer("/paragraph/elements")
                                                    .and_then(|v| v.as_array())
                                                    .map(|elems| {
                                                        elems
                                                            .iter()
                                                            .filter_map(|e| {
                                                                e.pointer("/textRun/content")
                                                                    .and_then(|v| v.as_str())
                                                            })
                                                            .collect::<String>()
                                                            .trim()
                                                            .to_string()
                                                    })
                                            })
                                            .collect::<Vec<_>>()
                                            .join(" ")
                                    })
                                    .unwrap_or_default()
                            })
                            .collect();
                        output.push_str(&format!("| {} |\n", cell_texts.join(" | ")));
                        if i == 0 {
                            let sep: Vec<&str> = cell_texts.iter().map(|_| "---").collect();
                            output.push_str(&format!("| {} |\n", sep.join(" | ")));
                        }
                    }
                }
                output.push('\n');
            }
        }
    }

    output
}

pub fn doc_to_plain(doc: &Value) -> String {
    let mut output = String::new();

    let Some(content) = doc.pointer("/body/content").and_then(|v| v.as_array()) else {
        return output;
    };

    for elem in content {
        if let Some(paragraph) = elem.get("paragraph") {
            if let Some(elements) = paragraph.get("elements").and_then(|v| v.as_array()) {
                for pe in elements {
                    if let Some(text) = pe.pointer("/textRun/content").and_then(|v| v.as_str()) {
                        output.push_str(text);
                    }
                }
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_format() {
        assert_eq!(parse_format(None), ContentFormat::Markdown);
        assert_eq!(parse_format(Some("markdown")), ContentFormat::Markdown);
        assert_eq!(parse_format(Some("plain")), ContentFormat::Plain);
        assert_eq!(parse_format(Some("text")), ContentFormat::Plain);
    }

    #[test]
    #[test]
    fn test_plain_to_batch_requests() {
        let reqs = plain_to_batch_requests("# Title\nBody text\n", 1);
        assert!(reqs.len() >= 3);
        assert_eq!(reqs[0]["insertText"]["text"], "Title\n");
        let has_title_style = reqs.iter().any(|r| {
            r.pointer("/updateParagraphStyle/paragraphStyle/namedStyleType")
                .and_then(|v| v.as_str())
                == Some("TITLE")
        });
        assert!(has_title_style);
    }

    #[test]
    fn test_detect_plain_style() {
        assert_eq!(strip_plain_prefix("# Title"), ("Title", Some("TITLE")));
        assert_eq!(
            strip_plain_prefix("## Heading"),
            ("Heading", Some("HEADING_1"))
        );
        assert_eq!(strip_plain_prefix("### Sub"), ("Sub", Some("HEADING_2")));
        assert_eq!(strip_plain_prefix("Normal text"), ("Normal text", None));
    }

    #[test]
    fn test_doc_to_plain() {
        let doc = json!({
            "body": {
                "content": [
                    { "paragraph": { "elements": [{ "textRun": { "content": "Hello " } }] } },
                    { "paragraph": { "elements": [{ "textRun": { "content": "World\n" } }] } }
                ]
            }
        });
        let plain = doc_to_plain(&doc);
        assert!(plain.contains("Hello"));
        assert!(plain.contains("World"));
    }

    #[test]
    fn test_doc_to_markdown_heading() {
        let doc = json!({
            "body": {
                "content": [{
                    "paragraph": {
                        "paragraphStyle": { "namedStyleType": "HEADING_1" },
                        "elements": [{ "textRun": { "content": "My Heading\n" } }]
                    }
                }]
            }
        });
        let md = doc_to_markdown(&doc);
        assert!(md.contains("## My Heading"));
    }

    #[test]
    fn test_doc_to_markdown_bold() {
        let doc = json!({
            "body": {
                "content": [{
                    "paragraph": {
                        "paragraphStyle": { "namedStyleType": "NORMAL_TEXT" },
                        "elements": [{
                            "textRun": {
                                "content": "important\n",
                                "textStyle": { "bold": true }
                            }
                        }]
                    }
                }]
            }
        });
        let md = doc_to_markdown(&doc);
        assert!(md.contains("**important**"));
    }

    #[test]
    fn test_doc_to_markdown_title() {
        let doc = json!({
            "body": {
                "content": [{
                    "paragraph": {
                        "paragraphStyle": { "namedStyleType": "TITLE" },
                        "elements": [{ "textRun": { "content": "Doc Title\n" } }]
                    }
                }]
            }
        });
        let md = doc_to_markdown(&doc);
        assert!(md.starts_with("# Doc Title"));
    }
}
