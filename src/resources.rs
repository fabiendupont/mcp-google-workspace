use std::collections::HashMap;
use std::sync::Arc;

use google_workspace::discovery::{RestDescription, RestResource};
use rmcp::model::{AnnotateAble, RawResource, RawResourceTemplate, ResourceTemplate};

use crate::policy::Policy;

pub struct ParsedUri {
    pub service: String,
    pub resource: String,
    pub id: String,
}

pub fn parse_gws_uri(uri: &str) -> Option<ParsedUri> {
    let rest = uri.strip_prefix("gws://")?;
    let mut parts = rest.splitn(3, '/');
    let service = parts.next()?.to_string();
    let resource = parts.next()?.to_string();
    let id = parts.next()?.to_string();
    if service.is_empty() || resource.is_empty() || id.is_empty() {
        return None;
    }
    Some(ParsedUri {
        service,
        resource,
        id,
    })
}

pub fn id_param_name(resource: &RestResource, method_name: &str) -> Option<String> {
    let method = resource.methods.get(method_name)?;
    method
        .parameters
        .iter()
        .find(|(_, p)| p.required && p.location.as_deref() == Some("path"))
        .map(|(name, _)| name.clone())
}

pub fn build_resource_templates(
    policy: &Policy,
    docs: &HashMap<String, Arc<RestDescription>>,
) -> Vec<ResourceTemplate> {
    let mut templates = Vec::new();

    for svc_name in policy.allowed_services() {
        let Some(doc) = docs.get(svc_name) else {
            continue;
        };

        collect_templates(svc_name, &doc.resources, "", &mut templates);
    }

    templates.sort_by(|a, b| a.raw.uri_template.cmp(&b.raw.uri_template));
    templates
}

fn collect_templates(
    service: &str,
    resources: &HashMap<String, RestResource>,
    prefix: &str,
    out: &mut Vec<ResourceTemplate>,
) {
    for (name, resource) in resources {
        let resource_path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };

        if let Some(get_method) = resource.methods.get("get") {
            if let Some(id_param) = get_method
                .parameters
                .iter()
                .find(|(_, p)| p.required && p.location.as_deref() == Some("path"))
                .map(|(n, _)| n.as_str())
            {
                let uri_template = format!("gws://{service}/{resource_path}/{{{id_param}}}");
                let description = get_method.description.as_deref().unwrap_or("").to_string();

                let template = RawResourceTemplate::new(&uri_template, &resource_path)
                    .with_description(format!("{service}.{resource_path}.get: {description}"))
                    .with_mime_type("application/json")
                    .no_annotation();

                out.push(template);
            }
        }

        if !resource.resources.is_empty() {
            collect_templates(service, &resource.resources, &resource_path, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_workspace::discovery::{MethodParameter, RestMethod};

    #[test]
    fn test_parse_gws_uri() {
        let parsed = parse_gws_uri("gws://drive/files/abc123").unwrap();
        assert_eq!(parsed.service, "drive");
        assert_eq!(parsed.resource, "files");
        assert_eq!(parsed.id, "abc123");
    }

    #[test]
    fn test_parse_gws_uri_with_slashes_in_id() {
        let parsed = parse_gws_uri("gws://gmail/messages/msg-id/with/extra").unwrap();
        assert_eq!(parsed.service, "gmail");
        assert_eq!(parsed.resource, "messages");
        assert_eq!(parsed.id, "msg-id/with/extra");
    }

    #[test]
    fn test_parse_gws_uri_invalid() {
        assert!(parse_gws_uri("https://example.com").is_none());
        assert!(parse_gws_uri("gws://").is_none());
        assert!(parse_gws_uri("gws://drive").is_none());
        assert!(parse_gws_uri("gws://drive/files").is_none());
        assert!(parse_gws_uri("gws://drive/files/").is_none());
    }

    #[test]
    fn test_id_param_name_found() {
        let mut resource = RestResource {
            methods: HashMap::new(),
            resources: HashMap::new(),
        };
        let mut params = HashMap::new();
        params.insert(
            "fileId".to_string(),
            MethodParameter {
                required: true,
                location: Some("path".to_string()),
                ..Default::default()
            },
        );
        resource.methods.insert(
            "get".to_string(),
            RestMethod {
                parameters: params,
                http_method: "GET".to_string(),
                ..Default::default()
            },
        );
        assert_eq!(id_param_name(&resource, "get"), Some("fileId".to_string()));
    }

    #[test]
    fn test_id_param_name_no_get() {
        let resource = RestResource {
            methods: HashMap::new(),
            resources: HashMap::new(),
        };
        assert_eq!(id_param_name(&resource, "get"), None);
    }
}
