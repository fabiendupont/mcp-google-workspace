use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use google_workspace::discovery::RestDescription;
use google_workspace::error::GwsError;

use crate::helpers::Position;
use crate::meta::RequestMeta;
use crate::policy::Policy;
use crate::tasks;
use crate::tools;

pub struct ServerState {
    pub(crate) tools: Option<Vec<rmcp::model::Tool>>,
    pub(crate) docs: HashMap<String, Arc<RestDescription>>,
    pub(crate) tasks: HashMap<String, tasks::Task>,
    pub(crate) token_cache: Option<crate::auth::TokenCache>,
    pub(crate) audit: Option<Arc<crate::audit::AuditLogger>>,
    pub(crate) prompts: Vec<crate::prompts::Prompt>,
    pub(crate) subscriptions: Arc<tokio::sync::Mutex<crate::subscriptions::SubscriptionMap>>,
    pub(crate) webhook_url: Option<String>,
    pub(crate) sheet_cache: crate::cache::SheetCache,
    pub(crate) activated_services: std::collections::HashSet<String>,
    pub(crate) eager_tools: bool,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            tools: None,
            docs: HashMap::new(),
            tasks: HashMap::new(),
            token_cache: None,
            audit: None,
            prompts: Vec::new(),
            subscriptions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            webhook_url: None,
            sheet_cache: crate::cache::SheetCache::new(20, 300),
            activated_services: std::collections::HashSet::new(),
            eager_tools: false,
        }
    }

    pub fn with_config(
        prompts: Vec<crate::prompts::Prompt>,
        audit: Option<Arc<crate::audit::AuditLogger>>,
        eager_tools: bool,
        webhook_url: Option<String>,
    ) -> Self {
        let mut s = Self::new();
        s.prompts = prompts;
        s.audit = audit;
        s.eager_tools = eager_tools;
        s.webhook_url = webhook_url;
        s
    }

    pub(crate) async fn get_doc(
        &mut self,
        svc_alias: &str,
    ) -> Result<Arc<RestDescription>, GwsError> {
        tools::get_or_fetch_doc(&mut self.docs, svc_alias).await
    }

    pub(crate) fn clean_expired_sessions(&mut self) {
        tasks::clean_expired_tasks(&mut self.tasks);
    }
}

pub fn check_api_result(result: &Value) -> Result<(), GwsError> {
    if let Some(err) = result.get("error") {
        let msg = if let Some(s) = err.as_str() {
            s.to_string()
        } else {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            let status = err.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status.is_empty() {
                format!("API error {code}: {message}")
            } else {
                format!("API error {code} ({status}): {message}")
            }
        };
        return Err(GwsError::Validation(msg));
    }
    if result.get("validation_error").is_some() {
        let msg = result["errors"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e["hint"].as_str())
            .unwrap_or("Validation failed");
        return Err(GwsError::Validation(msg.to_string()));
    }
    Ok(())
}

pub fn parse_position(arguments: &Value) -> Position {
    if let Some(idx) = arguments.get("index").and_then(|v| v.as_i64()) {
        return Position::Index(idx as i32);
    }
    match arguments.get("position").and_then(|v| v.as_str()) {
        Some("start") => Position::Start,
        _ => Position::End,
    }
}

pub async fn is_descendant_of(
    folder_id: &str,
    allowed_roots: &[&str],
    state: &mut ServerState,
    policy: &Policy,
    meta: &RequestMeta,
) -> bool {
    if allowed_roots.is_empty() || allowed_roots.contains(&folder_id) {
        return true;
    }
    let drive_doc = match state.get_doc("drive").await {
        Ok(d) => d,
        Err(_) => return false,
    };
    let files_resource = match tools::find_resource(&drive_doc.resources, "files") {
        Some(r) => r,
        None => return false,
    };
    let get_method = match files_resource.methods.get("get") {
        Some(m) => m,
        None => return false,
    };

    let mut current = folder_id.to_string();
    for _ in 0..10 {
        if allowed_roots.contains(&current.as_str()) {
            return true;
        }
        let args = json!({"params": {"fileId": current}, "fields": "parents"});
        match crate::execute::execute_tool(
            &drive_doc,
            get_method,
            "files",
            "get",
            &args,
            "drive",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await
        {
            Ok(result) => {
                let parent = result
                    .get("parents")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str());
                match parent {
                    Some(p) => current = p.to_string(),
                    None => return false,
                }
            }
            Err(_) => return false,
        }
    }
    false
}

pub async fn policy_for_folder(
    folder_id: Option<&str>,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<(Policy, Option<String>), GwsError> {
    let resolved = match folder_id {
        Some(fid) => Some(fid.to_string()),
        None => crate::elicitation::resolve_target_folder(None, policy)?,
    };
    let Some(ref fid) = resolved else {
        return Ok((policy.clone(), None));
    };
    let roots = policy.recursive_parent_values("drive");
    if roots.is_empty() || roots.contains(&fid.as_str()) {
        return Ok((policy.clone(), resolved));
    }
    if is_descendant_of(fid, &roots, state, policy, meta).await {
        Ok((policy.with_extra_parent("drive", fid), resolved))
    } else {
        Err(GwsError::Validation(format!(
            "Folder '{fid}' is not inside an allowed root folder. \
             Allowed roots: {}",
            roots.join(", ")
        )))
    }
}

pub async fn make_image_insertable(
    file_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<(String, Option<String>), GwsError> {
    let drive_doc = state.get_doc("drive").await?;
    let resource = tools::find_resource(&drive_doc.resources, "permissions")
        .ok_or_else(|| GwsError::Validation("permissions resource not found".into()))?;
    let create_method = resource
        .methods
        .get("create")
        .ok_or_else(|| GwsError::Validation("create method not found".into()))?;
    let perm_args = json!({
        "params": { "fileId": file_id },
        "body": { "role": "reader", "type": "anyone" }
    });
    let perm_result = crate::execute::execute_tool(
        &drive_doc,
        create_method,
        "permissions",
        "create",
        &perm_args,
        "drive",
        policy,
        meta,
        None,
        None,
        false,
        &mut state.token_cache,
    )
    .await?;
    if let Some(err) = perm_result.get("error") {
        let msg = err.as_str().unwrap_or("unknown error");
        return Err(GwsError::Api {
            code: perm_result["status"].as_u64().unwrap_or(403) as u16,
            message: format!(
                "Cannot make image publicly accessible for Docs insertion: {msg}. \
                 The image is in Drive (file ID: {file_id}). Insert via Docs UI: Insert > Image > Drive."
            ),
            reason: "sharingBlocked".into(),
            enable_url: None,
        });
    }
    let perm_id = perm_result["id"]
        .as_str()
        .unwrap_or("anyoneWithLink")
        .to_string();
    let url = format!("https://drive.google.com/uc?export=download&id={file_id}");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    Ok((url, Some(perm_id)))
}

pub async fn revoke_image_sharing(
    file_id: &str,
    permission_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) {
    if let Ok(drive_doc) = state.get_doc("drive").await
        && let Some(resource) = tools::find_resource(&drive_doc.resources, "permissions")
        && let Some(delete_method) = resource.methods.get("delete")
    {
        let _ = crate::execute::execute_tool(
            &drive_doc,
            delete_method,
            "permissions",
            "delete",
            &json!({"params": {"fileId": file_id, "permissionId": permission_id}}),
            "drive",
            policy,
            meta,
            None,
            None,
            false,
            &mut state.token_cache,
        )
        .await;
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}
