use serde_json::{Value, json};
use std::path::Path;

pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

pub struct Prompt {
    pub name: String,
    pub title: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
    pub body: String,
}

pub fn load_prompts(dir: Option<&Path>) -> Vec<Prompt> {
    let dir = match dir {
        Some(d) if d.is_dir() => d,
        _ => return Vec::new(),
    };

    let mut prompts = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(prompt) = parse_prompt_content(&content) {
                prompts.push(prompt);
            } else {
                tracing::warn!(file = %path.display(), "Failed to parse prompt file");
            }
        }
    }

    prompts.sort_by(|a, b| a.name.cmp(&b.name));
    prompts
}

fn parse_prompt_content(content: &str) -> Option<Prompt> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let frontmatter = &after_first[..end_pos];
    let body = after_first[end_pos + 4..]
        .trim_start_matches('\n')
        .to_string();

    let mut name = None;
    let mut title = None;
    let mut description = None;
    let mut arguments = Vec::new();
    let mut in_arguments = false;
    let mut current_arg: Option<(String, String, bool)> = None;

    for line in frontmatter.lines() {
        let indent = line.len() - line.trim_start().len();

        if indent == 0 && !line.trim().is_empty() {
            if in_arguments {
                if let Some((n, d, r)) = current_arg.take() {
                    arguments.push(PromptArgument {
                        name: n,
                        description: d,
                        required: r,
                    });
                }
                in_arguments = false;
            }

            if let Some(val) = line.strip_prefix("name:") {
                name = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("title:") {
                title = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("description:") {
                description = Some(val.trim().to_string());
            } else if line.trim() == "arguments:" {
                in_arguments = true;
            }
        } else if in_arguments {
            let stripped = line.trim();
            if let Some(val) = stripped.strip_prefix("- name:") {
                if let Some((n, d, r)) = current_arg.take() {
                    arguments.push(PromptArgument {
                        name: n,
                        description: d,
                        required: r,
                    });
                }
                current_arg = Some((val.trim().to_string(), String::new(), false));
            } else if let Some(val) = stripped.strip_prefix("description:") {
                if let Some(ref mut arg) = current_arg {
                    arg.1 = val.trim().to_string();
                }
            } else if let Some(val) = stripped.strip_prefix("required:")
                && let Some(ref mut arg) = current_arg
            {
                arg.2 = val.trim() == "true";
            }
        }
    }

    if let Some((n, d, r)) = current_arg.take() {
        arguments.push(PromptArgument {
            name: n,
            description: d,
            required: r,
        });
    }

    Some(Prompt {
        name: name?,
        title: title.unwrap_or_default(),
        description: description?,
        arguments,
        body,
    })
}

pub fn list_prompts(prompts: &[Prompt]) -> Value {
    let list: Vec<Value> = prompts
        .iter()
        .map(|p| {
            let args: Vec<Value> = p
                .arguments
                .iter()
                .map(|a| {
                    json!({
                        "name": a.name,
                        "description": a.description,
                        "required": a.required,
                    })
                })
                .collect();
            json!({
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "arguments": args,
            })
        })
        .collect();
    json!({ "prompts": list })
}

pub fn get_prompt(prompts: &[Prompt], name: &str, arguments: &Value) -> Result<Value, String> {
    let prompt = prompts
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("Prompt '{}' not found", name))?;

    let mut body = prompt.body.clone();
    for arg in &prompt.arguments {
        let placeholder = format!("{{{{{}}}}}", arg.name);
        let value = arguments
            .get(&arg.name)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        body = body.replace(&placeholder, value);
    }

    Ok(json!({
        "description": prompt.description,
        "messages": [{
            "role": "user",
            "content": {
                "type": "text",
                "text": body
            }
        }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PROMPT: &str = r#"---
name: create-document
title: Create a Formatted Google Doc
description: Step-by-step workflow for creating a doc.
arguments:
  - name: title
    description: Document title
    required: false
  - name: folder_id
    description: Drive folder ID
    required: false
---

Create a Google Doc called "{{title}}" in folder {{folder_id}}.

Follow these steps:
1. Create the document
2. Format it
"#;

    #[test]
    fn test_parse_frontmatter() {
        let prompt = parse_prompt_content(SAMPLE_PROMPT).unwrap();
        assert_eq!(prompt.name, "create-document");
        assert_eq!(prompt.title, "Create a Formatted Google Doc");
        assert_eq!(
            prompt.description,
            "Step-by-step workflow for creating a doc."
        );
        assert_eq!(prompt.arguments.len(), 2);
        assert_eq!(prompt.arguments[0].name, "title");
        assert_eq!(prompt.arguments[0].description, "Document title");
        assert!(!prompt.arguments[0].required);
        assert_eq!(prompt.arguments[1].name, "folder_id");
        assert!(prompt.body.contains("Create a Google Doc"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        assert!(parse_prompt_content("Just some text").is_none());
    }

    #[test]
    fn test_parse_missing_name() {
        let content = "---\ntitle: T\ndescription: D\n---\nbody";
        assert!(parse_prompt_content(content).is_none());
    }

    #[test]
    fn test_parse_missing_description() {
        let content = "---\nname: n\ntitle: T\n---\nbody";
        assert!(parse_prompt_content(content).is_none());
    }

    #[test]
    fn test_parse_no_arguments() {
        let content =
            "---\nname: simple\ntitle: Simple\ndescription: A simple prompt\n---\nDo the thing.";
        let prompt = parse_prompt_content(content).unwrap();
        assert_eq!(prompt.name, "simple");
        assert!(prompt.arguments.is_empty());
        assert!(prompt.body.contains("Do the thing"));
    }

    #[test]
    fn test_parse_required_argument() {
        let content = "---\nname: t\ntitle: T\ndescription: D\narguments:\n  - name: x\n    description: X\n    required: true\n---\nbody";
        let prompt = parse_prompt_content(content).unwrap();
        assert!(prompt.arguments[0].required);
    }

    #[test]
    fn test_list_prompts() {
        let prompts = vec![parse_prompt_content(SAMPLE_PROMPT).unwrap()];
        let result = list_prompts(&prompts);
        let list = result["prompts"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["name"], "create-document");
        assert_eq!(list[0]["arguments"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_list_prompts_empty() {
        let result = list_prompts(&[]);
        assert_eq!(result["prompts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_get_prompt_found() {
        let prompts = vec![parse_prompt_content(SAMPLE_PROMPT).unwrap()];
        let args = json!({"title": "My Report", "folder_id": "abc123"});
        let result = get_prompt(&prompts, "create-document", &args).unwrap();
        let text = result["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("My Report"));
        assert!(text.contains("abc123"));
        assert!(!text.contains("{{title}}"));
        assert!(!text.contains("{{folder_id}}"));
    }

    #[test]
    fn test_get_prompt_not_found() {
        let prompts = vec![parse_prompt_content(SAMPLE_PROMPT).unwrap()];
        let result = get_prompt(&prompts, "nonexistent", &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_argument_substitution_missing_args() {
        let prompts = vec![parse_prompt_content(SAMPLE_PROMPT).unwrap()];
        let result = get_prompt(&prompts, "create-document", &json!({})).unwrap();
        let text = result["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("called \"\""));
        assert!(!text.contains("{{title}}"));
    }

    #[test]
    fn test_load_prompts_nonexistent_dir() {
        let prompts = load_prompts(Some(Path::new("/nonexistent/dir")));
        assert!(prompts.is_empty());
    }

    #[test]
    fn test_load_prompts_none() {
        let prompts = load_prompts(None);
        assert!(prompts.is_empty());
    }

    #[test]
    fn test_load_prompts_from_dir() {
        let dir = std::env::temp_dir().join("mcp-gws-test-prompts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("create-doc.md"), SAMPLE_PROMPT).unwrap();
        std::fs::write(dir.join("not-a-prompt.txt"), "ignored").unwrap();

        let prompts = load_prompts(Some(&dir));
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "create-document");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
