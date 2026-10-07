//! OpenAPI contract tests — validates that the Rust API structs in src/repo_client.rs
//! and src/errors.rs are compatible with the server's OpenAPI specification.
//!
//! Requires `openapi.yaml` in the project root, or set `OPENAPI_SCHEMA` env var.
//!
//! In CI the schema is downloaded from the server repo before these tests run.
//! Locally:
//!   curl -fsSL \
//!     https://raw.githubusercontent.com/opentreecz/openrepo/main/schema/openapi.yaml \
//!     -o openapi.yaml

use serde_json::Value;

/// Load and parse the OpenAPI spec.
/// Returns `None` (and prints a skip message) when the file is absent,
/// so local `cargo test` does not fail without the schema file.
fn load_spec() -> Option<Value> {
    let path = std::env::var("OPENAPI_SCHEMA").unwrap_or_else(|_| "openapi.yaml".to_string());
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let v: Value = serde_yaml::from_str(&content)
                .unwrap_or_else(|e| panic!("openapi.yaml is not valid YAML: {e}"));
            Some(v)
        }
        Err(_) => {
            eprintln!(
                "SKIP: openapi.yaml not found at '{path}'.\n\
                 Download: curl -fsSL \
                 https://raw.githubusercontent.com/opentreecz/openrepo/main/schema/openapi.yaml \
                 -o openapi.yaml"
            );
            None
        }
    }
}

/// Resolve a JSON Reference like `#/components/schemas/Foo` within `spec`.
fn resolve_ref<'a>(spec: &'a Value, reference: &str) -> &'a Value {
    reference
        .trim_start_matches('#')
        .split('/')
        .filter(|s| !s.is_empty())
        .fold(spec, |cur, key| &cur[key])
}

/// If `node` is `{"$ref": "..."}`, resolve it; otherwise return `node` as-is.
fn deref<'a>(spec: &'a Value, node: &'a Value) -> &'a Value {
    if let Some(r) = node.get("$ref").and_then(Value::as_str) {
        resolve_ref(spec, r)
    } else {
        node
    }
}

fn assert_has_property(spec: &Value, schema_node: &Value, property: &str, context: &str) {
    let schema = deref(spec, schema_node);
    assert!(
        schema["properties"].get(property).is_some(),
        "{context}: schema is missing expected property '{property}'.\nSchema: {schema:#}"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// All endpoint paths used by openrepo-sync must exist in the spec.
#[test]
fn required_endpoint_paths_exist() {
    let Some(spec) = load_spec() else { return };

    let paths = spec["paths"]
        .as_object()
        .expect("spec['paths'] is not a YAML mapping");

    let required = [
        "/api/whoami",
        "/api/{repo_uid}/upload/",
        "/api/upload-status/{task_id}/",
        "/api/{repo_uid}/packages/",
        "/api/{repo_uid}/pkg/{package_uid}/",
    ];

    for path in required {
        assert!(
            paths.contains_key(path),
            "Required path is missing from the OpenAPI spec: {path}"
        );
    }
}

/// `UploadResponse` in src/repo_client.rs: `{ task_id: String }`
/// Spec: POST /api/{repo_uid}/upload/ → 202 → $ref UploadResponse → { task_id }
#[test]
fn upload_response_has_task_id() {
    let Some(spec) = load_spec() else { return };

    let schema = &spec["paths"]["/api/{repo_uid}/upload/"]["post"]["responses"]["202"]
        ["content"]["application/json"]["schema"];

    assert_has_property(
        &spec,
        schema,
        "task_id",
        "UploadResponse (POST /api/{repo_uid}/upload/ → 202)",
    );
}

/// `UploadStatusResponse` in src/repo_client.rs: `{ status, error_message, error_code }`
/// Spec: GET /api/upload-status/{task_id}/ → 200 → $ref UploadTask
#[test]
fn upload_task_has_status_and_error_fields() {
    let Some(spec) = load_spec() else { return };

    let schema = &spec["paths"]["/api/upload-status/{task_id}/"]["get"]["responses"]["200"]
        ["content"]["application/json"]["schema"];

    for field in ["status", "error_message", "error_code"] {
        assert_has_property(
            &spec,
            schema,
            field,
            &format!("UploadStatusResponse (GET /api/upload-status/{{task_id}}/ → 200) field '{field}'"),
        );
    }
}

/// `PaginatedResponse<ApiPackage>` in src/repo_client.rs:
///   outer: `{ results: Vec<ApiPackage>, next: Option<String> }`
///   inner: `{ package_uid, package_name, filename, architecture, version }`
/// Spec: GET /api/{repo_uid}/packages/ → 200 → $ref PaginatedPackageSummaryList
///       → results[*] → $ref PackageSummary
#[test]
fn packages_response_envelope_and_item_fields() {
    let Some(spec) = load_spec() else { return };

    let envelope_node = &spec["paths"]["/api/{repo_uid}/packages/"]["get"]["responses"]["200"]
        ["content"]["application/json"]["schema"];
    let envelope = deref(&spec, envelope_node);

    // PaginatedResponse outer fields
    for field in ["results", "next"] {
        assert!(
            envelope["properties"].get(field).is_some(),
            "PaginatedResponse (packages 200) missing field '{field}'"
        );
    }

    // ApiPackage inner fields (items inside results)
    let items_node = &envelope["properties"]["results"]["items"];
    for field in ["package_uid", "package_name", "filename", "architecture", "version"] {
        assert_has_property(
            &spec,
            items_node,
            field,
            &format!("ApiPackage (PackageSummary items) field '{field}'"),
        );
    }
}

/// `UserResponse` in src/repo_client.rs: `{ username: String }`
/// Spec: GET /api/whoami → 200 → $ref UserDetail → { username, ... }
#[test]
fn whoami_response_has_username() {
    let Some(spec) = load_spec() else { return };

    let schema =
        &spec["paths"]["/api/whoami"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"];

    assert_has_property(
        &spec,
        schema,
        "username",
        "UserResponse (GET /api/whoami → 200)",
    );
}

/// `ApiError` in src/errors.rs: `{ code: String, detail: String, status: u16 }`
/// Spec: components/schemas/ErrorResponse → { code, detail, status }
#[test]
fn error_response_component_has_required_fields() {
    let Some(spec) = load_spec() else { return };

    let schema = spec["components"]["schemas"]
        .get("ErrorResponse")
        .expect("components/schemas/ErrorResponse not found in spec");

    for field in ["code", "detail", "status"] {
        assert!(
            schema["properties"].get(field).is_some(),
            "ApiError (ErrorResponse component) missing field '{field}'"
        );
    }
}
