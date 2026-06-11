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

async fn parse_credential_file(
    path: &std::path::Path,
    content: &str,
) -> anyhow::Result<Credential> {
    let json: serde_json::Value = serde_json::from_str(content)
        .with_context(|| format!("Failed to parse credentials JSON at {}", path.display()))?;

    if json.get("type").and_then(|v| v.as_str()) == Some("service_account") {
        let key = yup_oauth2::parse_service_account_key(content).with_context(|| {
            format!(
                "Failed to parse service account key from {}",
                path.display()
            )
        })?;
        return Ok(Credential::ServiceAccount(key));
    }

    let secret: yup_oauth2::authorized_user::AuthorizedUserSecret = serde_json::from_value(json)
        .with_context(|| {
            format!(
                "Failed to parse authorized user credentials from {}",
                path.display()
            )
        })?;
    Ok(Credential::AuthorizedUser(secret))
}

/// Credential priority:
/// 0. GOOGLE_WORKSPACE_CLI_TOKEN env var (raw access token)
/// 1. GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE env var
/// 2. ~/.config/gws/credentials.json
/// 3. GOOGLE_APPLICATION_CREDENTIALS env var (ADC)
/// 4. ~/.config/gcloud/application_default_credentials.json
pub async fn get_token(scopes: &[&str]) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("GOOGLE_WORKSPACE_CLI_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }

    let creds = load_credentials().await?;
    get_token_inner(scopes, creds).await
}

async fn load_credentials() -> anyhow::Result<Credential> {
    if let Ok(path) = std::env::var("GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE") {
        let p = PathBuf::from(&path);
        if p.exists() {
            let content = tokio::fs::read_to_string(&p).await?;
            return parse_credential_file(&p, &content).await;
        }
        anyhow::bail!(
            "GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE points to {path}, but file does not exist"
        );
    }

    let default_path = config_dir().join("credentials.json");
    if default_path.exists() {
        let content = tokio::fs::read_to_string(&default_path).await?;
        return parse_credential_file(&default_path, &content).await;
    }

    if let Ok(adc_env) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        let adc_path = PathBuf::from(&adc_env);
        if adc_path.exists() {
            let content = tokio::fs::read_to_string(&adc_path).await?;
            return parse_credential_file(&adc_path, &content).await;
        }
        anyhow::bail!(
            "GOOGLE_APPLICATION_CREDENTIALS points to {adc_env}, but file does not exist"
        );
    }

    if let Some(well_known) = adc_well_known_path()
        && well_known.exists()
    {
        let content = tokio::fs::read_to_string(&well_known).await?;
        return parse_credential_file(&well_known, &content).await;
    }

    anyhow::bail!(
        "No credentials found. Set GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE, \
         GOOGLE_APPLICATION_CREDENTIALS, or run `gcloud auth application-default login`."
    )
}

async fn get_token_inner(scopes: &[&str], creds: Credential) -> anyhow::Result<String> {
    match creds {
        Credential::AuthorizedUser(secret) => {
            let auth = yup_oauth2::AuthorizedUserAuthenticator::builder(secret)
                .build()
                .await
                .context("Failed to build authorized user authenticator")?;
            let token = auth.token(scopes).await.context("Failed to get token")?;
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
            let token = auth.token(scopes).await.context("Failed to get token")?;
            Ok(token
                .token()
                .ok_or_else(|| anyhow::anyhow!("Token response contained no access token"))?
                .to_string())
        }
    }
}

pub fn get_quota_project() -> Option<String> {
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

    #[tokio::test]
    async fn test_parse_authorized_user() {
        let content = r#"{
            "client_id": "test-id",
            "client_secret": "test-secret",
            "refresh_token": "test-refresh",
            "type": "authorized_user"
        }"#;
        let path = PathBuf::from("/tmp/test-creds.json");
        let cred = parse_credential_file(&path, content).await.unwrap();
        assert!(matches!(cred, Credential::AuthorizedUser(_)));
    }

    #[tokio::test]
    async fn test_parse_service_account() {
        let content = r#"{
            "type": "service_account",
            "project_id": "test",
            "private_key_id": "key1",
            "private_key": "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy0AHB7MhgHcTz6sE2I2yPB\naFDrBz9vFqU4yoySN5Lkzpf/AB+c0LS3cD0lNJHPOb0K2GKi5YX0P54hiDRF4qv\ng6vDp15sObnzqFE7D0VNjm2b4UlRNR8pzGt/AE5Q0nG/KBIGfx2G+K4UL94Q8VE\nuvFN3s0FxnL1Fg2kE3R3CZ3R5KxV0d3bMYiC0lohgSUbh3QEIjxXKDU/GjFiEiA\nkl7S/GuTGTVpH5K0uyk/Mmji2RBj2TXH7yFNf/D2c2fNGmG3j7B0e+3m/VIsKEb\n9v+1j/L0sBFPG21FCBzx0G/9GPPLWMJ0sRMfJwIDAQABAoIBAC5RgZ+hBx7xHNaM\npPgwGMnCd6HEfyGI+K2gOzfEPLelML5LxFEr0KPsgz7K1NxTq2qFRQDi5kJ9k6B\n1A4IBypassword123456789012345678901234567890==\n-----END RSA PRIVATE KEY-----\n",
            "client_email": "test@test.iam.gserviceaccount.com",
            "client_id": "123",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token"
        }"#;
        let path = PathBuf::from("/tmp/test-sa.json");
        let cred = parse_credential_file(&path, content).await.unwrap();
        assert!(matches!(cred, Credential::ServiceAccount(_)));
    }

    #[tokio::test]
    async fn test_parse_invalid_json() {
        let path = PathBuf::from("/tmp/test.json");
        assert!(parse_credential_file(&path, "not json").await.is_err());
    }
}
