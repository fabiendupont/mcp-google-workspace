#![allow(dead_code, clippy::too_many_arguments, clippy::manual_async_fn)]

use mcp_google_workspace::*;
mod init;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use google_workspace::error::GwsError;
use opentelemetry::trace::TracerProvider;
use rmcp::ServiceExt;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// MCP server for Google Workspace APIs with per-project safety policies
#[derive(Parser, Debug)]
#[command(name = "mcp-google-workspace", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,

    /// Path to a gws-policy.json file
    #[arg(long, global = true)]
    policy: Option<PathBuf>,

    /// Comma-separated service names (e.g., drive,gmail,calendar)
    #[arg(long, short, global = true)]
    services: Option<String>,

    /// Run as HTTP server (e.g., 127.0.0.1:3000)
    #[arg(long)]
    http: Option<String>,

    /// External URL for webhook callbacks
    #[arg(long)]
    external_url: Option<String>,

    /// Use compact tool schemas (fewer tokens)
    #[arg(long)]
    compact_schemas: bool,

    /// Load all helper tools at startup instead of lazy discovery
    #[arg(long)]
    eager_tools: bool,

    /// Write structured audit log (JSONL) of all API calls
    #[arg(long)]
    audit_log: Option<PathBuf>,

    /// Directory containing prompt .md files
    #[arg(long)]
    prompts_dir: Option<PathBuf>,
}

#[derive(clap::Subcommand, Debug)]
enum CliCommand {
    /// Guided setup: check auth, pick services, folders, labels, calendars
    Init {
        /// Use a preset instead of interactive setup (analyst, assistant, admin-readonly)
        #[arg(long)]
        template: Option<String>,
    },

    /// Validate a policy file and show security warnings
    CheckPolicy {
        /// Path to the policy file to validate
        path: PathBuf,

        /// Also test credentials against Google APIs
        #[arg(long)]
        verify: bool,
    },

    /// Walk the credential chain and report what is available
    CheckAuth,

    /// Dry-run scenarios against a policy
    Simulate {
        /// Path to scenarios JSON file
        scenarios: PathBuf,
    },
}

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
    eager_tools: bool,
    audit_log: Option<PathBuf>,
    prompts_dir: Option<PathBuf>,
}

fn parse_args_from(args: &[String]) -> Result<Command, GwsError> {
    let mut full_args = vec!["mcp-google-workspace".to_string()];
    full_args.extend_from_slice(args);

    let cli = match Cli::try_parse_from(&full_args) {
        Ok(c) => c,
        Err(e)
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion =>
        {
            return Ok(Command::ShowHelp);
        }
        Err(e) => return Err(GwsError::Validation(e.to_string())),
    };

    cli_to_command(cli)
}

fn cli_to_command(cli: Cli) -> Result<Command, GwsError> {
    match cli.command {
        Some(CliCommand::Init { template }) => {
            let services = cli
                .services
                .map(|s| s.split(',').map(|s| s.trim().to_string()).collect());
            Ok(Command::InitPolicy { services, template })
        }
        Some(CliCommand::CheckPolicy { path, verify }) => Ok(Command::CheckPolicy { path, verify }),
        Some(CliCommand::CheckAuth) => Ok(Command::CheckAuth {
            policy_path: cli.policy,
        }),
        Some(CliCommand::Simulate { scenarios }) => {
            let policy_path = cli.policy.ok_or_else(|| {
                GwsError::Validation("simulate requires --policy to also be set".to_string())
            })?;
            Ok(Command::Simulate {
                policy_path,
                scenarios_path: scenarios,
            })
        }
        None => Ok(Command::Serve(ParsedArgs {
            policy_path: cli.policy,
            services_str: cli.services,
            http_addr: cli.http,
            external_url: cli.external_url,
            compact_schemas: cli.compact_schemas,
            eager_tools: cli.eager_tools,
            audit_log: cli.audit_log,
            prompts_dir: cli.prompts_dir,
        })),
    }
}

const POLICY_FILE_NAMES: &[&str] = &[".gws-policy.json", "gws-policy.json"];

fn discover_policy_file() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        for name in POLICY_FILE_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                tracing::info!(path = %candidate.display(), "Auto-discovered policy file");
                return Some(candidate);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn resolve_config(parsed: ParsedArgs) -> Result<(policy::Policy, Transport), GwsError> {
    let policy = if let Some(path) = parsed.policy_path {
        policy::Policy::from_file(&path)?
    } else if let Some(svc) = parsed.services_str {
        let names: Vec<String> = svc.split(',').map(|s| s.trim().to_string()).collect();
        policy::Policy::from_services(&names)
    } else if let Some(discovered) = discover_policy_file() {
        policy::Policy::from_file(&discovered)?
    } else {
        return Err(GwsError::Validation(
            "No policy found. Either:\n  \
             - Run: mcp-google-workspace init\n  \
             - Pass: --policy <path>\n  \
             - Place .gws-policy.json in the project directory"
                .to_string(),
        ));
    };

    let transport = match parsed.http_addr {
        Some(addr) => Transport::Http(addr),
        None => Transport::Stdio,
    };

    Ok((policy, transport))
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
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            e.exit();
        }
    };
    let cmd = match cli_to_command(cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let parsed = match cmd {
        Command::InitPolicy { services, template } => {
            let interactive = services.is_none() && template.is_none();
            let json = if let Some(ref tmpl) = template {
                if let Some((_, desc)) = init::TEMPLATES.iter().find(|(n, _)| *n == tmpl.as_str()) {
                    eprintln!("Using template '{tmpl}': {desc}");
                }
                match init::template_policy(tmpl) {
                    Ok(j) => j,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else if let Some(svc) = services {
                init::generate_policy(&svc)
            } else {
                match init::init_guided().await {
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
                    .default(".gws-policy.json".to_string())
                    .interact_text()
                    .unwrap_or_else(|_| ".gws-policy.json".to_string());

                if let Err(e) = std::fs::write(&path, &output) {
                    eprintln!("Error writing {path}: {e}");
                    std::process::exit(1);
                }
                eprintln!();
                eprintln!("Saved to {path}");
                if path.starts_with('.') {
                    eprintln!();
                    eprintln!("Add to .gitignore (contains project IDs and folder IDs):");
                    eprintln!("  echo '{path}' >> .gitignore");
                }
                eprintln!();
                eprintln!("The server auto-discovers {path} — just run:");
                eprintln!("  mcp-google-workspace");
                eprintln!();
                eprintln!("Or explicitly:");
                eprintln!("  mcp-google-workspace --policy {path}");
                eprintln!();
                eprintln!("Validate with:");
                eprintln!("  mcp-google-workspace check-policy {path}");
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
            Cli::parse_from(["mcp-google-workspace", "--help"]);
            unreachable!();
        }
        Command::Serve(p) => p,
    };

    init_telemetry();

    let audit_log_path = parsed.audit_log.clone();
    let policy_file_path = parsed.policy_path.clone();
    let prompts_dir_flag = parsed.prompts_dir.clone();
    let external_url = parsed.external_url.clone();
    let compact_schemas = parsed.compact_schemas;
    let eager_tools = parsed.eager_tools;

    let (mut policy, transport) = match resolve_config(parsed) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    policy.compact_schemas = compact_schemas;

    rate_limit::init_global(policy.rate_limit_rpm, policy.rate_limits.clone());

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

            let handler = handler::GwsHandler::new(policy, prompts, audit, eager_tools);
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

            let webhook = external_url
                .clone()
                .or_else(|| Some(format!("http://{addr}")));
            let state = shared::ServerState::with_config(prompts, audit, eager_tools, webhook);
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
    }

    #[test]
    fn test_parse_policy_missing_value() {
        let err = parse_args_from(&args(&["--policy"]));
        assert!(err.is_err());
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
            eager_tools: false,
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
            eager_tools: false,
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
            eager_tools: false,
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
            eager_tools: false,
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
            eager_tools: false,
            audit_log: None,
            prompts_dir: None,
        };
        assert!(resolve_config(parsed).is_err());
    }

    #[test]
    fn test_parse_init_policy_interactive() {
        let cmd = parse_args_from(&args(&["init"])).unwrap();
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
        let cmd = parse_args_from(&args(&["init", "--services", "drive,sheets"])).unwrap();
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
        let cmd = parse_args_from(&args(&["init", "--template", "analyst"])).unwrap();
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
        let json = init::template_policy("analyst").unwrap();
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
        let json = init::template_policy("assistant").unwrap();
        let services = json["services"].as_array().unwrap();
        assert!(services.iter().any(|s| s["name"] == "drive"));
        assert!(services.iter().any(|s| s["name"] == "calendar"));
    }

    #[test]
    fn test_template_admin_readonly() {
        let json = init::template_policy("admin-readonly").unwrap();
        assert_eq!(json["server"]["read_only"], true);
        let services = json["services"].as_array().unwrap();
        assert!(services.len() >= 6);
    }

    #[test]
    fn test_template_unknown() {
        assert!(init::template_policy("nonexistent").is_err());
    }

    #[test]
    fn test_parse_check_policy() {
        let cmd = parse_args_from(&args(&["check-policy", "/tmp/policy.json"])).unwrap();
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
            parse_args_from(&args(&["check-policy", "/tmp/policy.json", "--verify"])).unwrap();
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
        let err = parse_args_from(&args(&["check-policy"]));
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_check_auth() {
        let cmd = parse_args_from(&args(&["check-auth"])).unwrap();
        match cmd {
            Command::CheckAuth { policy_path } => {
                assert!(policy_path.is_none());
            }
            other => panic!("Expected CheckAuth, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_check_auth_with_policy() {
        let cmd = parse_args_from(&args(&["check-auth", "--policy", "/tmp/policy.json"])).unwrap();
        match cmd {
            Command::CheckAuth { policy_path } => {
                assert_eq!(policy_path, Some(PathBuf::from("/tmp/policy.json")));
            }
            other => panic!("Expected CheckAuth, got {other:?}"),
        }
    }

    #[test]
    fn test_generate_policy_drive() {
        let json = init::generate_policy(&["drive".to_string()]);
        let services = json["services"].as_array().unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0]["name"], "drive");
        assert!(services[0]["allowed_resources"].is_array());
    }

    #[test]
    fn test_generate_policy_unknown_service() {
        let json = init::generate_policy(&["tasks".to_string()]);
        let services = json["services"].as_array().unwrap();
        assert_eq!(services[0]["name"], "tasks");
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
            "simulate",
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
        let err = parse_args_from(&args(&["simulate", "/tmp/scenarios.json"]));
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_simulate_missing_value() {
        let err = parse_args_from(&args(&["--policy", "/tmp/policy.json", "simulate"]));
        assert!(err.is_err());
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
    }
}
