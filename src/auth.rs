use std::path::PathBuf;

use anyhow::Context;
#[derive(Debug)]
enum Credential {
    AuthorizedUser(yup_oauth2::authorized_user::AuthorizedUserSecret),
    ServiceAccount(yup_oauth2::ServiceAccountKey),
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
pub async fn get_token(scopes: &[&str], credentials_file: Option<&str>) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("GOOGLE_WORKSPACE_CLI_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }

    let creds = load_credentials(credentials_file).await?;
    get_token_inner(scopes, creds).await
}

async fn load_credentials(credentials_file: Option<&str>) -> anyhow::Result<Credential> {
    if let Some(path) = credentials_file {
        let p = PathBuf::from(path);
        if p.exists() {
            let content = tokio::fs::read_to_string(&p).await?;
            tracing::info!(path = path, "Using credentials from policy file");
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

pub fn get_quota_project(policy_project_id: Option<&str>) -> Option<String> {
    if let Some(pid) = policy_project_id {
        return Some(pid.to_string());
    }

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
}
