use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use google_workspace::error::GwsError;
use serde_json::{Value, json};

/// Prefix marking a policy refusal whose details must be hidden from the caller.
///
/// The policy engine builds refusals by `format!`-ing operator-configured
/// values (allowed roots, allowed values, denied methods). Classification used
/// to be a substring search over those messages, which silently mislabelled a
/// refusal as "allowed" whenever its wording changed. Every refusal is instead
/// tagged at construction time with one of these two sentinels, so the audit
/// `action`, the 403 status and client masking all derive from the same
/// unforgeable marker rather than from English prose.
///
/// `U+0001` is a control character: it cannot be produced by a policy file or a
/// tool argument that survives JSON parsing, and it is stripped before any
/// message reaches a caller or an audit line.
pub(crate) const POLICY_DENIAL_MASK: &str = "\u{1}gws-policy-denied:mask\u{1}";

/// Prefix marking a policy refusal whose remediation must reach the caller.
///
/// Same typing as [`POLICY_DENIAL_MASK`], but the message is returned verbatim
/// because only the caller can act on it (for example "use the helper tool
/// instead"), while still being audited as a denial.
pub(crate) const POLICY_DENIAL_VERBATIM: &str = "\u{1}gws-policy-denied:verbatim\u{1}";

/// Build a policy refusal whose internals must not reach the caller.
pub(crate) fn policy_denial(detail: impl std::fmt::Display) -> GwsError {
    GwsError::Validation(format!("{POLICY_DENIAL_MASK}{detail}"))
}

/// Build a policy refusal whose remediation is safe (and necessary) to return.
pub(crate) fn policy_denial_verbatim(detail: impl std::fmt::Display) -> GwsError {
    GwsError::Validation(format!("{POLICY_DENIAL_VERBATIM}{detail}"))
}

/// How a refused call must be reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DenialKind {
    /// Not a policy refusal: validation, upstream API error, internal failure.
    NotDenied,
    /// Policy refusal whose details name allowed roots/values and must be
    /// replaced by a generic message for the caller.
    Masked,
    /// Policy refusal whose message must reach the caller verbatim.
    Verbatim,
}

impl DenialKind {
    pub(crate) fn is_denial(self) -> bool {
        !matches!(self, Self::NotDenied)
    }
}

/// Classify a refusal from its sentinel, falling back to the legacy wording.
///
/// The wording table only exists so refusals constructed before the sentinels
/// were introduced keep being audited correctly. New refusals must be built
/// with [`policy_denial`] or [`policy_denial_verbatim`]; a missing entry in the
/// table is then a non-issue rather than an audit false negative.
pub(crate) fn classify_denial(message: &str) -> DenialKind {
    if message.starts_with(POLICY_DENIAL_MASK) {
        return DenialKind::Masked;
    }
    if message.starts_with(POLICY_DENIAL_VERBATIM) {
        return DenialKind::Verbatim;
    }
    classify_denial_legacy(message)
}

fn classify_denial_legacy(message: &str) -> DenialKind {
    // Refusals that carry an actionable remediation the caller needs.
    const VERBATIM: &[&str] = &[
        // Helper-only services (`server.rs`, `execute.rs`).
        "uses helper tools",
        "gws_batch does not support service",
        // Delegation subject allowlist (`policy::resolve_delegation_subject`).
        "not in server.allowed_subjects",
        // Generated-image cleanup precheck (`shared::ensure_image_sharing_cleanup_allowed`).
        "blocked by policy",
    ];
    // Refusals that name operator-configured policy internals.
    const MASKED: &[&str] = &[
        "Operation not allowed by policy",
        "not allowed by policy",
        "denied by policy",
        // `Policy::check_method`.
        "is read-only;",
        "is read-only.",
        "are read-only",
        // Constraint enforcement (`Policy::enforce_constraints` and friends).
        "Write denied",
        "Constraints are configured for",
        "is protected as read-only",
        "is not in the allowed list",
        // Per-service resource allowlist (`execute::ensure_resource_allowed`).
        "not in allowed_resources",
        // Read-by-ID folder scope (`execute::enforce_read_scope`).
        "Read denied:",
        "cannot enforce folder scoping",
        "is not inside an allowed root folder",
        // Batch pre-validation (`server::execute_batch`).
        "Batch rejected:",
        // Discovery (`tools::handle_discover`).
        "is not enabled. Enabled:",
        // Folder resolution (`elicitation::resolve_target_folder`).
        "no read-write folders",
        "Multiple writable folders available",
    ];

    if VERBATIM.iter().any(|marker| message.contains(marker)) {
        return DenialKind::Verbatim;
    }
    if MASKED.iter().any(|marker| message.contains(marker)) {
        return DenialKind::Masked;
    }
    DenialKind::NotDenied
}

/// Strip a denial sentinel before a message is shown or audited.
pub(crate) fn denial_detail(message: &str) -> &str {
    message
        .strip_prefix(POLICY_DENIAL_MASK)
        .or_else(|| message.strip_prefix(POLICY_DENIAL_VERBATIM))
        .unwrap_or(message)
}

pub struct AuditLogger {
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl AuditLogger {
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Record an allowed operation without a tool name.
    pub fn log_allowed(
        &self,
        service: &str,
        resource: &str,
        method: &str,
        http_method: &str,
        status: u16,
        duration_ms: u64,
    ) {
        self.log_allowed_with_tool(
            None,
            service,
            resource,
            method,
            http_method,
            status,
            duration_ms,
        );
    }

    /// Legacy entry point: allowed operation with an optional tool name.
    ///
    /// Kept so existing callers keep compiling. Prefer
    /// [`Self::log_allowed_with_outcome`], which also records the delegation
    /// subject.
    pub fn log_allowed_with_tool(
        &self,
        tool_name: Option<&str>,
        service: &str,
        resource: &str,
        method: &str,
        http_method: &str,
        status: u16,
        duration_ms: u64,
    ) {
        self.log_allowed_with_outcome(
            service,
            resource,
            method,
            http_method,
            status,
            duration_ms,
            tool_name.unwrap_or(""),
            None,
        );
    }

    /// Record an allowed operation with the real HTTP status, duration, tool
    /// name and (when present) the delegated subject.
    #[allow(clippy::too_many_arguments)]
    pub fn log_allowed_with_outcome(
        &self,
        service: &str,
        resource: &str,
        method: &str,
        http_method: &str,
        status: u16,
        duration_ms: u64,
        tool: &str,
        subject: Option<&str>,
    ) {
        let mut entry = json!({
            "timestamp": timestamp(),
            "action": "allowed",
            "outcome": "allowed",
            "service": service,
            "resource": resource,
            "method": method,
            "http_method": http_method,
            "status": status,
            "duration_ms": duration_ms,
        });
        if !tool.is_empty() {
            entry["tool"] = json!(tool);
        }
        if let Some(subject) = subject {
            entry["subject"] = json!(subject);
        }
        self.write(entry);
    }

    /// Record a denied operation.
    pub fn log_denied(&self, service: &str, resource: &str, method: &str, reason: &str) {
        self.log_denied_with_subject(service, resource, method, reason, None);
    }

    /// Record a denied operation, optionally attributing it to a delegated
    /// subject. The reason is passed through `serde_json`, so newlines and other
    /// control characters cannot inject extra JSONL lines.
    pub fn log_denied_with_subject(
        &self,
        service: &str,
        resource: &str,
        method: &str,
        reason: &str,
        subject: Option<&str>,
    ) {
        let mut entry = json!({
            "timestamp": timestamp(),
            "action": "denied",
            "outcome": "denied",
            "service": service,
            "resource": resource,
            "method": method,
            "status": 403,
            "reason": reason,
        });
        if let Some(subject) = subject {
            entry["subject"] = json!(subject);
        }
        self.write(entry);
    }

    /// Record a call that policy allowed but that failed for a non-policy
    /// reason (upstream API error, transport failure, internal bug).
    ///
    /// Such a call must never appear as `"action":"allowed"`: the documented
    /// `jq 'select(.action == "denied")'` workflow is only trustworthy when
    /// `allowed` really means "nothing went wrong".
    #[allow(clippy::too_many_arguments)]
    pub fn log_error_with_subject(
        &self,
        service: &str,
        resource: &str,
        method: &str,
        status: u16,
        reason: &str,
        duration_ms: u64,
        tool: &str,
        subject: Option<&str>,
    ) {
        let mut entry = json!({
            "timestamp": timestamp(),
            "action": "error",
            "outcome": "error",
            "service": service,
            "resource": resource,
            "method": method,
            "status": status,
            "duration_ms": duration_ms,
            "reason": reason,
        });
        if !tool.is_empty() {
            entry["tool"] = json!(tool);
        }
        if let Some(subject) = subject {
            entry["subject"] = json!(subject);
        }
        self.write(entry);
    }

    fn write(&self, entry: Value) {
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let line = serde_json::to_string(&entry).unwrap_or_default();
        let _ = writeln!(f, "{line}");
    }
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let millis = d.subsec_millis();
            format!("{secs}.{millis:03}")
        })
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_logger(name: &str) -> (AuditLogger, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mcp-gws-audit-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        let logger = AuditLogger::new(path.clone()).unwrap();
        (logger, path, dir)
    }

    fn read_lines(path: &PathBuf) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn test_audit_log_writes_jsonl() {
        let (logger, path, dir) = test_logger("basic");
        logger.log_allowed("drive", "files", "list", "GET", 200, 42);
        logger.log_denied("docs", "documents", "create", "read-only");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2);

        assert_eq!(lines[0]["action"], "allowed");
        assert_eq!(lines[0]["outcome"], "allowed");
        assert_eq!(lines[0]["service"], "drive");
        assert_eq!(lines[0]["status"], 200);

        assert_eq!(lines[1]["action"], "denied");
        assert_eq!(lines[1]["outcome"], "denied");
        assert_eq!(lines[1]["reason"], "read-only");
        assert_eq!(lines[1]["status"], 403);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_audit_records_real_status() {
        let (logger, path, dir) = test_logger("status");
        logger.log_allowed_with_outcome(
            "drive",
            "files",
            "get",
            "GET",
            200,
            12,
            "gws_drive_get",
            None,
        );
        logger.log_allowed_with_outcome(
            "drive",
            "files",
            "delete",
            "DELETE",
            403,
            9,
            "gws_drive_delete",
            None,
        );

        let lines = read_lines(&path);
        assert_eq!(lines[0]["status"], 200);
        assert_eq!(lines[1]["status"], 403);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_audit_subject_present() {
        let (logger, path, dir) = test_logger("subject-present");
        logger.log_allowed_with_outcome(
            "gmail",
            "users.messages",
            "list",
            "GET",
            200,
            5,
            "gws_gmail_search",
            Some("alice@example.com"),
        );
        logger.log_denied_with_subject(
            "drive",
            "files",
            "delete",
            "read-only",
            Some("alice@example.com"),
        );

        let lines = read_lines(&path);
        assert_eq!(lines[0]["tool"], "gws_gmail_search");
        assert_eq!(lines[0]["subject"], "alice@example.com");
        assert_eq!(lines[1]["subject"], "alice@example.com");
        assert_eq!(lines[1]["outcome"], "denied");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_audit_subject_absent_when_none() {
        let (logger, path, dir) = test_logger("subject-absent");
        logger.log_allowed_with_outcome(
            "drive",
            "files",
            "list",
            "GET",
            200,
            1,
            "gws_drive_search",
            None,
        );
        logger.log_denied_with_subject("drive", "files", "delete", "read-only", None);

        let lines = read_lines(&path);
        assert!(lines[0].get("subject").is_none());
        assert!(lines[1].get("subject").is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_audit_reason_newline_cannot_inject_lines() {
        let (logger, path, dir) = test_logger("injection");
        let reason = "denied\n{\"action\":\"allowed\",\"service\":\"drive\"}\r\n";
        logger.log_denied_with_subject(
            "docs",
            "documents",
            "create",
            reason,
            Some("bob@example.com"),
        );

        // Exactly one JSONL line, and the payload survives intact as a string.
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["reason"], reason);
        assert_eq!(lines[0]["action"], "denied");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_audit_error_entry_is_not_allowed() {
        let (logger, path, dir) = test_logger("error-action");
        logger.log_error_with_subject(
            "drive",
            "files",
            "create",
            500,
            "transport failure",
            11,
            "gws_drive_upload",
            Some("alice@example.com"),
        );

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        // A failed call must never look successful to `action == "allowed"`.
        assert_eq!(lines[0]["action"], "error");
        assert_eq!(lines[0]["outcome"], "error");
        assert_eq!(lines[0]["status"], 500);
        assert_eq!(lines[0]["reason"], "transport failure");
        assert_eq!(lines[0]["tool"], "gws_drive_upload");
        assert_eq!(lines[0]["subject"], "alice@example.com");
        assert_eq!(lines[0]["duration_ms"], 11);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_audit_legacy_tool_wrapper() {
        let (logger, path, dir) = test_logger("legacy");
        logger.log_allowed_with_tool(Some("gws_batch"), "drive", "files", "list", "GET", 200, 7);

        let lines = read_lines(&path);
        assert_eq!(lines[0]["tool"], "gws_batch");
        assert_eq!(lines[0]["outcome"], "allowed");
        assert_eq!(lines[0]["status"], 200);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_timestamp_format() {
        let ts = timestamp();
        assert!(ts.contains('.'));
        let parts: Vec<&str> = ts.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].parse::<u64>().is_ok());
    }
}
