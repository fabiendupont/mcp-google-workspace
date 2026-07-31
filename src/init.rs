use console::style;
use google_workspace::error::GwsError;
use inquire::{Confirm, MultiSelect, Select, Text};

use crate::auth;

const SERVICES: &[(&str, &str)] = &[
    ("drive", "Google Drive — files and folders"),
    ("gmail", "Gmail — messages, threads, labels"),
    ("calendar", "Google Calendar — events and scheduling"),
    ("sheets", "Google Sheets — spreadsheets"),
    ("docs", "Google Docs — documents"),
    ("slides", "Google Slides — presentations"),
    ("people", "Google Contacts — contact lookup"),
    ("generativelanguage", "Gemini AI — image generation"),
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
    let entries: Vec<serde_json::Value> = services.iter().map(|n| default_entry(n)).collect();
    serde_json::json!({ "server": { "read_only": false }, "services": entries })
}

fn default_entry(name: &str) -> serde_json::Value {
    match name {
        "drive" => serde_json::json!({ "name": "drive", "allowed_resources": ["files"] }),
        "gmail" => {
            serde_json::json!({ "name": "gmail", "allowed_resources": ["users.messages","users.threads","users.labels","users.drafts"], "denied_methods": gmail_deny() })
        }
        "calendar" => {
            serde_json::json!({ "name": "calendar", "allowed_resources": ["events"], "constraints": [{"param":"calendarId","values":["primary"],"access":"read-write"}] })
        }
        "sheets" => serde_json::json!({ "name": "sheets", "allowed_resources": ["spreadsheets"] }),
        "docs" => serde_json::json!({ "name": "docs", "allowed_resources": ["documents"] }),
        "slides" => serde_json::json!({ "name": "slides", "allowed_resources": ["presentations"] }),
        "people" => {
            serde_json::json!({ "name": "people", "read_only": true, "allowed_resources": ["people","people.connections"] })
        }
        _ => serde_json::json!({ "name": name }),
    }
}

fn gmail_deny() -> serde_json::Value {
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
        eprintln!("\n{}\n", style("Available policy templates:").bold());
        for (n, d) in TEMPLATES {
            eprintln!("  {} — {d}", style(n).cyan());
        }
        eprintln!("\nUsage: mcp-google-workspace init --template <name>");
        std::process::exit(0);
    }
    match name {
        "analyst" => Ok(
            serde_json::json!({ "server": {"read_only":false}, "services": [
                {"name":"drive","read_only":true,"allowed_resources":["files"]},
                {"name":"sheets","read_only":true,"allowed_resources":["spreadsheets"]},
                {"name":"docs","read_only":true,"allowed_resources":["documents"]},
                {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels","users.drafts"],"denied_methods":gmail_deny()}
            ]}),
        ),
        "assistant" => Ok(
            serde_json::json!({ "server": {"read_only":false}, "services": [
                {"name":"drive","allowed_resources":["files"]},
                {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels","users.drafts"],"denied_methods":gmail_deny()},
                {"name":"calendar","allowed_resources":["events"],"constraints":[{"param":"calendarId","values":["primary"],"access":"read-write"}]},
                {"name":"sheets","allowed_resources":["spreadsheets"]},
                {"name":"docs","allowed_resources":["documents"]},
                {"name":"people","read_only":true,"allowed_resources":["people","people.connections"]}
            ]}),
        ),
        "admin-readonly" => Ok(
            serde_json::json!({ "server": {"read_only":true}, "services": [
                {"name":"drive","allowed_resources":["files"]},
                {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels"]},
                {"name":"calendar","allowed_resources":["events"]},
                {"name":"sheets","allowed_resources":["spreadsheets"]},
                {"name":"docs","allowed_resources":["documents"]},
                {"name":"slides","allowed_resources":["presentations"]}
            ]}),
        ),
        _ => Err(GwsError::Validation(format!(
            "Unknown template '{name}'. Use --template list"
        ))),
    }
}

fn err(e: inquire::InquireError) -> GwsError {
    GwsError::Validation(format!("{e}"))
}

pub async fn init_guided() -> Result<serde_json::Value, GwsError> {
    eprintln!(
        "\n{}\n",
        style("  MCP Google Workspace — Setup Wizard").bold().cyan()
    );

    // 1. Auth
    eprintln!("{}", style("Step 1: Authentication").bold());
    let creds_file = Text::new("Credentials JSON path:")
        .with_help_message("Leave empty for default chain (gws auth, ADC)")
        .with_default("")
        .prompt()
        .map_err(err)?;
    let creds = if creds_file.is_empty() {
        None
    } else {
        Some(creds_file.as_str())
    };

    match auth::get_token(
        &["https://www.googleapis.com/auth/drive.readonly"],
        creds,
        None,
    )
    .await
    {
        Ok(_) => eprintln!("  {} Authentication working\n", style("\u{2713}").green()),
        Err(e) => {
            eprintln!("  {} {e}\n", style("\u{2717}").red());
            eprintln!(
                "  Run: {}  then re-run init",
                style("gws auth login").yellow()
            );
            return Err(GwsError::Validation("Authentication required".into()));
        }
    }

    // 2. Services
    eprintln!("{}", style("Step 2: Services").bold());
    let labels: Vec<&str> = SERVICES.iter().map(|(_, d)| *d).collect();
    let defaults: Vec<usize> = (0..SERVICES.len()).collect();
    let selected = MultiSelect::new("Enable:", labels)
        .with_default(&defaults)
        .with_help_message("\u{2191}\u{2193} move  space toggle  enter confirm")
        .prompt()
        .map_err(err)?;

    if selected.is_empty() {
        return Err(GwsError::Validation("At least one service required".into()));
    }
    let chosen: Vec<&str> = selected
        .iter()
        .filter_map(|desc| SERVICES.iter().find(|(_, d)| d == desc).map(|(n, _)| *n))
        .collect();
    eprintln!();

    // 3. Per-service
    eprintln!("{}", style("Step 3: Configure services").bold());
    let mut entries = Vec::new();
    for &name in &chosen {
        entries.push(match name {
            "drive" => drive_setup(creds).await?,
            "gmail" => gmail_setup(creds).await?,
            "calendar" => calendar_setup(creds).await?,
            _ => default_entry(name),
        });
    }
    eprintln!();

    // 4. Project
    eprintln!("{}", style("Step 4: Google Cloud project").bold());
    let project_id = pick_project()?;
    eprintln!();

    let mut server = serde_json::json!({});
    if !project_id.is_empty() {
        server["project_id"] = serde_json::json!(project_id);
    }
    if !creds_file.is_empty() {
        server["credentials_file"] = serde_json::json!(creds_file);
    }

    // 5. Save
    eprintln!("{}", style("Step 5: Save").bold());
    let paths = vec![
        ".gws-policy.json  (per-project, gitignored) \u{2190} recommended",
        "gws-policy.json   (shared, committable)",
        "~/.config/gws/policy.json  (global)",
        "Custom path",
    ];
    let choice = Select::new("Save to:", paths)
        .with_help_message("\u{2191}\u{2193} move  enter select")
        .prompt()
        .map_err(err)?;

    let path = if choice.starts_with(".gws") {
        ".gws-policy.json".into()
    } else if choice.starts_with("gws") {
        "gws-policy.json".into()
    } else if choice.starts_with("~/") {
        let d = dirs_next::config_dir()
            .unwrap_or_else(|| ".".into())
            .join("gws");
        std::fs::create_dir_all(&d).ok();
        d.join("policy.json").display().to_string()
    } else {
        Text::new("Path:")
            .with_default(".gws-policy.json")
            .prompt()
            .map_err(err)?
    };

    let policy = serde_json::json!({ "server": server, "services": entries });
    std::fs::write(&path, serde_json::to_string_pretty(&policy).unwrap())
        .map_err(|e| GwsError::Validation(format!("Write failed: {e}")))?;

    eprintln!(
        "\n  {} Saved to {}",
        style("\u{2713}").green(),
        style(&path).cyan()
    );
    if path.starts_with('.') {
        eprintln!(
            "\n  Add to .gitignore:\n    {}",
            style(format!("echo '{path}' >> .gitignore")).yellow()
        );
    }
    eprintln!(
        "\n  Auto-discovers {} — just run:\n    {}",
        style(&path).cyan(),
        style("mcp-google-workspace").yellow()
    );
    eprintln!(
        "\n  Validate:\n    {}\n",
        style(format!("mcp-google-workspace check-policy {path}")).yellow()
    );
    std::process::exit(0);
}

fn pick_project() -> Result<String, GwsError> {
    let mut projects = Vec::new();
    if let Ok(out) = std::process::Command::new("gcloud")
        .args([
            "projects",
            "list",
            "--format=json(projectId,name)",
            "--limit=20",
        ])
        .output()
        && out.status.success()
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        && let Some(arr) = v.as_array()
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
        let mut opts: Vec<String> = projects
            .iter()
            .map(|(id, name)| format!("{name} ({id})"))
            .collect();
        opts.push("Enter manually".into());
        opts.push("Skip".into());
        let choice = Select::new("Project:", opts.clone())
            .with_help_message("\u{2191}\u{2193} move  type to filter  enter select")
            .prompt()
            .map_err(err)?;
        let idx = opts.iter().position(|o| o == &choice).unwrap_or(0);
        if idx < projects.len() {
            return Ok(projects[idx].0.clone());
        }
        if choice == "Skip" {
            return Ok(String::new());
        }
    }

    Text::new("Project ID:")
        .with_help_message("For API quota. Empty to skip.")
        .with_default("")
        .prompt()
        .map_err(err)
}

async fn drive_setup(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    eprintln!("  {} Fetching folders...", style("Drive").cyan());
    let mut entry = serde_json::json!({ "name": "drive", "allowed_resources": ["files"] });
    let token = auth::get_token(
        &["https://www.googleapis.com/auth/drive.readonly"],
        creds,
        None,
    )
    .await
    .map_err(|e| GwsError::Validation(format!("Auth: {e}")))?;

    let search = Text::new("  Search folders:")
        .with_help_message("Empty shows root folders")
        .with_default("")
        .prompt()
        .map_err(err)?;

    let q = if search.is_empty() {
        "mimeType='application/vnd.google-apps.folder' and 'root' in parents and trashed=false"
            .into()
    } else {
        format!(
            "mimeType='application/vnd.google-apps.folder' and name contains '{}' and trashed=false",
            search.replace('\'', "\\'")
        )
    };

    let data: serde_json::Value = reqwest::Client::new()
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(&token)
        .query(&[
            ("q", q.as_str()),
            ("fields", "files(id,name)"),
            ("pageSize", "50"),
        ])
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("API: {e}")))?
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Parse: {e}")))?;

    let folders: Vec<(String, String)> = data
        .get("files")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some((
                        f.get("id")?.as_str()?.into(),
                        f.get("name")?.as_str()?.into(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    if folders.is_empty() {
        eprintln!("    {} No folders found", style("!").yellow());
        return Ok(entry);
    }

    let labels: Vec<String> = folders
        .iter()
        .map(|(id, name)| format!("{name}  ({})", &id[..id.len().min(12)]))
        .collect();
    let selected = MultiSelect::new("  Writable folders:", labels.clone())
        .with_help_message("\u{2191}\u{2193} move  space toggle  enter confirm")
        .prompt()
        .map_err(err)?;

    if !selected.is_empty() {
        let ids: Vec<&str> = selected
            .iter()
            .filter_map(|l| {
                labels
                    .iter()
                    .position(|x| x == l)
                    .map(|i| folders[i].0.as_str())
            })
            .collect();
        entry["constraints"] = serde_json::json!([{"param":"parents","values":ids,"access":"read-write","location":"body-write-only","mode":"restrict","recursive":true}]);
    }

    if Confirm::new("  Block permanent deletion?")
        .with_default(true)
        .prompt()
        .map_err(err)?
    {
        entry["denied_methods"] = serde_json::json!(["files.delete"]);
    }
    Ok(entry)
}

async fn gmail_setup(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    eprintln!("  {} Fetching labels...", style("Gmail").cyan());
    let mut entry = serde_json::json!({ "name": "gmail", "allowed_resources": ["users.messages","users.threads","users.labels","users.drafts"], "denied_methods": gmail_deny() });

    let token = auth::get_token(
        &["https://www.googleapis.com/auth/gmail.readonly"],
        creds,
        None,
    )
    .await
    .map_err(|e| GwsError::Validation(format!("Auth: {e}")))?;
    let data: serde_json::Value = reqwest::Client::new()
        .get("https://www.googleapis.com/gmail/v1/users/me/labels")
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("API: {e}")))?
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Parse: {e}")))?;

    let user_labels: Vec<String> = data
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    if l.get("type").and_then(|t| t.as_str()) == Some("user") {
                        l.get("name")?.as_str().map(String::from)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    if Confirm::new("  Restrict to specific labels?")
        .with_default(false)
        .with_help_message("If yes, agent only sees messages with selected labels")
        .prompt()
        .map_err(err)?
        && !user_labels.is_empty()
    {
        let mut all = vec!["INBOX".into(), "SENT".into(), "STARRED".into()];
        all.extend(user_labels);
        let picked = MultiSelect::new("  Allowed labels:", all)
            .with_help_message("\u{2191}\u{2193} move  space toggle  enter confirm")
            .prompt()
            .map_err(err)?;
        if !picked.is_empty() {
            entry["allowed_labels"] = serde_json::json!(picked);
        }
    }
    Ok(entry)
}

async fn calendar_setup(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    eprintln!("  {} Fetching calendars...", style("Calendar").cyan());
    let mut entry = serde_json::json!({ "name": "calendar", "allowed_resources": ["events"] });

    let token = auth::get_token(
        &["https://www.googleapis.com/auth/calendar.readonly"],
        creds,
        None,
    )
    .await
    .map_err(|e| GwsError::Validation(format!("Auth: {e}")))?;
    let data: serde_json::Value = reqwest::Client::new()
        .get("https://www.googleapis.com/calendar/v3/users/me/calendarList")
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| GwsError::Validation(format!("API: {e}")))?
        .json()
        .await
        .map_err(|e| GwsError::Validation(format!("Parse: {e}")))?;

    let cals: Vec<(String, String, String)> = data
        .get("items")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let id = c.get("id")?.as_str()?.to_string();
                    let name = c
                        .get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or(&id)
                        .to_string();
                    let role = c
                        .get("accessRole")
                        .and_then(|a| a.as_str())
                        .unwrap_or("reader")
                        .to_string();
                    if role == "freeBusyReader" {
                        return None;
                    }
                    Some((id, name, role))
                })
                .collect()
        })
        .unwrap_or_default();

    if cals.is_empty() {
        entry["constraints"] =
            serde_json::json!([{"param":"calendarId","values":["primary"],"access":"read-write"}]);
        return Ok(entry);
    }

    let labels: Vec<String> = cals
        .iter()
        .map(|(_, name, role)| format!("{name} [{role}]"))
        .collect();
    let selected = MultiSelect::new("  Read-write calendars:", labels.clone())
        .with_help_message(
            "\u{2191}\u{2193} move  space toggle  enter confirm. Others → read-only.",
        )
        .prompt()
        .map_err(err)?;

    let rw_idx: Vec<usize> = selected
        .iter()
        .filter_map(|l| labels.iter().position(|x| x == l))
        .collect();
    let mut constraints = Vec::new();
    let rw: Vec<&str> = rw_idx.iter().map(|&i| cals[i].0.as_str()).collect();
    if !rw.is_empty() {
        constraints
            .push(serde_json::json!({"param":"calendarId","values":rw,"access":"read-write"}));
    }
    let ro: Vec<&str> = cals
        .iter()
        .enumerate()
        .filter(|(i, _)| !rw_idx.contains(i))
        .map(|(_, (id, _, _))| id.as_str())
        .collect();
    if !ro.is_empty() {
        constraints
            .push(serde_json::json!({"param":"calendarId","values":ro,"access":"read-only"}));
    }
    if !constraints.is_empty() {
        entry["constraints"] = serde_json::json!(constraints);
    }
    Ok(entry)
}
