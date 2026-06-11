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

#[derive(Debug, Deserialize)]
pub struct FolderPolicy {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub path: Option<String>,
    pub access: Access,
}

#[derive(Debug, Deserialize)]
pub struct CalendarPolicy {
    pub id: String,
    #[serde(default = "default_read_write")]
    pub access: Access,
}

fn default_read_write() -> Access {
    Access::ReadWrite
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
}

impl Default for ServerPolicy {
    fn default() -> Self {
        Self {
            read_only: false,
            max_request_bytes: default_max_request_bytes(),
            rate_limit_rpm: None,
            allowed_origins: Vec::new(),
        }
    }
}

fn default_max_request_bytes() -> usize {
    16 * 1024 * 1024
}

#[derive(Debug, Deserialize, Default)]
pub struct ServicePolicy {
    pub name: String,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub denied_methods: Vec<String>,
    #[serde(default)]
    pub folders: Vec<FolderPolicy>,
    #[serde(default)]
    pub calendars: Vec<CalendarPolicy>,
}

#[derive(Debug)]
pub struct Policy {
    pub global_read_only: bool,
    pub max_request_bytes: usize,
    pub rate_limit_rpm: Option<u32>,
    pub allowed_origins: Vec<String>,
    services: HashMap<String, ServicePolicy>,
}

impl Policy {
    pub fn from_file(path: &Path) -> Result<Self, GwsError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| GwsError::Validation(format!("Failed to read policy file: {e}")))?;
        let file: PolicyFile = toml::from_str(&content)
            .map_err(|e| GwsError::Validation(format!("Invalid policy TOML: {e}")))?;
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
            services,
        }
    }

    /// Resolve any `path`-based folder entries to IDs via the Drive API.
    /// Must be called before the server starts accepting requests.
    pub async fn resolve_folder_paths(&mut self) -> Result<(), GwsError> {
        let Some(svc) = self.services.get("drive") else {
            return Ok(());
        };

        let needs_resolution = svc
            .folders
            .iter()
            .any(|f| f.path.is_some() && f.id.is_empty());
        if !needs_resolution {
            return Ok(());
        }

        let scopes = &["https://www.googleapis.com/auth/drive.metadata.readonly"];
        let token = crate::auth::get_token(scopes).await.map_err(|e| {
            GwsError::Auth(format!(
                "Cannot resolve Drive folder paths without authentication: {e}"
            ))
        })?;

        let svc = self.services.get_mut("drive").unwrap();
        for folder in &mut svc.folders {
            if let Some(ref path) = folder.path
                && folder.id.is_empty()
            {
                let id = crate::resolve::resolve_drive_path(path, &token).await?;
                tracing::info!(path = %path, folder_id = %id, "Resolved Drive folder path");
                folder.id = id;
            }
        }

        // Validate no empty IDs remain
        for folder in &svc.folders {
            if folder.id.is_empty() {
                return Err(GwsError::Validation(format!(
                    "Folder entry has neither 'id' nor 'path': {:?}",
                    folder
                )));
            }
        }

        Ok(())
    }

    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        if self.allowed_origins.is_empty() {
            return origin.contains("localhost") || origin.contains("127.0.0.1");
        }
        self.allowed_origins
            .iter()
            .any(|o| origin.contains(o.as_str()))
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

    pub fn has_folder_restrictions(&self, service: &str) -> bool {
        self.services
            .get(service)
            .is_some_and(|s| !s.folders.is_empty())
    }

    pub fn folder_ids_with_access(&self, service: &str, access: Access) -> Vec<&str> {
        self.services
            .get(service)
            .map(|s| {
                s.folders
                    .iter()
                    .filter(|f| f.access == access)
                    .map(|f| f.id.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn all_folder_ids(&self, service: &str) -> Vec<&str> {
        self.services
            .get(service)
            .map(|s| s.folders.iter().map(|f| f.id.as_str()).collect())
            .unwrap_or_default()
    }

    pub fn folder_access(&self, service: &str, folder_id: &str) -> Option<Access> {
        self.services
            .get(service)?
            .folders
            .iter()
            .find(|f| f.id == folder_id)
            .map(|f| f.access)
    }

    pub fn calendars(&self, service: &str) -> &[CalendarPolicy] {
        self.services
            .get(service)
            .map(|s| s.calendars.as_slice())
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
                "Service '{service}' is not allowed by policy"
            )));
        }

        let is_write = method.http_method != "GET";

        // Service-level read-only check (separate from folder-level)
        if self.is_read_only(service) && is_write {
            return Err(GwsError::Validation(format!(
                "Service '{service}' is read-only; {method_name} ({}) is not allowed",
                method.http_method
            )));
        }

        let denied = self.denied_methods(service);
        let full_name = format!("{resource}.{method_name}");
        if denied.contains(method_name) || denied.contains(full_name.as_str()) {
            return Err(GwsError::Validation(format!(
                "Method '{full_name}' is denied by policy"
            )));
        }

        Ok(())
    }

    /// For list operations: constrain `q` to all allowed folder IDs.
    pub fn enforce_drive_folder_list(
        &self,
        service: &str,
        params: &mut serde_json::Map<String, serde_json::Value>,
    ) {
        if !self.has_folder_restrictions(service) {
            return;
        }

        let all_ids = self.all_folder_ids(service);
        if all_ids.is_empty() {
            return;
        }

        let folder_query: Vec<String> = all_ids
            .iter()
            .map(|id| format!("'{id}' in parents"))
            .collect();
        let constraint = folder_query.join(" or ");

        if let Some(serde_json::Value::String(existing)) = params.get("q") {
            let combined = format!("({existing}) and ({constraint})");
            params.insert("q".to_string(), serde_json::Value::String(combined));
        } else {
            params.insert("q".to_string(), serde_json::Value::String(constraint));
        }
    }

    /// For write operations: verify that target folders have read-write access.
    pub fn enforce_drive_folder_write(
        &self,
        service: &str,
        body: &Option<serde_json::Value>,
    ) -> Result<(), GwsError> {
        if !self.has_folder_restrictions(service) {
            return Ok(());
        }

        let rw_ids = self.folder_ids_with_access(service, Access::ReadWrite);

        let parent_ids: Vec<&str> = body
            .as_ref()
            .and_then(|b| b.get("parents"))
            .and_then(|p| p.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        if parent_ids.is_empty() {
            if rw_ids.is_empty() {
                return Err(GwsError::Validation(
                    "Write denied: no folders have read-write access in this policy".to_string(),
                ));
            }
            return Ok(());
        }

        for pid in &parent_ids {
            match self.folder_access(service, pid) {
                Some(Access::ReadWrite) => {}
                Some(Access::ReadOnly) => {
                    return Err(GwsError::Validation(format!(
                        "Write denied: folder '{pid}' is read-only"
                    )));
                }
                None => {
                    return Err(GwsError::Validation(format!(
                        "Write denied: folder '{pid}' is not in the allowed list. \
                         Allowed read-write folders: {}",
                        rw_ids.join(", ")
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn enforce_drive_folder_params(
        &self,
        service: &str,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), GwsError> {
        if !self.has_folder_restrictions(service) {
            return Ok(());
        }

        for key in ["addParents", "removeParents"] {
            if let Some(serde_json::Value::String(ids)) = params.get(key) {
                for id in ids.split(',').map(|s| s.trim()) {
                    if id.is_empty() {
                        continue;
                    }
                    match self.folder_access(service, id) {
                        Some(Access::ReadWrite) => {}
                        Some(Access::ReadOnly) => {
                            return Err(GwsError::Validation(format!(
                                "Write denied via {key}: folder '{id}' is read-only"
                            )));
                        }
                        None => {
                            let rw_ids = self.folder_ids_with_access(service, Access::ReadWrite);
                            return Err(GwsError::Validation(format!(
                                "Write denied via {key}: folder '{id}' is not in the allowed list. \
                                 Allowed read-write folders: {}",
                                rw_ids.join(", ")
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn enforce_calendar(
        &self,
        service: &str,
        method: &RestMethod,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), GwsError> {
        let cals = self.calendars(service);
        if cals.is_empty() {
            return Ok(());
        }

        let Some(serde_json::Value::String(cal_id)) = params.get("calendarId") else {
            return Ok(());
        };

        let cal = cals.iter().find(|c| c.id == *cal_id);
        match cal {
            Some(c) => {
                if c.access == Access::ReadOnly && method.http_method != "GET" {
                    return Err(GwsError::Validation(format!(
                        "Calendar '{cal_id}' is read-only; {} is not allowed",
                        method.http_method
                    )));
                }
                Ok(())
            }
            None => Err(GwsError::Validation(format!(
                "Calendar '{cal_id}' is not allowed by policy. Allowed: {}",
                cals.iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> Policy {
        let toml_str = r#"
[server]
read_only = false

[[services]]
name = "drive"

[[services.folders]]
id = "folder-readonly"
access = "read-only"

[[services.folders]]
id = "folder-readwrite"
access = "read-write"

[[services]]
name = "calendar"

[[services.calendars]]
id = "primary"
access = "read-write"

[[services.calendars]]
id = "holidays"
access = "read-only"

[[services]]
name = "gmail"
denied_methods = ["messages.delete", "messages.trash"]

[[services]]
name = "sheets"
read_only = true
"#;
        let file: PolicyFile = toml::from_str(toml_str).unwrap();
        Policy::from_policy_file(file)
    }

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
        let toml_str = r#"
[server]
read_only = true

[[services]]
name = "drive"
"#;
        let file: PolicyFile = toml::from_str(toml_str).unwrap();
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

    // -- Drive folder tests --

    #[test]
    fn test_folder_ids_by_access() {
        let p = test_policy();
        let ro = p.folder_ids_with_access("drive", Access::ReadOnly);
        assert_eq!(ro, vec!["folder-readonly"]);
        let rw = p.folder_ids_with_access("drive", Access::ReadWrite);
        assert_eq!(rw, vec!["folder-readwrite"]);
    }

    #[test]
    fn test_folder_list_constrains_query() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        p.enforce_drive_folder_list("drive", &mut params);
        let q = params.get("q").unwrap().as_str().unwrap();
        assert!(q.contains("'folder-readonly' in parents"));
        assert!(q.contains("'folder-readwrite' in parents"));
    }

    #[test]
    fn test_folder_list_merges_existing_query() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        params.insert(
            "q".to_string(),
            serde_json::Value::String("mimeType='application/pdf'".to_string()),
        );
        p.enforce_drive_folder_list("drive", &mut params);
        let q = params.get("q").unwrap().as_str().unwrap();
        assert!(q.contains("mimeType='application/pdf'"));
        assert!(q.contains("'folder-readonly' in parents"));
    }

    #[test]
    fn test_folder_write_allowed_to_rw() {
        let p = test_policy();
        let body = Some(serde_json::json!({
            "parents": ["folder-readwrite"],
            "name": "test.txt"
        }));
        assert!(p.enforce_drive_folder_write("drive", &body).is_ok());
    }

    #[test]
    fn test_folder_write_denied_to_readonly() {
        let p = test_policy();
        let body = Some(serde_json::json!({
            "parents": ["folder-readonly"],
            "name": "test.txt"
        }));
        let err = p.enforce_drive_folder_write("drive", &body).unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_folder_write_denied_to_unknown() {
        let p = test_policy();
        let body = Some(serde_json::json!({
            "parents": ["unknown-folder"],
            "name": "test.txt"
        }));
        let err = p.enforce_drive_folder_write("drive", &body).unwrap_err();
        assert!(err.to_string().contains("not in the allowed list"));
    }

    #[test]
    fn test_folder_write_no_restrictions_passes() {
        let p = test_policy();
        let body = Some(serde_json::json!({ "name": "test.txt" }));
        // gmail has no folder restrictions
        assert!(p.enforce_drive_folder_write("gmail", &body).is_ok());
    }

    // -- Drive folder param tests --

    #[test]
    fn test_folder_params_add_to_rw_allowed() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("folder-readwrite".to_string()),
        );
        assert!(p.enforce_drive_folder_params("drive", &params).is_ok());
    }

    #[test]
    fn test_folder_params_add_to_readonly_denied() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("folder-readonly".to_string()),
        );
        let err = p.enforce_drive_folder_params("drive", &params).unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_folder_params_remove_from_unknown_denied() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        params.insert(
            "removeParents".to_string(),
            serde_json::Value::String("unknown-folder".to_string()),
        );
        let err = p.enforce_drive_folder_params("drive", &params).unwrap_err();
        assert!(err.to_string().contains("not in the allowed list"));
    }

    #[test]
    fn test_folder_params_multiple_ids() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("folder-readwrite, folder-readonly".to_string()),
        );
        let err = p.enforce_drive_folder_params("drive", &params).unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_folder_params_no_restrictions_passes() {
        let p = test_policy();
        let mut params = serde_json::Map::new();
        params.insert(
            "addParents".to_string(),
            serde_json::Value::String("anything".to_string()),
        );
        assert!(p.enforce_drive_folder_params("gmail", &params).is_ok());
    }

    // -- Calendar tests --

    #[test]
    fn test_calendar_read_allowed() {
        let p = test_policy();
        let method = RestMethod {
            http_method: "GET".to_string(),
            ..Default::default()
        };
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("holidays".to_string()),
        );
        assert!(p.enforce_calendar("calendar", &method, &params).is_ok());
    }

    #[test]
    fn test_calendar_write_denied_on_readonly() {
        let p = test_policy();
        let method = RestMethod {
            http_method: "POST".to_string(),
            ..Default::default()
        };
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("holidays".to_string()),
        );
        let err = p
            .enforce_calendar("calendar", &method, &params)
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn test_calendar_write_allowed_on_rw() {
        let p = test_policy();
        let method = RestMethod {
            http_method: "POST".to_string(),
            ..Default::default()
        };
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("primary".to_string()),
        );
        assert!(p.enforce_calendar("calendar", &method, &params).is_ok());
    }

    #[test]
    fn test_calendar_unknown_denied() {
        let p = test_policy();
        let method = RestMethod {
            http_method: "GET".to_string(),
            ..Default::default()
        };
        let mut params = serde_json::Map::new();
        params.insert(
            "calendarId".to_string(),
            serde_json::Value::String("secret@group.calendar.google.com".to_string()),
        );
        let err = p
            .enforce_calendar("calendar", &method, &params)
            .unwrap_err();
        assert!(err.to_string().contains("not allowed by policy"));
    }

    #[test]
    fn test_no_folder_restrictions_skips_enforcement() {
        let p = test_policy();
        assert!(!p.has_folder_restrictions("gmail"));
        let mut params = serde_json::Map::new();
        p.enforce_drive_folder_list("gmail", &mut params);
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
    fn test_origin_custom_allowlist() {
        let toml_str = r#"
[server]
allowed_origins = ["internal.corp.com", "dashboard.example.com"]
[[services]]
name = "drive"
"#;
        let file: PolicyFile = toml::from_str(toml_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert!(p.is_origin_allowed("https://internal.corp.com"));
        assert!(p.is_origin_allowed("https://dashboard.example.com"));
        assert!(!p.is_origin_allowed("http://localhost:3000"));
        assert!(!p.is_origin_allowed("https://evil.com"));
    }

    // -- Security config tests --

    #[test]
    fn test_default_max_request_bytes() {
        let p = test_policy();
        assert_eq!(p.max_request_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn test_custom_max_request_bytes() {
        let toml_str = r#"
[server]
max_request_bytes = 1048576
[[services]]
name = "drive"
"#;
        let file: PolicyFile = toml::from_str(toml_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert_eq!(p.max_request_bytes, 1_048_576);
    }

    #[test]
    fn test_rate_limit_config() {
        let toml_str = r#"
[server]
rate_limit_rpm = 60
[[services]]
name = "drive"
"#;
        let file: PolicyFile = toml::from_str(toml_str).unwrap();
        let p = Policy::from_policy_file(file);
        assert_eq!(p.rate_limit_rpm, Some(60));
    }

    #[test]
    fn test_no_rate_limit_by_default() {
        let p = test_policy();
        assert!(p.rate_limit_rpm.is_none());
    }
}
