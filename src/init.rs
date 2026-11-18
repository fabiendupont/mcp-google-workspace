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
        "Read-only Drive, Sheets, Docs. Gmail read-write (no delete/forwarding).",
    ),
    (
        "assistant",
        "Drive read-write, Gmail (safe), Calendar, Tasks, Sheets, Docs read-only, People read-only.",
    ),
    (
        "admin-readonly",
        "All services in read-only mode. Safe for auditing.",
    ),
    (
        "content-writer",
        "Drive, Docs, Slides, Sheets read-write. Gmail read-write without send. No Calendar.",
    ),
    (
        "it-admin",
        "Drive/Gmail/Sheets/Docs/Slides/People read-only, Calendar read-write on the primary calendar.",
    ),
];

/// JSON has no comments, so the generated `server` block carries this hint in
/// a `_comment` field. `confirm_writes` itself keeps its default value (false).
const CONFIRM_WRITES_COMMENT: &str =
    "Set \"confirm_writes\": true to require user approval before write tools run (default: false)";

pub fn generate_policy(services: &[String]) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = services.iter().map(|n| default_entry(n)).collect();
    serde_json::json!({
        "server": { "read_only": false, "_comment": CONFIRM_WRITES_COMMENT },
        "services": entries
    })
}

fn default_entry(name: &str) -> serde_json::Value {
    match name {
        "drive" => serde_json::json!({
            "name": "drive",
            "allowed_resources": ["files"],
            "denied_methods": crate::policy::baseline_denials("drive")
        }),
        "gmail" => {
            serde_json::json!({ "name": "gmail", "allowed_resources": ["users.messages","users.threads","users.labels","users.drafts"], "denied_methods": crate::policy::baseline_denials("gmail") })
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
        // Read-heavy by design: Gmail stays read-write so the agent can draft
        // and send analysis output. The baseline denials remove delete/trash
        // and forwarding/delegation, which is what the docs table describes
        // ("Gmail (RW, no delete/forwarding)"), not "send-only". Both halves
        // are pinned by `test_template_docs_table_matches_emitted_policy`.
        "analyst" => Ok(
            serde_json::json!({ "server": {"read_only":false}, "services": [
                {"name":"drive","read_only":true,"allowed_resources":["files"],"denied_methods":crate::policy::baseline_denials("drive")},
                {"name":"sheets","read_only":true,"allowed_resources":["spreadsheets"]},
                {"name":"docs","read_only":true,"allowed_resources":["documents"]},
                {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels","users.drafts"],"denied_methods":crate::policy::baseline_denials("gmail")}
            ]}),
        ),
        "assistant" => Ok(
            serde_json::json!({ "server": {"read_only":false}, "services": [
                {"name":"drive","allowed_resources":["files"],"denied_methods":crate::policy::baseline_denials("drive")},
                {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels","users.drafts"],"denied_methods":crate::policy::baseline_denials("gmail")},
                {"name":"calendar","allowed_resources":["events"],"constraints":[{"param":"calendarId","values":["primary"],"access":"read-write"}]},
                {"name":"tasks","allowed_resources":["tasks","tasklists"]},
                {"name":"sheets","allowed_resources":["spreadsheets"]},
                {"name":"docs","read_only":true,"allowed_resources":["documents"]},
                {"name":"people","read_only":true,"allowed_resources":["people","people.connections"]}
            ]}),
        ),
        "admin-readonly" => Ok(
            serde_json::json!({ "server": {"read_only":true}, "services": [
                {"name":"drive","allowed_resources":["files"],"denied_methods":crate::policy::baseline_denials("drive")},
                {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels"]},
                {"name":"calendar","allowed_resources":["events"]},
                {"name":"sheets","allowed_resources":["spreadsheets"]},
                {"name":"docs","allowed_resources":["documents"]},
                {"name":"slides","allowed_resources":["presentations"]}
            ]}),
        ),
        "content-writer" => {
            // Not drafts-only: reads stay (a reply has to be drafted from
            // context) and label mutation stays. Denying `messages.send`
            // blocks the whole send family, including `drafts.send`
            // (see the deny-alias table in `policy.rs`), so nothing can leave
            // the mailbox. The docs row says "Gmail (RW, no send)" to match.
            let mut deny = crate::policy::baseline_denials("gmail");
            deny.push("messages.send".to_string());
            Ok(
                serde_json::json!({ "server": {"read_only":false}, "services": [
                    {"name":"drive","allowed_resources":["files"],"denied_methods":crate::policy::baseline_denials("drive")},
                    {"name":"docs","allowed_resources":["documents"]},
                    {"name":"slides","allowed_resources":["presentations"]},
                    {"name":"sheets","allowed_resources":["spreadsheets"]},
                    {"name":"gmail","allowed_resources":["users.messages","users.threads","users.labels","users.drafts"],"denied_methods":deny}
                ]}),
            )
        }
        "it-admin" => Ok(
            serde_json::json!({ "server": {"read_only":false}, "services": [
                {"name":"drive","read_only":true,"allowed_resources":["files"],"denied_methods":crate::policy::baseline_denials("drive")},
                {"name":"gmail","read_only":true,"allowed_resources":["users.messages","users.threads","users.labels"]},
                {"name":"calendar","allowed_resources":["events"],"constraints":[{"param":"calendarId","values":["primary"],"access":"read-write"}]},
                {"name":"sheets","read_only":true,"allowed_resources":["spreadsheets"]},
                {"name":"docs","read_only":true,"allowed_resources":["documents"]},
                {"name":"slides","read_only":true,"allowed_resources":["presentations"]},
                {"name":"people","read_only":true,"allowed_resources":["people","people.connections"]}
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

/// Split a comma-separated subject prompt into a deduplicated allowlist.
fn parse_subject_list(input: &str) -> Vec<String> {
    let mut subjects: Vec<String> = Vec::new();
    for part in input.split(',') {
        let subject = part.trim();
        if subject.is_empty() {
            continue;
        }
        if !subjects
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(subject))
        {
            subjects.push(subject.to_string());
        }
    }
    subjects
}

/// Whether a credentials JSON document describes a service account.
fn is_service_account_json(contents: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(contents)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .is_some_and(|kind| kind == "service_account")
}

/// Whether the active credential is a service account, which is the only case
/// where `server.allowed_subjects` gates per-request delegation.
fn uses_service_account_credential(creds_file: &str) -> bool {
    let path = if creds_file.is_empty() {
        std::env::var("GOOGLE_APPLICATION_CREDENTIALS").unwrap_or_default()
    } else {
        creds_file.to_string()
    };
    if path.is_empty() {
        return false;
    }
    std::fs::read_to_string(path)
        .map(|contents| is_service_account_json(&contents))
        .unwrap_or(false)
}

/// Whether the local `.gitignore` already lists `path` verbatim.
fn gitignore_covers(path: &str) -> bool {
    std::fs::read_to_string(".gitignore")
        .map(|contents| gitignore_covers_str(&contents, path))
        .unwrap_or(false)
}

fn gitignore_covers_str(contents: &str, path: &str) -> bool {
    contents.lines().map(str::trim).any(|line| line == path)
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

    // 5. Delegation (service accounts only). Client-supplied `_meta.user_email`
    // is rejected unless it is listed in server.allowed_subjects.
    let service_account = uses_service_account_credential(&creds_file);
    let mut allowed_subjects: Vec<String> = Vec::new();
    let mut step = 5;
    if service_account {
        eprintln!(
            "{}",
            style(format!("Step {step}: Delegation (service account)")).bold()
        );
        let raw = Text::new("Allowed delegation subjects (comma-separated, blank = none):")
            .with_help_message(
                "Subjects clients may request via _meta.user_email; blank disables per-request delegation",
            )
            .with_default("")
            .prompt()
            .map_err(err)?;
        allowed_subjects = parse_subject_list(&raw);
        if !allowed_subjects.is_empty() {
            server["allowed_subjects"] = serde_json::json!(allowed_subjects);
        }
        eprintln!();
        step += 1;
    }

    // Approval is the mitigation for write tools; keep the default (false) but
    // leave a commented hint in the generated server block.
    server["_comment"] = serde_json::json!(CONFIRM_WRITES_COMMENT);

    // 6. Save
    eprintln!("{}", style(format!("Step {step}: Save")).bold());
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
    write_policy_file(&path, &serde_json::to_string_pretty(&policy).unwrap())?;

    eprintln!(
        "\n  {} Saved to {}",
        style("\u{2713}").green(),
        style(&path).cyan()
    );
    if path.starts_with('.') {
        if gitignore_covers(&path) {
            eprintln!(
                "\n  {} is already listed in .gitignore",
                style(&path).cyan()
            );
        } else {
            eprintln!(
                "\n  Add to .gitignore:\n    {}",
                style(format!("echo '{path}' >> .gitignore")).yellow()
            );
        }
    }
    eprintln!(
        "\n  Auto-discovers {} — just run:\n    {}",
        style(&path).cyan(),
        style("mcp-google-workspace").yellow()
    );
    eprintln!(
        "\n  Write tools run without approval by default. Set \"confirm_writes\": true \
         in \"server\" to require user approval before writes."
    );
    eprintln!(
        "\n  Folder scope is not a general write barrier. Two escape hatches cover what \
         it does not:\n    \
         \"denied_methods\" — block a method family outright (e.g. \"files.delete\", \
         \"messages.send\");\n    \
         \"read_only\": true — make a whole service read-only.\n  \
         \"location\": \"body-write-only\" means the \"parents\" constraint is checked on \
         writes; list results and reads by id are scoped separately."
    );
    if service_account {
        if allowed_subjects.is_empty() {
            eprintln!(
                "  Per-request delegation is disabled: \"allowed_subjects\" is empty, so \
                 client-supplied _meta.user_email values are rejected."
            );
        } else {
            eprintln!(
                "  Per-request delegation allowed for: {}",
                allowed_subjects.join(", ")
            );
        }
    }
    eprintln!(
        "\n  Validate:\n    {}\n",
        style(format!("mcp-google-workspace check-policy {path}")).yellow()
    );
    std::process::exit(0);
}

fn pick_project() -> Result<String, GwsError> {
    let mut gws_project = String::new();
    if let Ok(out) = std::process::Command::new("gws")
        .args(["auth", "status"])
        .output()
        && out.status.success()
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        && let Some(pid) = v.get("project_id").and_then(|v| v.as_str())
    {
        gws_project = pid.to_string();
    }

    let mut current_project = gws_project.clone();
    if current_project.is_empty()
        && let Ok(out) = std::process::Command::new("gcloud")
            .args(["config", "get-value", "project"])
            .output()
        && out.status.success()
    {
        current_project = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }

    let mut projects = Vec::new();
    if let Ok(out) = std::process::Command::new("gcloud")
        .args([
            "projects",
            "list",
            "--format=json(projectId,name)",
            "--sort-by=name",
            "--limit=50",
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

    if !current_project.is_empty() && !projects.iter().any(|(id, _)| id == &current_project) {
        projects.insert(
            0,
            (
                current_project.clone(),
                format!("(current) {current_project}"),
            ),
        );
    }

    if !projects.is_empty() {
        let mut opts: Vec<String> = projects
            .iter()
            .map(|(id, name)| format!("{name} ({id})"))
            .collect();
        opts.push("Enter manually".into());
        opts.push("Skip".into());

        let default_idx = if !current_project.is_empty() {
            projects
                .iter()
                .position(|(id, _)| id == &current_project)
                .unwrap_or(0)
        } else {
            0
        };

        let choice = Select::new("Project:", opts.clone())
            .with_starting_cursor(default_idx)
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

/// Constraint emitted for the folder list picked by the wizard. The shape is
/// load-bearing (`body-write-only` keeps reads by id scoped without requiring
/// `parents` on rename-style writes), so it lives here and is pinned by a test.
fn drive_folder_constraint(ids: &[&str]) -> serde_json::Value {
    serde_json::json!([{
        "param": "parents",
        "values": ids,
        "access": "read-write",
        "location": "body-write-only",
        "mode": "restrict",
        "recursive": true
    }])
}

/// What sits at a policy path, without following symlinks.
#[derive(Debug, PartialEq, Eq)]
enum TargetState {
    Missing,
    Regular,
    Symlink,
}

fn target_state(path: &str) -> Result<TargetState, GwsError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(TargetState::Symlink),
        Ok(_) => Ok(TargetState::Regular),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TargetState::Missing),
        Err(e) => Err(GwsError::Validation(format!(
            "Cannot inspect '{path}': {e}"
        ))),
    }
}

/// Write the generated policy without clobbering an existing file or following
/// a symlink. The wizard is interactive, so an explicit confirmation prompt is
/// the overwrite guard (there is no `--force` flag yet).
fn write_policy_file(path: &str, contents: &str) -> Result<(), GwsError> {
    match target_state(path)? {
        TargetState::Symlink => {
            return Err(GwsError::Validation(format!(
                "Refusing to write through the symlink '{path}'. Choose a regular file path."
            )));
        }
        TargetState::Regular => {
            if !Confirm::new(&format!("  {path} exists. Overwrite it?"))
                .with_default(false)
                .with_help_message("Enter y to replace the existing policy file, n to abort")
                .prompt()
                .map_err(err)?
            {
                return Err(GwsError::Validation(format!(
                    "Aborted: '{path}' was left untouched"
                )));
            }
        }
        TargetState::Missing => {}
    }
    std::fs::write(path, contents).map_err(|e| GwsError::Validation(format!("Write failed: {e}")))
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
    eprintln!(
        "    {} This list is not a general write barrier. It controls where the agent may \
         create/copy files, which folders `files.list` is narrowed to, and the ancestry \
         check for reads by id.",
        style("i").cyan()
    );
    let selected = MultiSelect::new(
        "  Folders the agent may create files in and read:",
        labels.clone(),
    )
    .with_help_message(
        "\u{2191}\u{2193} move  space toggle  enter confirm. By-id writes such as \
                 rename and trash are NOT confined by this list — use denied_methods.",
    )
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
        entry["constraints"] = drive_folder_constraint(&ids);
        eprintln!(
            "    {} \"location\": \"body-write-only\" checks \"parents\" on writes (create/copy \
             must name one of these folders) and enforces read-by-id ancestry; writes that \
             omit \"parents\" (rename, metadata updates) are not confined by it.",
            style("i").cyan()
        );
    }

    if Confirm::new("  Block permanent deletion?")
        .with_default(true)
        .prompt()
        .map_err(err)?
    {
        entry["denied_methods"] = serde_json::json!(crate::policy::baseline_denials("drive"));
    }
    Ok(entry)
}

async fn gmail_setup(creds: Option<&str>) -> Result<serde_json::Value, GwsError> {
    eprintln!("  {} Fetching labels...", style("Gmail").cyan());
    let mut entry = serde_json::json!({ "name": "gmail", "allowed_resources": ["users.messages","users.threads","users.labels","users.drafts"], "denied_methods": crate::policy::baseline_denials("gmail") });

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Policy, PolicyFile, baseline_denials};

    fn policy_denied(json: &serde_json::Value, service: &str) -> Vec<String> {
        let file: PolicyFile = serde_json::from_value(json.clone()).unwrap();
        let policy = Policy::from_policy_file(file);
        policy
            .denied_methods(service)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn test_parse_subject_list() {
        assert_eq!(
            parse_subject_list("alice@example.com, bob@example.com"),
            vec!["alice@example.com", "bob@example.com"]
        );
        assert_eq!(
            parse_subject_list("  alice@example.com ,, ALICE@example.com , "),
            vec!["alice@example.com"]
        );
        assert!(parse_subject_list("").is_empty());
        assert!(parse_subject_list(" , , ").is_empty());
    }

    #[test]
    fn test_is_service_account_json() {
        assert!(is_service_account_json(
            r#"{"type":"service_account","client_email":"sa@project.iam.gserviceaccount.com"}"#
        ));
        assert!(!is_service_account_json(r#"{"type":"authorized_user"}"#));
        assert!(!is_service_account_json("{not json"));
    }

    #[test]
    fn test_generate_policy_uses_gmail_baseline_denials() {
        let json = generate_policy(&["gmail".to_string()]);
        let denied = policy_denied(&json, "gmail");
        for method in baseline_denials("gmail") {
            assert!(denied.contains(&method), "missing gmail denial {method}");
        }
    }

    #[test]
    fn test_generate_policy_uses_drive_baseline_denials() {
        let json = generate_policy(&["drive".to_string()]);
        let denied = policy_denied(&json, "drive");
        for method in baseline_denials("drive") {
            assert!(denied.contains(&method), "missing drive denial {method}");
        }
    }

    #[test]
    fn test_templates_use_baseline_denials() {
        for (name, _) in TEMPLATES {
            let json = template_policy(name).unwrap();
            let services = json["services"].as_array().unwrap();

            for entry in services {
                let svc = entry["name"].as_str().unwrap_or("");
                match svc {
                    "drive" => {
                        let denied: Vec<&str> = entry["denied_methods"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        for method in baseline_denials("drive") {
                            assert!(
                                denied.contains(&method.as_str()),
                                "template '{name}' drive entry is missing denial {method}"
                            );
                        }
                    }
                    "gmail" if entry.get("denied_methods").is_some() => {
                        let denied: Vec<&str> = entry["denied_methods"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        for method in baseline_denials("gmail") {
                            assert!(
                                denied.contains(&method.as_str()),
                                "template '{name}' gmail entry is missing denial {method}"
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn test_content_writer_adds_send_to_baseline() {
        let json = template_policy("content-writer").unwrap();
        let denied = policy_denied(&json, "gmail");
        for method in baseline_denials("gmail") {
            assert!(denied.contains(&method), "missing gmail denial {method}");
        }
        assert!(denied.contains(&"messages.send".to_string()));
    }

    #[test]
    fn test_gitignore_covers_str() {
        let contents = "/target\n.gws-policy.json\ngws-policy.json\n";
        assert!(gitignore_covers_str(contents, ".gws-policy.json"));
        assert!(gitignore_covers_str(contents, "gws-policy.json"));
        assert!(!gitignore_covers_str(contents, ".other-policy.json"));
        assert!(!gitignore_covers_str("", ".gws-policy.json"));
    }

    /// One service entry rendered the way the docs table lists it: access level
    /// plus the single qualifier the template actually implements.
    fn doc_service_label(entry: &serde_json::Value, global_read_only: bool) -> String {
        let name = entry["name"].as_str().unwrap_or_default();
        let label = match name {
            "drive" => "Drive",
            "gmail" => "Gmail",
            "calendar" => "Calendar",
            "sheets" => "Sheets",
            "docs" => "Docs",
            "slides" => "Slides",
            "tasks" => "Tasks",
            "people" => "People",
            other => other,
        };
        let read_only = global_read_only || entry["read_only"].as_bool().unwrap_or(false);
        let access = if read_only { "RO" } else { "RW" };
        let denied: Vec<&str> = entry["denied_methods"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let mut qualifiers: Vec<&str> = Vec::new();
        if name == "gmail" {
            if denied.contains(&"messages.send") {
                qualifiers.push("no send");
            } else if denied.contains(&"messages.delete") {
                qualifiers.push("no delete/forwarding");
            }
        }
        let primary_calendar = entry["constraints"].as_array().is_some_and(|constraints| {
            constraints.iter().any(|c| {
                c["param"].as_str() == Some("calendarId")
                    && c["values"]
                        .as_array()
                        .is_some_and(|v| v.iter().any(|id| id.as_str() == Some("primary")))
            })
        });
        if name == "calendar" && primary_calendar {
            qualifiers.push("primary");
        }
        if qualifiers.is_empty() {
            format!("{label} ({access})")
        } else {
            format!("{label} ({access}, {})", qualifiers.join(", "))
        }
    }

    /// Parse the template table out of the rendered policy reference so the
    /// docs cannot drift from the templates again.
    fn doc_template_rows() -> std::collections::HashMap<String, (String, String)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/site/content/docs/configuration/policy-reference.md"
        );
        let text = std::fs::read_to_string(path)
            .expect("policy-reference.md should exist in the repository");
        let mut rows = std::collections::HashMap::new();
        let mut in_table = false;
        for line in text.lines() {
            let line = line.trim();
            if line == "| Template | Services | Description |" {
                in_table = true;
                continue;
            }
            if !in_table {
                continue;
            }
            if !line.starts_with('|') {
                break;
            }
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            if cells.len() != 3 || cells[0] == "Template" || cells[0].starts_with('-') {
                continue;
            }
            rows.insert(
                cells[0].trim_matches('`').to_string(),
                (cells[1].to_string(), cells[2].to_string()),
            );
        }
        rows
    }

    #[test]
    fn test_template_docs_table_matches_emitted_policy() {
        let rows = doc_template_rows();
        for (name, _) in TEMPLATES {
            assert!(
                rows.contains_key(*name),
                "policy-reference.md has no template row for '{name}'"
            );
        }

        // Machine-checked rows: every service entry, its access level, and its
        // one-line qualifier must match the emitted JSON exactly.
        for name in ["analyst", "assistant", "content-writer"] {
            let json = template_policy(name).unwrap();
            let server_ro = json["server"]["read_only"].as_bool().unwrap_or(false);
            let derived = json["services"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| doc_service_label(entry, server_ro))
                .collect::<Vec<_>>()
                .join(", ");
            let (services_cell, _) = &rows[name];
            assert_eq!(
                *services_cell, derived,
                "docs row for template '{name}' drifted from the emitted JSON"
            );
        }

        // The remaining rows are prose ("All services"), so only pin the claims
        // those words make.
        let admin = template_policy("admin-readonly").unwrap();
        assert!(admin["server"]["read_only"].as_bool().unwrap_or(false));
        assert!(rows["admin-readonly"].0.contains("All services"));
        assert!(rows["it-admin"].0.contains("Calendar (RW)"));
    }

    #[test]
    fn test_assistant_template_docs_is_read_only() {
        let json = template_policy("assistant").unwrap();
        let docs = json["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "docs")
            .expect("assistant grants docs");
        assert!(
            docs["read_only"].as_bool().unwrap_or(false),
            "the docs table promises Docs (RO) for the assistant template"
        );
    }

    #[test]
    fn test_drive_folder_constraint_keeps_scoped_shape() {
        let constraint = drive_folder_constraint(&["folder-1", "folder-2"]);
        assert_eq!(
            constraint,
            serde_json::json!([{
                "param": "parents",
                "values": ["folder-1", "folder-2"],
                "access": "read-write",
                "location": "body-write-only",
                "mode": "restrict",
                "recursive": true
            }])
        );
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("gws-init-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn test_target_state_missing_and_regular() {
        let dir = unique_temp_dir("state");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("policy.json");
        let file_s = file.display().to_string();

        assert_eq!(target_state(&file_s).unwrap(), TargetState::Missing);
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(target_state(&file_s).unwrap(), TargetState::Regular);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn test_write_policy_file_refuses_symlink() {
        use std::os::unix::fs::symlink;

        let dir = unique_temp_dir("symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.json");
        std::fs::write(&real, "{}").unwrap();
        let link = dir.join("link.json");
        symlink(&real, &link).unwrap();

        let link_s = link.display().to_string();
        assert_eq!(target_state(&link_s).unwrap(), TargetState::Symlink);
        let err = write_policy_file(&link_s, "{\"server\":{}}").unwrap_err();
        assert!(format!("{err}").contains("symlink"), "{err}");
        // The symlink target must be untouched.
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_generate_policy_mentions_confirm_writes_without_enabling_it() {
        let json = generate_policy(&["drive".to_string()]);
        let comment = json["server"]["_comment"].as_str().unwrap();
        assert!(comment.contains("confirm_writes"), "{comment}");
        let file: PolicyFile = serde_json::from_value(json).unwrap();
        let policy = Policy::from_policy_file(file);
        assert!(!policy.confirm_writes);
    }

    #[test]
    fn test_server_comment_keeps_confirm_writes_default_false() {
        let json = serde_json::json!({
            "server": {
                "_comment": "Set \"confirm_writes\": true to require approval",
                "read_only": false
            },
            "services": [{ "name": "drive" }]
        });
        let file: PolicyFile = serde_json::from_value(json).unwrap();
        let policy = Policy::from_policy_file(file);
        assert!(!policy.confirm_writes);
    }
}
