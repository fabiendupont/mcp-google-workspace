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

use std::path::PathBuf;
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
        "  mcp-google-workspace --policy <path>        Load services and constraints from a TOML policy file"
    );
    eprintln!(
        "  mcp-google-workspace --services drive,gmail  Expose specific services (no constraints)"
    );
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --policy <path>       Path to a gws-policy.toml file");
    eprintln!("  --services <list>     Comma-separated service names (e.g., drive,gmail,calendar)");
    eprintln!("  --http <addr:port>    Run as HTTP server (e.g., 127.0.0.1:3000)");
    eprintln!("  --help                Show this help message");
}

fn parse_args_from(args: &[String]) -> Result<ParsedArgs, GwsError> {
    let mut policy_path: Option<PathBuf> = None;
    let mut services_str: Option<String> = None;
    let mut http_addr: Option<String> = None;

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
            other => {
                return Err(GwsError::Validation(format!("Unknown argument: {other}")));
            }
        }
        i += 1;
    }

    Ok(ParsedArgs {
        policy_path,
        services_str,
        http_addr,
    })
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

fn parse_args() -> Result<(policy::Policy, Transport), GwsError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = parse_args_from(&args)?;
    resolve_config(parsed)
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
    init_telemetry();

    let (mut policy, transport) = match parse_args() {
        Ok(p) => p,
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

    #[test]
    fn test_parse_services_flag() {
        let parsed = parse_args_from(&args(&["--services", "drive,gmail"])).unwrap();
        assert_eq!(parsed.services_str.as_deref(), Some("drive,gmail"));
        assert!(parsed.policy_path.is_none());
        assert!(parsed.http_addr.is_none());
    }

    #[test]
    fn test_parse_services_short_flag() {
        let parsed = parse_args_from(&args(&["-s", "calendar"])).unwrap();
        assert_eq!(parsed.services_str.as_deref(), Some("calendar"));
    }

    #[test]
    fn test_parse_policy_flag() {
        let parsed = parse_args_from(&args(&["--policy", "/tmp/gws.toml"])).unwrap();
        assert_eq!(parsed.policy_path, Some(PathBuf::from("/tmp/gws.toml")));
    }

    #[test]
    fn test_parse_http_flag() {
        let parsed =
            parse_args_from(&args(&["--services", "drive", "--http", "127.0.0.1:3000"])).unwrap();
        assert_eq!(parsed.http_addr.as_deref(), Some("127.0.0.1:3000"));
        assert_eq!(parsed.services_str.as_deref(), Some("drive"));
    }

    #[test]
    fn test_parse_no_args() {
        let parsed = parse_args_from(&args(&[])).unwrap();
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
            policy_path: Some(PathBuf::from("/nonexistent/path/policy.toml")),
            services_str: None,
            http_addr: None,
        };
        assert!(resolve_config(parsed).is_err());
    }
}
