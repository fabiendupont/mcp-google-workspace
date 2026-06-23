use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{ArgumentInfo, CompletionInfo, Reference};
use google_workspace::discovery::RestDescription;

use crate::policy::Policy;

pub fn complete_request(
    reference: &Reference,
    argument: &ArgumentInfo,
    policy: &Policy,
    docs: &HashMap<String, Arc<RestDescription>>,
    prompts: &[crate::prompts::Prompt],
) -> CompletionInfo {
    match reference {
        Reference::Resource(rr) => complete_resource_uri(&rr.uri, argument, policy),
        Reference::Prompt(pr) => complete_prompt_arg(&pr.name, argument, prompts),
    }
}

fn complete_resource_uri(
    _uri: &str,
    argument: &ArgumentInfo,
    policy: &Policy,
) -> CompletionInfo {
    if argument.name == "uri" {
        let prefix = &argument.value;
        let services = policy.allowed_services();
        let suggestions: Vec<String> = if prefix.is_empty() || "gws://".starts_with(prefix) {
            services
                .into_iter()
                .map(|s| format!("gws://{s}/"))
                .collect()
        } else if let Some(rest) = prefix.strip_prefix("gws://") {
            services
                .into_iter()
                .filter(|s| s.starts_with(rest) || rest.starts_with(*s))
                .map(|s| format!("gws://{s}/"))
                .collect()
        } else {
            vec![]
        };
        CompletionInfo {
            values: suggestions,
            total: None,
            has_more: None,
        }
    } else {
        CompletionInfo::default()
    }
}

fn complete_prompt_arg(
    prompt_name: &str,
    argument: &ArgumentInfo,
    prompts: &[crate::prompts::Prompt],
) -> CompletionInfo {
    let Some(prompt) = prompts.iter().find(|p| p.name == prompt_name) else {
        return CompletionInfo::default();
    };

    let Some(_arg) = prompt.arguments.iter().find(|a| a.name == argument.name) else {
        return CompletionInfo::default();
    };

    match (prompt_name, argument.name.as_str()) {
        (_, "paragraph_style") => prefix_filter(
            &argument.value,
            &[
                "TITLE", "SUBTITLE", "HEADING_1", "HEADING_2", "HEADING_3",
                "HEADING_4", "HEADING_5", "HEADING_6", "NORMAL_TEXT",
            ],
        ),
        _ => CompletionInfo::default(),
    }
}

fn prefix_filter(prefix: &str, options: &[&str]) -> CompletionInfo {
    let lower = prefix.to_lowercase();
    let values: Vec<String> = options
        .iter()
        .filter(|o| o.to_lowercase().starts_with(&lower))
        .map(|o| o.to_string())
        .collect();
    CompletionInfo {
        total: Some(values.len() as u32),
        values,
        has_more: Some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_filter_empty() {
        let result = prefix_filter("", &["foo", "bar"]);
        assert_eq!(result.values.len(), 2);
    }

    #[test]
    fn test_prefix_filter_match() {
        let result = prefix_filter("HEA", &["HEADING_1", "HEADING_2", "NORMAL_TEXT"]);
        assert_eq!(result.values, vec!["HEADING_1", "HEADING_2"]);
    }

    #[test]
    fn test_prefix_filter_case_insensitive() {
        let result = prefix_filter("head", &["HEADING_1", "TITLE"]);
        assert_eq!(result.values, vec!["HEADING_1"]);
    }

    #[test]
    fn test_complete_resource_uri_empty() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string(), "docs".to_string()]);
        let arg = ArgumentInfo { name: "uri".to_string(), value: "".to_string() };
        let result = complete_resource_uri("", &arg, &policy);
        assert!(result.values.iter().any(|v| v.contains("drive")));
        assert!(result.values.iter().any(|v| v.contains("docs")));
    }

    #[test]
    fn test_complete_resource_uri_partial() {
        let policy = crate::policy::Policy::from_services(&["drive".to_string(), "docs".to_string()]);
        let arg = ArgumentInfo { name: "uri".to_string(), value: "gws://dr".to_string() };
        let result = complete_resource_uri("", &arg, &policy);
        assert_eq!(result.values, vec!["gws://drive/"]);
    }
}
