use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;

use google_workspace::discovery::RestMethod;
use google_workspace::error::GwsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

fn default_read_write() -> Access {
    Access::ReadWrite
}

#[derive(Debug, Clone, Deserialize)]
pub struct Constraint {
    pub param: String,
    pub values: Vec<String>,
    #[serde(default = "default_read_write")]
    pub access: Access,
    pub location: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PolicyFile {
    #[serde(default)]
    pub server: ServerPolicy,
    #[serde(default)]
    pub services: Vec<ServicePolicy>,
}

#[derive(Debug, Deserialize)]
pub struct ServerPolicy {
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default)]
    pub rate_limit_rpm: Option<u32>,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub credentials_file: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

impl Default for ServerPolicy {
    fn default() -> Self {
        Self {
            read_only: false,
            max_request_bytes: default_max_request_bytes(),
            rate_limit_rpm: None,
            allowed_origins: Vec::new(),
            credentials_file: None,
            project_id: None,
        }
    }
}

fn default_max_request_bytes() -> usize {
    16 * 1024 * 1024
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServicePolicy {
    pub name: String,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub denied_methods: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub global_read_only: bool,
    pub max_request_bytes: usize,
    pub rate_limit_rpm: Option<u32>,
    pub allowed_origins: Vec<String>,
    pub credentials_file: Option<String>,
    pub project_id: Option<String>,
    services: HashMap<String, ServicePolicy>,
}

impl Policy {
    pub fn from_file(path: &Path) -> Result<Self, GwsError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| GwsError::Validation(format!("Failed to read policy file: {e}")))?;
        let file: PolicyFile = serde_json::from_str(&content)
            .map_err(|e| GwsError::Validation(format!("Invalid policy JSON: {e}")))?;
        Ok(Self::from_policy_file(file))
    }

    pub fn from_services(names: &[String]) -> Self {
        let file = PolicyFile {
            server: ServerPolicy::default(),
            services: names
                .iter()
                .map(|n| ServicePolicy {
                    name: n.clone(),
                    ..Default::default()
                })
                .collect(),
        };
        Self::from_policy_file(file)
    }

    pub(crate) fn from_policy_file(file: PolicyFile) -> Self {
        let services = file
            .services
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();
        Self {
            global_read_only: file.server.read_only,
            max_request_bytes: file.server.max_request_bytes,
            rate_limit_rpm: file.server.rate_limit_rpm,
            allowed_origins: file.server.allowed_origins,
            credentials_file: file.server.credentials_file,
            project_id: file.server.project_id,
            services,
        }
    }

    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        let host = url::Url::parse(origin)
            .ok()
            .and_then(|u| u.host_str().map(String::from));
        let Some(host) = host else {
            return false;
        };

        if self.allowed_origins.is_empty() {
            return host == "localhost" || host == "127.0.0.1" || host == "::1";
        }
        self.allowed_origins.iter().any(|o| host == o.as_str())
    }

    pub fn allowed_services(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.services.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    pub fn is_service_allowed(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }

    pub fn is_read_only(&self, service: &str) -> bool {
        if self.global_read_only {
            return true;
        }
        self.services
            .get(service)
            .and_then(|s| s.read_only)
            .unwrap_or(false)
    }

    pub fn denied_methods(&self, service: &str) -> HashSet<&str> {
        self.services
            .get(service)
            .map(|s| s.denied_methods.iter().map(|m| m.as_str()).collect())
            .unwrap_or_default()
    }

    pub fn constraints(&self, service: &str) -> &[Constraint] {
        self.services
            .get(service)
            .map(|s| s.constraints.as_slice())
            .unwrap_or(&[])
    }

    pub fn check_method(
        &self,
        service: &str,
        resource: &str,
        method_name: &str,
        method: &RestMethod,
    ) -> Result<(), GwsError> {
        if !self.is_service_allowed(service) {
            return Err(GwsError::Validation(format!(
                "Service '{service}' is not allowed by policy. \
                 Fix: add {{\"name\": \"{service}\"}} to the \"services\" array in your policy file"
            )));
        }

        let is_write = method.http_method != "GET";

        if self.is_read_only(service) && is_write {
            return Err(GwsError::Validation(format!(
                "Service '{service}' is read-only; {method_name} ({}) is not allowed. \
                 Fix: set \"read_only\": false on the \"{service}\" service in your policy file",
                method.http_method
            )));
        }

        let denied = self.denied_methods(service);
        let full_name = format!("{resource}.{method_name}");
        if denied.contains(method_name) || denied.contains(full_name.as_str()) {
            return Err(GwsError::Validation(format!(
                "Method '{full_name}' is denied by policy. \
                 Fix: remove \"{method_name}\" from \"denied_methods\" in the \"{service}\" service"
            )));
        }

        Ok(())
    }

    pub fn enforce_constraints(
        &self,
        service: &str,
        method: &RestMethod,
        params: &mut serde_json::Map<String, serde_json::Value>,
        body: &Option<serde_json::Value>,
    ) -> Result<(), GwsError> {
        let constraints = self.constraints(service);
        if constraints.is_empty() {
            return Ok(());
        }

        let is_write = method.http_method != "GET";

        let mut by_param: HashMap<&str, Vec<&Constraint>> = HashMap::new();
        for c in constraints {
            by_param.entry(c.param.as_str()).or_default().push(c);
        }

        for (param, group) in &by_param {
            let all_values: Vec<&str> = group
                .iter()
                .flat_map(|c| c.values.iter().map(|v| v.as_str()))
                .collect();
            let rw_values: Vec<&str> = group
                .iter()
                .filter(|c| c.access == Access::ReadWrite)
                .flat_map(|c| c.values.iter().map(|v| v.as_str()))
                .collect();

            let location = group[0].location.as_deref();
            let is_body = location == Some("body");

            if is_body {
                self.enforce_body_constraint(
                    param,
                    &all_values,
                    &rw_values,
                    is_write,
                    params,
                    body,
                )?;
                if is_write {
                    self.enforce_parent_params(param, &all_values, &rw_values, params)?;
                }
            } else {
                self.enforce_param_constraint(param, &all_values, &rw_values, is_write, params)?;
            }
        }

        Ok(())
    }

    fn enforce_body_constraint(
        &self,
        param: &str,
        all_values: &[&str],
        rw_values: &[&str],
        is_write: bool,
        params: &mut serde_json::Map<String, serde_json::Value>,
        body: &Option<serde_json::Value>,
    ) -> Result<(), GwsError> {
        if !is_write {
            let filter_parts: Vec<String> = all_values
                .iter()
                .map(|id| format!("'{id}' in {param}"))
                .collect();
            let constraint = filter_parts.join(" or ");

            if let Some(serde_json::Value::String(existing)) = params.get("q") {
                let combined = format!("({existing}) and ({constraint})");
                params.insert("q".to_string(), serde_json::Value::String(combined));
            } else {
                params.insert("q".to_string(), serde_json::Value::String(constraint));
            }
            return Ok(());
        }

        let body_values: Vec<&str> = body
            .as_ref()
            .and_then(|b| b.get(param))
            .map(|v| match v {
                serde_json::Value::Array(arr) => arr.iter().filter_map(|v| v.as_str()).collect(),
                serde_json::Value::String(s) => vec![s.as_str()],
                _ => vec![],
            })
            .unwrap_or_default();

        if body_values.is_empty() {
            return Err(GwsError::Validation(format!(
                "Write denied: '{param}' must be specified when constraints are configured. \
                 Allowed read-write values: {rw}. \
                 Fix: include \"{param}\" in the request body",
                rw = rw_values.join(", ")
            )));
        }

        for val in &body_values {
            if !all_values.contains(val) {
                return Err(GwsError::Validation(format!(
                    "Write denied: {param} value '{val}' is not in the allowed list. \
                     Fix: add it to a constraint on \"{param}\" in your policy file"
                )));
            }
            if !rw_values.contains(val) {
                return Err(GwsError::Validation(format!(
                    "Write denied: {param} value '{val}' is read-only. \
                     Fix: change its access to \"read-write\" in your policy file"
                )));
            }
        }

        Ok(())
    }

    fn enforce_parent_params(
        &self,
        param: &str,
        all_values: &[&str],
        rw_values: &[&str],
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), GwsError> {
        let add_key = format!("add{}", capitalize(param));
        let remove_key = format!("remove{}", capitalize(param));

        for key in [&add_key, &remove_key] {
            if let Some(serde_json::Value::String(ids)) = params.get(key.as_str()) {
                for id in ids.split(',').map(|s| s.trim()) {
                    if id.is_empty() {
                        continue;
                    }
                    if !all_values.contains(&id) {
                        return Err(GwsError::Validation(format!(
                            "Write denied via {key}: '{id}' is not in the allowed list. \
                             Fix: add it to a constraint on \"{param}\" in your policy file"
                        )));
                    }
                    if !rw_values.contains(&id) {
                        return Err(GwsError::Validation(format!(
                            "Write denied via {key}: '{id}' is read-only. \
                             Fix: change its access to \"read-write\" in your policy file"
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    fn enforce_param_constraint(
        &self,
        param: &str,
        all_values: &[&str],
        rw_values: &[&str],
        is_write: bool,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), GwsError> {
        let Some(serde_json::Value::String(value)) = params.get(param) else {
            return Err(GwsError::Validation(format!(
                "Constraints are configured for '{param}' but it was not specified. \
                 Allowed: {all}. \
                 Fix: include \"{param}\" in the request params",
                all = all_values.join(", ")
            )));
        };

        if !all_values.contains(&value.as_str()) {
            return Err(GwsError::Validation(format!(
                "Value '{value}' for '{param}' is not allowed by policy. Allowed: {all}. \
                 Fix: add {{\"param\": \"{param}\", \"values\": [\"{value}\"]}} \
                 to the \"constraints\" array in your policy file",
                all = all_values.join(", ")
            )));
        }

        if is_write && !rw_values.contains(&value.as_str()) {
            return Err(GwsError::Validation(format!(
                "'{param}' value '{value}' is read-only; write operations are not allowed. \
                 Fix: change its access to \"read-write\" in your policy file"
            )));
        }

        Ok(())
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> Policy {
        let json_str = r#"{
            "server": { "read_only": false },
            "services": [
                {
                    "name": "drive",
                    "constraints": [
                        { "param": "parents", "values": ["folder-readonly"], "access": "read-only", "location": "body" },
                        { "param": "parents", "values": ["folder-readwrite"], "access": "read-write", "location": "body" }
                    ]
                },
                {
                    "name": "calendar",
                    "constraints": [
                        { "param": "calendarId", "values": ["primary"], "access": "read-write" },
                        { "param": "calendarId", "values": ["holidays"], "access": "read-only" }
                    ]
                },
                {
                    "name": "gmail",
                    "denied_methods": ["messages.delete", "messages.trash"]
                },
                {
                    "name": "sheets",
                    "read_only": true
                }
            ]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        Policy::from_policy_file(file)
    }

    fn get_method() -> RestMethod {
        RestMethod {
            http_method: "GET".to_string(),
            ..Default::default()
        }
    }

    fn post_method() -> RestMethod {
        RestMethod {
            http_method: "POST".to_string(),
            ..Default::default()
        }
    }

    // -- Service & method tests --

    #[test]
    fn test_service_allowed() {
        let p = test_policy();
        assert!(p.is_service_allowed("drive"));
        assert!(p.is_service_allowed("gmail"));
        assert!(p.is_service_allowed("sheets"));
        assert!(!p.is_service_allowed("docs"));
    }

    #[test]
    fn test_read_only_service() {
        let p = test_policy();
        assert!(p.is_read_only("sheets"));
        assert!(!p.is_read_only("drive"));
        assert!(!p.is_read_only("gmail"));
    }

    #[test]
    fn test_global_read_only() {
        let json_str = r#"{
            "server": { "read_only": true },
            "services": [{ "name": "drive" }]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert!(p.is_read_only("drive"));
    }

    #[test]
    fn test_denied_methods() {
        let p = test_policy();
        let denied = p.denied_methods("gmail");
        assert!(denied.contains("messages.delete"));
        assert!(denied.contains("messages.trash"));
        assert!(!denied.contains("messages.list"));
    }

    // -- Body constraint tests (Drive folders pattern) --

    #[test]
    fn test_body_constraint_list_injects_query() {
        let p = test_policy();
        let method = get_method();
        let mut params = serde_json::Map::new();
        p.enforce_constraints("drive", &method, &mut params, &None)
            .unwrap();
        let q = params.get("q").unwrap().as_str().unwrap();
        assert!(q.contains("'folder-readonly' in parents"));
        assert!(q.contains("'folder-readwrite' in parents"));
    }

    #[test]
    fn test_body_constraint_list_merges_existing_query() {
        let p = test_policy();
        let method = get_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "q".to_string(),
            serde_json::Value::String("mimeType='application/pdf'".to_string()),
        );
        p.enforce_constraints("drive", &method, &mut params, &None)
            .unwrap();
        let q = params.get("q").unwrap().as_str().unwrap();
        assert!(q.contains("mimeType='application/pdf'"));
        assert!(q.contains("'folder-readonly' in parents"));
    }

    #[test]
    fn test_body_constraint_write_allowed_to_rw() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        let body = Some(serde_json::json!({
            "parents": ["folder-readwrite"],
            "name": "test.txt"
        }));
        assert!(
            p.enforce_constraints("drive", &method, &mut params, &body)
                .is_ok()
        );
    }

    #[test]
    fn test_body_constraint_write_denied_to_readonly() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        let body = Some(serde_json::json!({
            "parents": ["folder-readonly"],
            "name": "test.txt"
        }));
        let err = p
            .enforce_constraints("drive", &method, &mut params, &body)
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_body_constraint_write_denied_to_unknown() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        let body = Some(serde_json::json!({
            "parents": ["unknown-folder"],
            "name": "test.txt"
        }));
        let err = p
            .enforce_constraints("drive", &method, &mut params, &body)
            .unwrap_err();
        assert!(err.to_string().contains("not in the allowed list"));
    }

    #[test]
    fn test_body_constraint_write_denied_without_parents() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        let body = Some(serde_json::json!({ "name": "test.txt" }));
        let err = p
            .enforce_constraints("drive", &method, &mut params, &body)
            .unwrap_err();
        assert!(err.to_string().contains("parents"));
    }

    #[test]
    fn test_body_constraint_no_restrictions_passes() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        let body = Some(serde_json::json!({ "name": "test.txt" }));
        assert!(
            p.enforce_constraints("gmail", &method, &mut params, &body)
                .is_ok()
        );
    }

    // -- addParents/removeParents tests --

    #[test]
    fn test_parent_params_add_to_rw_allowed() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("folder-readwrite".to_string()),
        );
        let body = Some(serde_json::json!({ "parents": ["folder-readwrite"] }));
        assert!(
            p.enforce_constraints("drive", &method, &mut params, &body)
                .is_ok()
        );
    }

    #[test]
    fn test_parent_params_add_to_readonly_denied() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("folder-readonly".to_string()),
        );
        let body = Some(serde_json::json!({ "parents": ["folder-readwrite"] }));
        let err = p
            .enforce_constraints("drive", &method, &mut params, &body)
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_parent_params_remove_unknown_denied() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "removeParents".to_string(),
            serde_json::Value::String("unknown-folder".to_string()),
        );
        let body = Some(serde_json::json!({ "parents": ["folder-readwrite"] }));
        let err = p
            .enforce_constraints("drive", &method, &mut params, &body)
            .unwrap_err();
        assert!(err.to_string().contains("not in the allowed list"));
    }

    #[test]
    fn test_parent_params_multiple_ids() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("folder-readwrite, folder-readonly".to_string()),
        );
        let body = Some(serde_json::json!({ "parents": ["folder-readwrite"] }));
        let err = p
            .enforce_constraints("drive", &method, &mut params, &body)
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    // -- Param constraint tests (Calendar pattern) --

    #[test]
    fn test_param_constraint_read_allowed() {
        let p = test_policy();
        let method = get_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("holidays".to_string()),
        );
        assert!(
            p.enforce_constraints("calendar", &method, &mut params, &None)
                .is_ok()
        );
    }

    #[test]
    fn test_param_constraint_write_denied_on_readonly() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("holidays".to_string()),
        );
        let err = p
            .enforce_constraints("calendar", &method, &mut params, &None)
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_param_constraint_write_allowed_on_rw() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("primary".to_string()),
        );
        assert!(
            p.enforce_constraints("calendar", &method, &mut params, &None)
                .is_ok()
        );
    }

    #[test]
    fn test_param_constraint_unknown_denied() {
        let p = test_policy();
        let method = get_method();
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("secret@group.calendar.google.com".to_string()),
        );
        let err = p
            .enforce_constraints("calendar", &method, &mut params, &None)
            .unwrap_err();
        assert!(err.to_string().contains("not allowed by policy"));
    }

    #[test]
    fn test_param_constraint_missing_denied() {
        let p = test_policy();
        let method = get_method();
        let mut params = serde_json::Map::new();
        let err = p
            .enforce_constraints("calendar", &method, &mut params, &None)
            .unwrap_err();
        assert!(err.to_string().contains("calendarId"));
    }

    // -- Generic constraint tests (new patterns) --

    #[test]
    fn test_constraint_on_spreadsheet_id() {
        let json_str = r#"{
            "services": [{
                "name": "sheets",
                "constraints": [
                    { "param": "spreadsheetId", "values": ["abc123"], "access": "read-write" }
                ]
            }]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        let p = Policy::from_policy_file(file);

        let mut method = get_method();
        method
            .parameters
            .insert("spreadsheetId".to_string(), Default::default());

        let mut params = serde_json::Map::new();
        params.insert(
            "spreadsheetId".to_string(),
            serde_json::Value::String("abc123".to_string()),
        );
        assert!(
            p.enforce_constraints("sheets", &method, &mut params, &None)
                .is_ok()
        );

        params.insert(
            "spreadsheetId".to_string(),
            serde_json::Value::String("other".to_string()),
        );
        assert!(
            p.enforce_constraints("sheets", &method, &mut params, &None)
                .is_err()
        );
    }

    #[test]
    fn test_no_constraints_passes() {
        let p = test_policy();
        let method = post_method();
        let mut params = serde_json::Map::new();
        let body = Some(serde_json::json!({ "name": "test.txt" }));
        assert!(
            p.enforce_constraints("gmail", &method, &mut params, &body)
                .is_ok()
        );
    }

    #[test]
    fn test_no_constraints_skips_query_injection() {
        let p = test_policy();
        let method = get_method();
        let mut params = serde_json::Map::new();
        p.enforce_constraints("gmail", &method, &mut params, &None)
            .unwrap();
        assert!(params.get("q").is_none());
    }

    // -- Origin tests --

    #[test]
    fn test_origin_default_allows_localhost() {
        let p = test_policy();
        assert!(p.is_origin_allowed("http://localhost:3000"));
        assert!(p.is_origin_allowed("http://127.0.0.1:8080"));
    }

    #[test]
    fn test_origin_default_rejects_remote() {
        let p = test_policy();
        assert!(!p.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn test_origin_rejects_substring_bypass() {
        let p = test_policy();
        assert!(!p.is_origin_allowed("https://evil-localhost.com"));
        assert!(!p.is_origin_allowed("https://localhost.evil.com"));
        assert!(!p.is_origin_allowed("https://127.0.0.1.evil.com"));
        assert!(!p.is_origin_allowed("https://not-localhost.com"));
    }

    #[test]
    fn test_origin_rejects_invalid() {
        let p = test_policy();
        assert!(!p.is_origin_allowed("not-a-url"));
        assert!(!p.is_origin_allowed(""));
    }

    #[test]
    fn test_origin_custom_allowlist() {
        let json_str = r#"{
            "server": { "allowed_origins": ["internal.corp.com", "dashboard.example.com"] },
            "services": [{ "name": "drive" }]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert!(p.is_origin_allowed("https://internal.corp.com"));
        assert!(p.is_origin_allowed("https://dashboard.example.com"));
        assert!(!p.is_origin_allowed("http://localhost:3000"));
        assert!(!p.is_origin_allowed("https://evil.com"));
    }

    #[test]
    fn test_origin_custom_allowlist_rejects_substring() {
        let json_str = r#"{
            "server": { "allowed_origins": ["corp.example.com"] },
            "services": [{ "name": "drive" }]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert!(p.is_origin_allowed("https://corp.example.com"));
        assert!(!p.is_origin_allowed("https://evil-corp.example.com"));
        assert!(!p.is_origin_allowed("https://corp.example.com.evil.com"));
    }

    // -- Security config tests --

    #[test]
    fn test_default_max_request_bytes() {
        let p = test_policy();
        assert_eq!(p.max_request_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn test_custom_max_request_bytes() {
        let json_str = r#"{
            "server": { "max_request_bytes": 1048576 },
            "services": [{ "name": "drive" }]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert_eq!(p.max_request_bytes, 1_048_576);
    }

    #[test]
    fn test_rate_limit_config() {
        let json_str = r#"{
            "server": { "rate_limit_rpm": 60 },
            "services": [{ "name": "drive" }]
        }"#;
        let file: PolicyFile = serde_json::from_str(json_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert_eq!(p.rate_limit_rpm, Some(60));
    }

    #[test]
    fn test_no_rate_limit_by_default() {
        let p = test_policy();
        assert!(p.rate_limit_rpm.is_none());
    }
}
