mod auth;
mod execute;
mod http;
mod meta;
mod metrics;
mod policy;
mod protocol;
mod resolve;
mod server;
mod tasks;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use google_workspace::error::GwsError;
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
    InitPolicy { services: Option<Vec<String>> },
    CheckPolicy { path: PathBuf, verify: bool },
}

#[derive(Debug)]
struct ParsedArgs {
    policy_path: Option<PathBuf>,
    services_str: Option<String>,
    http_addr: Option<String>,
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
        "  --verify              With --check-policy: test credentials and resolve folder paths"
    );
    eprintln!("  --help                Show this help message");
}

fn parse_args_from(args: &[String]) -> Result<Command, GwsError> {
    let mut policy_path: Option<PathBuf> = None;
    let mut services_str: Option<String> = None;
    let mut http_addr: Option<String> = None;
    let mut init_policy = false;
    let mut check_policy_path: Option<PathBuf> = None;
    let mut verify = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                return Err(GwsError::Validation("help".to_string()));
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
            "--init-policy" => {
                init_policy = true;
            }
            "--verify" => {
                verify = true;
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
            other => {
                return Err(GwsError::Validation(format!("Unknown argument: {other}")));
            }
        }
        i += 1;
    }

    if let Some(path) = check_policy_path {
        return Ok(Command::CheckPolicy { path, verify });
    }

    if init_policy {
        let services = services_str.map(|s| s.split(',').map(|s| s.trim().to_string()).collect());
        return Ok(Command::InitPolicy { services });
    }

    Ok(Command::Serve(ParsedArgs {
        policy_path,
        services_str,
        http_addr,
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
            "folders": [
                { "path": "My Drive/Projects", "access": "read-write" }
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
            "calendars": [
                { "id": "primary", "access": "read-write" }
            ]
        }),
        _ => serde_json::json!({
            "name": name,
            "read_only": true
        }),
    }
}

fn init_policy_interactive() -> Result<serde_json::Value, GwsError> {
    use dialoguer::{Confirm, Input, MultiSelect};

    eprintln!();
    eprintln!("  MCP Google Workspace — Policy Generator");
    eprintln!();

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

    let mut folders: Vec<serde_json::Value> = Vec::new();
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
        folders.push(serde_json::json!({ "path": path, "access": access }));
    }

    if folders.is_empty() {
        Ok(serde_json::json!({ "name": "drive" }))
    } else {
        Ok(serde_json::json!({ "name": "drive", "folders": folders }))
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

    let mut calendars: Vec<serde_json::Value> = Vec::new();

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
        calendars.push(serde_json::json!({ "id": "primary", "access": access }));
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
        calendars.push(serde_json::json!({ "id": id, "access": access }));
    }

    if calendars.is_empty() {
        Ok(serde_json::json!({ "name": "calendar" }))
    } else {
        Ok(serde_json::json!({ "name": "calendar", "calendars": calendars }))
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
        if p.has_folder_restrictions(svc) {
            let count = p.all_folder_ids(svc).len();
            flags.push(format!("{count} folder ACL(s)"));
        }
        let cals = p.calendars(svc);
        if !cals.is_empty() {
            flags.push(format!("{} calendar ACL(s)", cals.len()));
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
                if !p.has_folder_restrictions(svc) && !p.is_read_only(svc) {
                    warnings.push(
                        "drive: no folder restrictions and not read-only — \
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

    match p.resolve_folder_paths().await {
        Ok(()) => {
            let services = p.allowed_services();
            for svc in &services {
                if p.has_folder_restrictions(svc) {
                    let ids = p.all_folder_ids(svc);
                    eprintln!("  {svc} folders: {} resolved", ids.len());
                }
            }
        }
        Err(e) => {
            return Err(GwsError::Validation(format!(
                "Drive folder resolution failed: {e}"
            )));
        }
    }

    for svc in p.allowed_services() {
        let cals = p.calendars(svc);
        if !cals.is_empty() {
            eprintln!(
                "  {svc} calendars: {} configured (IDs not verified — no list API call)",
                cals.len()
            );
        }
    }

    if let Some(ref project_id) = p.project_id {
        eprintln!("  project_id: {project_id}");
    }

    eprintln!();
    eprintln!("Verification complete");
    Ok(())
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

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match parse_args_from(&args) {
        Ok(c) => c,
        Err(e) if e.to_string() == "help" => {
            print_usage();
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            print_usage();
            std::process::exit(1);
        }
    };

    let parsed = match cmd {
        Command::InitPolicy { services } => {
            let interactive = services.is_none();
            let json = match services {
                Some(svc) => generate_policy(&svc),
                None => match init_policy_interactive() {
                    Ok(j) => j,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                },
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
        Command::Serve(p) => p,
    };

    init_telemetry();

    let (mut policy, transport) = match resolve_config(parsed) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            print_usage();
            std::process::exit(1);
        }
    };

    if let Err(e) = policy.resolve_folder_paths().await {
        eprintln!("Error resolving Drive folder paths: {e}");
        std::process::exit(1);
    }

    let result = match transport {
        Transport::Stdio => server::run_stdio(policy).await,
        Transport::Http(addr) => server::run_http(Arc::new(policy), &addr).await,
    };

    if let Err(e) = result {
        eprintln!("Fatal: {e}");
        std::process::exit(1);
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
        let err = parse_args_from(&args(&["--help"]));
        assert!(err.is_err());
    }

    #[test]
    fn test_resolve_services_creates_policy() {
        let parsed = ParsedArgs {
            policy_path: None,
            services_str: Some("drive,gmail".to_string()),
            http_addr: None,
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
        };
        assert!(resolve_config(parsed).is_err());
    }

    #[test]
    fn test_resolve_http_transport() {
        let parsed = ParsedArgs {
            policy_path: None,
            services_str: Some("drive".to_string()),
            http_addr: Some("0.0.0.0:8080".to_string()),
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
        };
        assert!(resolve_config(parsed).is_err());
    }

    #[test]
    fn test_parse_init_policy_interactive() {
        let cmd = parse_args_from(&args(&["--init-policy"])).unwrap();
        match cmd {
            Command::InitPolicy { services } => {
                assert!(services.is_none());
            }
            other => panic!("Expected InitPolicy, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_init_policy_with_services() {
        let cmd = parse_args_from(&args(&["--init-policy", "--services", "drive,sheets"])).unwrap();
        match cmd {
            Command::InitPolicy { services } => {
                assert_eq!(
                    services,
                    Some(vec!["drive".to_string(), "sheets".to_string()])
                );
            }
            other => panic!("Expected InitPolicy, got {other:?}"),
        }
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
    fn test_generate_policy_drive() {
        let json = generate_policy(&["drive".to_string()]);
        let services = json["services"].as_array().unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0]["name"], "drive");
        assert!(services[0]["folders"].is_array());
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
}
