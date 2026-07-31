use console::style;
use google_workspace::error::GwsError;

use crate::auth;

const SERVICES: &[(&str, &str)] = &[
    ("drive", "Google Drive — files and folders"),
    ("gmail", "Gmail — messages, threads, labels"),
    ("calendar", "Google Calendar — events and scheduling"),
    ("sheets", "Google Sheets — spreadsheets"),
    ("docs", "Google Docs — documents"),
    ("slides", "Google Slides — presentations"),
    ("people", "Google Contacts — contact lookup"),
];

pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "analyst",
        "Read-only Drive, Sheets, Docs. Gmail with safety blocks.",
    ),
    (
        "assistant",
        "Drive read-write, Gmail with safety blocks, Calendar, Sheets, Docs.",
    ),
    (
        "admin-readonly",
        "All services in read-only mode. Safe for auditing.",
    ),
];

pub fn generate_policy(services: &[String]) -> serde_json::Value {
    let svc_entries: Vec<serde_json::Value> = services
        .iter()
        .map(|name| default_service_entry(name))
        .collect();
    serde_json::json!({
        "server": { "read_only": false },
        "services": svc_entries
    })
}

fn default_service_entry(name: &str) -> serde_json::Value {
    match name {
        "drive" => serde_json::json!({ "name": "drive", "allowed_resources": ["files"] }),
        "gmail" => serde_json::json!({
            "name": "gmail",
            "allowed_resources": ["users.messages", "users.threads", "users.labels", "users.drafts"],
            "denied_methods": gmail_safety_denylists()
        }),
        "calendar" => serde_json::json!({
            "name": "calendar",
            "allowed_resources": ["events"],
            "constraints": [{ "param": "calendarId", "values": ["primary"], "access": "read-write" }]
        }),
        "sheets" => serde_json::json!({ "name": "sheets", "allowed_resources": ["spreadsheets"] }),
        "docs" => serde_json::json!({ "name": "docs", "allowed_resources": ["documents"] }),
        "slides" => serde_json::json!({ "name": "slides", "allowed_resources": ["presentations"] }),
        "people" => {
            serde_json::json!({ "name": "people", "read_only": true, "allowed_resources": ["people", "people.connections"] })
        }
        _ => serde_json::json!({ "name": name }),
    }
}

fn gmail_safety_denylists() -> serde_json::Value {
    serde_json::json!([
        "messages.delete",
        "messages.trash",
        "messages.batchDelete",
        "settings.updateAutoForwarding",
        "settings.delegates.create",
        "settings.forwardingAddresses.create"
    ])
}

pub fn template_policy(name: &str) -> Result<serde_json::Value, GwsError> {
    if name == "list" {
        eprintln!();
        eprintln!("{}", style("Available policy templates:").bold());
        eprintln!();
        for (name, desc) in TEMPLATES {
            eprintln!("  {} — {desc}", style(name).cyan());
        }
        eprintln!();
        eprintln!("Usage: mcp-google-workspace init --template <name>");
        std::process::exit(0);
    }

    match name {
        "analyst" => Ok(serde_json::json!({
            "server": { "read_only": false },
            "services": [
                { "name": "drive", "read_only": true, "allowed_resources": ["files"] },
                { "name": "sheets", "read_only": true, "allowed_resources": ["spreadsheets"] },
                { "name": "docs", "read_only": true, "allowed_resources": ["documents"] },
                { "name": "gmail", "allowed_resources": ["users.messages", "users.threads", "users.labels", "users.drafts"], "denied_methods": gmail_safety_denylists() }
            ]
        })),
        "assistant" => Ok(serde_json::json!({
            "server": { "read_only": false },
            "services": [
                { "name": "drive", "allowed_resources": ["files"] },
                { "name": "gmail", "allowed_resources": ["users.messages", "users.threads", "users.labels", "users.drafts"], "denied_methods": gmail_safety_denylists() },
                { "name": "calendar", "allowed_resources": ["events"], "constraints": [{ "param": "calendarId", "values": ["primary"], "access": "read-write" }] },
                { "name": "sheets", "allowed_resources": ["spreadsheets"] },
                { "name": "docs", "allowed_resources": ["documents"] },
                { "name": "people", "read_only": true, "allowed_resources": ["people", "people.connections"] }
            ]
        })),
        "admin-readonly" => Ok(serde_json::json!({
            "server": { "read_only": true },
            "services": [
                { "name": "drive", "allowed_resources": ["files"] },
                { "name": "gmail", "allowed_resources": ["users.messages", "users.threads", "users.labels"] },
                { "name": "calendar", "allowed_resources": ["events"] },
                { "name": "sheets", "allowed_resources": ["spreadsheets"] },
                { "name": "docs", "allowed_resources": ["documents"] },
                { "name": "slides", "allowed_resources": ["presentations"] }
            ]
        })),
        _ => {
            let names: Vec<&str> = TEMPLATES.iter().map(|(n, _)| *n).collect();
            Err(GwsError::Validation(format!(
                "Unknown template '{name}'. Available: {}. Use --template list for details",
                names.join(", ")
            )))
        }
    }
}

pub async fn init_guided() -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Input, MultiSelect, Select};
    let theme = dialoguer::theme::ColorfulTheme::default();

    eprintln!();
    eprintln!(
        "{}",
        style("  MCP Google Workspace — Setup Wizard").bold().cyan()
    );
    eprintln!();

    // Step 1: Auth
    eprintln!("{}", style("Step 1: Authentication").bold());
    let creds_file: String = Input::with_theme(&theme)
        .with_prompt("Credentials JSON path (empty = default chain)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;
    let creds_opt = if creds_file.is_empty() {
        None
    } else {
        Some(creds_file.as_str())
    };

    match auth::get_token(
        &["https://www.googleapis.com/auth/drive.readonly"],
        creds_opt,
        None,
    )
    .await
    {
        Ok(_) => eprintln!("  {} Authentication working", style("\u{2713}").green()),
        Err(e) => {
            eprintln!("  {} Authentication failed: {e}", style("\u{2717}").red());
            eprintln!();
            eprintln!("  Run: {}", style("gws auth login").yellow());
            eprintln!("  Then: {}", style("mcp-google-workspace init").yellow());
            return Err(GwsError::Validation("Authentication required".into()));
        }
    }
    eprintln!();

    // Step 2: Services
    eprintln!("{}", style("Step 2: Services").bold());

    let labels: Vec<String> = SERVICES
        .iter()
        .map(|(name, desc)| format!("{name} — {desc}"))
        .collect();
    let defaults = vec![true; SERVICES.len()];
    let selected = MultiSelect::with_theme(&theme)
        .with_prompt("Enable services (space=toggle, enter=confirm)")
        .items(&labels)
        .defaults(&defaults)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if selected.is_empty() {
        return Err(GwsError::Validation("At least one service required".into()));
    }
    let chosen: Vec<&str> = selected.iter().map(|&i| SERVICES[i].0).collect();
    eprintln!();

    // Step 3: Per-service config
    eprintln!("{}", style("Step 3: Service configuration").bold());
    let mut svc_entries: Vec<serde_json::Value> = Vec::new();

    for &name in &chosen {
        let entry = match name {
            "drive" => configure_drive_guided(creds_opt).await?,
            "gmail" => configure_gmail_guided(creds_opt).await?,
            "calendar" => configure_calendar_guided(creds_opt).await?,
            _ => default_service_entry(name),
        };
        svc_entries.push(entry);
    }
    eprintln!();

    // Step 4: Project ID
    eprintln!("{}", style("Step 4: Google Cloud project").bold());
    let project_id = detect_project_id(&theme)?;
    eprintln!();

    // Step 5: Server settings
    let mut server = serde_json::json!({});
    if !project_id.is_empty() {
        server["project_id"] = serde_json::json!(project_id);
    }
    if !creds_file.is_empty() {
        server["credentials_file"] = serde_json::json!(creds_file);
    }

    // Step 6: Save location
    eprintln!("{}", style("Step 5: Save policy").bold());
    let save_options = vec![
        format!(
            ".gws-policy.json  {} (gitignored, per-project)",
            style("← recommended").dim()
        ),
        "gws-policy.json   (shared, committable)".to_string(),
        "~/.config/gws/policy.json  (global, all projects)".to_string(),
        "Custom path".to_string(),
    ];
    let save_choice = Select::with_theme(&theme)
        .with_prompt("Where to save?")
        .items(&save_options)
        .default(0)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let save_path = match save_choice {
        0 => ".gws-policy.json".to_string(),
        1 => "gws-policy.json".to_string(),
        2 => {
            let dir = dirs_next::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("gws");
            std::fs::create_dir_all(&dir).ok();
            dir.join("policy.json").display().to_string()
        }
        _ => Input::with_theme(&theme)
            .with_prompt("Path")
            .default(".gws-policy.json".to_string())
            .interact_text()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?,
    };

    let policy = serde_json::json!({
        "server": server,
        "services": svc_entries
    });
    let output = serde_json::to_string_pretty(&policy).unwrap();

    if let Err(e) = std::fs::write(&save_path, &output) {
        return Err(GwsError::Validation(format!(
            "Failed to write {save_path}: {e}"
        )));
    }

    eprintln!();
    eprintln!(
        "  {} Saved to {}",
        style("\u{2713}").green(),
        style(&save_path).cyan()
    );
    if save_path.starts_with('.') {
        eprintln!();
        eprintln!("  Add to .gitignore (contains project IDs and folder IDs):");
        eprintln!(
            "    {}",
            style(format!("echo '{save_path}' >> .gitignore")).yellow()
        );
    }
    eprintln!();
    eprintln!(
        "  The server auto-discovers {} — just run:",
        style(&save_path).cyan()
    );
    eprintln!("    {}", style("mcp-google-workspace").yellow());
    eprintln!();
    eprintln!("  Validate with:");
    eprintln!(
        "    {}",
        style(format!("mcp-google-workspace check-policy {save_path}")).yellow()
    );

    std::process::exit(0);
}

fn detect_project_id(theme: &dialoguer::theme::ColorfulTheme) -> Result<String, GwsError> {
    use dialoguer::{Input, Select};

    let mut projects = Vec::new();
    if let Ok(output) = std::process::Command::new("gcloud")
        .args([
            "projects",
            "list",
            "--format=json(projectId,name)",
            "--limit=20",
        ])
        .output()
        && output.status.success()
        && let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(arr) = parsed.as_array()
    {
        for p in arr {
            let id = p.get("projectId").and_then(|v| v.as_str()).unwrap_or("");
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or(id);
            if !id.is_empty() {
                projects.push((id.to_string(), name.to_string()));
            }
        }
    }

    if !projects.is_empty() {
        let mut labels: Vec<String> = projects
            .iter()
            .map(|(id, name)| format!("{name} ({id})"))
            .collect();
        labels.push("Enter manually".to_string());
        labels.push("Skip (no project ID)".to_string());

        let choice = Select::with_theme(theme)
            .with_prompt("Google Cloud project (for API quota)")
            .items(&labels)
            .default(0)
            .interact()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        if choice < projects.len() {
            return Ok(projects[choice].0.clone());
        }
        if choice == labels.len() - 1 {
            return Ok(String::new());
        }
    }

    let id: String = Input::with_theme(theme)
        .with_prompt("Google Cloud project ID (empty to skip)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;
    Ok(id)
}

async fn configure_drive_guided(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, Input, MultiSelect};
    let theme = dialoguer::theme::ColorfulTheme::default();

    eprintln!("  {} Fetching Drive folders...", style("Drive:").cyan());
    let mut entry = serde_json::json!({ "name": "drive", "allowed_resources": ["files"] });

    let token = auth::get_token(
        &["https://www.googleapis.com/auth/drive.readonly"],
        creds,
        None,
    )
    .await
    .map_err(|e| GwsError::Validation(format!("Drive auth failed: {e}")))?;

    let client = reqwest::Client::new();

    let search_query: String = Input::with_theme(&theme)
        .with_prompt("  Search folders by name (empty for root folders)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let q = if search_query.is_empty() {
        "mimeType='application/vnd.google-apps.folder' and 'root' in parents and trashed=false"
            .to_string()
    } else {
        format!(
            "mimeType='application/vnd.google-apps.folder' and name contains '{}' and trashed=false",
            search_query.replace('\'', "\\'")
        )
    };

    let resp = client
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(&token)
        .query(&[
            ("q", q.as_str()),
            ("fields", "files(id,name)"),
            ("pageSize", "50"),
        ])
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("Drive API failed: {e}")))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Drive parse failed: {e}")))?;

    let folders: Vec<(String, String)> = data
        .get("files")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some((
                        f.get("id")?.as_str()?.to_string(),
                        f.get("name")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    if folders.is_empty() {
        eprintln!(
            "    {} No folders found — Drive will be unrestricted",
            style("!").yellow()
        );
        return Ok(entry);
    }

    eprintln!(
        "    {}",
        style("Use space to toggle, enter to confirm").dim()
    );
    let labels: Vec<String> = folders
        .iter()
        .map(|(id, name)| format!("{name}  {}", style(id).dim()))
        .collect();
    let selected = MultiSelect::with_theme(&theme)
        .with_prompt("  Writable folders (space=toggle, enter=confirm)")
        .items(&labels)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if !selected.is_empty() {
        let ids: Vec<&str> = selected.iter().map(|&i| folders[i].0.as_str()).collect();
        entry["constraints"] = serde_json::json!([{
            "param": "parents", "values": ids,
            "access": "read-write", "location": "body-write-only",
            "mode": "restrict", "recursive": true
        }]);
    }

    let block_delete = Confirm::with_theme(&theme)
        .with_prompt("  Block permanent file deletion?")
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;
    if block_delete {
        entry["denied_methods"] = serde_json::json!(["files.delete"]);
    }

    Ok(entry)
}

async fn configure_gmail_guided(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, MultiSelect};
    let theme = dialoguer::theme::ColorfulTheme::default();

    eprintln!("  {} Fetching labels...", style("Gmail:").cyan());
    let mut entry = serde_json::json!({
        "name": "gmail",
        "allowed_resources": ["users.messages", "users.threads", "users.labels", "users.drafts"],
        "denied_methods": gmail_safety_denylists()
    });

    let token = auth::get_token(
        &["https://www.googleapis.com/auth/gmail.readonly"],
        creds,
        None,
    )
    .await
    .map_err(|e| GwsError::Validation(format!("Gmail auth failed: {e}")))?;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://www.googleapis.com/gmail/v1/users/me/labels")
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("Gmail API failed: {e}")))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Gmail parse failed: {e}")))?;

    let user_labels: Vec<String> = data
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let name = l.get("name")?.as_str()?;
                    let ltype = l.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if ltype == "user" {
                        Some(name.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let restrict = Confirm::with_theme(&theme)
        .with_prompt("  Restrict to specific labels?")
        .default(false)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if restrict && !user_labels.is_empty() {
        let mut labels = vec![
            "INBOX".to_string(),
            "SENT".to_string(),
            "STARRED".to_string(),
        ];
        labels.extend(user_labels);

        eprintln!(
            "    {}",
            style("Use space to toggle, enter to confirm").dim()
        );
        let selected = MultiSelect::with_theme(&theme)
            .with_prompt("  Allowed labels (space=toggle, enter=confirm)")
            .items(&labels)
            .interact()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        if !selected.is_empty() {
            let allowed: Vec<&str> = selected.iter().map(|&i| labels[i].as_str()).collect();
            entry["allowed_labels"] = serde_json::json!(allowed);
        }
    }

    Ok(entry)
}

async fn configure_calendar_guided(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    use dialoguer::MultiSelect;
    let theme = dialoguer::theme::ColorfulTheme::default();

    eprintln!("  {} Fetching calendars...", style("Calendar:").cyan());
    let mut entry = serde_json::json!({ "name": "calendar", "allowed_resources": ["events"] });

    let token = auth::get_token(
        &["https://www.googleapis.com/auth/calendar.readonly"],
        creds,
        None,
    )
    .await
    .map_err(|e| GwsError::Validation(format!("Calendar auth failed: {e}")))?;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://www.googleapis.com/calendar/v3/users/me/calendarList")
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("Calendar API failed: {e}")))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Calendar parse failed: {e}")))?;

    let calendars: Vec<(String, String, String)> = data
        .get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c.get("id")?.as_str()?.to_string();
                    let summary = c
                        .get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or(&id)
                        .to_string();
                    let access = c
                        .get("accessRole")
                        .and_then(|a| a.as_str())
                        .unwrap_or("reader")
                        .to_string();
                    if access == "freeBusyReader" {
                        return None;
                    }
                    Some((id, summary, access))
                })
                .collect()
        })
        .unwrap_or_default();

    if calendars.is_empty() {
        entry["constraints"] = serde_json::json!([
            { "param": "calendarId", "values": ["primary"], "access": "read-write" }
        ]);
        return Ok(entry);
    }

    let labels: Vec<String> = calendars
        .iter()
        .map(|(_, name, role)| {
            let role_style = match role.as_str() {
                "owner" => style(role).green(),
                "writer" => style(role).yellow(),
                _ => style(role).dim(),
            };
            format!("{name}  [{role_style}]")
        })
        .collect();

    eprintln!(
        "    {}",
        style("Use space to toggle, enter to confirm").dim()
    );
    let rw_selected = MultiSelect::with_theme(&theme)
        .with_prompt("  Read-write calendars (space=toggle, enter=confirm)")
        .items(&labels)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let mut constraints = Vec::new();
    let rw_ids: Vec<&str> = rw_selected
        .iter()
        .map(|&i| calendars[i].0.as_str())
        .collect();
    if !rw_ids.is_empty() {
        constraints.push(
            serde_json::json!({ "param": "calendarId", "values": rw_ids, "access": "read-write" }),
        );
    }
    let ro_ids: Vec<&str> = calendars
        .iter()
        .enumerate()
        .filter(|(i, _)| !rw_selected.contains(i))
        .map(|(_, (id, _, _))| id.as_str())
        .collect();
    if !ro_ids.is_empty() {
        constraints.push(
            serde_json::json!({ "param": "calendarId", "values": ro_ids, "access": "read-only" }),
        );
    }
    if !constraints.is_empty() {
        entry["constraints"] = serde_json::json!(constraints);
    }

    Ok(entry)
}
