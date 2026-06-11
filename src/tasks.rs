use std::collections::HashMap;
use std::time::Instant;

use serde_json::{Value, json};

use google_workspace::error::GwsError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskStatus {
    Working,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

pub(crate) struct UploadData {
    pub session_uri: String,
    pub total_size: u64,
    pub bytes_uploaded: u64,
    pub content_type: String,
}

pub(crate) struct DownloadData {
    pub b64_data: String,
    pub content_type: String,
    pub total_size: usize,
}

#[allow(dead_code)]
pub(crate) enum TaskKind {
    Upload(UploadData),
    Download(DownloadData),
    Generic,
}

pub(crate) struct Task {
    pub task_id: String,
    pub status: TaskStatus,
    pub status_message: String,
    pub created_at: Instant,
    pub updated_at: Instant,
    pub ttl_ms: u64,
    pub poll_interval_ms: u64,
    pub result: Option<Value>,
    pub kind: TaskKind,
}

impl Task {
    pub(crate) fn new(task_id: String, ttl_ms: u64, kind: TaskKind) -> Self {
        let now = Instant::now();
        Self {
            task_id,
            status: TaskStatus::Working,
            status_message: String::new(),
            created_at: now,
            updated_at: now,
            ttl_ms,
            poll_interval_ms: 2000,
            result: None,
            kind,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut j = json!({
            "taskId": self.task_id,
            "status": self.status.as_str(),
            "statusMessage": self.status_message,
            "ttl": self.ttl_ms,
            "pollInterval": self.poll_interval_ms
        });
        match &self.kind {
            TaskKind::Upload(u) => {
                j["kind"] = json!("upload");
                j["bytesUploaded"] = json!(u.bytes_uploaded);
                j["totalSize"] = json!(u.total_size);
            }
            TaskKind::Download(d) => {
                j["kind"] = json!("download");
                j["totalSize"] = json!(d.total_size);
            }
            TaskKind::Generic => {}
        }
        j
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

fn extract_task_id<'a>(params: &'a Value, method: &str) -> Result<&'a str, GwsError> {
    params
        .get("taskId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GwsError::Validation(format!("Missing 'taskId' in {method}")))
}

fn get_task<'a>(
    params: &'a Value,
    tasks: &'a HashMap<String, Task>,
    method: &str,
) -> Result<(&'a str, &'a Task), GwsError> {
    let task_id = extract_task_id(params, method)?;
    let task = tasks
        .get(task_id)
        .ok_or_else(|| GwsError::Validation(format!("Task '{task_id}' not found")))?;
    Ok((task_id, task))
}

pub(crate) fn handle_tasks_get(
    params: &Value,
    tasks: &HashMap<String, Task>,
) -> Result<Value, GwsError> {
    let (_, task) = get_task(params, tasks, "tasks/get")?;

    Ok(task.to_json())
}

pub(crate) fn handle_tasks_result(
    params: &Value,
    tasks: &HashMap<String, Task>,
) -> Result<Value, GwsError> {
    let (task_id, task) = get_task(params, tasks, "tasks/result")?;

    if !task.status.is_terminal() {
        return Ok(json!({
            "task": task.to_json(),
            "_meta": {
                "io.modelcontextprotocol/related-task": { "taskId": task_id }
            }
        }));
    }

    match &task.result {
        Some(result) => Ok(json!({
            "content": result.get("content").cloned().unwrap_or(json!([])),
            "isError": task.status == TaskStatus::Failed,
            "_meta": {
                "io.modelcontextprotocol/related-task": { "taskId": task_id }
            }
        })),
        None => Ok(json!({
            "content": [{ "type": "text", "text": task.status_message }],
            "isError": task.status == TaskStatus::Failed,
            "_meta": {
                "io.modelcontextprotocol/related-task": { "taskId": task_id }
            }
        })),
    }
}

pub(crate) fn handle_tasks_cancel(
    params: &Value,
    tasks: &mut HashMap<String, Task>,
) -> Result<Value, GwsError> {
    let task_id = extract_task_id(params, "tasks/cancel")?;
    let task = tasks
        .get_mut(task_id)
        .ok_or_else(|| GwsError::Validation(format!("Task '{task_id}' not found")))?;

    if task.status.is_terminal() {
        return Ok(task.to_json());
    }

    task.status = TaskStatus::Cancelled;
    task.status_message = "Cancelled by request".to_string();
    task.updated_at = Instant::now();

    Ok(task.to_json())
}

pub(crate) fn handle_tasks_list(
    _params: &Value,
    tasks: &HashMap<String, Task>,
) -> Result<Value, GwsError> {
    let mut task_list: Vec<Value> = tasks.values().map(|t| t.to_json()).collect();
    task_list.sort_by(|a, b| {
        a["taskId"]
            .as_str()
            .unwrap_or("")
            .cmp(b["taskId"].as_str().unwrap_or(""))
    });

    Ok(json!({ "tasks": task_list }))
}

pub(crate) fn clean_expired_tasks(tasks: &mut HashMap<String, Task>) {
    tasks.retain(|_, t| !t.is_expired());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_lifecycle() {
        let mut task = Task::new("t1".to_string(), 60000, TaskKind::Generic);
        assert_eq!(task.status, TaskStatus::Working);
        assert!(!task.status.is_terminal());

        task.complete(json!({"content": [{"type": "text", "text": "done"}]}));
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.status.is_terminal());
        assert!(task.result.is_some());
    }

    #[test]
    fn test_task_cancel() {
        let mut tasks = HashMap::new();
        tasks.insert(
            "t1".to_string(),
            Task::new("t1".to_string(), 60000, TaskKind::Generic),
        );

        let result = handle_tasks_cancel(&json!({"taskId": "t1"}), &mut tasks).unwrap();
        assert_eq!(result["status"], "cancelled");
    }

    #[test]
    fn test_task_cancel_already_terminal() {
        let mut tasks = HashMap::new();
        let mut task = Task::new("t1".to_string(), 60000, TaskKind::Generic);
        task.complete(json!({}));
        tasks.insert("t1".to_string(), task);

        let result = handle_tasks_cancel(&json!({"taskId": "t1"}), &mut tasks).unwrap();
        assert_eq!(result["status"], "completed");
    }

    #[test]
    fn test_task_get_not_found() {
        let tasks = HashMap::new();
        assert!(handle_tasks_get(&json!({"taskId": "nonexistent"}), &tasks).is_err());
    }

    #[test]
    fn test_task_result_working() {
        let mut tasks = HashMap::new();
        tasks.insert(
            "t1".to_string(),
            Task::new("t1".to_string(), 60000, TaskKind::Generic),
        );

        let result = handle_tasks_result(&json!({"taskId": "t1"}), &tasks).unwrap();
        assert!(result.get("task").is_some());
    }

    #[test]
    fn test_task_result_completed() {
        let mut tasks = HashMap::new();
        let mut task = Task::new("t1".to_string(), 60000, TaskKind::Generic);
        task.complete(json!({"content": [{"type": "text", "text": "done"}]}));
        tasks.insert("t1".to_string(), task);

        let result = handle_tasks_result(&json!({"taskId": "t1"}), &tasks).unwrap();
        assert!(result.get("content").is_some());
        assert_eq!(result["isError"], false);
    }

    #[test]
    fn test_task_list() {
        let mut tasks = HashMap::new();
        tasks.insert(
            "t1".to_string(),
            Task::new("t1".to_string(), 60000, TaskKind::Generic),
        );
        tasks.insert(
            "t2".to_string(),
            Task::new("t2".to_string(), 60000, TaskKind::Generic),
        );

        let result = handle_tasks_list(&json!({}), &tasks).unwrap();
        assert_eq!(result["tasks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_task_fail() {
        let mut task = Task::new("t1".to_string(), 60000, TaskKind::Generic);
        task.fail("something broke");
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task.status.is_terminal());
    }

    #[test]
    fn test_task_expired() {
        let task = Task::new("t1".to_string(), 0, TaskKind::Generic);
        std::thread::sleep(std::time::Duration::from_millis(1));
        assert!(task.is_expired());
    }

    #[test]
    fn test_upload_task_json() {
        let task = Task::new(
            "u1".to_string(),
            60000,
            TaskKind::Upload(UploadData {
                session_uri: "https://example.com/upload".to_string(),
                total_size: 1000,
                bytes_uploaded: 500,
                content_type: "image/png".to_string(),
            }),
        );
        let j = task.to_json();
        assert_eq!(j["kind"], "upload");
        assert_eq!(j["bytesUploaded"], 500);
        assert_eq!(j["totalSize"], 1000);
    }

    #[test]
    fn test_download_task_json() {
        let task = Task::new(
            "d1".to_string(),
            60000,
            TaskKind::Download(DownloadData {
                b64_data: "abc".to_string(),
                content_type: "application/pdf".to_string(),
                total_size: 2048,
            }),
        );
        let j = task.to_json();
        assert_eq!(j["kind"], "download");
        assert_eq!(j["totalSize"], 2048);
    }
}
