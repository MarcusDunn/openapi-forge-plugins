//! Runtime tests for the hand-written "any JSON" union pattern.
//!
//! The fixture's `JsonValue` schema is a 6-variant `oneOf` whose
//! array/object branches recurse back into `JsonValue`. The rust-tower
//! generator emits this as a `#[serde(untagged)]` enum with one Rust
//! variant per `oneOf` branch — no `serde_json::Value` indirection — so
//! callers see typed Rust shapes that round-trip the wire format.
//!
//! These tests pin three things:
//!  - the codegen shape (assert against the generated source);
//!  - every `JsonValue` variant decodes correctly through the generated
//!    operation;
//!  - the `HashMap<String, JsonValue>` field inside `Envelope`
//!    round-trips a real wire payload.

use compile_test::gen;
use compile_test::gen::models::{Envelope, JsonValue};
use compile_test::gen::operations::{GetValue, GetValueOutput, PostEnvelope, PostEnvelopeOutput};
use http_body_util::{BodyExt, Full};
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

/// Codegen shape: the `oneOf` lands as a `#[serde(untagged)]` enum with
/// one Rust variant per `oneOf` branch, not a `pub type ... =
/// serde_json::Value` alias and not a silent fallback placeholder.
#[test]
fn jsonvalue_emitted_as_untagged_enum() {
    let models = include_str!("../../out/models.rs");
    assert!(
        models.contains("#[serde(untagged)]"),
        "missing #[serde(untagged)] on JsonValue:\n{models}"
    );
    assert!(
        models.contains("pub enum JsonValue"),
        "expected `pub enum JsonValue` definition:\n{models}"
    );
    for variant in ["String(", "Number(", "Bool(", "Array(", "Object(", "Null"] {
        assert!(
            models.contains(variant),
            "expected variant `{variant}` in JsonValue:\n{models}"
        );
    }
    assert!(
        !models.contains("serde_json::Value"),
        "JsonValue should not fall back to serde_json::Value:\n{models}"
    );
}

/// Every primitive variant decodes through the untagged enum's serde
/// machinery. `Null` is the load-bearing case — unit variants in untagged
/// enums serialize/deserialize via `serialize_unit` (i.e. JSON `null`),
/// which is what the spec demands.
#[tokio::test]
async fn get_value_decodes_all_primitive_variants() {
    let cases: Vec<(&[u8], JsonValue)> = vec![
        (b"null", JsonValue::Null),
        (br#""hi""#, JsonValue::String("hi".into())),
        (b"true", JsonValue::Bool(true)),
        (b"42", JsonValue::Number(42.0)),
        (b"3.14", JsonValue::Number(3.14)),
    ];
    for (wire, expected) in cases {
        let body = wire.to_vec();
        let mut svc = tower::service_fn(move |_req: http::Request<ReqBody>| {
            let body = body.clone();
            async move { Ok::<_, Infallible>(ok_json(&body)) }
        });
        let out = gen::execute(&mut svc, GetValue {}).await.unwrap();
        let GetValueOutput::Ok(got) = out;
        assert_eq!(got, expected, "wire={}", std::str::from_utf8(wire).unwrap());
    }
}

/// Recursive variants: an array of mixed JsonValues, and an object whose
/// values are themselves JsonValues. The generator emits
/// `Array(Vec<JsonValue>)` and `Object(HashMap<String, JsonValue>)`, both
/// of which heap-allocate without needing `Box`.
#[tokio::test]
async fn get_value_decodes_recursive_variants() {
    let wire = br#"[1,"two",null,[true]]"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetValueOutput::Ok(got) = gen::execute(&mut svc, GetValue {}).await.unwrap();
    let JsonValue::Array(items) = got else {
        panic!("expected Array variant");
    };
    assert_eq!(items.len(), 4);
    assert!(matches!(items[0], JsonValue::Number(n) if (n - 1.0).abs() < f64::EPSILON));
    assert!(matches!(&items[1], JsonValue::String(s) if s == "two"));
    assert!(matches!(items[2], JsonValue::Null));
    let JsonValue::Array(ref inner) = items[3] else {
        panic!("expected nested Array variant");
    };
    assert!(matches!(inner[0], JsonValue::Bool(true)));

    let wire = br#"{"k":"v","n":7}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetValueOutput::Ok(got) = gen::execute(&mut svc, GetValue {}).await.unwrap();
    let JsonValue::Object(map) = got else {
        panic!("expected Object variant");
    };
    assert!(matches!(&map["k"], JsonValue::String(s) if s == "v"));
    assert!(matches!(map["n"], JsonValue::Number(n) if (n - 7.0).abs() < f64::EPSILON));
}

/// Serialization round-trip: every variant must serialize back to its
/// wire form. `Null` in particular must produce JSON `null`, not the
/// string `"Null"` (which is what an untagged unit variant would emit if
/// serde didn't override the unit-variant serialization in untagged
/// mode).
#[test]
fn jsonvalue_serializes_to_wire_form() {
    let cases: Vec<(JsonValue, &str)> = vec![
        (JsonValue::Null, "null"),
        (JsonValue::String("hi".into()), "\"hi\""),
        (JsonValue::Bool(true), "true"),
        (JsonValue::Number(2.5), "2.5"),
    ];
    for (val, wire) in cases {
        let encoded = serde_json::to_string(&val).unwrap();
        assert_eq!(encoded, wire, "value={val:?}");
    }
}

/// The driving use case: `extras: HashMap<String, JsonValue>` carries an
/// open-ended bag of per-key JSON values. The server echoes the payload
/// — the client must serialize *and* deserialize it without losing
/// structure.
///
/// Numeric values use a decimal point on the wire because the spec
/// declares the `number` variant only (no `integer`), so the IR maps it
/// to `f64` and `1` would round-trip as `1.0`. Users who want
/// integer-vs-float fidelity should add an explicit `{"type": "integer"}`
/// branch to the union.
#[tokio::test]
async fn envelope_extras_roundtrip() {
    let wire = br#"{"name":"flow-42","extras":{"int":7.0,"str":"hi","arr":[1.0,2.0,3.0],"nested":{"k":null}}}"#;
    let mut svc = tower::service_fn(|req: http::Request<ReqBody>| async move {
        let body = req.into_body().collect().await.unwrap().to_bytes();
        // Compare structurally — JSON object key ordering on the wire
        // depends on HashMap iteration order which isn't deterministic.
        let sent: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let expected: serde_json::Value = serde_json::from_slice(wire).unwrap();
        assert_eq!(sent, expected, "request body did not match");
        Ok::<_, Infallible>(ok_json(wire))
    });

    let mut extras: HashMap<String, JsonValue> = HashMap::new();
    extras.insert("int".into(), JsonValue::Number(7.0));
    extras.insert("str".into(), JsonValue::String("hi".into()));
    extras.insert(
        "arr".into(),
        JsonValue::Array(vec![
            JsonValue::Number(1.0),
            JsonValue::Number(2.0),
            JsonValue::Number(3.0),
        ]),
    );
    let mut nested: HashMap<String, JsonValue> = HashMap::new();
    nested.insert("k".into(), JsonValue::Null);
    extras.insert("nested".into(), JsonValue::Object(nested));

    let op = PostEnvelope {
        body: Envelope {
            name: "flow-42".into(),
            extras,
        },
    };
    let PostEnvelopeOutput::Ok(echoed) = gen::execute(&mut svc, op).await.unwrap();
    assert_eq!(echoed.name, "flow-42");
    assert_eq!(echoed.extras["int"], JsonValue::Number(7.0));
    assert_eq!(echoed.extras["nested"], {
        let mut m: HashMap<String, JsonValue> = HashMap::new();
        m.insert("k".into(), JsonValue::Null);
        JsonValue::Object(m)
    });
}
