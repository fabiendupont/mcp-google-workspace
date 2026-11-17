//! Cross-service composite workflows.
//!
//! Composites chain existing helper tools into multi-service pipelines:
//! Gmail thread → Google Doc, and Google Doc → Marp → Slides. They reuse
//! the curated helpers (formatting, policy enforcement, elicitation) and
//! therefore inherit `confirm_writes`, constraints, and label policies.

use google_workspace::error::GwsError;
use serde_json::{Value, json};

use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::shared::ServerState;

pub fn compose_thread_to_doc_tool_schema() -> Value {
    json!({
        "name": "gws_compose_thread_to_doc",
        "title": "Thread to Doc",
        "description": "Convert a Gmail thread into a formatted Google Doc (one section per message).",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "thread_id": {
                    "type": "string",
                    "description": "Gmail thread ID (from search results or a message's threadId)"
                },
                "doc_title": {
                    "type": "string",
                    "description": "Title for the new document (default: 'Thread: <subject>')"
                },
                "folder_id": {
                    "type": "string",
                    "description": "Drive folder for the new doc (default: policy-resolved folder)"
                }
            },
            "required": ["thread_id"]
        }
    })
}

pub fn compose_doc_to_slides_tool_schema() -> Value {
    json!({
        "name": "gws_compose_doc_to_slides",
        "title": "Doc to Slides",
        "description": "Turn a Google Doc into a slide deck: each H2 section becomes a slide.",
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_id": {
                    "type": "string",
                    "description": "Google Docs document ID to convert"
                },
                "title": {
                    "type": "string",
                    "description": "Presentation title (default: the document's first heading)"
                },
                "folder_id": {
                    "type": "string",
                    "description": "Drive folder for the new presentation"
                },
                "template": {
                    "type": "string",
                    "description": "Presentation template name (from gws_templates)"
                }
            },
            "required": ["document_id"]
        }
    })
}

/// Convert a `gws_gmail_thread` structuredContent into a document title and
/// Markdown body (one `##` section per message).
pub fn thread_to_markdown(thread: &Value) -> (String, String) {
    let empty = vec![];
    let messages = thread
        .get("messages")
        .and_then(|m| m.as_array())
        .unwrap_or(&empty);

    let subject = messages
        .first()
        .and_then(|m| m.get("subject"))
        .and_then(|s| s.as_str())
        .unwrap_or("(no subject)")
        .to_string();

    let mut participants: Vec<String> = Vec::new();
    let mut md = format!("# Thread: {subject}\n");

    for msg in messages {
        let from = msg
            .get("from")
            .and_then(|f| f.as_str())
            .unwrap_or("unknown sender");
        let date = msg.get("date").and_then(|d| d.as_str()).unwrap_or("");
        let body = msg
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .trim();

        if !participants.iter().any(|p| p == from) {
            participants.push(from.to_string());
        }

        md.push_str(&format!("\n## {from}"));
        if !date.is_empty() {
            md.push_str(&format!(" — {date}"));
        }
        md.push('\n');
        if !body.is_empty() {
            md.push_str(&format!("\n{body}\n"));
        }
    }

    if participants.len() > 1 {
        md.push_str(&format!(
            "\n---\n\n**Participants:** {}\n",
            participants.join(", ")
        ));
    }

    (subject, md)
}

/// Convert document Markdown to Marp Markdown: every H1 or H2 heading
/// starts a new slide, everything else passes through.
pub fn markdown_to_marp(md: &str) -> String {
    let mut slides: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("# ") || trimmed.starts_with("## ") {
            if !current.trim().is_empty() {
                slides.push(current.trim().to_string());
            }
            current = format!("{trimmed}\n");
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        slides.push(current.trim().to_string());
    }

    slides.join("\n\n---\n\n")
}

/// Extract the first H1 heading from Markdown, for default titles.
fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .find_map(|l| l.trim().strip_prefix("# "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn require_services(policy: &Policy, needed: &[&str]) -> Result<(), GwsError> {
    let missing: Vec<&str> = needed
        .iter()
        .filter(|svc| !policy.is_service_allowed(svc))
        .copied()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(GwsError::Validation(format!(
            "This composite requires the following services to be enabled in the policy: {}. \
             Enabled: {}",
            missing.join(", "),
            policy.allowed_services().join(", ")
        )))
    }
}

pub(crate) async fn execute_composite(
    tool_name: &str,
    arguments: &Value,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
    dry_run: bool,
) -> Result<Value, GwsError> {
    match tool_name {
        "gws_compose_thread_to_doc" => {
            require_services(policy, &["gmail", "docs"])?;

            let thread_id = arguments
                .get("thread_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'thread_id'".into()))?;

            // Step 1: read the thread (reuses the Gmail helper: label
            // policies, quoted-text stripping, attachment listing)
            let thread_result = crate::gmail_helpers::execute_gmail_helper(
                "gws_gmail_thread",
                &json!({ "thread_id": thread_id }),
                policy,
                meta,
                state,
            )
            .await?;

            let thread = thread_result
                .get("structuredContent")
                .cloned()
                .unwrap_or_else(|| json!({}));

            let (subject, md) = thread_to_markdown(&thread);
            let doc_title = arguments
                .get("doc_title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("Thread: {subject}"));

            let folder_id = match arguments.get("folder_id").and_then(|v| v.as_str()) {
                Some(f) => Some(f.to_string()),
                None => crate::elicitation::resolve_target_folder(None, policy)?,
            };

            if dry_run {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!(
                        "Dry run: would create document '{doc_title}' from thread {thread_id}.\n\n{md}"
                    )}],
                    "structuredContent": {
                        "dry_run": true,
                        "thread_id": thread_id,
                        "doc_title": doc_title,
                        "folder_id": folder_id,
                        "message_count": thread.get("messageCount"),
                        "markdown": md
                    },
                    "isError": false
                }));
            }

            // Step 2: write the document (reuses the Docs helper: markdown
            // conversion, folder policy, template styles)
            let mut write_args = json!({ "content": md, "title": doc_title });
            if let Some(f) = folder_id {
                write_args["folder_id"] = json!(f);
            }
            let write_result = crate::helpers::execute_docs_helper(
                "gws_docs_write",
                &write_args,
                policy,
                meta,
                state,
                dry_run,
            )
            .await?;

            if write_result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Ok(write_result);
            }

            let document_id = write_result
                .pointer("/structuredContent/document_id")
                .cloned()
                .unwrap_or(Value::Null);
            let output = json!({
                "threadId": thread_id,
                "subject": subject,
                "messageCount": thread.get("messageCount"),
                "documentId": document_id,
                "docTitle": doc_title,
            });
            Ok(json!({
                "content": [{ "type": "text", "text": format!(
                    "Thread '{subject}' saved as document '{doc_title}'.\ndocument_id: {}",
                    document_id.as_str().unwrap_or("(unknown)")
                )}],
                "structuredContent": output,
                "isError": false
            }))
        }

        "gws_compose_doc_to_slides" => {
            require_services(policy, &["docs", "slides"])?;

            let document_id = arguments
                .get("document_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GwsError::Validation("Missing 'document_id'".into()))?;

            // Step 1: read the document as Markdown (reuses the Docs helper)
            let read_result = crate::helpers::execute_docs_helper(
                "gws_docs_read",
                &json!({ "document_id": document_id, "format": "markdown" }),
                policy,
                meta,
                state,
                dry_run,
            )
            .await?;

            if read_result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Ok(read_result);
            }

            let md = read_result
                .pointer("/structuredContent/text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    read_result
                        .pointer("/content/0/text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .ok_or_else(|| {
                    GwsError::Validation("Could not read document content as Markdown".into())
                })?;

            if md.trim().is_empty() {
                return Err(GwsError::Validation(
                    "Document is empty — nothing to convert to slides".into(),
                ));
            }

            // Step 2: convert to Marp (each H1/H2 becomes a slide)
            let marp = markdown_to_marp(&md);
            let slide_count = marp.split("\n\n---\n\n").count();
            let title = arguments
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .or_else(|| first_heading(&md))
                .unwrap_or_else(|| "Presentation".to_string());

            let folder_id = arguments.get("folder_id").and_then(|v| v.as_str());

            if dry_run {
                return Ok(json!({
                    "content": [{ "type": "text", "text": format!(
                        "Dry run: would create presentation '{title}' ({slide_count} slides) from document {document_id}.\n\n{marp}"
                    )}],
                    "structuredContent": {
                        "dry_run": true,
                        "document_id": document_id,
                        "title": title,
                        "slide_count": slide_count,
                        "marp": marp
                    },
                    "isError": false
                }));
            }

            // Step 3: import as slides (reuses the Marp pipeline: templates,
            // speaker notes, tables, images)
            let mut import_args = json!({ "marp": marp, "title": title });
            if let Some(f) = folder_id {
                import_args["folder_id"] = json!(f);
            }
            if let Some(t) = arguments.get("template").and_then(|v| v.as_str()) {
                import_args["template"] = json!(t);
            }
            let import_result = crate::slides_helpers::execute_slides_helper(
                "gws_slides_import_marp",
                &import_args,
                policy,
                meta,
                state,
                dry_run,
            )
            .await?;

            if import_result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Ok(import_result);
            }

            let presentation_id = import_result
                .pointer("/structuredContent/presentation_id")
                .or_else(|| import_result.pointer("/structuredContent/presentationId"))
                .cloned()
                .unwrap_or(Value::Null);
            let output = json!({
                "documentId": document_id,
                "presentationId": presentation_id,
                "title": title,
                "slideCount": slide_count,
            });
            Ok(json!({
                "content": [{ "type": "text", "text": format!(
                    "Document {document_id} converted to presentation '{title}' ({slide_count} slides).\npresentation_id: {}",
                    presentation_id.as_str().unwrap_or("(unknown)")
                )}],
                "structuredContent": output,
                "isError": false
            }))
        }

        _ => Err(GwsError::Validation(format!(
            "Unknown composite tool '{tool_name}'. Available: gws_compose_thread_to_doc, \
             gws_compose_doc_to_slides"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_fixture() -> Value {
        json!({
            "threadId": "t1",
            "messageCount": 2,
            "messages": [
                {
                    "id": "m1",
                    "from": "alice@example.com",
                    "to": "bob@example.com",
                    "subject": "Q3 planning",
                    "date": "Mon, 10 Nov 2026 10:00:00 +0100",
                    "body": "Let's plan Q3.\nKey items:\n- budget\n- hiring"
                },
                {
                    "id": "m2",
                    "from": "bob@example.com",
                    "to": "alice@example.com",
                    "subject": "Re: Q3 planning",
                    "date": "Mon, 10 Nov 2026 11:30:00 +0100",
                    "body": "Sounds good."
                }
            ]
        })
    }

    #[test]
    fn test_thread_to_markdown_structure() {
        let (subject, md) = thread_to_markdown(&thread_fixture());
        assert_eq!(subject, "Q3 planning");
        assert!(md.starts_with("# Thread: Q3 planning"));
        assert!(md.contains("## alice@example.com — Mon, 10 Nov 2026 10:00:00 +0100"));
        assert!(md.contains("## bob@example.com — Mon, 10 Nov 2026 11:30:00 +0100"));
        assert!(md.contains("- budget"));
        assert!(md.contains("**Participants:** alice@example.com, bob@example.com"));
    }

    #[test]
    fn test_thread_to_markdown_empty() {
        let (subject, md) = thread_to_markdown(&json!({ "messages": [] }));
        assert_eq!(subject, "(no subject)");
        assert_eq!(md, "# Thread: (no subject)\n");
    }

    #[test]
    fn test_markdown_to_marp_splits_on_headings() {
        let md = "# Intro\n\nfirst slide\n\n## Details\n\nsecond slide\n\nmore\n\n## End\n\nlast";
        let marp = markdown_to_marp(md);
        let slides: Vec<&str> = marp.split("\n\n---\n\n").collect();
        assert_eq!(slides.len(), 3);
        assert!(slides[0].contains("# Intro"));
        assert!(slides[1].starts_with("## Details"));
        assert!(slides[1].contains("second slide"));
        assert!(slides[2].starts_with("## End"));
    }

    #[test]
    fn test_markdown_to_marp_h1_also_splits() {
        let md = "# One\n\na\n\n# Two\n\nb";
        let marp = markdown_to_marp(md);
        let slides: Vec<&str> = marp.split("\n\n---\n\n").collect();
        assert_eq!(slides.len(), 2);
    }

    #[test]
    fn test_markdown_to_marp_no_headings() {
        let md = "just text\nmore text";
        assert_eq!(markdown_to_marp(md), "just text\nmore text");
    }

    #[test]
    fn test_markdown_to_marp_skips_h3_and_deeper() {
        let md = "## Slide\n\n### Sub heading stays\ncontent";
        let marp = markdown_to_marp(md);
        let slides: Vec<&str> = marp.split("\n\n---\n\n").collect();
        assert_eq!(slides.len(), 1);
        assert!(slides[0].contains("### Sub heading stays"));
    }

    #[test]
    fn test_markdown_to_marp_empty() {
        assert_eq!(markdown_to_marp(""), "");
        assert_eq!(markdown_to_marp("\n\n  \n"), "");
    }

    #[test]
    fn test_first_heading() {
        assert_eq!(
            first_heading("# My Title\n\nbody"),
            Some("My Title".to_string())
        );
        assert_eq!(first_heading("## only h2"), None);
        assert_eq!(first_heading("no headings"), None);
    }

    #[test]
    fn test_require_services_ok() {
        let policy = Policy::from_services(&["gmail".to_string(), "docs".to_string()]);
        assert!(require_services(&policy, &["gmail", "docs"]).is_ok());
    }

    #[test]
    fn test_require_services_missing() {
        let policy = Policy::from_services(&["gmail".to_string()]);
        let err = require_services(&policy, &["gmail", "docs"]).unwrap_err();
        assert!(err.to_string().contains("docs"));
    }

    #[test]
    fn test_tool_schemas() {
        for schema in [
            compose_thread_to_doc_tool_schema(),
            compose_doc_to_slides_tool_schema(),
        ] {
            let name = schema["name"].as_str().unwrap();
            assert!(name.starts_with("gws_compose_"));
            assert_eq!(schema["annotations"]["readOnlyHint"], false);
            let desc = schema["description"].as_str().unwrap();
            assert!(desc.len() < 100, "{name} description too long");
        }
    }

    #[test]
    fn test_required_args() {
        let t2d = compose_thread_to_doc_tool_schema();
        assert!(
            t2d["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("thread_id"))
        );
        let d2s = compose_doc_to_slides_tool_schema();
        assert!(
            d2s["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("document_id"))
        );
    }
}
