//! Guards for the registry manifests (server.json, glama.json).
//! These prevent the manifests from drifting out of sync with the crate.

use serde_json::Value;

fn repo_root(name: &str) -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push(name);
    path
}

fn load_json(name: &str) -> Value {
    let path = repo_root(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse {} as JSON: {e}", path.display()))
}

#[test]
fn server_json_has_correct_identity() {
    let server = load_json("server.json");
    assert_eq!(
        server["name"], "io.github.fabiendupont/mcp-google-workspace",
        "server.json name must be reverse-DNS for this repository"
    );
    let name = server["name"].as_str().unwrap();
    assert!(
        name.matches('/').count() == 1,
        "name must contain exactly one forward slash"
    );
}

#[test]
fn server_json_version_matches_crate() {
    let server = load_json("server.json");
    assert_eq!(
        server["version"],
        env!("CARGO_PKG_VERSION"),
        "server.json version must match Cargo.toml version — update it on release"
    );
    for package in server["packages"]
        .as_array()
        .expect("packages must be an array")
    {
        assert_eq!(
            package["version"],
            env!("CARGO_PKG_VERSION"),
            "every package version must match Cargo.toml version"
        );
    }
}

#[test]
fn server_json_description_within_schema_limit() {
    let server = load_json("server.json");
    let desc = server["description"].as_str().expect("description");
    assert!(
        !desc.is_empty() && desc.len() <= 100,
        "description must be 1-100 chars per the schema, got {}",
        desc.len()
    );
}

#[test]
fn server_json_package_shape() {
    let server = load_json("server.json");
    let packages = server["packages"].as_array().expect("packages");
    assert!(!packages.is_empty(), "at least one package required");
    for package in packages {
        for key in ["registryType", "identifier"] {
            assert!(
                package[key].is_string(),
                "package missing required field '{key}'"
            );
        }
        assert!(
            package["transport"]["type"].is_string(),
            "package missing required transport.type"
        );
    }
    // The OCI package must point at the image this repository publishes
    let oci = packages
        .iter()
        .find(|p| p["registryType"] == "oci")
        .expect("an oci package must exist");
    assert_eq!(
        oci["identifier"], "ghcr.io/fabiendupont/mcp-google-workspace",
        "oci package must reference the ghcr.io image published by release.yml"
    );
}

#[test]
fn glama_json_declares_maintainer() {
    let glama = load_json("glama.json");
    let maintainers = glama["maintainers"].as_array().expect("maintainers array");
    assert!(maintainers.iter().any(|m| m == "fabiendupont"));
}

#[test]
fn docker_mcp_catalog_payload_exists() {
    let path = repo_root("registry/docker-mcp-catalog.yaml");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    assert!(content.contains("name: mcp-google-workspace"));
    assert!(content.contains("fabiendupont/mcp-google-workspace"));
}
