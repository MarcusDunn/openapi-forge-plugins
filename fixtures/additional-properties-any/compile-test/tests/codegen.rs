//! Regression tests for issue #20 — `additionalProperties: {}` and
//! `additionalProperties: true` must not produce empty structs that
//! silently drop the map on the wire.
//!
//! Three layers of pinning:
//!  1. Codegen shape: assert what `models.rs` does (and *doesn't*)
//!     contain, so future churn that re-introduces the empty struct
//!     fails immediately rather than at deserialize time.
//!  2. Wire-level deserialization: drive the generated operation with
//!     payloads that match (and mismatch) the bug-report scenario.
//!  3. Round-trip: build the typed value, serialize it, compare to the
//!     original wire.

use compile_test::gen;
use compile_test::gen::models::Function;
use compile_test::gen::operations::{
    GetExplicitTrue, GetExplicitTrueOutput, GetFreeform, GetFreeformOutput, GetFunction,
    GetFunctionOutput,
};
use http_body_util::Full;
use std::collections::HashMap;
use std::convert::Infallible;

type ReqBody = Full<bytes::Bytes>;
type RespBody = Full<bytes::Bytes>;

fn ok_json(body: &[u8]) -> http::Response<RespBody> {
    http::Response::builder()
        .status(200)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(bytes::Bytes::copy_from_slice(body)))
        .unwrap()
}

/// Codegen shape: pre-fix, every `additionalProperties: {}` site landed
/// in `models.rs` as a fieldless `pub struct …AdditionalProperties {}`
/// (and the parent struct lost the map field entirely). Pin both halves
/// — the absent empty struct *and* the present `HashMap` field — so the
/// regression can't sneak back in via either path.
#[test]
fn function_struct_has_hashmap_fields_not_empty_struct() {
    let models = include_str!("../../out/models.rs");

    assert!(
        models.contains("pub parameters: std::collections::HashMap<String, serde_json::Value>"),
        "expected `parameters: HashMap<String, serde_json::Value>` field:\n{models}"
    );
    assert!(
        models.contains(
            "pub internal_parameters: std::collections::HashMap<String, serde_json::Value>"
        ),
        "expected `internal_parameters: HashMap<String, serde_json::Value>` field:\n{models}"
    );

    // The two halves of the bug:
    //  - no hoisted `…AdditionalProperties` empty struct (the symbol
    //    that appeared in the original deserializer error message);
    //  - no nested `HashMap<HashMap<...>>` (which is what a naive fix
    //    that only handled item #1 of the issue produces — it leaves
    //    the hoisted name resolving to a map-of-anything-objects rather
    //    than to `serde_json::Value`, and the inner level then rejects
    //    non-object wire values like the `""` in the bug report).
    assert!(
        !models.contains("AdditionalProperties"),
        "no `…AdditionalProperties` hoisted name should leak into models.rs:\n{models}"
    );
    assert!(
        !models.contains("HashMap<String, std::collections::HashMap"),
        "the `additionalProperties: {{}}` map value should resolve to `serde_json::Value`, \
         not a nested `HashMap<String, HashMap<...>>`:\n{models}"
    );
}

/// The exact failure mode from the bug report: the server returns a map
/// whose values are strings (not objects). Pre-fix this errored with
/// `invalid type: string "", expected struct …AdditionalProperties`.
/// Post-fix it must round-trip into a `serde_json::Value::String`.
#[tokio::test]
async fn function_deserializes_string_values_in_parameters() {
    let wire = br#"{"parameters":{"a":"","b":"hello"},"internalParameters":{"flag":"on"}}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetFunctionOutput::Ok(got) = gen::execute(&mut svc, GetFunction {}).await.unwrap();
    assert_eq!(got.parameters.len(), 2);
    assert_eq!(got.parameters["a"], serde_json::Value::String(String::new()));
    assert_eq!(got.parameters["b"], serde_json::Value::String("hello".into()));
    assert_eq!(
        got.internal_parameters["flag"],
        serde_json::Value::String("on".into())
    );
}

/// Mixed-type wire values: the spec only constrains *that* values
/// exist, not their type. `additionalProperties: {}` permits anything
/// JSON allows. Verify every primitive shape, including nested objects
/// and arrays.
#[tokio::test]
async fn function_deserializes_mixed_value_types() {
    let wire = br#"{
        "parameters": {
            "s": "txt",
            "n": 3.14,
            "i": 42,
            "b": true,
            "z": null,
            "arr": [1, 2, 3],
            "obj": {"k": "v"}
        },
        "internalParameters": {}
    }"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetFunctionOutput::Ok(got) = gen::execute(&mut svc, GetFunction {}).await.unwrap();
    assert!(matches!(got.parameters["s"], serde_json::Value::String(ref s) if s == "txt"));
    assert!(matches!(got.parameters["b"], serde_json::Value::Bool(true)));
    assert!(matches!(got.parameters["z"], serde_json::Value::Null));
    assert!(matches!(got.parameters["arr"], serde_json::Value::Array(ref a) if a.len() == 3));
    assert!(matches!(got.parameters["obj"], serde_json::Value::Object(ref o) if o.contains_key("k")));
    assert!(got.internal_parameters.is_empty());
}

/// Serialize the same value back out: the wire-level round-trip must
/// preserve the map structure. Pre-fix the field didn't exist at all,
/// so this would serialize to `{}` and lose everything.
#[test]
fn function_serializes_map_back_to_wire() {
    let mut parameters = HashMap::new();
    parameters.insert(
        "name".to_string(),
        serde_json::Value::String("flow-1".into()),
    );
    parameters.insert("count".to_string(), serde_json::Value::from(7));

    let value = Function {
        parameters,
        internal_parameters: HashMap::new(),
    };

    let encoded = serde_json::to_value(&value).unwrap();
    // Compare structurally — HashMap iteration order isn't stable, so
    // string comparison is fragile.
    let expected = serde_json::json!({
        "parameters": { "name": "flow-1", "count": 7 },
        "internalParameters": {}
    });
    assert_eq!(encoded, expected);
}

/// A named top-level open map (`{type: object, additionalProperties:
/// {}}`) must inline as `HashMap<String, serde_json::Value>` at the
/// use site — no `pub struct FreeformObject {}` left over.
#[tokio::test]
async fn freeform_object_decodes_as_hashmap() {
    let wire = br#"{"a":1,"b":"two","c":null}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetFreeformOutput::Ok(got) = gen::execute(&mut svc, GetFreeform {}).await.unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got["b"], serde_json::Value::String("two".into()));
    assert_eq!(got["c"], serde_json::Value::Null);

    let models = include_str!("../../out/models.rs");
    assert!(
        !models.contains("FreeformObject"),
        "FreeformObject should be inlined as HashMap, not emitted as a struct:\n{models}"
    );
}

/// `additionalProperties: true` lowers to `Object{props:[], AP=Any}` in
/// the IR — same shape as a bare `{}` schema. The IR carries no
/// constraint that values be objects, so the use site must resolve to
/// `serde_json::Value`. A `HashMap` rendering would over-restrict and
/// reject e.g. a top-level wire string.
#[tokio::test]
async fn explicit_true_decodes_any_json_value() {
    // Top-level string is valid JSON; with the right typing the
    // operation accepts it.
    let wire = br#""hello""#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetExplicitTrueOutput::Ok(got) = gen::execute(&mut svc, GetExplicitTrue {}).await.unwrap();
    assert_eq!(got, serde_json::Value::String("hello".into()));

    let models = include_str!("../../out/models.rs");
    assert!(
        !models.contains("ExplicitTrueObject"),
        "ExplicitTrueObject should be inlined, not emitted as a struct:\n{models}"
    );
}
