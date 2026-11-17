//! Multi-user mode: one process, many users.
//!
//! Each user is a subdirectory of a users directory containing a
//! `gws-policy.json` (or `.gws-policy.json`) with its own services,
//! constraints, credentials, and optional `server.api_token` for
//! authenticating the user's agent. Every user gets an isolated
//! [`ServerState`] (own OAuth token cache, task map, subscriptions) and
//! is served under `/mcp/<user>`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use google_workspace::error::GwsError;

use crate::policy::Policy;
use crate::prompts::Prompt;
use crate::shared::ServerState;

pub const POLICY_FILE_NAMES: &[&str] = &["gws-policy.json", ".gws-policy.json"];

/// A single user's running instance: isolated policy and state.
pub struct UserInstance {
    pub name: String,
    pub policy_path: PathBuf,
    pub policy: Arc<tokio::sync::RwLock<Policy>>,
    pub state: Arc<tokio::sync::Mutex<ServerState>>,
}

impl UserInstance {
    /// The bearer token required to access this user's endpoint, if configured.
    pub async fn api_token(&self) -> Option<String> {
        self.policy
            .read()
            .await
            .api_token
            .clone()
            .filter(|t| !t.is_empty())
    }
}

/// Validate a user directory name: it is used verbatim in a URL path,
/// so restrict it to a safe character set.
pub fn is_valid_user_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn find_policy_file(user_dir: &Path) -> Option<PathBuf> {
    POLICY_FILE_NAMES
        .iter()
        .map(|n| user_dir.join(n))
        .find(|p| p.is_file())
}

/// Load all users from a users directory.
///
/// Each immediate subdirectory that contains a policy file becomes a user.
/// Per-user audit logs are written to `<user_dir>/audit.jsonl`.
#[allow(clippy::too_many_arguments)]
pub fn load_users(
    users_dir: &Path,
    prompts: &[Prompt],
    eager_tools: bool,
    webhook_url: Option<String>,
) -> Result<Vec<UserInstance>, GwsError> {
    let entries = std::fs::read_dir(users_dir).map_err(|e| {
        GwsError::Validation(format!(
            "Cannot read users directory {}: {e}",
            users_dir.display()
        ))
    })?;

    let mut users = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|e| GwsError::Validation(format!("Cannot read users directory entry: {e}")))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_valid_user_name(&name) {
            return Err(GwsError::Validation(format!(
                "Invalid user directory name '{name}'. Use only letters, digits, '-' and '_' \
                 (max 64 chars) — the name becomes part of the URL path /mcp/{name}."
            )));
        }

        let policy_path = match find_policy_file(&path) {
            Some(p) => p,
            None => continue,
        };

        let policy = Policy::from_file(&policy_path)?;

        // Per-user audit log for accountable multi-user deployments
        let audit = {
            let audit_path = path.join("audit.jsonl");
            match crate::audit::AuditLogger::new(audit_path) {
                Ok(logger) => {
                    let logger = Arc::new(logger);
                    tracing::info!(
                        user = %name,
                        path = %logger.path().display(),
                        "Per-user audit log enabled"
                    );
                    Some(logger)
                }
                Err(e) => {
                    tracing::warn!(user = %name, error = %e, "Cannot open per-user audit log");
                    None
                }
            }
        };

        let state =
            ServerState::with_config(prompts.to_vec(), audit, eager_tools, webhook_url.clone());
        let services = policy.allowed_services();

        tracing::info!(
            user = %name,
            services = %services.join(", "),
            policy = %policy_path.display(),
            "Loaded user"
        );

        users.push(UserInstance {
            name,
            policy_path,
            policy: Arc::new(tokio::sync::RwLock::new(policy)),
            state: Arc::new(tokio::sync::Mutex::new(state)),
        });
    }

    users.sort_by(|a, b| a.name.cmp(&b.name));

    if users.is_empty() {
        return Err(GwsError::Validation(format!(
            "No users found in {}. Each user needs a subdirectory containing \
             a gws-policy.json file.",
            users_dir.display()
        )));
    }

    Ok(users)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_user(dir: &Path, name: &str, policy_json: &str) {
        let user_dir = dir.join(name);
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(user_dir.join("gws-policy.json"), policy_json).unwrap();
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mcp-gws-test-users-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_is_valid_user_name() {
        assert!(is_valid_user_name("alice"));
        assert!(is_valid_user_name("user-2"));
        assert!(is_valid_user_name("a_b"));
        assert!(is_valid_user_name("ABC123"));
        assert!(!is_valid_user_name(""));
        assert!(!is_valid_user_name("bad name"));
        assert!(!is_valid_user_name("bad/name"));
        assert!(!is_valid_user_name(".."));
        assert!(!is_valid_user_name(&"x".repeat(65)));
    }

    #[test]
    fn test_load_users_basic() {
        let dir = tmp_dir("basic");
        write_user(
            &dir,
            "alice",
            r#"{"services": [{"name": "drive", "read_only": true}]}"#,
        );
        write_user(&dir, "bob", r#"{"services": [{"name": "gmail"}]}"#);

        let users = load_users(&dir, &[], false, None).unwrap();
        let names: Vec<&str> = users.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["alice", "bob"]); // sorted
        drop(users);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_users_skips_dirs_without_policy() {
        let dir = tmp_dir("skip");
        write_user(&dir, "alice", r#"{"services": [{"name": "drive"}]}"#);
        std::fs::create_dir_all(dir.join("not-a-user")).unwrap();

        let users = load_users(&dir, &[], false, None).unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "alice");
        drop(users);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_users_no_users_errors() {
        let dir = tmp_dir("empty");
        std::fs::create_dir_all(dir.join("nouser")).unwrap();
        assert!(load_users(&dir, &[], false, None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_users_missing_dir_errors() {
        assert!(load_users(Path::new("/nonexistent/users"), &[], false, None).is_err());
    }

    #[test]
    fn test_load_users_invalid_name_errors() {
        let dir = tmp_dir("badname");
        write_user(&dir, "bad name", r#"{"services": [{"name": "drive"}]}"#);
        assert!(load_users(&dir, &[], false, None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_users_invalid_policy_errors() {
        let dir = tmp_dir("badpolicy");
        write_user(&dir, "alice", "not json");
        assert!(load_users(&dir, &[], false, None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_users_creates_audit_log() {
        let dir = tmp_dir("audit");
        let alice = dir.join("alice");
        write_user(&dir, "alice", r#"{"services": [{"name": "drive"}]}"#);

        let users = load_users(&dir, &[], false, None).unwrap();
        drop(users);
        assert!(alice.join("audit.jsonl").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_users_hidden_policy_name() {
        let dir = tmp_dir("hidden");
        let bob = dir.join("bob");
        std::fs::create_dir_all(&bob).unwrap();
        std::fs::write(
            bob.join(".gws-policy.json"),
            r#"{"services": [{"name": "tasks"}]}"#,
        )
        .unwrap();

        let users = load_users(&dir, &[], false, None).unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "bob");
        drop(users);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_user_instance_api_token() {
        let dir = tmp_dir("token");
        write_user(
            &dir,
            "alice",
            r#"{"server": {"api_token": "secret123"}, "services": [{"name": "drive"}]}"#,
        );
        let users = load_users(&dir, &[], false, None).unwrap();
        assert_eq!(users[0].api_token().await.as_deref(), Some("secret123"));
        drop(users);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_policy_api_token_parsed() {
        let file: crate::policy::PolicyFile = serde_json::from_value(serde_json::json!({
            "server": { "api_token": "tok" },
            "services": [{ "name": "drive" }]
        }))
        .unwrap();
        let p = crate::policy::Policy::from_policy_file(file);
        assert_eq!(p.api_token.as_deref(), Some("tok"));
    }

    #[test]
    fn test_policy_api_token_default_none() {
        let file: crate::policy::PolicyFile = serde_json::from_value(serde_json::json!({
            "services": [{ "name": "drive" }]
        }))
        .unwrap();
        let p = crate::policy::Policy::from_policy_file(file);
        assert!(p.api_token.is_none());
    }
}
