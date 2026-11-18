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

/// Build the refusal returned when policy would block the cleanup of a
/// temporary public permission.
fn image_cleanup_denied(detail: &str) -> GwsError {
    GwsError::Validation(format!(
        "Refusing to grant temporary anyone-with-link access to the generated image: \
         policy would block the cleanup that revokes it afterwards ({detail}). \
         Leaving the file public is not an acceptable outcome, so no permission was created. \
         Fix: allow the 'drive' service, keep 'permissions' in its \"allowed_resources\" list, \
         and do not deny 'delete' (remove it from \"denied_methods\")."
    ))
}

/// Verify — before anything is shared — that policy also permits the
/// `drive` `permissions.delete` call that later revokes the temporary
/// anyone-with-link permission.
///
/// [`make_image_insertable`] grants public read and
/// [`revoke_image_sharing_checked`] removes it, but the revoke goes through the
/// normal policy checks. A policy that allows the grant while denying the
/// delete would leave the image publicly readable forever, so the grant must
/// never happen when cleanup cannot run.
///
/// Pure (no I/O), so the decision is unit-testable: pass the cached drive
/// discovery document.
pub fn ensure_image_sharing_cleanup_allowed(
    drive_doc: &RestDescription,
    policy: &Policy,
) -> Result<(), GwsError> {
    if !policy.is_service_allowed("drive") {
        return Err(image_cleanup_denied(
            "the 'drive' service is not enabled in policy",
        ));
    }

    let resource = tools::find_resource(&drive_doc.resources, "permissions")
        .ok_or_else(|| image_cleanup_denied("Drive discovery has no 'permissions' resource"))?;

    crate::execute::ensure_resource_allowed("drive", "permissions", policy)
        .map_err(|err| image_cleanup_denied(&err.to_string()))?;

    let delete_method = resource.methods.get("delete").ok_or_else(|| {
        image_cleanup_denied("Drive discovery has no 'permissions.delete' method")
    })?;

    // Keep the refusal self-contained instead of echoing `check_method`'s text:
    // the transport masks messages containing "denied by policy"/"is read-only;",
    // which would hide the remediation from the caller.
    policy
        .check_method("drive", "permissions", "delete", delete_method)
        .map_err(|err| {
            tracing::debug!(error = %err, "image cleanup policy check failed");
            image_cleanup_denied(
                "the 'permissions.delete' cleanup call is blocked by policy \
                 (read-only 'drive' service or a 'delete' entry in \"denied_methods\")",
            )
        })?;

    Ok(())
}

/// Read the permission ID out of a successful `permissions.create` response.
///
/// The caller must revoke this permission once the image has been inserted, so
/// the ID has to be real. Synthesising one from the response shape (the old
/// `unwrap_or("anyoneWithLink")` fallback) would make the revoke target a
/// permission that does not exist: the API answers 404 and the file stays
/// link-readable while everything looks fine. A missing, blank or non-string
/// `id` is therefore a hard error that names the file so an operator can revoke
/// by hand.
///
/// Pure (no I/O), so the failure mode is unit-testable.
fn permission_id_from_response(result: &Value, file_id: &str) -> Result<String, GwsError> {
    if let Some(id) = result.get("id").and_then(|v| v.as_str())
        && !id.trim().is_empty()
    {
        return Ok(id.to_string());
    }
    Err(GwsError::Api {
        code: 500,
        message: format!(
            "Cannot make image publicly accessible for Docs insertion: Drive returned a \
             permissions.create response without an 'id' (file ID: {file_id}), so the temporary \
             public permission cannot be tracked for revocation. Treat the file as \
             link-readable and remove its 'anyone' permission manually (Drive > Share). \
             Alternatively insert via the Docs UI: Insert > Image > Drive."
        ),
        reason: "missingPermissionId".into(),
        enable_url: None,
    })
}

pub async fn make_image_insertable(
    file_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<(String, Option<String>), GwsError> {
    let drive_doc = state.get_doc("drive").await?;
    // Never create the permission when the matching revoke would be denied.
    ensure_image_sharing_cleanup_allowed(&drive_doc, policy)?;
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
    // Never invent an ID here: the revoke would 404 and leave the file public.
    let perm_id = permission_id_from_response(&perm_result, file_id)?;
    let url = format!("https://drive.google.com/uc?export=download&id={file_id}");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    Ok((url, Some(perm_id)))
}

/// Revoke public sharing for an image that was inserted into a Doc.
///
/// Returns the failure instead of swallowing it so the caller can surface that
/// the file is still publicly accessible.
///
/// No unit test: this requires live Drive credentials and network access. The
/// error plumbing is the shared `check_api_result` path exercised elsewhere;
/// the revoke path itself is covered by the e2e image-insert tests.
pub async fn revoke_image_sharing_checked(
    file_id: &str,
    permission_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) -> Result<(), GwsError> {
    let drive_doc = state.get_doc("drive").await?;
    let resource = tools::find_resource(&drive_doc.resources, "permissions")
        .ok_or_else(|| GwsError::Validation("permissions resource not found".into()))?;
    let delete_method = resource
        .methods
        .get("delete")
        .ok_or_else(|| GwsError::Validation("permissions.delete method not found".into()))?;

    let result = crate::execute::execute_tool(
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
    .await?;

    check_api_result(&result)
}

pub async fn revoke_image_sharing(
    file_id: &str,
    permission_id: &str,
    policy: &Policy,
    meta: &RequestMeta,
    state: &mut ServerState,
) {
    if let Err(err) =
        revoke_image_sharing_checked(file_id, permission_id, policy, meta, state).await
    {
        tracing::warn!(
            file_id = file_id,
            permission_id = permission_id,
            error = %err,
            "failed to revoke public image sharing; the file may still be publicly accessible"
        );
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_workspace::discovery::{RestMethod, RestResource};
    use std::collections::HashMap;

    fn drive_doc_with_permissions() -> RestDescription {
        let delete = RestMethod {
            http_method: "DELETE".to_string(),
            path: "files/{fileId}/permissions/{permissionId}".to_string(),
            ..Default::default()
        };
        let create = RestMethod {
            http_method: "POST".to_string(),
            path: "files/{fileId}/permissions".to_string(),
            ..Default::default()
        };

        let mut methods = HashMap::new();
        methods.insert("create".to_string(), create);
        methods.insert("delete".to_string(), delete);

        let mut resources = HashMap::new();
        resources.insert(
            "permissions".to_string(),
            RestResource {
                methods,
                resources: HashMap::new(),
            },
        );

        RestDescription {
            name: "drive".to_string(),
            version: "v3".to_string(),
            root_url: "https://www.googleapis.com/".to_string(),
            service_path: "drive/v3/".to_string(),
            resources,
            ..Default::default()
        }
    }

    fn policy_with_drive(denied: &[&str], allowed_resources: &[&str]) -> Policy {
        Policy::from_policy_file(crate::policy::PolicyFile {
            services: vec![crate::policy::ServicePolicy {
                name: "drive".to_string(),
                denied_methods: denied.iter().map(|s| s.to_string()).collect(),
                allowed_resources: allowed_resources.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    #[test]
    fn test_cleanup_allowed_when_policy_permits_delete() {
        let doc = drive_doc_with_permissions();
        let policy = policy_with_drive(&["files.delete"], &[]);
        assert!(ensure_image_sharing_cleanup_allowed(&doc, &policy).is_ok());
    }

    #[test]
    fn test_cleanup_denied_when_drive_not_enabled() {
        let doc = drive_doc_with_permissions();
        let policy = Policy::from_policy_file(crate::policy::PolicyFile {
            services: vec![crate::policy::ServicePolicy {
                name: "docs".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });

        let err = ensure_image_sharing_cleanup_allowed(&doc, &policy).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("block the cleanup"), "{msg}");
        assert!(msg.contains("drive"), "{msg}");
    }

    #[test]
    fn test_cleanup_denied_when_permissions_resource_not_allowed() {
        let doc = drive_doc_with_permissions();
        let policy = policy_with_drive(&[], &["files"]);

        let err = ensure_image_sharing_cleanup_allowed(&doc, &policy).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("allowed_resources"), "{msg}");
        assert!(msg.contains("permissions"), "{msg}");
    }

    #[test]
    fn test_cleanup_denied_when_delete_denied() {
        let doc = drive_doc_with_permissions();
        let policy = policy_with_drive(&["permissions.delete"], &[]);

        let err = ensure_image_sharing_cleanup_allowed(&doc, &policy).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("block the cleanup"), "{msg}");
        assert!(msg.contains("blocked by policy"), "{msg}");
        assert!(msg.contains("denied_methods"), "{msg}");
    }

    #[test]
    fn test_cleanup_denied_when_drive_is_read_only() {
        let doc = drive_doc_with_permissions();
        let policy = Policy::from_policy_file(crate::policy::PolicyFile {
            services: vec![crate::policy::ServicePolicy {
                name: "drive".to_string(),
                read_only: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        });

        let err = ensure_image_sharing_cleanup_allowed(&doc, &policy).unwrap_err();
        assert!(err.to_string().contains("read-only"), "{err}");
    }

    #[test]
    fn test_cleanup_denied_when_permissions_resource_missing() {
        let doc = RestDescription {
            name: "drive".to_string(),
            version: "v3".to_string(),
            ..Default::default()
        };
        let policy = policy_with_drive(&[], &[]);

        let err = ensure_image_sharing_cleanup_allowed(&doc, &policy).unwrap_err();
        assert!(err.to_string().contains("permissions"), "{err}");
    }

    #[test]
    fn test_permission_id_missing_is_an_error_not_a_synthesised_id() {
        let err = permission_id_from_response(&json!({ "role": "reader" }), "file-1")
            .expect_err("a response without 'id' must not yield a permission id");
        match &err {
            GwsError::Api {
                code,
                message,
                reason,
                ..
            } => {
                assert_eq!(*code, 500);
                assert_eq!(reason.as_str(), "missingPermissionId");
                assert!(message.contains("without an 'id'"), "{message}");
                assert!(message.contains("file-1"), "{message}");
                assert!(message.contains("link-readable"), "{message}");
            }
            other => panic!("expected GwsError::Api, got {other:?}"),
        }
        // The old fallback must be gone.
        assert!(!err.to_string().contains("anyoneWithLink"));
    }

    #[test]
    fn test_permission_id_non_string_or_empty_is_an_error() {
        assert!(permission_id_from_response(&json!({ "id": 42 }), "file-1").is_err());
        assert!(permission_id_from_response(&json!({ "id": null }), "file-1").is_err());
        assert!(permission_id_from_response(&json!({ "id": "" }), "file-1").is_err());
        assert!(permission_id_from_response(&json!({ "id": "   " }), "file-1").is_err());
    }

    #[test]
    fn test_permission_id_present_is_returned() {
        let id = permission_id_from_response(&json!({ "id": "anyoneWithLink" }), "file-1").unwrap();
        assert_eq!(id, "anyoneWithLink");
    }
}
