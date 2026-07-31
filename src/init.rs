use google_workspace::error::GwsError;

use crate::auth;

pub const KNOWN_SERVICES: &[(&str, &str)] = &[
    ("drive", "Google Drive — files, folders, permissions"),
    ("gmail", "Gmail — messages, threads, labels, drafts"),
    ("calendar", "Google Calendar — events, calendars"),
    ("sheets", "Google Sheets — spreadsheets, values"),
    ("docs", "Google Docs — documents, content"),
    ("slides", "Google Slides — presentations, pages"),
    ("admin", "Admin SDK — users, groups, org units"),
    ("chat", "Google Chat — spaces, messages"),
    (
        "generativelanguage",
        "Google Generative AI — models, content generation",
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
        "drive" => serde_json::json!({
            "name": "drive",
            "constraints": [
                { "param": "parents", "values": ["<your-folder-id>"], "access": "read-write", "location": "body" }
            ]
        }),
        "gmail" => serde_json::json!({
            "name": "gmail",
            "denied_methods": [
                "messages.delete", "messages.trash", "messages.batchDelete",
                "settings.updateAutoForwarding",
                "settings.delegates.create",
                "settings.forwardingAddresses.create"
            ]
        }),
        "calendar" => serde_json::json!({
            "name": "calendar",
            "constraints": [
                { "param": "calendarId", "values": ["primary"], "access": "read-write" }
            ]
        }),
        _ => serde_json::json!({
            "name": name,
            "read_only": true
        }),
    }
}

pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "analyst",
        "Read-only Drive, Sheets, Docs. Gmail send-only. No calendar.",
    ),
    (
        "assistant",
        "Drive read-write, Gmail with safety blocks, Calendar primary read-write.",
    ),
    (
        "admin-readonly",
        "All services in read-only mode. Safe for auditing.",
    ),
];

fn list_templates() {
    eprintln!();
    eprintln!("Available policy templates:");
    eprintln!();
    for (name, desc) in TEMPLATES {
        eprintln!("  {name}");
        eprintln!("    {desc}");
        eprintln!();
    }
    eprintln!("Usage: mcp-google-workspace init --template <name>");
}

pub fn template_policy(name: &str) -> Result<serde_json::Value, GwsError> {
    if name == "list" {
        list_templates();
        std::process::exit(0);
    }

    let gmail_safety = serde_json::json!([
        "messages.delete",
        "messages.trash",
        "messages.batchDelete",
        "settings.updateAutoForwarding",
        "settings.delegates.create",
        "settings.forwardingAddresses.create"
    ]);

    match name {
        "analyst" => Ok(serde_json::json!({
            "server": { "read_only": false },
            "services": [
                { "name": "drive", "read_only": true },
                { "name": "sheets", "read_only": true },
                { "name": "docs", "read_only": true },
                {
                    "name": "gmail",
                    "denied_methods": gmail_safety
                }
            ]
        })),
        "assistant" => Ok(serde_json::json!({
            "server": { "read_only": false },
            "services": [
                { "name": "drive" },
                {
                    "name": "gmail",
                    "denied_methods": gmail_safety
                },
                {
                    "name": "calendar",
                    "constraints": [
                        { "param": "calendarId", "values": ["primary"], "access": "read-write" }
                    ]
                },
                { "name": "sheets" },
                { "name": "docs", "read_only": true }
            ]
        })),
        "admin-readonly" => Ok(serde_json::json!({
            "server": { "read_only": true },
            "services": [
                { "name": "drive" },
                { "name": "gmail" },
                { "name": "calendar" },
                { "name": "sheets" },
                { "name": "docs" },
                { "name": "slides" },
                { "name": "admin" },
                { "name": "chat" }
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
    use dialoguer::{Confirm, Input, MultiSelect};

    eprintln!();
    eprintln!("  MCP Google Workspace — Guided Setup");
    eprintln!();

    // Step 1: Auth check
    eprintln!("Step 1: Checking authentication...");
    let creds_file: String = Input::new()
        .with_prompt("Path to credentials JSON (leave empty for default chain)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;
    let creds_opt = if creds_file.is_empty() {
        None
    } else {
        Some(creds_file.as_str())
    };

    let scopes = &["https://www.googleapis.com/auth/drive.readonly"];
    match auth::get_token(scopes, creds_opt, None).await {
        Ok(_) => eprintln!("  \u{2713} Authentication working"),
        Err(e) => {
            eprintln!("  \u{2717} Authentication failed: {e}");
            eprintln!();
            eprintln!("  Run: gws auth login");
            eprintln!("  Then re-run: mcp-google-workspace init");
            return Err(GwsError::Validation("Authentication required".into()));
        }
    }

    eprintln!();

    // Step 2: Service selection
    eprintln!("Step 2: Select services");
    let labels: Vec<String> = KNOWN_SERVICES
        .iter()
        .map(|(name, desc)| format!("{name} — {desc}"))
        .collect();
    let defaults = vec![true, true, true, true, true, true, false, false, false];
    let selected = MultiSelect::new()
        .with_prompt("Which services?")
        .items(&labels)
        .defaults(&defaults[..labels.len().min(defaults.len())])
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if selected.is_empty() {
        return Err(GwsError::Validation("At least one service required".into()));
    }
    let chosen: Vec<&str> = selected.iter().map(|&i| KNOWN_SERVICES[i].0).collect();

    eprintln!();

    // Step 3: Per-service configuration with live API data
    let mut svc_entries: Vec<serde_json::Value> = Vec::new();

    for &name in &chosen {
        let entry = match name {
            "drive" => configure_drive_guided(creds_opt).await?,
            "gmail" => configure_gmail_guided(creds_opt).await?,
            "calendar" => configure_calendar_guided(creds_opt).await?,
            _ => configure_generic(name)?,
        };
        svc_entries.push(entry);
    }

    eprintln!();

    // Step 4: Server settings
    eprintln!("Step 4: Server settings");
    let read_only = Confirm::new()
        .with_prompt("Global read-only mode?")
        .default(false)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let project_id: String = Input::new()
        .with_prompt("Google Cloud project ID (for quota)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let mut server = serde_json::json!({ "read_only": read_only });
    if !project_id.is_empty() {
        server["project_id"] = serde_json::json!(project_id);
    }
    if !creds_file.is_empty() {
        server["credentials_file"] = serde_json::json!(creds_file);
    }

    Ok(serde_json::json!({
        "server": server,
        "services": svc_entries
    }))
}

async fn configure_drive_guided(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, MultiSelect};

    eprintln!("  Drive: Fetching your folders...");
    let mut entry = serde_json::json!({ "name": "drive" });
    entry["allowed_resources"] = serde_json::json!(["files"]);

    let scopes = &["https://www.googleapis.com/auth/drive.readonly"];
    let token = auth::get_token(scopes, creds, None)
        .await
        .map_err(|e| GwsError::Validation(format!("Drive auth failed: {e}")))?;

    let client = reqwest::Client::new();
    let resp = client
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(&token)
        .query(&[
            ("q", "mimeType='application/vnd.google-apps.folder' and 'root' in parents and trashed=false"),
            ("fields", "files(id,name)"),
            ("pageSize", "50"),
        ])
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("Drive API failed: {e}")))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Drive response parse failed: {e}")))?;

    let folders: Vec<(&str, &str)> = data
        .get("files")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let id = f.get("id")?.as_str()?;
                    let name = f.get("name")?.as_str()?;
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default();

    if folders.is_empty() {
        eprintln!("  No root folders found — Drive will be unrestricted");
        return Ok(entry);
    }

    let labels: Vec<String> = folders
        .iter()
        .map(|(id, name)| format!("{name} ({id})"))
        .collect();
    let selected = MultiSelect::new()
        .with_prompt("  Select writable Drive folders (space to toggle)")
        .items(&labels)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if !selected.is_empty() {
        let folder_ids: Vec<&str> = selected.iter().map(|&i| folders[i].0).collect();
        entry["constraints"] = serde_json::json!([{
            "param": "parents",
            "values": folder_ids,
            "access": "read-write",
            "location": "body-write-only",
            "mode": "restrict",
            "recursive": true
        }]);
    }

    let block_delete = Confirm::new()
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

    eprintln!("  Gmail: Fetching your labels...");
    let mut entry = configure_gmail()?;
    entry["allowed_resources"] = serde_json::json!([
        "users.messages",
        "users.threads",
        "users.labels",
        "users.drafts"
    ]);

    let scopes = &["https://www.googleapis.com/auth/gmail.readonly"];
    let token = auth::get_token(scopes, creds, None)
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
        .map_err(|e| GwsError::Validation(format!("Gmail response parse failed: {e}")))?;

    let user_labels: Vec<(&str, &str)> = data
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let id = l.get("id")?.as_str()?;
                    let name = l.get("name")?.as_str()?;
                    let ltype = l.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if ltype == "user" {
                        Some((id, name))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let restrict = Confirm::new()
        .with_prompt("  Restrict Gmail to specific labels?")
        .default(false)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if restrict && !user_labels.is_empty() {
        let mut labels: Vec<String> = vec![
            "INBOX".to_string(),
            "SENT".to_string(),
            "STARRED".to_string(),
        ];
        labels.extend(user_labels.iter().map(|(_, name)| name.to_string()));

        let selected = MultiSelect::new()
            .with_prompt("  Select allowed labels (space to toggle)")
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

    eprintln!("  Calendar: Fetching your calendars...");
    let mut entry = serde_json::json!({ "name": "calendar" });
    entry["allowed_resources"] = serde_json::json!(["events"]);

    let scopes = &["https://www.googleapis.com/auth/calendar.readonly"];
    let token = auth::get_token(scopes, creds, None)
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
        .map_err(|e| GwsError::Validation(format!("Calendar response parse failed: {e}")))?;

    let calendars: Vec<(&str, &str, &str)> = data
        .get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c.get("id")?.as_str()?;
                    let summary = c.get("summary").and_then(|s| s.as_str()).unwrap_or(id);
                    let access = c
                        .get("accessRole")
                        .and_then(|a| a.as_str())
                        .unwrap_or("reader");
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
        .map(|(id, name, role)| {
            if *id == "primary" || name.contains('@') {
                format!("{name} [{role}]")
            } else {
                format!("{name} [{role}] ({id})")
            }
        })
        .collect();

    let rw_selected = MultiSelect::new()
        .with_prompt("  Select read-write calendars (space to toggle)")
        .items(&labels)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let mut constraints = Vec::new();

    let rw_ids: Vec<&str> = rw_selected.iter().map(|&i| calendars[i].0).collect();
    if !rw_ids.is_empty() {
        constraints.push(serde_json::json!({
            "param": "calendarId",
            "values": rw_ids,
            "access": "read-write"
        }));
    }

    let ro_ids: Vec<&str> = calendars
        .iter()
        .enumerate()
        .filter(|(i, _)| !rw_selected.contains(i))
        .map(|(_, (id, _, _))| *id)
        .collect();
    if !ro_ids.is_empty() {
        constraints.push(serde_json::json!({
            "param": "calendarId",
            "values": ro_ids,
            "access": "read-only"
        }));
    }

    if !constraints.is_empty() {
        entry["constraints"] = serde_json::json!(constraints);
    }

    Ok(entry)
}

pub fn init_policy_interactive() -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, Input, MultiSelect, Select};

    eprintln!();
    eprintln!("  MCP Google Workspace — Policy Generator");
    eprintln!();

    let mut template_labels: Vec<String> = TEMPLATES
        .iter()
        .map(|(name, desc)| format!("{name} — {desc}"))
        .collect();
    template_labels.push("Custom — configure services individually".to_string());

    let choice = Select::new()
        .with_prompt("Start from a template or configure manually?")
        .items(&template_labels)
        .default(template_labels.len() - 1)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if choice < TEMPLATES.len() {
        return template_policy(TEMPLATES[choice].0);
    }

    let labels: Vec<String> = KNOWN_SERVICES
        .iter()
        .map(|(name, desc)| format!("{name} — {desc}"))
        .collect();

    let defaults = vec![true, true, true, false, false, false, false, false];
    let selected = MultiSelect::new()
        .with_prompt("Which services do you want to enable?")
        .items(&labels)
        .defaults(&defaults)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if selected.is_empty() {
        return Err(GwsError::Validation(
            "At least one service must be selected".to_string(),
        ));
    }

    let chosen: Vec<&str> = selected.iter().map(|&i| KNOWN_SERVICES[i].0).collect();

    let mut svc_entries: Vec<serde_json::Value> = Vec::new();

    for &name in &chosen {
        let entry = match name {
            "drive" => configure_drive()?,
            "gmail" => configure_gmail()?,
            "calendar" => configure_calendar()?,
            _ => configure_generic(name)?,
        };
        svc_entries.push(entry);
    }

    eprintln!();
    let read_only = Confirm::new()
        .with_prompt("Global read-only mode? (blocks all writes across all services)")
        .default(false)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let project_id: String = Input::new()
        .with_prompt("Google Cloud project ID (for quota)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let credentials_file: String = Input::new()
        .with_prompt("Path to credentials JSON (leave empty to use default chain)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let mut server = serde_json::json!({ "read_only": read_only });
    if !project_id.is_empty() {
        server["project_id"] = serde_json::json!(project_id);
    }
    if !credentials_file.is_empty() {
        server["credentials_file"] = serde_json::json!(credentials_file);
    }

    Ok(serde_json::json!({
        "server": server,
        "services": svc_entries
    }))
}

fn configure_drive() -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, Input};

    eprintln!();
    let restrict = Confirm::new()
        .with_prompt("Drive: Restrict access to specific folders?")
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if !restrict {
        return Ok(serde_json::json!({ "name": "drive" }));
    }

    let mut constraints: Vec<serde_json::Value> = Vec::new();
    loop {
        let path: String = Input::new()
            .with_prompt("  Folder path (e.g. Projects/output, or empty to finish)")
            .allow_empty(true)
            .interact_text()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        if path.is_empty() {
            break;
        }

        let rw = Confirm::new()
            .with_prompt(format!("  Allow writes to '{path}'?"))
            .default(true)
            .interact()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        let access = if rw { "read-write" } else { "read-only" };
        constraints.push(serde_json::json!({
            "param": "parents", "values": [path], "access": access, "location": "body"
        }));
    }

    if constraints.is_empty() {
        Ok(serde_json::json!({ "name": "drive" }))
    } else {
        Ok(serde_json::json!({ "name": "drive", "constraints": constraints }))
    }
}

fn configure_gmail() -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, Input};

    eprintln!();
    let block_delete = Confirm::new()
        .with_prompt("Gmail: Block message deletion? (recommended)")
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let block_forwarding = Confirm::new()
        .with_prompt("Gmail: Block auto-forwarding and delegate changes? (recommended)")
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let restrict_labels = Confirm::new()
        .with_prompt("Gmail: Restrict access to specific labels only?")
        .default(false)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    let mut denied = Vec::new();
    if block_delete {
        denied.extend_from_slice(&["messages.delete", "messages.trash", "messages.batchDelete"]);
    }
    if block_forwarding {
        denied.extend_from_slice(&[
            "settings.updateAutoForwarding",
            "settings.delegates.create",
            "settings.forwardingAddresses.create",
        ]);
    }

    let mut policy = serde_json::json!({ "name": "gmail" });
    if !denied.is_empty() {
        policy["denied_methods"] = serde_json::json!(denied);
    }

    if restrict_labels {
        let labels_input: String = Input::new()
            .with_prompt("Allowed label names (comma-separated)")
            .interact_text()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;
        let labels: Vec<String> = labels_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !labels.is_empty() {
            policy["allowed_labels"] = serde_json::json!(labels);
        }
    }

    Ok(policy)
}

fn configure_calendar() -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, Input};

    eprintln!();
    let restrict = Confirm::new()
        .with_prompt("Calendar: Restrict to specific calendars?")
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if !restrict {
        return Ok(serde_json::json!({ "name": "calendar" }));
    }

    let mut constraints: Vec<serde_json::Value> = Vec::new();

    let use_primary = Confirm::new()
        .with_prompt("  Include your primary calendar?")
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if use_primary {
        let rw = Confirm::new()
            .with_prompt("  Allow writes to primary calendar?")
            .default(true)
            .interact()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        let access = if rw { "read-write" } else { "read-only" };
        constraints.push(serde_json::json!({
            "param": "calendarId", "values": ["primary"], "access": access
        }));
    }

    loop {
        let id: String = Input::new()
            .with_prompt("  Additional calendar ID (or empty to finish)")
            .allow_empty(true)
            .interact_text()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        if id.is_empty() {
            break;
        }

        let rw = Confirm::new()
            .with_prompt(format!("  Allow writes to '{id}'?"))
            .default(false)
            .interact()
            .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

        let access = if rw { "read-write" } else { "read-only" };
        constraints.push(serde_json::json!({
            "param": "calendarId", "values": [id], "access": access
        }));
    }

    if constraints.is_empty() {
        Ok(serde_json::json!({ "name": "calendar" }))
    } else {
        Ok(serde_json::json!({ "name": "calendar", "constraints": constraints }))
    }
}

fn configure_generic(name: &str) -> Result<serde_json::Value, GwsError> {
    use dialoguer::Confirm;

    eprintln!();
    let read_only = Confirm::new()
        .with_prompt(format!("{name}: Read-only access?"))
        .default(true)
        .interact()
        .map_err(|e| GwsError::Validation(format!("Prompt failed: {e}")))?;

    if read_only {
        Ok(serde_json::json!({ "name": name, "read_only": true }))
    } else {
        Ok(serde_json::json!({ "name": name }))
    }
}
