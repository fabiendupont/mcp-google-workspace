use google_workspace::discovery::RestMethod;
use mcp_google_workspace::init;
use mcp_google_workspace::policy::{Policy, PolicyFile};
use serde_json::json;

fn get_method() -> RestMethod {
    RestMethod {
        http_method: "GET".to_string(),
        ..Default::default()
    }
}

fn post_method() -> RestMethod {
    RestMethod {
        http_method: "POST".to_string(),
        ..Default::default()
    }
}

fn complex_policy() -> Policy {
    let file: PolicyFile = serde_json::from_value(json!({
        "server": { "read_only": false },
        "services": [
            {
                "name": "drive",
                "constraints": [
                    { "param": "parents", "values": ["folder-rw"], "access": "read-write", "location": "body" },
                    { "param": "parents", "values": ["folder-ro"], "access": "read-only", "location": "body" }
                ]
            },
            {
                "name": "gmail",
                "denied_methods": [
                    "messages.delete", "messages.trash", "messages.batchDelete",
                    "settings.updateAutoForwarding", "settings.delegates.create"
                ],
                "allowed_labels": ["INBOX", "SENT"]
            },
            {
                "name": "sheets",
                "read_only": true
            },
            {
                "name": "calendar",
                "constraints": [
                    { "param": "calendarId", "values": ["primary"], "access": "read-write" },
                    { "param": "calendarId", "values": ["holidays@group.calendar.google.com"], "access": "read-only" }
                ]
            },
            {
                "name": "docs",
                "constraints": [
                    { "param": "documentId", "values": ["template-doc-id"], "access": "read-only", "mode": "protect" }
                ]
            },
            {
                "name": "slides",
                "constraints": [
                    { "param": "presentationId", "values": ["template-pres-id"], "access": "read-only", "mode": "protect" }
                ]
            }
        ]
    }))
    .unwrap();
    Policy::from_policy_file(file)
}

// -- Template round-trip tests --

#[test]
fn template_round_trip_all() {
    for (name, _desc) in init::TEMPLATES {
        let json = init::template_policy(name)
            .unwrap_or_else(|e| panic!("template_policy({name}) failed: {e}"));

        let file: PolicyFile = serde_json::from_value(json.clone())
            .unwrap_or_else(|e| panic!("template '{name}' JSON didn't parse as PolicyFile: {e}"));

        let policy = Policy::from_policy_file(file);
        let services = policy.allowed_services();
        assert!(
            !services.is_empty(),
            "template '{name}' produced no allowed services"
        );
    }
}

#[test]
fn template_analyst_is_read_heavy() {
    let json = init::template_policy("analyst").unwrap();
    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.is_read_only("drive"));
    assert!(policy.is_read_only("sheets"));
    assert!(policy.is_read_only("docs"));
    assert!(!policy.is_service_allowed("calendar"));
}

#[test]
fn template_assistant_has_calendar() {
    let json = init::template_policy("assistant").unwrap();
    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.is_service_allowed("drive"));
    assert!(policy.is_service_allowed("gmail"));
    assert!(policy.is_service_allowed("calendar"));
    assert!(policy.is_service_allowed("sheets"));
    assert!(policy.is_service_allowed("docs"));
    assert!(!policy.is_read_only("drive"));
}

#[test]
fn template_admin_readonly_all_read_only() {
    let json = init::template_policy("admin-readonly").unwrap();
    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.global_read_only);
    for svc in policy.allowed_services() {
        assert!(
            policy.is_read_only(svc),
            "service '{svc}' should be read-only in admin-readonly template"
        );
    }
}

#[test]
fn template_unknown_returns_error() {
    assert!(init::template_policy("nonexistent").is_err());
}

// -- Policy enforcement matrix --

#[test]
fn drive_get_injects_query_filter() {
    let policy = complex_policy();
    let method = get_method();
    let mut params = serde_json::Map::new();

    policy
        .enforce_constraints("drive", "list", &method, &mut params, &None)
        .unwrap();

    let q = params["q"].as_str().unwrap();
    assert!(q.contains("'folder-rw' in parents"));
    assert!(q.contains("'folder-ro' in parents"));
}

#[test]
fn drive_get_merges_existing_query() {
    let policy = complex_policy();
    let method = get_method();
    let mut params = serde_json::Map::new();
    params.insert(
        "q".into(),
        json!("mimeType='application/pdf'"),
    );

    policy
        .enforce_constraints("drive", "list", &method, &mut params, &None)
        .unwrap();

    let q = params["q"].as_str().unwrap();
    assert!(q.contains("mimeType='application/pdf'"));
    assert!(q.contains("'folder-rw' in parents"));
}

#[test]
fn drive_post_to_allowed_folder_passes() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    let body = Some(json!({ "parents": ["folder-rw"], "name": "doc.txt" }));

    assert!(policy
        .enforce_constraints("drive", "create", &method, &mut params, &body)
        .is_ok());
}

#[test]
fn drive_post_to_disallowed_folder_fails() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    let body = Some(json!({ "parents": ["unknown-folder"], "name": "doc.txt" }));

    let err = policy
        .enforce_constraints("drive", "create", &method, &mut params, &body)
        .unwrap_err();
    assert!(err.to_string().contains("not in the allowed list"));
}

#[test]
fn drive_post_to_readonly_folder_fails() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    let body = Some(json!({ "parents": ["folder-ro"], "name": "doc.txt" }));

    let err = policy
        .enforce_constraints("drive", "create", &method, &mut params, &body)
        .unwrap_err();
    assert!(err.to_string().contains("read-only"));
}

#[test]
fn drive_post_without_parents_fails() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    let body = Some(json!({ "name": "doc.txt" }));

    assert!(policy
        .enforce_constraints("drive", "create", &method, &mut params, &body)
        .is_err());
}

#[test]
fn gmail_list_allowed() {
    let policy = complex_policy();
    let method = get_method();

    assert!(policy
        .check_method("gmail", "users.messages", "list", &method)
        .is_ok());
}

#[test]
fn gmail_delete_denied() {
    let policy = complex_policy();
    let method = post_method();

    let err = policy
        .check_method("gmail", "users.messages", "delete", &method)
        .unwrap_err();
    assert!(err.to_string().contains("denied by policy"));
}

#[test]
fn gmail_trash_denied() {
    let policy = complex_policy();
    let method = post_method();

    assert!(policy
        .check_method("gmail", "users.messages", "trash", &method)
        .is_err());
}

#[test]
fn gmail_batch_delete_denied() {
    let policy = complex_policy();
    let method = post_method();

    assert!(policy
        .check_method("gmail", "users.messages", "batchDelete", &method)
        .is_err());
}

#[test]
fn gmail_allowed_labels() {
    let policy = complex_policy();
    let labels = policy.allowed_labels("gmail");
    assert_eq!(labels, &["INBOX", "SENT"]);
}

#[test]
fn sheets_write_denied_read_only() {
    let policy = complex_policy();
    let method = post_method();

    let err = policy
        .check_method("sheets", "spreadsheets", "create", &method)
        .unwrap_err();
    assert!(err.to_string().contains("read-only"));
}

#[test]
fn sheets_read_allowed() {
    let policy = complex_policy();
    let method = get_method();

    assert!(policy
        .check_method("sheets", "spreadsheets", "get", &method)
        .is_ok());
}

#[test]
fn calendar_get_with_allowed_id_passes() {
    let policy = complex_policy();
    let method = get_method();
    let mut params = serde_json::Map::new();
    params.insert("calendarId".into(), json!("primary"));

    assert!(policy
        .enforce_constraints("calendar", "get", &method, &mut params, &None)
        .is_ok());
}

#[test]
fn calendar_get_with_readonly_id_passes() {
    let policy = complex_policy();
    let method = get_method();
    let mut params = serde_json::Map::new();
    params.insert(
        "calendarId".into(),
        json!("holidays@group.calendar.google.com"),
    );

    assert!(policy
        .enforce_constraints("calendar", "get", &method, &mut params, &None)
        .is_ok());
}

#[test]
fn calendar_post_to_readonly_id_fails() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert(
        "calendarId".into(),
        json!("holidays@group.calendar.google.com"),
    );

    let err = policy
        .enforce_constraints("calendar", "create", &method, &mut params, &None)
        .unwrap_err();
    assert!(err.to_string().contains("read-only"));
}

#[test]
fn calendar_post_to_primary_passes() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("calendarId".into(), json!("primary"));

    assert!(policy
        .enforce_constraints("calendar", "create", &method, &mut params, &None)
        .is_ok());
}

#[test]
fn calendar_missing_calendar_id_fails() {
    let policy = complex_policy();
    let method = get_method();
    let mut params = serde_json::Map::new();

    assert!(policy
        .enforce_constraints("calendar", "get", &method, &mut params, &None)
        .is_err());
}

#[test]
fn unknown_service_denied() {
    let policy = complex_policy();
    let method = get_method();

    let err = policy
        .check_method("admin", "users", "list", &method)
        .unwrap_err();
    assert!(err.to_string().contains("not allowed by policy"));
}

#[test]
fn docs_protect_mode_allows_read() {
    let policy = complex_policy();
    let method = get_method();
    let mut params = serde_json::Map::new();
    params.insert("documentId".into(), json!("template-doc-id"));

    assert!(policy
        .enforce_constraints("docs", "get", &method, &mut params, &None)
        .is_ok());
}

#[test]
fn docs_protect_mode_blocks_write() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("documentId".into(), json!("template-doc-id"));

    let err = policy
        .enforce_constraints("docs", "batchUpdate", &method, &mut params, &None)
        .unwrap_err();
    assert!(err.to_string().contains("protected as read-only"));
}

#[test]
fn docs_protect_mode_allows_write_to_other() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("documentId".into(), json!("my-own-doc"));

    assert!(policy
        .enforce_constraints("docs", "batchUpdate", &method, &mut params, &None)
        .is_ok());
}

#[test]
fn slides_protect_allows_copy_of_template() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("presentationId".into(), json!("template-pres-id"));

    assert!(policy
        .enforce_constraints("slides", "copy", &method, &mut params, &None)
        .is_ok());
}

#[test]
fn slides_protect_blocks_update_of_template() {
    let policy = complex_policy();
    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("presentationId".into(), json!("template-pres-id"));

    assert!(policy
        .enforce_constraints("slides", "batchUpdate", &method, &mut params, &None)
        .is_err());
}

// -- Policy file validation (policy.example.json) --

#[test]
fn policy_example_json_parses() {
    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("policy.example.json"),
    )
    .expect("policy.example.json should exist at repo root");

    let file: PolicyFile =
        serde_json::from_str(&content).expect("policy.example.json should be valid PolicyFile");

    let policy = Policy::from_policy_file(file);
    let services = policy.allowed_services();
    assert!(services.contains(&"drive"));
    assert!(services.contains(&"gmail"));
    assert!(services.contains(&"calendar"));
    assert!(services.contains(&"sheets"));
    assert!(services.contains(&"docs"));
    assert!(services.contains(&"slides"));
}

#[test]
fn policy_example_json_gmail_has_denylists() {
    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("policy.example.json"),
    )
    .unwrap();
    let file: PolicyFile = serde_json::from_str(&content).unwrap();
    let policy = Policy::from_policy_file(file);

    let denied = policy.denied_methods("gmail");
    assert!(denied.contains("messages.delete"));
    assert!(denied.contains("messages.trash"));
}

#[test]
fn policy_example_json_sheets_is_read_only() {
    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("policy.example.json"),
    )
    .unwrap();
    let file: PolicyFile = serde_json::from_str(&content).unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.is_read_only("sheets"));
}

#[test]
fn policy_example_json_has_templates() {
    let content = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("policy.example.json"),
    )
    .unwrap();
    let file: PolicyFile = serde_json::from_str(&content).unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(!policy.templates().is_empty());
    assert!(policy.find_template("standard").is_some());
}

// -- generate_policy round-trip --

#[test]
fn generate_policy_round_trip_single_service() {
    let services = vec!["drive".to_string()];
    let json = init::generate_policy(&services);

    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);
    assert!(policy.is_service_allowed("drive"));
    assert!(!policy.is_service_allowed("gmail"));
}

#[test]
fn generate_policy_round_trip_all_services() {
    let services: Vec<String> = ["drive", "gmail", "calendar", "sheets", "docs", "slides", "people"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let json = init::generate_policy(&services);

    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);
    for svc in &services {
        assert!(
            policy.is_service_allowed(svc),
            "service '{svc}' should be allowed after generate_policy"
        );
    }
}

#[test]
fn generate_policy_gmail_has_safety_denylists() {
    let services = vec!["gmail".to_string()];
    let json = init::generate_policy(&services);

    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);
    let denied = policy.denied_methods("gmail");
    assert!(
        denied.contains("messages.delete"),
        "generate_policy for gmail should include messages.delete in denied_methods"
    );
}

#[test]
fn generate_policy_calendar_has_primary_constraint() {
    let services = vec!["calendar".to_string()];
    let json = init::generate_policy(&services);

    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);
    let constraints = policy.constraints("calendar");
    assert!(
        !constraints.is_empty(),
        "generate_policy for calendar should include calendarId constraints"
    );
    assert!(constraints
        .iter()
        .any(|c| c.param == "calendarId" && c.values.contains(&"primary".to_string())));
}

#[test]
fn generate_policy_people_is_read_only() {
    let services = vec!["people".to_string()];
    let json = init::generate_policy(&services);

    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);
    assert!(policy.is_read_only("people"));
}

#[test]
fn generate_policy_unknown_service_passthrough() {
    let services = vec!["customapi".to_string()];
    let json = init::generate_policy(&services);

    let file: PolicyFile = serde_json::from_value(json).unwrap();
    let policy = Policy::from_policy_file(file);
    assert!(policy.is_service_allowed("customapi"));
    assert!(!policy.is_read_only("customapi"));
}

// -- Origin security --

#[test]
fn origin_default_allows_localhost_variants() {
    let policy = complex_policy();
    assert!(policy.is_origin_allowed("http://localhost:3000"));
    assert!(policy.is_origin_allowed("http://localhost:8080"));
    assert!(policy.is_origin_allowed("http://localhost"));
    assert!(policy.is_origin_allowed("http://127.0.0.1:3000"));
    assert!(policy.is_origin_allowed("http://127.0.0.1"));
    assert!(policy.is_origin_allowed("http://[::1]:3000"));
}

#[test]
fn origin_default_rejects_remote() {
    let policy = complex_policy();
    assert!(!policy.is_origin_allowed("https://evil.com"));
    assert!(!policy.is_origin_allowed("https://example.com"));
    assert!(!policy.is_origin_allowed("https://google.com"));
}

#[test]
fn origin_substring_bypass_blocked() {
    let policy = complex_policy();
    assert!(!policy.is_origin_allowed("https://evil-localhost.com"));
    assert!(!policy.is_origin_allowed("https://localhost.evil.com"));
    assert!(!policy.is_origin_allowed("https://not-localhost.com"));
    assert!(!policy.is_origin_allowed("https://127.0.0.1.evil.com"));
    assert!(!policy.is_origin_allowed("https://localhostevil.com"));
}

#[test]
fn origin_invalid_inputs() {
    let policy = complex_policy();
    assert!(!policy.is_origin_allowed("not-a-url"));
    assert!(!policy.is_origin_allowed(""));
    assert!(!policy.is_origin_allowed("javascript:alert(1)"));
    assert!(!policy.is_origin_allowed("file:///etc/passwd"));
    assert!(!policy.is_origin_allowed("data:text/html,<h1>hi</h1>"));
}

#[test]
fn origin_custom_allowlist() {
    let file: PolicyFile = serde_json::from_value(json!({
        "server": { "allowed_origins": ["internal.corp.com", "dashboard.example.com"] },
        "services": [{ "name": "drive" }]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.is_origin_allowed("https://internal.corp.com"));
    assert!(policy.is_origin_allowed("https://dashboard.example.com"));
    assert!(policy.is_origin_allowed("http://internal.corp.com:8080"));
    assert!(!policy.is_origin_allowed("http://localhost:3000"));
    assert!(!policy.is_origin_allowed("https://evil.com"));
}

#[test]
fn origin_custom_allowlist_no_subdomain_bypass() {
    let file: PolicyFile = serde_json::from_value(json!({
        "server": { "allowed_origins": ["corp.example.com"] },
        "services": [{ "name": "drive" }]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.is_origin_allowed("https://corp.example.com"));
    assert!(!policy.is_origin_allowed("https://evil-corp.example.com"));
    assert!(!policy.is_origin_allowed("https://corp.example.com.evil.com"));
    assert!(!policy.is_origin_allowed("https://sub.corp.example.com"));
}

// -- Edge cases --

#[test]
fn empty_policy_denies_everything() {
    let file: PolicyFile = serde_json::from_value(json!({
        "services": []
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.allowed_services().is_empty());
    assert!(!policy.is_service_allowed("drive"));
    assert!(policy
        .check_method("drive", "files", "list", &get_method())
        .is_err());
}

#[test]
fn global_read_only_overrides_service() {
    let file: PolicyFile = serde_json::from_value(json!({
        "server": { "read_only": true },
        "services": [{ "name": "drive" }, { "name": "gmail" }]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);

    assert!(policy.is_read_only("drive"));
    assert!(policy.is_read_only("gmail"));
    assert!(policy
        .check_method("drive", "files", "create", &post_method())
        .is_err());
}

#[test]
fn allowed_services_sorted() {
    let policy = complex_policy();
    let services = policy.allowed_services();
    let mut sorted = services.clone();
    sorted.sort();
    assert_eq!(services, sorted);
}

#[test]
fn with_extra_parent_adds_folder() {
    let policy = complex_policy();
    let extended = policy.with_extra_parent("drive", "new-folder-id");

    let constraints = extended.constraints("drive");
    let all_values: Vec<&str> = constraints
        .iter()
        .flat_map(|c| c.values.iter().map(|v| v.as_str()))
        .collect();
    assert!(all_values.contains(&"new-folder-id"));
    assert!(all_values.contains(&"folder-rw"));
}

#[test]
fn templates_auto_protect_slides() {
    let file: PolicyFile = serde_json::from_value(json!({
        "services": [{ "name": "slides" }],
        "templates": [
            { "id": "tpl-123", "name": "corporate" }
        ]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);

    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("presentationId".into(), json!("tpl-123"));

    let err = policy
        .enforce_constraints("slides", "batchUpdate", &method, &mut params, &None)
        .unwrap_err();
    assert!(err.to_string().contains("protected as read-only"));
}

#[test]
fn templates_auto_protect_docs() {
    let file: PolicyFile = serde_json::from_value(json!({
        "services": [{ "name": "docs" }],
        "templates": [
            { "id": "doc-tpl-456", "name": "memo" }
        ]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);

    let method = post_method();
    let mut params = serde_json::Map::new();
    params.insert("documentId".into(), json!("doc-tpl-456"));

    assert!(policy
        .enforce_constraints("docs", "batchUpdate", &method, &mut params, &None)
        .is_err());
}

#[test]
fn allowed_resources_parsed() {
    let file: PolicyFile = serde_json::from_value(json!({
        "services": [{
            "name": "drive",
            "allowed_resources": ["files", "permissions"]
        }]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);
    assert_eq!(policy.allowed_resources("drive"), &["files", "permissions"]);
    assert!(policy.allowed_resources("gmail").is_empty());
}

#[test]
fn rate_limit_config_parsed() {
    let file: PolicyFile = serde_json::from_value(json!({
        "server": { "rate_limit_rpm": 60 },
        "services": [{ "name": "drive" }]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);
    assert_eq!(policy.rate_limit_rpm, Some(60));
}

#[test]
fn delegate_subject_parsed() {
    let file: PolicyFile = serde_json::from_value(json!({
        "server": { "delegate_subject": "user@example.com" },
        "services": [{ "name": "drive" }]
    }))
    .unwrap();
    let policy = Policy::from_policy_file(file);
    assert_eq!(
        policy.delegate_subject.as_deref(),
        Some("user@example.com")
    );
}
