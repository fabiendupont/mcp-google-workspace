use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskStatus {
    Working,
    Completed,
    Failed,
}

pub(crate) struct UploadData {
    pub session_uri: String,
    pub total_size: u64,
    pub bytes_uploaded: u64,
    pub content_type: String,
}

pub(crate) struct DownloadData {
    pub raw_data: Vec<u8>,
    pub content_type: String,
    pub total_size: usize,
}

pub(crate) enum TaskKind {
    Upload(UploadData),
    Download(DownloadData),
    #[allow(dead_code)]
    Generic,
}

pub(crate) struct Task {
    pub status: TaskStatus,
    pub status_message: String,
    pub created_at: Instant,
    pub updated_at: Instant,
    pub ttl_ms: u64,
    pub result: Option<Value>,
    pub kind: TaskKind,
}

impl Task {
    pub(crate) fn new(_task_id: String, ttl_ms: u64, kind: TaskKind) -> Self {
        let now = Instant::now();
        Self {
            status: TaskStatus::Working,
            status_message: String::new(),
            created_at: now,
            updated_at: now,
            ttl_ms,
            result: None,
            kind,
        }
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.created_at.elapsed().as_millis() as u64 > self.ttl_ms
    }

    pub(crate) fn complete(&mut self, result: Value) {
        self.status = TaskStatus::Completed;
        self.status_message = "Completed".to_string();
        self.updated_at = Instant::now();
        self.result = Some(result);
    }

    #[allow(dead_code)]
    pub(crate) fn fail(&mut self, message: &str) {
        self.status = TaskStatus::Failed;
        self.status_message = message.to_string();
        self.updated_at = Instant::now();
    }
}

pub(crate) fn clean_expired_tasks(tasks: &mut HashMap<String, Task>) {
    tasks.retain(|_, t| !t.is_expired());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_lifecycle() {
        let task = Task::new(
            "t1".to_string(),
            60_000,
            TaskKind::Upload(UploadData {
                session_uri: "https://example.com".to_string(),
                total_size: 1000,
                bytes_uploaded: 0,
                content_type: "application/pdf".to_string(),
            }),
        );
        assert_eq!(task.status, TaskStatus::Working);
        assert!(!task.is_expired());
    }

    #[test]
    fn test_task_complete() {
        let mut task = Task::new(
            "t1".to_string(),
            60_000,
            TaskKind::Upload(UploadData {
                session_uri: "https://example.com".to_string(),
                total_size: 1000,
                bytes_uploaded: 0,
                content_type: "application/pdf".to_string(),
            }),
        );
        task.complete(serde_json::json!({"id": "file123"}));
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.result.is_some());
    }

    #[test]
    fn test_task_fail() {
        let mut task = Task::new(
            "t1".to_string(),
            60_000,
            TaskKind::Upload(UploadData {
                session_uri: "https://example.com".to_string(),
                total_size: 1000,
                bytes_uploaded: 0,
                content_type: "application/pdf".to_string(),
            }),
        );
        task.fail("Upload failed");
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.status_message, "Upload failed");
    }

    #[test]
    fn test_task_expired() {
        let task = Task::new("t1".to_string(), 0, TaskKind::Generic);
        std::thread::sleep(std::time::Duration::from_millis(1));
        assert!(task.is_expired());
    }

    #[test]
    fn test_clean_expired_tasks() {
        let mut tasks = HashMap::new();
        tasks.insert(
            "old".to_string(),
            Task::new("old".to_string(), 0, TaskKind::Generic),
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
        tasks.insert(
            "recent".to_string(),
            Task::new("recent".to_string(), 3_600_000, TaskKind::Generic),
        );
        clean_expired_tasks(&mut tasks);
        assert!(!tasks.contains_key("old"));
        assert!(tasks.contains_key("recent"));
    }
}
