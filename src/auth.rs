use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::Context;

#[derive(Debug)]
enum Credential {
    AuthorizedUser(yup_oauth2::authorized_user::AuthorizedUserSecret),
    ServiceAccount(yup_oauth2::ServiceAccountKey),
}

#[derive(Debug)]
pub(crate) struct TokenCache {
    token: String,
    expires_at: Instant,
}

impl TokenCache {
    fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GOOGLE_WORKSPACE_CLI_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gws")
}

fn adc_well_known_path() -> Option<PathBuf> {
    dirs_next::home_dir().map(|d| {
        d.join(".config")
            .join("gcloud")
            .join("application_default_credentials.json")
    })
}

fn parse_credentials(content: &str, source: &str) -> anyhow::Result<Credential> {
    let json: serde_json::Value = serde_json::from_str(content)
        .with_context(|| format!("Failed to parse credentials JSON from {source}"))?;

    if json.get("type").and_then(|v| v.as_str()) == Some("service_account") {
        let key = yup_oauth2::parse_service_account_key(content)
            .with_context(|| format!("Failed to parse service account key from {source}"))?;
        return Ok(Credential::ServiceAccount(key));
    }

    let secret: yup_oauth2::authorized_user::AuthorizedUserSecret = serde_json::from_value(json)
        .with_context(|| format!("Failed to parse authorized user credentials from {source}"))?;
    Ok(Credential::AuthorizedUser(secret))
}

/// Credential priority:
/// 0. GOOGLE_WORKSPACE_CLI_TOKEN env var (raw access token)
/// 1. Policy credentials_file
/// 2. GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE env var
/// 3. ~/.config/gws/credentials.json
/// 4. `gws auth export` (reads from OS keyring)
/// 5. GOOGLE_APPLICATION_CREDENTIALS env var (ADC)
/// 6. ~/.config/gcloud/application_default_credentials.json
pub async fn get_token(
    scopes: &[&str],
    credentials_file: Option<&str>,
    cache: Option<&mut Option<TokenCache>>,
) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("GOOGLE_WORKSPACE_CLI_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }

    if let Some(&mut Some(ref c)) = cache
        && c.is_valid()
    {
        return Ok(c.token.clone());
    }

    let creds = load_credentials(credentials_file).await?;
    let token = get_token_inner(scopes, creds).await?;

    if let Some(slot) = cache {
        *slot = Some(TokenCache {
            token: token.clone(),
            expires_at: Instant::now() + std::time::Duration::from_secs(3500),
        });
    }

    Ok(token)
}

async fn load_credentials(credentials_file: Option<&str>) -> anyhow::Result<Credential> {
    if let Some(path) = credentials_file {
        let p = PathBuf::from(path);
        if p.exists() {
            let content = tokio::fs::read_to_string(&p).await?;
            tracing::debug!(path = path, "Using credentials from policy file");
            return parse_credentials(&content, &p.display().to_string());
        }
        anyhow::bail!("credentials_file in policy points to {path}, but file does not exist");
    }

    if let Ok(path) = std::env::var("GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE") {
        let p = PathBuf::from(&path);
        if p.exists() {
            let content = tokio::fs::read_to_string(&p).await?;
            return parse_credentials(&content, &p.display().to_string());
        }
        anyhow::bail!(
            "GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE points to {path}, but file does not exist"
        );
    }

    let default_path = config_dir().join("credentials.json");
    if default_path.exists() {
        let content = tokio::fs::read_to_string(&default_path).await?;
        return parse_credentials(&content, &default_path.display().to_string());
    }

    if let Some(cred) = try_gws_export().await {
        return Ok(cred);
    }

    if let Ok(adc_env) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        let adc_path = PathBuf::from(&adc_env);
        if adc_path.exists() {
            let content = tokio::fs::read_to_string(&adc_path).await?;
            return parse_credentials(&content, &adc_path.display().to_string());
        }
        anyhow::bail!(
            "GOOGLE_APPLICATION_CREDENTIALS points to {adc_env}, but file does not exist"
        );
    }

    if let Some(well_known) = adc_well_known_path()
        && well_known.exists()
    {
        let content = tokio::fs::read_to_string(&well_known).await?;
        return parse_credentials(&content, &well_known.display().to_string());
    }

    anyhow::bail!(
        "No credentials found. Options:\n\
         - Set credentials_file in policy TOML\n\
         - Run `gws auth login` (credentials read via `gws auth export`)\n\
         - Set GOOGLE_APPLICATION_CREDENTIALS env var\n\
         - Run `gcloud auth application-default login`"
    )
}

async fn try_gws_export() -> Option<Credential> {
    let output = tokio::process::Command::new("gws")
        .args(["auth", "export", "--unmasked"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let json_start = stdout.find('{')?;
    let json_str = &stdout[json_start..];

    match parse_credentials(json_str, "gws auth export") {
        Ok(cred) => {
            tracing::info!("Using credentials from gws auth export");
            Some(cred)
        }
        Err(_) => None,
    }
}

async fn get_token_inner(scopes: &[&str], creds: Credential) -> anyhow::Result<String> {
    match creds {
        Credential::AuthorizedUser(secret) => {
            let auth = yup_oauth2::AuthorizedUserAuthenticator::builder(secret)
                .build()
                .await
                .context("Failed to build authorized user authenticator")?;
            let token = auth
                .token(scopes)
                .await
                .map_err(|e| anyhow::anyhow!("Token refresh failed: {e}"))?;
            Ok(token
                .token()
                .ok_or_else(|| anyhow::anyhow!("Token response contained no access token"))?
                .to_string())
        }
        Credential::ServiceAccount(key) => {
            let auth = yup_oauth2::ServiceAccountAuthenticator::builder(key)
                .build()
                .await
                .context("Failed to build service account authenticator")?;
            let token = auth
                .token(scopes)
                .await
                .map_err(|e| anyhow::anyhow!("Token refresh failed: {e}"))?;
            Ok(token
                .token()
                .ok_or_else(|| anyhow::anyhow!("Token response contained no access token"))?
                .to_string())
        }
    }
}

#[derive(Debug)]
pub struct DiagResult {
    pub source: String,
    pub found: bool,
    pub parseable: bool,
    pub detail: String,
}

pub async fn diagnose_chain(policy_creds_path: Option<&str>) -> Vec<DiagResult> {
    let mut results = Vec::new();

    // Source 0: GOOGLE_WORKSPACE_CLI_TOKEN
    results.push(match std::env::var("GOOGLE_WORKSPACE_CLI_TOKEN") {
        Ok(val) if !val.is_empty() => DiagResult {
            source: "GOOGLE_WORKSPACE_CLI_TOKEN".to_string(),
            found: true,
            parseable: true,
            detail: "Raw access token set".to_string(),
        },
        _ => DiagResult {
            source: "GOOGLE_WORKSPACE_CLI_TOKEN".to_string(),
            found: false,
            parseable: false,
            detail: "Not set".to_string(),
        },
    });

    // Source 1: Policy credentials_file
    results.push(match policy_creds_path {
        Some(path) => {
            let p = PathBuf::from(path);
            if p.exists() {
                match tokio::fs::read_to_string(&p).await {
                    Ok(content) => match credential_type_label(&content) {
                        Some(label) => DiagResult {
                            source: "Policy credentials_file".to_string(),
                            found: true,
                            parseable: true,
                            detail: format!("{path} ({label})"),
                        },
                        None => DiagResult {
                            source: "Policy credentials_file".to_string(),
                            found: true,
                            parseable: false,
                            detail: format!("{path} (not valid JSON credentials)"),
                        },
                    },
                    Err(_) => DiagResult {
                        source: "Policy credentials_file".to_string(),
                        found: true,
                        parseable: false,
                        detail: format!("{path} (unreadable)"),
                    },
                }
            } else {
                DiagResult {
                    source: "Policy credentials_file".to_string(),
                    found: false,
                    parseable: false,
                    detail: format!("{path} (file not found)"),
                }
            }
        }
        None => DiagResult {
            source: "Policy credentials_file".to_string(),
            found: false,
            parseable: false,
            detail: "Not configured".to_string(),
        },
    });

    // Source 2: GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE
    results.push(diag_env_file("GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE").await);

    // Source 3: ~/.config/gws/credentials.json
    let default_path = config_dir().join("credentials.json");
    results.push(diag_file_path("Default config", &default_path).await);

    // Source 4: gws auth export
    results.push(diag_gws_export().await);

    // Source 5: GOOGLE_APPLICATION_CREDENTIALS (ADC)
    results.push(diag_env_file("GOOGLE_APPLICATION_CREDENTIALS").await);

    // Source 6: ~/.config/gcloud/application_default_credentials.json
    results.push(match adc_well_known_path() {
        Some(p) => diag_file_path("ADC (gcloud)", &p).await,
        None => DiagResult {
            source: "ADC (gcloud)".to_string(),
            found: false,
            parseable: false,
            detail: "Cannot determine home directory".to_string(),
        },
    });

    results
}

fn credential_type_label(content: &str) -> Option<&'static str> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    match json.get("type").and_then(|v| v.as_str()) {
        Some("service_account") => Some("service_account"),
        Some("authorized_user") => Some("authorized_user"),
        Some(other) if !other.is_empty() => Some("authorized_user"),
        _ => {
            if json.get("client_id").is_some() && json.get("refresh_token").is_some() {
                Some("authorized_user")
            } else {
                None
            }
        }
    }
}

async fn diag_env_file(var: &str) -> DiagResult {
    match std::env::var(var) {
        Ok(path) if !path.is_empty() => {
            let p = PathBuf::from(&path);
            if p.exists() {
                match tokio::fs::read_to_string(&p).await {
                    Ok(content) => match credential_type_label(&content) {
                        Some(label) => DiagResult {
                            source: var.to_string(),
                            found: true,
                            parseable: true,
                            detail: format!("{path} ({label})"),
                        },
                        None => DiagResult {
                            source: var.to_string(),
                            found: true,
                            parseable: false,
                            detail: format!("{path} (not valid JSON credentials)"),
                        },
                    },
                    Err(_) => DiagResult {
                        source: var.to_string(),
                        found: true,
                        parseable: false,
                        detail: format!("{path} (unreadable)"),
                    },
                }
            } else {
                DiagResult {
                    source: var.to_string(),
                    found: false,
                    parseable: false,
                    detail: format!("{path} (file not found)"),
                }
            }
        }
        _ => DiagResult {
            source: var.to_string(),
            found: false,
            parseable: false,
            detail: "Not set".to_string(),
        },
    }
}

async fn diag_file_path(label: &str, path: &PathBuf) -> DiagResult {
    let display = path.display().to_string();
    if path.exists() {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => match credential_type_label(&content) {
                Some(cred_type) => DiagResult {
                    source: label.to_string(),
                    found: true,
                    parseable: true,
                    detail: format!("{display} ({cred_type})"),
                },
                None => DiagResult {
                    source: label.to_string(),
                    found: true,
                    parseable: false,
                    detail: format!("{display} (not valid JSON credentials)"),
                },
            },
            Err(_) => DiagResult {
                source: label.to_string(),
                found: true,
                parseable: false,
                detail: format!("{display} (unreadable)"),
            },
        }
    } else {
        DiagResult {
            source: label.to_string(),
            found: false,
            parseable: false,
            detail: format!("{display} not found"),
        }
    }
}

async fn diag_gws_export() -> DiagResult {
    let output = tokio::process::Command::new("gws")
        .args(["auth", "export", "--unmasked"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some(start) = stdout.find('{') {
                match credential_type_label(&stdout[start..]) {
                    Some(label) => DiagResult {
                        source: "gws auth export".to_string(),
                        found: true,
                        parseable: true,
                        detail: format!("Credentials from keyring ({label})"),
                    },
                    None => DiagResult {
                        source: "gws auth export".to_string(),
                        found: true,
                        parseable: false,
                        detail: "Output not valid JSON credentials".to_string(),
                    },
                }
            } else {
                DiagResult {
                    source: "gws auth export".to_string(),
                    found: true,
                    parseable: false,
                    detail: "No JSON in output".to_string(),
                }
            }
        }
        Ok(_) => DiagResult {
            source: "gws auth export".to_string(),
            found: false,
            parseable: false,
            detail: "Command failed (not logged in or gws not installed)".to_string(),
        },
        Err(_) => DiagResult {
            source: "gws auth export".to_string(),
            found: false,
            parseable: false,
            detail: "gws CLI not found".to_string(),
        },
    }
}

static QUOTA_PROJECT: OnceLock<Option<String>> = OnceLock::new();

pub fn get_quota_project(policy_project_id: Option<&str>) -> Option<String> {
    if let Some(pid) = policy_project_id {
        return Some(pid.to_string());
    }

    QUOTA_PROJECT
        .get_or_init(|| {
            if let Ok(project_id) = std::env::var("GOOGLE_WORKSPACE_PROJECT_ID")
                && !project_id.is_empty()
            {
                return Some(project_id);
            }

            if let Ok(adc_env) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
                && let Ok(content) = std::fs::read_to_string(adc_env)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(qp) = json.get("quota_project_id").and_then(|v| v.as_str())
            {
                return Some(qp.to_string());
            }

            if let Some(well_known) = adc_well_known_path()
                && let Ok(content) = std::fs::read_to_string(well_known)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
            {
                return json
                    .get("quota_project_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }

            None
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adc_well_known_path_exists() {
        let path = adc_well_known_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(
            p.to_str()
                .unwrap()
                .contains("application_default_credentials.json")
        );
    }

    #[test]
    fn test_parse_credentials_authorized_user() {
        let content = r#"{
            "client_id": "test-id",
            "client_secret": "test-secret",
            "refresh_token": "test-refresh",
            "type": "authorized_user"
        }"#;
        let cred = parse_credentials(content, "test").unwrap();
        assert!(matches!(cred, Credential::AuthorizedUser(_)));
    }

    #[test]
    fn test_parse_credentials_invalid() {
        assert!(parse_credentials("not json", "test").is_err());
    }

    #[test]
    fn test_credential_type_label_service_account() {
        let content = r#"{"type": "service_account", "project_id": "test"}"#;
        assert_eq!(credential_type_label(content), Some("service_account"));
    }

    #[test]
    fn test_credential_type_label_authorized_user() {
        let content = r#"{"type": "authorized_user", "client_id": "id", "refresh_token": "tok"}"#;
        assert_eq!(credential_type_label(content), Some("authorized_user"));
    }

    #[test]
    fn test_credential_type_label_no_type_but_has_client_fields() {
        let content = r#"{"client_id": "id", "refresh_token": "tok"}"#;
        assert_eq!(credential_type_label(content), Some("authorized_user"));
    }

    #[test]
    fn test_credential_type_label_invalid_json() {
        assert_eq!(credential_type_label("not json"), None);
    }

    #[test]
    fn test_credential_type_label_empty_object() {
        assert_eq!(credential_type_label("{}"), None);
    }
}
