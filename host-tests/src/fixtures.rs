//! Minimal `Ir` fixtures usable by any generator plugin's tests.
//!
//! Each fixture is a `serde_json::Value` deserialised into an `Ir` —
//! lets us spell out only the fields a given test cares about and let
//! the IR's default-empty serde annotations fill in the rest. Plugin-
//! specific fixtures live in the plugin's own test file.

use forge_ir::Ir;
use serde_json::{json, Value};

pub fn from_json(v: Value) -> Ir {
    serde_json::from_value(v).expect("deserialise Ir from fixture")
}

/// Smallest spec every generator should handle without diagnostics:
/// one no-body GET that returns 204.
pub fn ir_minimal() -> Ir {
    from_json(json!({
        "info": { "title": "Test API", "version": "1.0.0" },
        "operations": [{
            "id": "ping",
            "method": "get",
            "path_template": "/ping",
            "responses": [
                { "status": { "kind": "explicit", "code": 204 } }
            ]
        }],
        "types": [],
        "security_schemes": [],
        "servers": [{ "url": "https://example.com" }]
    }))
}

/// POST /pets with a body and two response variants. Drives any
/// code path that emits request-body handling and response-status
/// branching.
pub fn ir_with_body() -> Ir {
    from_json(json!({
        "info": { "title": "Pets", "version": "1.0.0" },
        "operations": [{
            "id": "createPet",
            "method": "post",
            "path_template": "/pets",
            "request_body": {
                "required": true,
                "content": [{
                    "media_type": "application/json",
                    "type": "Pet"
                }]
            },
            "responses": [
                {
                    "status": { "kind": "explicit", "code": 201 },
                    "content": [{ "media_type": "application/json", "type": "Pet" }]
                },
                {
                    "status": { "kind": "explicit", "code": 400 },
                    "content": [{ "media_type": "application/json", "type": "Error" }]
                }
            ]
        }],
        "types": [
            {
                "id": "Pet",
                "definition": {
                    "def": "object",
                    "properties": [
                        { "name": "id",   "type": "Pet.id",   "required": true },
                        { "name": "name", "type": "Pet.name", "required": true }
                    ],
                    "additional_properties": { "kind": "forbidden" },
                    "constraints": {}
                }
            },
            {
                "id": "Pet.id",
                "definition": { "def": "primitive", "kind": "string", "constraints": {} }
            },
            {
                "id": "Pet.name",
                "definition": { "def": "primitive", "kind": "string", "constraints": {} }
            },
            {
                "id": "Error",
                "definition": {
                    "def": "object",
                    "properties": [
                        { "name": "message", "type": "Error.message", "required": true }
                    ],
                    "additional_properties": { "kind": "any" },
                    "constraints": {}
                }
            },
            {
                "id": "Error.message",
                "definition": { "def": "primitive", "kind": "string", "constraints": {} }
            }
        ],
        "security_schemes": [],
        "servers": [{ "url": "https://example.com" }]
    }))
}
