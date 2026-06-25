mod audit;
mod auth;
mod completions;
mod elicitation;
mod execute;
mod handler;
mod helpers;
mod http;
mod image_gen;
mod marp;
mod meta;
mod metrics;
mod policy;
mod prompts;
mod resources;
mod server;
mod subscriptions;
mod slides_helpers;
mod tasks;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use google_workspace::error::GwsError;
use rmcp::ServiceExt;
use opentelemetry::trace::TracerProvider;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

enum Transport {
    Stdio,
    Http(String),
}

#[derive(Debug)]
enum Command {
    Serve(ParsedArgs),
    InitPolicy {
        services: Option<Vec<String>>,
        template: Option<String>,
    },
    CheckPolicy {
        path: PathBuf,
        verify: bool,
    },
    CheckAuth {
        policy_path: Option<PathBuf>,
    },
    Simulate {
        policy_path: PathBuf,
        scenarios_path: PathBuf,
    },
    ShowHelp,
}

#[derive(Debug)]
struct ParsedArgs {
    policy_path: Option<PathBuf>,
    services_str: Option<String>,
    http_addr: Option<String>,
    external_url: Option<String>,
    compact_schemas: bool,
    audit_log: Option<PathBuf>,
    prompts_dir: Option<PathBuf>,
}

fn print_usage() {
    eprintln!("mcp-google-workspace — MCP server for Google Workspace APIs");
    eprintln!();
    eprintln!("Usage:");
    eprintln!(
        "  mcp-google-workspace --policy <path>        Load services and constraints from a JSON policy file"
    );
    eprintln!(
        "  mcp-google-workspace --services drive,gmail  Expose specific services (no constraints)"
    );
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --policy <path>       Path to a gws-policy.json file");
    eprintln!("  --services <list>     Comma-separated service names (e.g., drive,gmail,calendar)");
    eprintln!("  --http <addr:port>    Run as HTTP server (e.g., 127.0.0.1:3000)");
    eprintln!(
        "  --init-policy         Interactive policy wizard, or use with --services for quick generation"
    );
    eprintln!("  --check-policy <path> Validate a policy file without starting the server");
    eprintln!(
        "  --template <name>     With --init-policy: use a preset (analyst, assistant, admin-readonly)"
    );
    eprintln!(
        "  --verify              With --check-policy: test credentials and resolve folder paths"
    );
    eprintln!("  --prompts-dir <path>  Directory containing prompt .md files");
    eprintln!("  --audit-log <path>    Write structured audit log (JSONL) of all API calls");
    eprintln!("  --check-auth          Walk the credential chain and report what is available");
    eprintln!("  --simulate <path>     Dry-run scenarios against a policy (requires --policy)");
    eprintln!("  --help                Show this help message");
}

fn parse_args_from(args: &[String]) -> Result<Command, GwsError> {
    let mut policy_path: Option<PathBuf> = None;
    let mut services_str: Option<String> = None;
    let mut http_addr: Option<String> = None;
    let mut external_url: Option<String> = None;
    let mut compact_schemas = false;
    let mut init_policy = false;
    let mut check_policy_path: Option<PathBuf> = None;
    let mut verify = false;
    let mut template: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;

    let mut prompts_dir: Option<PathBuf> = None;
    let mut check_auth = false;
    let mut simulate_path: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                return Ok(Command::ShowHelp);
            }
            "--check-auth" => {
                check_auth = true;
            }
            "--policy" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation("--policy requires a path".to_string()));
                }
                policy_path = Some(PathBuf::from(&args[i]));
            }
            "--services" | "-s" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--services requires a comma-separated list".to_string(),
                    ));
                }
                services_str = Some(args[i].clone());
            }
            "--http" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--http requires an address (e.g., 127.0.0.1:3000)".to_string(),
                    ));
                }
                http_addr = Some(args[i].clone());
            }
            "--external-url" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--external-url requires a URL (e.g., https://mcp.example.com)".to_string(),
                    ));
                }
                external_url = Some(args[i].clone());
            }
            "--compact-schemas" => {
                compact_schemas = true;
            }
            "--init-policy" => {
                init_policy = true;
            }
            "--verify" => {
                verify = true;
            }
            "--template" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--template requires a name (analyst, assistant, admin-readonly)"
                            .to_string(),
                    ));
                }
                template = Some(args[i].clone());
            }
            "--audit-log" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--audit-log requires a file path".to_string(),
                    ));
                }
                audit_log = Some(PathBuf::from(&args[i]));
            }
            "--check-policy" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--check-policy requires a path".to_string(),
                    ));
                }
                check_policy_path = Some(PathBuf::from(&args[i]));
            }
            "--prompts-dir" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--prompts-dir requires a path".to_string(),
                    ));
                }
                prompts_dir = Some(PathBuf::from(&args[i]));
            }
            "--simulate" => {
                i += 1;
                if i >= args.len() {
                    return Err(GwsError::Validation(
                        "--simulate requires a path to a scenarios JSON file".to_string(),
                    ));
                }
                simulate_path = Some(PathBuf::from(&args[i]));
            }
            other => {
                return Err(GwsError::Validation(format!("Unknown argument: {other}")));
            }
        }
        i += 1;
    }

    if let Some(path) = check_policy_path {
        return Ok(Command::CheckPolicy { path, verify });
    }

    if check_auth {
        return Ok(Command::CheckAuth { policy_path });
    }

    if let Some(scenarios_path) = simulate_path {
        let policy_path = policy_path.ok_or_else(|| {
            GwsError::Validation("--simulate requires --policy to also be set".to_string())
        })?;
        return Ok(Command::Simulate {
            policy_path,
            scenarios_path,
        });
    }

    if init_policy {
        let services = services_str.map(|s| s.split(',').map(|s| s.trim().to_string()).collect());
        return Ok(Command::InitPolicy { services, template });
    }

    Ok(Command::Serve(ParsedArgs {
        policy_path,
        services_str,
        http_addr,
        external_url,
        compact_schemas,
        audit_log,
        prompts_dir,
    }))
}

fn resolve_config(parsed: ParsedArgs) -> Result<(policy::Policy, Transport), GwsError> {
    let policy = if let Some(path) = parsed.policy_path {
        policy::Policy::from_file(&path)?
    } else if let Some(svc) = parsed.services_str {
        let names: Vec<String> = svc.split(',').map(|s| s.trim().to_string()).collect();
        policy::Policy::from_services(&names)
    } else {
        return Err(GwsError::Validation(
            "Either --policy <path> or --services <list> is required".to_string(),
        ));
    };

    let transport = match parsed.http_addr {
        Some(addr) => Transport::Http(addr),
        None => Transport::Stdio,
    };

    Ok((policy, transport))
}

const KNOWN_SERVICES: &[(&str, &str)] = &[
    ("drive", "Google Drive — files, folders, permissions"),
    ("gmail", "Gmail — messages, threads, labels, drafts"),
    ("calendar", "Google Calendar — events, calendars"),
    ("sheets", "Google Sheets — spreadsheets, values"),
    ("docs", "Google Docs — documents, content"),
    ("slides", "Google Slides — presentations, pages"),
    ("admin", "Admin SDK — users, groups, org units"),
    ("chat", "Google Chat — spaces, messages"),
    ("generativelanguage", "Google Generative AI — models, content generation"),
];

fn generate_policy(services: &[String]) -> serde_json::Value {
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

const TEMPLATES: &[(&str, &str)] = &[
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
    eprintln!("Usage: mcp-google-workspace --init-policy --template <name>");
}

fn template_policy(name: &str) -> Result<serde_json::Value, GwsError> {
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

fn init_policy_interactive() -> Result<serde_json::Value, GwsError> {
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
    use dialoguer::Confirm;

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

    if denied.is_empty() {
        Ok(serde_json::json!({ "name": "gmail" }))
    } else {
        Ok(serde_json::json!({ "name": "gmail", "denied_methods": denied }))
    }
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

fn check_policy(path: &Path) -> Result<(), GwsError> {
    let p = policy::Policy::from_file(path)?;

    let services = p.allowed_services();
    eprintln!("Policy OK: {} service(s) configured", services.len());
    for svc in &services {
        let mut flags = Vec::new();
        if p.is_read_only(svc) {
            flags.push("read-only".to_string());
        }
        let denied = p.denied_methods(svc);
        if !denied.is_empty() {
            flags.push(format!("{} denied method(s)", denied.len()));
        }
        let constraints = p.constraints(svc);
        if !constraints.is_empty() {
            flags.push(format!("{} constraint(s)", constraints.len()));
        }
        if flags.is_empty() {
            eprintln!("  {svc}: no constraints");
        } else {
            eprintln!("  {svc}: {}", flags.join(", "));
        }
    }

    if let Some(rpm) = p.rate_limit_rpm {
        eprintln!("  rate limit: {rpm} req/min");
    }
    eprintln!("  max request size: {} bytes", p.max_request_bytes);

    let warnings = check_policy_warnings(&p);
    if !warnings.is_empty() {
        eprintln!();
        eprintln!("Warnings:");
        for w in &warnings {
            eprintln!("  ! {w}");
        }
    }

    Ok(())
}

fn check_policy_warnings(p: &policy::Policy) -> Vec<String> {
    let mut warnings = Vec::new();

    for svc in p.allowed_services() {
        match svc {
            "gmail" => {
                let denied = p.denied_methods(svc);
                if !denied.contains("settings.updateAutoForwarding") {
                    warnings.push(
                        "gmail: settings.updateAutoForwarding is not denied — \
                         an agent could silently forward all mail to an external address"
                            .to_string(),
                    );
                }
                if !denied.contains("settings.delegates.create") {
                    warnings.push(
                        "gmail: settings.delegates.create is not denied — \
                         an agent could grant another account full access to the mailbox"
                            .to_string(),
                    );
                }
                if !denied.contains("settings.forwardingAddresses.create") {
                    warnings.push(
                        "gmail: settings.forwardingAddresses.create is not denied — \
                         an agent could add forwarding addresses"
                            .to_string(),
                    );
                }
            }
            "drive" => {
                if p.constraints(svc).is_empty() && !p.is_read_only(svc) {
                    warnings.push(
                        "drive: no constraints and not read-only — \
                         agent has full access to all Drive files"
                            .to_string(),
                    );
                }
            }
            "admin" if !p.is_read_only(svc) => {
                warnings.push(
                    "admin: not read-only — \
                     agent can modify users, groups, and org units"
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    warnings
}

async fn verify_policy(p: &mut policy::Policy) -> Result<(), GwsError> {
    eprintln!();
    eprintln!("Verifying against Google APIs...");

    let scopes = &["https://www.googleapis.com/auth/drive.metadata.readonly"];
    match auth::get_token(scopes, p.credentials_file.as_deref(), None).await {
        Ok(_) => eprintln!("  credentials: OK"),
        Err(e) => {
            return Err(GwsError::Auth(format!(
                "Cannot obtain OAuth token: {e}. \
                 Check your credentials setup (see --help or docs)"
            )));
        }
    }

    for svc in p.allowed_services() {
        let constraints = p.constraints(svc);
        if !constraints.is_empty() {
            eprintln!("  {svc}: {} constraint(s) configured", constraints.len());
        }
    }

    if let Some(ref project_id) = p.project_id {
        eprintln!("  project_id: {project_id}");
    }

    eprintln!();
    eprintln!("Verification complete");
    Ok(())
}

fn print_effective_policy(policy: &policy::Policy) {
    let services = policy.allowed_services();
    tracing::info!(
        count = services.len(),
        "Policy loaded: {} service(s)",
        services.len()
    );

    for svc in &services {
        let access = if policy.is_read_only(svc) {
            "read-only"
        } else {
            "read-write"
        };

        let constraints = policy.constraints(svc);
        let constraint_detail = if constraints.is_empty() {
            "0 constraints".to_string()
        } else {
            let params: Vec<&str> = constraints
                .iter()
                .map(|c| c.param.as_str())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            format!(
                "{} constraint(s) ({})",
                constraints.len(),
                params.join(", ")
            )
        };

        let denied = policy.denied_methods(svc);
        let denied_detail = if denied.is_empty() {
            "0 denied methods".to_string()
        } else if denied.len() <= 5 {
            let mut methods: Vec<&str> = denied.into_iter().collect();
            methods.sort();
            format!(
                "{} denied method(s) [{}]",
                methods.len(),
                methods.join(", ")
            )
        } else {
            format!("{} denied method(s)", denied.len())
        };

        tracing::info!(
            service = svc,
            access,
            "  {svc}: {access}, {constraint_detail}, {denied_detail}"
        );
    }

    let read_only_label = if policy.global_read_only { "yes" } else { "no" };
    let rate_limit_label = match policy.rate_limit_rpm {
        Some(rpm) => format!("{rpm} rpm"),
        None => "none".to_string(),
    };
    let max_bytes = policy.max_request_bytes;
    let max_request_label = if max_bytes >= 1024 * 1024 {
        format!("{} MB", max_bytes / (1024 * 1024))
    } else if max_bytes >= 1024 {
        format!("{} KB", max_bytes / 1024)
    } else {
        format!("{max_bytes} B")
    };

    tracing::info!(
        global_read_only = policy.global_read_only,
        rate_limit_rpm = ?policy.rate_limit_rpm,
        max_request_bytes = policy.max_request_bytes,
        "  Global: read-only {read_only_label}, rate limit {rate_limit_label}, max request {max_request_label}"
    );
}

fn init_telemetry() {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(tracing::Level::INFO.into());

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .with_filter(env_filter);

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build()
            .expect("Failed to create OTLP exporter");

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name("mcp-google-workspace")
                    .build(),
            )
            .build();

        let tracer = provider.tracer("mcp-google-workspace");
        opentelemetry::global::set_tracer_provider(provider);
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(otel_layer)
            .init();

        eprintln!("[mcp-gws] OTel tracing enabled");
    } else {
        tracing_subscriber::registry().with(fmt_layer).init();
    }
}

#[derive(Debug, serde::Deserialize)]
struct Scenario {
    service: String,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    body: Option<serde_json::Value>,
}

fn simulate_policy(policy_path: &Path, scenarios_path: &Path) -> Result<(), GwsError> {
    let p = policy::Policy::from_file(policy_path)?;

    let content = std::fs::read_to_string(scenarios_path)
        .map_err(|e| GwsError::Validation(format!("Failed to read scenarios file: {e}")))?;
    let scenarios: Vec<Scenario> = serde_json::from_str(&content)
        .map_err(|e| GwsError::Validation(format!("Invalid scenarios JSON: {e}")))?;

    if scenarios.is_empty() {
        eprintln!("No scenarios to simulate.");
        return Ok(());
    }

    for (i, scenario) in scenarios.iter().enumerate() {
        let resource = scenario.resource.as_deref().unwrap_or("*");
        let method = scenario.method.as_deref().unwrap_or("*");
        let label = format!("{}.{}.{}", scenario.service, resource, method);
        eprintln!("Scenario {}: {label}", i + 1);

        let mut verdict = "ALLOWED";

        let service_ok = p.is_service_allowed(&scenario.service);
        if service_ok {
            eprintln!("  Service: \u{2713} allowed");
        } else {
            eprintln!("  Service: \u{2717} not in policy");
            eprintln!("  Verdict: DENIED (service not allowed)");
            eprintln!();
            continue;
        }

        let denied = p.denied_methods(&scenario.service);
        let full_name = format!("{resource}.{method}");
        let method_denied = denied.contains(method) || denied.contains(full_name.as_str());
        if method_denied {
            eprintln!("  Method:  \u{2717} denied by denylist");
            verdict = "DENIED (method in denylist)";
        } else {
            eprintln!("  Method:  \u{2713} not denied");
        }

        let read_only = p.is_read_only(&scenario.service);
        let is_write = scenario.body.is_some()
            || method.starts_with("create")
            || method.starts_with("insert")
            || method.starts_with("update")
            || method.starts_with("patch")
            || method.starts_with("delete")
            || method.starts_with("send")
            || method.starts_with("trash");
        if read_only && is_write {
            eprintln!("  Access:  \u{2717} write blocked (read-only)");
            if verdict == "ALLOWED" {
                verdict = "DENIED (read-only)";
            }
        } else if read_only {
            eprintln!("  Access:  \u{2713} read operation (read-only OK)");
        } else {
            eprintln!("  Access:  \u{2713} read-write permitted");
        }

        let constraints = p.constraints(&scenario.service);
        if !constraints.is_empty() {
            let params = scenario.params.as_ref();
            let mut constraint_ok = true;

            for c in constraints {
                let is_body = c.location.as_deref() == Some("body");
                if is_body {
                    if is_write {
                        let body_values: Vec<&str> = scenario
                            .body
                            .as_ref()
                            .and_then(|b| b.get(&c.param))
                            .map(|v| match v {
                                serde_json::Value::Array(arr) => {
                                    arr.iter().filter_map(|v| v.as_str()).collect()
                                }
                                serde_json::Value::String(s) => vec![s.as_str()],
                                _ => vec![],
                            })
                            .unwrap_or_default();

                        if body_values.is_empty() {
                            eprintln!(
                                "  Params:  \u{2717} '{}' required in body for writes",
                                c.param
                            );
                            constraint_ok = false;
                        } else {
                            for val in &body_values {
                                if !c.values.iter().any(|v| v == val) {
                                    eprintln!(
                                        "  Params:  \u{2717} '{}' value '{}' not in allowed list",
                                        c.param, val
                                    );
                                    constraint_ok = false;
                                } else if is_write && c.access == policy::Access::ReadOnly {
                                    eprintln!(
                                        "  Params:  \u{2717} '{}' value '{}' is read-only",
                                        c.param, val
                                    );
                                    constraint_ok = false;
                                }
                            }
                        }
                    }
                } else if let Some(params) = params {
                    if let Some(serde_json::Value::String(value)) = params.get(&c.param) {
                        if !c.values.iter().any(|v| v == value) {
                            eprintln!(
                                "  Params:  \u{2717} '{}' value '{}' not allowed",
                                c.param, value
                            );
                            constraint_ok = false;
                        } else if is_write && c.access == policy::Access::ReadOnly {
                            eprintln!(
                                "  Params:  \u{2717} '{}' value '{}' is read-only",
                                c.param, value
                            );
                            constraint_ok = false;
                        }
                    } else {
                        eprintln!("  Params:  \u{2717} '{}' not specified", c.param);
                        constraint_ok = false;
                    }
                } else {
                    eprintln!("  Params:  \u{2717} '{}' not specified", c.param);
                    constraint_ok = false;
                }
            }

            if constraint_ok {
                eprintln!("  Params:  \u{2713} constraints satisfied");
            } else if verdict == "ALLOWED" {
                verdict = "DENIED (constraint violation)";
            }
        }

        eprintln!("  Verdict: {verdict}");
        eprintln!();
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args_from(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            print_usage();
            std::process::exit(1);
        }
    };

    let parsed = match cmd {
        Command::InitPolicy { services, template } => {
            let interactive = services.is_none() && template.is_none();
            let json = if let Some(ref tmpl) = template {
                if let Some((_, desc)) = TEMPLATES.iter().find(|(n, _)| *n == tmpl.as_str()) {
                    eprintln!("Using template '{tmpl}': {desc}");
                }
                match template_policy(tmpl) {
                    Ok(j) => j,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else if let Some(svc) = services {
                generate_policy(&svc)
            } else {
                match init_policy_interactive() {
                    Ok(j) => j,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            };
            let output = serde_json::to_string_pretty(&json).unwrap();

            if interactive {
                use dialoguer::Input;
                eprintln!();
                let path: String = Input::new()
                    .with_prompt("Save to")
                    .default("policy.json".to_string())
                    .interact_text()
                    .unwrap_or_else(|_| "policy.json".to_string());

                if let Err(e) = std::fs::write(&path, &output) {
                    eprintln!("Error writing {path}: {e}");
                    std::process::exit(1);
                }
                eprintln!();
                eprintln!("Saved to {path}");
                eprintln!();
                eprintln!("Start the server with:");
                eprintln!("  mcp-google-workspace --policy {path}");
                eprintln!();
                eprintln!("Validate with:");
                eprintln!("  mcp-google-workspace --check-policy {path}");
            } else {
                println!("{output}");
            }
            std::process::exit(0);
        }
        Command::CheckPolicy { path, verify } => {
            match check_policy(&path) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Policy error: {e}");
                    std::process::exit(1);
                }
            }
            if verify {
                let mut p = match policy::Policy::from_file(&path) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Policy error: {e}");
                        std::process::exit(1);
                    }
                };
                match verify_policy(&mut p).await {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("Verification failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            std::process::exit(0);
        }
        Command::Simulate {
            policy_path,
            scenarios_path,
        } => {
            match simulate_policy(&policy_path, &scenarios_path) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Simulation error: {e}");
                    std::process::exit(1);
                }
            }
            std::process::exit(0);
        }
        Command::CheckAuth { policy_path } => {
            let creds_path = policy_path.as_ref().and_then(|path| {
                let content = std::fs::read_to_string(path).ok()?;
                let json: serde_json::Value = serde_json::from_str(&content).ok()?;
                json.get("server")
                    .and_then(|s| s.get("credentials_file"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });

            let results = auth::diagnose_chain(creds_path.as_deref()).await;

            eprintln!("Credential chain diagnostics:");
            let mut active_source: Option<usize> = None;
            for (i, r) in results.iter().enumerate() {
                let mark = if r.found && r.parseable {
                    "\u{2713}"
                } else {
                    "\u{2717}"
                };
                eprintln!("  {}. [{}] {}: {}", i + 1, mark, r.source, r.detail);
                if active_source.is_none() && r.found && r.parseable {
                    active_source = Some(i);
                }
            }

            eprintln!();
            match active_source {
                Some(idx) => {
                    eprintln!(
                        "Active credential source: #{} ({})",
                        idx + 1,
                        results[idx].source
                    );
                }
                None => {
                    eprintln!("No usable credentials found.");
                    eprintln!("Run --help for credential setup options.");
                }
            }

            std::process::exit(0);
        }
        Command::ShowHelp => {
            print_usage();
            std::process::exit(0);
        }
        Command::Serve(p) => p,
    };

    init_telemetry();

    let audit_log_path = parsed.audit_log.clone();
    let policy_file_path = parsed.policy_path.clone();
    let prompts_dir_flag = parsed.prompts_dir.clone();
    let external_url = parsed.external_url.clone();
    let compact_schemas = parsed.compact_schemas;

    let (mut policy, transport) = match resolve_config(parsed) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            print_usage();
            std::process::exit(1);
        }
    };
    policy.compact_schemas = compact_schemas;

    print_effective_policy(&policy);

    let prompts_dir = prompts_dir_flag
        .or_else(|| {
            policy_file_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|d| d.join("prompts"))
        })
        .filter(|d| d.is_dir());

    let prompts = prompts::load_prompts(prompts_dir.as_deref());
    if !prompts.is_empty() {
        tracing::info!(count = prompts.len(), "Loaded MCP prompts");
    }

    let audit = audit_log_path.map(|path| {
        let logger = audit::AuditLogger::new(path.clone()).unwrap_or_else(|e| {
            eprintln!("Error opening audit log {}: {e}", path.display());
            std::process::exit(1);
        });
        eprintln!("[mcp-gws] Audit log: {}", logger.path().display());
        Arc::new(logger)
    });

    match transport {
        Transport::Stdio => {
            let svc_list = policy.allowed_services();
            if svc_list.is_empty() {
                tracing::warn!("No services configured. Zero tools will be exposed.");
            } else {
                tracing::info!(services = %svc_list.join(", "), "Starting MCP server");
            }

            let handler = handler::GwsHandler::new(policy, prompts, audit);
            let service = handler
                .serve(rmcp::transport::io::stdio())
                .await
                .map_err(|e| {
                    eprintln!("Fatal: failed to start MCP server: {e}");
                    std::process::exit(1);
                })
                .unwrap();

            if let Err(e) = service.waiting().await {
                eprintln!("Fatal: {e}");
                std::process::exit(1);
            }
        }
        Transport::Http(addr) => {
            let svc_list = policy.allowed_services();
            if svc_list.is_empty() {
                tracing::warn!("No services configured. Zero tools will be exposed.");
            } else {
                tracing::info!(services = %svc_list.join(", "), "Starting MCP HTTP server");
            }

            let mut state = server::ServerState::new();
            state.prompts = prompts;
            state.audit = audit;
            state.webhook_url = external_url.clone()
                .or_else(|| Some(format!("http://{addr}")));
            let state = Arc::new(tokio::sync::Mutex::new(state));
            let policy = Arc::new(tokio::sync::RwLock::new(policy));

            let result = http::serve(policy, policy_file_path, state, &addr).await;
            if let Err(e) = result {
                eprintln!("Fatal: {e}");
                std::process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    fn unwrap_serve(cmd: Command) -> ParsedArgs {
        match cmd {
            Command::Serve(p) => p,
            other => panic!("Expected Command::Serve, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_services_flag() {
        let parsed = unwrap_serve(parse_args_from(&args(&["--services", "drive,gmail"])).unwrap());
        assert_eq!(parsed.services_str.as_deref(), Some("drive,gmail"));
        assert!(parsed.policy_path.is_none());
        assert!(parsed.http_addr.is_none());
    }

    #[test]
    fn test_parse_services_short_flag() {
        let parsed = unwrap_serve(parse_args_from(&args(&["-s", "calendar"])).unwrap());
        assert_eq!(parsed.services_str.as_deref(), Some("calendar"));
    }

    #[test]
    fn test_parse_policy_flag() {
        let parsed =
            unwrap_serve(parse_args_from(&args(&["--policy", "/tmp/gws-policy.json"])).unwrap());
        assert_eq!(
            parsed.policy_path,
            Some(PathBuf::from("/tmp/gws-policy.json"))
        );
    }

    #[test]
    fn test_parse_http_flag() {
        let parsed = unwrap_serve(
            parse_args_from(&args(&["--services", "drive", "--http", "127.0.0.1:3000"])).unwrap(),
        );
        assert_eq!(parsed.http_addr.as_deref(), Some("127.0.0.1:3000"));
        assert_eq!(parsed.services_str.as_deref(), Some("drive"));
    }

    #[test]
    fn test_parse_no_args() {
        let parsed = unwrap_serve(parse_args_from(&args(&[])).unwrap());
        assert!(parsed.policy_path.is_none());
        assert!(parsed.services_str.is_none());
        assert!(parsed.http_addr.is_none());
    }

    #[test]
    fn test_parse_unknown_flag() {
        let err = parse_args_from(&args(&["--bogus"]));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("Unknown argument"));
    }

    #[test]
    fn test_parse_policy_missing_value() {
        let err = parse_args_from(&args(&["--policy"]));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("requires a path"));
    }

    #[test]
    fn test_parse_services_missing_value() {
        let err = parse_args_from(&args(&["--services"]));
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_http_missing_value() {
        let err = parse_args_from(&args(&["--http"]));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("requires an address"));
    }

    #[test]
    fn test_parse_help_flag() {
        let cmd = parse_args_from(&args(&["--help"])).unwrap();
        assert!(matches!(cmd, Command::ShowHelp));
    }

    #[test]
    fn test_resolve_services_creates_policy() {
        let parsed = ParsedArgs {
            policy_path: None,
            services_str: Some("drive,gmail".to_string()),
            http_addr: None,
            external_url: None,
            compact_schemas: false,
            audit_log: None,
            prompts_dir: None,
        };
        let (policy, _) = resolve_config(parsed).unwrap();
        assert!(policy.is_service_allowed("drive"));
        assert!(policy.is_service_allowed("gmail"));
        assert!(!policy.is_service_allowed("sheets"));
    }

    #[test]
    fn test_resolve_no_source_errors() {
        let parsed = ParsedArgs {
            policy_path: None,
            services_str: None,
            http_addr: None,
            external_url: None,
            compact_schemas: false,
            audit_log: None,
            prompts_dir: None,
        };
        assert!(resolve_config(parsed).is_err());
    }

    #[test]
    fn test_resolve_http_transport() {
        let parsed = ParsedArgs {
            policy_path: None,
            services_str: Some("drive".to_string()),
            http_addr: Some("0.0.0.0:8080".to_string()),
            external_url: None,
            compact_schemas: false,
            audit_log: None,
            prompts_dir: None,
        };
        let (_, transport) = resolve_config(parsed).unwrap();
        assert!(matches!(transport, Transport::Http(addr) if addr == "0.0.0.0:8080"));
    }

    #[test]
    fn test_resolve_stdio_transport_default() {
        let parsed = ParsedArgs {
            policy_path: None,
            services_str: Some("drive".to_string()),
            http_addr: None,
            external_url: None,
            compact_schemas: false,
            audit_log: None,
            prompts_dir: None,
        };
        let (_, transport) = resolve_config(parsed).unwrap();
        assert!(matches!(transport, Transport::Stdio));
    }

    #[test]
    fn test_resolve_policy_file_not_found() {
        let parsed = ParsedArgs {
            policy_path: Some(PathBuf::from("/nonexistent/path/policy.json")),
            services_str: None,
            http_addr: None,
            external_url: None,
            compact_schemas: false,
            audit_log: None,
            prompts_dir: None,
        };
        assert!(resolve_config(parsed).is_err());
    }

    #[test]
    fn test_parse_init_policy_interactive() {
        let cmd = parse_args_from(&args(&["--init-policy"])).unwrap();
        match cmd {
            Command::InitPolicy { services, template } => {
                assert!(services.is_none());
                assert!(template.is_none());
            }
            other => panic!("Expected InitPolicy, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_init_policy_with_services() {
        let cmd = parse_args_from(&args(&["--init-policy", "--services", "drive,sheets"])).unwrap();
        match cmd {
            Command::InitPolicy { services, template } => {
                assert_eq!(
                    services,
                    Some(vec!["drive".to_string(), "sheets".to_string()])
                );
                assert!(template.is_none());
            }
            other => panic!("Expected InitPolicy, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_init_policy_with_template() {
        let cmd = parse_args_from(&args(&["--init-policy", "--template", "analyst"])).unwrap();
        match cmd {
            Command::InitPolicy { services, template } => {
                assert!(services.is_none());
                assert_eq!(template, Some("analyst".to_string()));
            }
            other => panic!("Expected InitPolicy, got {other:?}"),
        }
    }

    #[test]
    fn test_template_analyst() {
        let json = template_policy("analyst").unwrap();
        let services = json["services"].as_array().unwrap();
        assert!(
            services
                .iter()
                .any(|s| s["name"] == "drive" && s["read_only"] == true)
        );
        assert!(services.iter().any(|s| s["name"] == "gmail"));
    }

    #[test]
    fn test_template_assistant() {
        let json = template_policy("assistant").unwrap();
        let services = json["services"].as_array().unwrap();
        assert!(services.iter().any(|s| s["name"] == "drive"));
        assert!(services.iter().any(|s| s["name"] == "calendar"));
    }

    #[test]
    fn test_template_admin_readonly() {
        let json = template_policy("admin-readonly").unwrap();
        assert_eq!(json["server"]["read_only"], true);
        let services = json["services"].as_array().unwrap();
        assert!(services.len() >= 8);
    }

    #[test]
    fn test_template_unknown() {
        assert!(template_policy("nonexistent").is_err());
    }

    #[test]
    fn test_parse_check_policy() {
        let cmd = parse_args_from(&args(&["--check-policy", "/tmp/policy.json"])).unwrap();
        match cmd {
            Command::CheckPolicy { path, verify } => {
                assert_eq!(path, PathBuf::from("/tmp/policy.json"));
                assert!(!verify);
            }
            other => panic!("Expected CheckPolicy, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_check_policy_with_verify() {
        let cmd =
            parse_args_from(&args(&["--check-policy", "/tmp/policy.json", "--verify"])).unwrap();
        match cmd {
            Command::CheckPolicy { path, verify } => {
                assert_eq!(path, PathBuf::from("/tmp/policy.json"));
                assert!(verify);
            }
            other => panic!("Expected CheckPolicy, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_check_policy_missing_value() {
        let err = parse_args_from(&args(&["--check-policy"]));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("requires a path"));
    }

    #[test]
    fn test_parse_check_auth() {
        let cmd = parse_args_from(&args(&["--check-auth"])).unwrap();
        match cmd {
            Command::CheckAuth { policy_path } => {
                assert!(policy_path.is_none());
            }
            other => panic!("Expected CheckAuth, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_check_auth_with_policy() {
        let cmd =
            parse_args_from(&args(&["--check-auth", "--policy", "/tmp/policy.json"])).unwrap();
        match cmd {
            Command::CheckAuth { policy_path } => {
                assert_eq!(policy_path, Some(PathBuf::from("/tmp/policy.json")));
            }
            other => panic!("Expected CheckAuth, got {other:?}"),
        }
    }

    #[test]
    fn test_generate_policy_drive() {
        let json = generate_policy(&["drive".to_string()]);
        let services = json["services"].as_array().unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0]["name"], "drive");
        assert!(services[0]["constraints"].is_array());
    }

    #[test]
    fn test_generate_policy_unknown_service() {
        let json = generate_policy(&["tasks".to_string()]);
        let services = json["services"].as_array().unwrap();
        assert_eq!(services[0]["name"], "tasks");
        assert_eq!(services[0]["read_only"], true);
    }

    #[test]
    fn test_check_policy_valid() {
        let dir = std::env::temp_dir().join("mcp-gws-test-check");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.json");
        std::fs::write(
            &path,
            r#"{"services": [{"name": "drive"}, {"name": "gmail"}]}"#,
        )
        .unwrap();
        assert!(check_policy(&path).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_check_policy_invalid_json() {
        let dir = std::env::temp_dir().join("mcp-gws-test-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(check_policy(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_parse_simulate_flag() {
        let cmd = parse_args_from(&args(&[
            "--policy",
            "/tmp/policy.json",
            "--simulate",
            "/tmp/scenarios.json",
        ]))
        .unwrap();
        match cmd {
            Command::Simulate {
                policy_path,
                scenarios_path,
            } => {
                assert_eq!(policy_path, PathBuf::from("/tmp/policy.json"));
                assert_eq!(scenarios_path, PathBuf::from("/tmp/scenarios.json"));
            }
            other => panic!("Expected Simulate, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_simulate_requires_policy() {
        let err = parse_args_from(&args(&["--simulate", "/tmp/scenarios.json"]));
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("--simulate requires --policy")
        );
    }

    #[test]
    fn test_parse_simulate_missing_value() {
        let err = parse_args_from(&args(&["--policy", "/tmp/policy.json", "--simulate"]));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("requires a path"));
    }

    #[test]
    fn test_simulate_allowed_scenario() {
        let dir = std::env::temp_dir().join("mcp-gws-test-sim-allow");
        std::fs::create_dir_all(&dir).unwrap();
        let policy_path = dir.join("policy.json");
        std::fs::write(
            &policy_path,
            r#"{"services": [{"name": "drive"}, {"name": "gmail"}]}"#,
        )
        .unwrap();
        let scenarios_path = dir.join("scenarios.json");
        std::fs::write(
            &scenarios_path,
            r#"[{"service": "drive", "resource": "files", "method": "list"}]"#,
        )
        .unwrap();
        assert!(simulate_policy(&policy_path, &scenarios_path).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_simulate_denied_service() {
        let dir = std::env::temp_dir().join("mcp-gws-test-sim-deny-svc");
        std::fs::create_dir_all(&dir).unwrap();
        let policy_path = dir.join("policy.json");
        std::fs::write(&policy_path, r#"{"services": [{"name": "drive"}]}"#).unwrap();
        let scenarios_path = dir.join("scenarios.json");
        std::fs::write(
            &scenarios_path,
            r#"[{"service": "gmail", "resource": "messages", "method": "list"}]"#,
        )
        .unwrap();
        assert!(simulate_policy(&policy_path, &scenarios_path).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_simulate_denied_method() {
        let dir = std::env::temp_dir().join("mcp-gws-test-sim-deny-method");
        std::fs::create_dir_all(&dir).unwrap();
        let policy_path = dir.join("policy.json");
        std::fs::write(
            &policy_path,
            r#"{"services": [{"name": "gmail", "denied_methods": ["messages.send"]}]}"#,
        )
        .unwrap();
        let scenarios_path = dir.join("scenarios.json");
        std::fs::write(
            &scenarios_path,
            r#"[{"service": "gmail", "resource": "messages", "method": "send"}]"#,
        )
        .unwrap();
        assert!(simulate_policy(&policy_path, &scenarios_path).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_parse_prompts_dir() {
        let parsed = unwrap_serve(
            parse_args_from(&args(&[
                "--services",
                "drive",
                "--prompts-dir",
                "/tmp/prompts",
            ]))
            .unwrap(),
        );
        assert_eq!(parsed.prompts_dir, Some(PathBuf::from("/tmp/prompts")));
    }

    #[test]
    fn test_parse_prompts_dir_missing_value() {
        let err = parse_args_from(&args(&["--prompts-dir"]));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("requires a path"));
    }
}
