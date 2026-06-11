use google_workspace::client;
use google_workspace::error::GwsError;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct FileListResponse {
    #[serde(default)]
    files: Vec<FileEntry>,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    id: String,
}

/// Resolve a Drive path like "My Drive/Customers/ACME/Bills" to a folder ID.
///
/// Walks the path one segment at a time using `files.list` queries.
/// "My Drive" is mapped to the root alias "root".
pub async fn resolve_drive_path(path: &str, token: &str) -> Result<String, GwsError> {
    let (start_id, remaining) = parse_path_segments(path)?;

    let mut current_id = start_id;
    let mut resolved_so_far = String::new();

    for segment in remaining {
        if !resolved_so_far.is_empty() {
            resolved_so_far.push('/');
        }
        resolved_so_far.push_str(segment);

        current_id = find_child_folder(&current_id, segment, token)
            .await
            .map_err(|e| {
                GwsError::Validation(format!(
                    "Failed to resolve Drive path '{path}' at '{resolved_so_far}': {e}"
                ))
            })?;
    }

    Ok(current_id)
}

async fn find_child_folder(parent_id: &str, name: &str, token: &str) -> Result<String, GwsError> {
    let escaped_name = escape_folder_name(name);
    let q = format!(
        "'{parent_id}' in parents and name='{escaped_name}' and \
         mimeType='application/vnd.google-apps.folder' and trashed=false"
    );

    let http_client = client::shared_client()?;
    let response = client::send_with_retry(|| {
        http_client
            .get("https://www.googleapis.com/drive/v3/files")
            .bearer_auth(token)
            .query(&[
                ("q", q.as_str()),
                ("fields", "files(id)"),
                ("pageSize", "2"),
            ])
    })
    .await
    .map_err(|e| GwsError::Other(anyhow::anyhow!("Drive API request failed: {e}")))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Failed to read Drive response: {e}")))?;

    if !status.is_success() {
        return Err(GwsError::Api {
            code: status.as_u16(),
            message: body,
            reason: String::new(),
            enable_url: None,
        });
    }

    let list: FileListResponse = serde_json::from_str(&body)
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Invalid Drive JSON: {e}")))?;

    match list.files.len() {
        0 => Err(GwsError::Validation(format!(
            "Folder '{name}' not found under parent '{parent_id}'"
        ))),
        1 => Ok(list.files.into_iter().next().unwrap().id),
        n => Err(GwsError::Validation(format!(
            "Ambiguous: {n} folders named '{name}' found under parent '{parent_id}'. \
             Use folder IDs instead."
        ))),
    }
}

fn parse_path_segments(path: &str) -> Result<(String, Vec<&str>), GwsError> {
    let segments: Vec<&str> = path
        .split('/')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if segments.is_empty() {
        return Err(GwsError::Validation(format!(
            "Drive path is empty: '{path}'"
        )));
    }

    if segments[0].eq_ignore_ascii_case("My Drive") {
        Ok(("root".to_string(), segments[1..].to_vec()))
    } else {
        Ok(("root".to_string(), segments))
    }
}

fn escape_folder_name(name: &str) -> String {
    name.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_path() {
        let (start, segments) = parse_path_segments("Projects/docs").unwrap();
        assert_eq!(start, "root");
        assert_eq!(segments, vec!["Projects", "docs"]);
    }

    #[test]
    fn test_parse_my_drive_prefix_stripped() {
        let (start, segments) = parse_path_segments("My Drive/Projects/docs").unwrap();
        assert_eq!(start, "root");
        assert_eq!(segments, vec!["Projects", "docs"]);
    }

    #[test]
    fn test_parse_my_drive_case_insensitive() {
        let (_, segments) = parse_path_segments("my drive/Stuff").unwrap();
        assert_eq!(segments, vec!["Stuff"]);
    }

    #[test]
    fn test_parse_empty_path() {
        assert!(parse_path_segments("").is_err());
        assert!(parse_path_segments("   ").is_err());
    }

    #[test]
    fn test_parse_trailing_slashes() {
        let (_, segments) = parse_path_segments("Projects/docs/").unwrap();
        assert_eq!(segments, vec!["Projects", "docs"]);
    }

    #[test]
    fn test_escape_folder_name_quotes() {
        assert_eq!(escape_folder_name("it's mine"), "it\\'s mine");
    }

    #[test]
    fn test_escape_folder_name_backslash() {
        assert_eq!(escape_folder_name("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_folder_name_plain() {
        assert_eq!(escape_folder_name("normal"), "normal");
    }

    #[test]
    fn test_file_list_response_parse_empty() {
        let json = r#"{"files":[]}"#;
        let list: FileListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(list.files.len(), 0);
    }

    #[test]
    fn test_file_list_response_parse_one() {
        let json = r#"{"files":[{"id":"abc123"}]}"#;
        let list: FileListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(list.files.len(), 1);
        assert_eq!(list.files[0].id, "abc123");
    }

    #[test]
    fn test_file_list_response_missing_files() {
        let json = r#"{}"#;
        let list: FileListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(list.files.len(), 0);
    }
}
