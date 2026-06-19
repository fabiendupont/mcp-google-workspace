use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::{Value, json};

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

    pub fn log_allowed(
        &self,
        service: &str,
        resource: &str,
        method: &str,
        http_method: &str,
        status: u16,
        duration_ms: u64,
    ) {
        let entry = json!({
            "timestamp": timestamp(),
            "action": "allowed",
            "service": service,
            "resource": resource,
            "method": method,
            "http_method": http_method,
            "status": status,
            "duration_ms": duration_ms,
        });
        self.write(entry);
    }

    pub fn log_denied(&self, service: &str, resource: &str, method: &str, reason: &str) {
        let entry = json!({
            "timestamp": timestamp(),
            "action": "denied",
            "service": service,
            "resource": resource,
            "method": method,
            "reason": reason,
        });
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

    #[test]
    fn test_audit_log_writes_jsonl() {
        let dir = std::env::temp_dir().join("mcp-gws-audit-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");

        let logger = AuditLogger::new(path.clone()).unwrap();
        logger.log_allowed("drive", "files", "list", "GET", 200, 42);
        logger.log_denied("docs", "documents", "create", "read-only");

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["action"], "allowed");
        assert_eq!(first["service"], "drive");
        assert_eq!(first["status"], 200);

        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["action"], "denied");
        assert_eq!(second["reason"], "read-only");

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
