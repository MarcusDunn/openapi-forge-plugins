//! Host-side tests specific to `generator-rust-clap`. Built around the
//! emit shapes the clap plugin owns end-to-end — the clap-derive CLI
//! struct, `BODY_SCHEMA_*` / `RESPONSE_SCHEMA_*` constants, the
//! schema-flag relaxation on required positionals, and the
//! long_about OAuth / tenancy gates.

use host_tests::fixtures::{from_json, ir_minimal, ir_with_body};
use host_tests::plugins::CLAP;
use host_tests::{file_named, paths};
use serde_json::json;

// ---------------------------------------------------------------------------
// Plugin-specific fixtures
// ---------------------------------------------------------------------------

/// Body whose schema references itself. Exercises the `$defs`/`$ref`
/// recursion path in `BODY_SCHEMA_*`.
fn ir_recursive_type() -> forge_ir::Ir {
    from_json(json!({
        "info": { "title": "Tree", "version": "1.0.0" },
        "operations": [{
            "id": "addNode",
            "method": "post",
            "path_template": "/nodes",
            "request_body": {
                "required": true,
                "content": [{ "media_type": "application/json", "type": "Node" }]
            },
            "responses": []
        }],
        "types": [
            {
                "id": "Node",
                "definition": {
                    "def": "object",
                    "properties": [
                        { "name": "name",     "type": "Node.name",     "required": true },
                        { "name": "children", "type": "Node.children", "required": false }
                    ],
                    "additional_properties": { "kind": "forbidden" },
                    "constraints": {}
                }
            },
            {
                "id": "Node.name",
                "definition": { "def": "primitive", "kind": "string", "constraints": {} }
            },
            {
                "id": "Node.children",
                "definition": {
                    "def": "array",
                    "items": "Node",
                    "constraints": {}
                }
            }
        ],
        "security_schemes": [],
        "servers": [{ "url": "https://example.com" }]
    }))
}

/// Required path positional alongside a body — triggers the
/// `required_unless_present_any = ["body_schema"]` relaxation so
/// `--body-schema` short-circuits before clap rejects the missing
/// positional. Regression from v0.0.14.
fn ir_path_param_with_body() -> forge_ir::Ir {
    from_json(json!({
        "info": { "title": "Files", "version": "1.0.0" },
        "operations": [{
            "id": "editFile",
            "method": "put",
            "path_template": "/files/{file_name}",
            "path_params": [
                { "name": "file_name", "type": "FileName", "required": true }
            ],
            "request_body": {
                "required": true,
                "content": [{ "media_type": "application/json", "type": "FileBody" }]
            },
            "responses": []
        }],
        "types": [
            {
                "id": "FileName",
                "definition": { "def": "primitive", "kind": "string", "constraints": {} }
            },
            {
                "id": "FileBody",
                "definition": { "def": "primitive", "kind": "string", "constraints": {} }
            }
        ],
        "security_schemes": [],
        "servers": [{ "url": "https://example.com" }]
    }))
}

/// OAuth2 + `x-token-exchange` + tenant-scoped op. Drives the
/// long_about's auth + tenancy sections.
fn ir_oauth_with_tenancy() -> forge_ir::Ir {
    from_json(json!({
        "info": { "title": "Multi", "version": "1.0.0" },
        "operations": [{
            "id": "listThings",
            "method": "get",
            "path_template": "/org/{tenant}/things",
            "path_params": [
                { "name": "tenant", "type": "Tenant", "required": true }
            ],
            "security": [
                { "scheme_id": "oauth", "scopes": ["openid"] }
            ],
            "responses": []
        }],
        "types": [{
            "id": "Tenant",
            "definition": { "def": "primitive", "kind": "string", "constraints": {} }
        }],
        "security_schemes": [{
            "id": "oauth",
            "kind": {
                "type": "oauth2",
                "flows": [{
                    "kind": "authorization-code",
                    "authorization_url": "https://auth.example/authorize",
                    "token_url": "https://auth.example/token",
                    "scopes": [["openid", "OpenID"]]
                }]
            },
            "extensions": [
                ["x-token-exchange", 0]
            ]
        }],
        "servers": [{ "url": "https://example.com" }],
        "values": [
            { "kind": "object", "fields": [
                ["audience-template", 1]
            ]},
            { "kind": "string", "value": "urn:test:tenant:{tenant}" }
        ]
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn smoke_minimal_spec_emits_expected_file_set() {
    let out = CLAP.run(ir_minimal(), json!({}));
    let names = paths(&out);
    for expected in &[
        "Cargo.toml",
        "src/main.rs",
        "src/client.rs",
        "src/runtime.rs",
        "README.md",
    ] {
        assert!(
            names.contains(expected),
            "missing {expected:?} in output: {names:?}"
        );
    }
    assert!(
        out.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        out.diagnostics
    );
}

#[test]
fn body_schema_constant_is_emitted_when_op_has_a_body() {
    let out = CLAP.run(ir_with_body(), json!({}));
    let main_rs = file_named(&out, "src/main.rs");
    assert!(
        main_rs.contains("const BODY_SCHEMA_CREATE_PET"),
        "expected BODY_SCHEMA_CREATE_PET constant in main.rs"
    );
    assert!(main_rs.contains("https://json-schema.org/draft/2020-12/schema"));
}

#[test]
fn response_schema_is_a_flat_status_keyed_map() {
    let out = CLAP.run(ir_with_body(), json!({}));
    let main_rs = file_named(&out, "src/main.rs");
    assert!(
        main_rs.contains("const RESPONSE_SCHEMA_CREATE_PET"),
        "expected RESPONSE_SCHEMA_CREATE_PET constant in main.rs"
    );
    // Top-level keys are status codes + $defs, not wrapped under "properties".
    assert!(main_rs.contains("\\\"201\\\""));
    assert!(main_rs.contains("\\\"400\\\""));
    // No wrapper "type": "object" sitting above the status map.
    let resp_const = main_rs
        .split("RESPONSE_SCHEMA_CREATE_PET")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .unwrap_or("");
    let outer_type_object = resp_const.matches("\\\"type\\\": \\\"object\\\"").count();
    let status_count = 2; // 201, 400
    assert!(
        outer_type_object <= status_count,
        "looks like the response-schema regained its wrapper schema: {outer_type_object} occurrences"
    );
}

#[test]
fn required_path_positional_relaxes_when_schema_flag_present() {
    let out = CLAP.run(ir_path_param_with_body(), json!({}));
    let main_rs = file_named(&out, "src/main.rs");
    assert!(
        main_rs.contains("required_unless_present_any"),
        "expected required_unless_present_any on a positional with --body-schema"
    );
    assert!(
        main_rs.contains("\"body_schema\""),
        "expected body_schema in the unless list"
    );
    assert!(
        main_rs.contains("file_name.expect"),
        "expected file_name.expect(...) on the API-call branch"
    );
}

#[test]
fn recursive_type_uses_dollar_ref_in_body_schema() {
    let out = CLAP.run(ir_recursive_type(), json!({}));
    let main_rs = file_named(&out, "src/main.rs");
    assert!(
        main_rs.contains("BODY_SCHEMA_ADD_NODE"),
        "expected BODY_SCHEMA_ADD_NODE constant"
    );
    let const_blob = main_rs
        .split("BODY_SCHEMA_ADD_NODE")
        .nth(1)
        .and_then(|s| s.split("const ").next())
        .unwrap_or(main_rs);
    assert!(
        const_blob.contains("$ref") && const_blob.contains("$defs"),
        "expected $ref + $defs in recursive body schema"
    );
}

#[test]
fn long_about_includes_oauth_and_tenancy_when_active() {
    let out = CLAP.run(
        ir_oauth_with_tenancy(),
        json!({ "oauth": { "clientId": "test" } }),
    );
    let main_rs = file_named(&out, "src/main.rs");
    assert!(
        main_rs.contains("Authentication and profiles"),
        "long_about should mention auth/profiles when OAuth is active"
    );
    assert!(
        main_rs.contains("Multi-tenant operations"),
        "long_about should mention tenancy when x-token-exchange is present"
    );
    assert!(
        main_rs.contains("set-tenant"),
        "long_about should reference the set-tenant helper for the placeholder"
    );
}

#[test]
fn long_about_omits_oauth_section_when_oauth_is_off() {
    let out = CLAP.run(ir_minimal(), json!({}));
    let main_rs = file_named(&out, "src/main.rs");
    assert!(
        !main_rs.contains("Authentication and profiles"),
        "long_about should not mention auth/profiles when OAuth is inactive"
    );
    assert!(
        !main_rs.contains("Multi-tenant operations"),
        "long_about should not mention tenancy when no x-token-exchange"
    );
}
