//! Regression tests for issue #26 — `{type: object, additionalProperties:
//! false}` with no `properties` block must not produce an empty struct
//! that silently drops the response body. Two layers of pinning:
//!
//!  1. Codegen shape: assert what `models.rs` does (and *doesn't*)
//!     contain, so future churn that re-introduces the empty struct
//!     fails immediately rather than at deserialize time.
//!  2. Wire-level deserialization: drive the generated operation with
//!     a real server payload (multiple fields, mixed value types) and
//!     verify the body survives — pre-fix it would collapse to `{}`.

use compile_test::gen;
use compile_test::gen::operations::{GetInline, GetInlineOutput, GetOpaque, GetOpaqueOutput};
use http_body_util::Full;
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

/// Codegen shape: pre-fix, the named `Opaque` schema landed in
/// `models.rs` as `pub struct Opaque {}` and the inline response shape
/// got a synthesized hoisted empty struct. Pin both halves:
///  - no `pub struct Opaque {…}` left over (the named-case symbol);
///  - no `Response` hoisted empty struct from the inline case.
///
/// The fix inlines both as `serde_json::Value` at the use site, the
/// same direction #20 took for `additionalProperties: {}`.
#[test]
fn closed_empty_schema_emits_no_empty_struct() {
    let models = include_str!("../../out/models.rs");

    assert!(
        !models.contains("pub struct Opaque"),
        "named `Opaque` (additionalProperties: false, no properties) \
         should be inlined as serde_json::Value, not emitted as a struct:\n{models}"
    );

    // The original bug: an empty `pub struct Foo {}` parsing every wire
    // payload into nothing. Guard against the shape returning under any
    // name (the parser sometimes hoists inline response bodies under a
    // generated id).
    assert!(
        !models.contains("pub struct") || !models.contains("{}\n"),
        "no fieldless `pub struct … {{}}` should be emitted — every empty \
         shape in this fixture must inline as serde_json::Value:\n{models}"
    );
}

/// The exact failure mode from the bug report: the server returns a
/// JSON object with several fields, the client must surface what it
/// actually got — not collapse it to `{}`. Pre-fix the response body
/// deserialized into a zero-field struct, so `serde_json::to_value(&got)`
/// produced `{}` and the original payload was lost. Post-fix the
/// response type is `serde_json::Value` and the body round-trips.
#[tokio::test]
async fn opaque_response_surfaces_full_body() {
    let wire = br#"{"name":"foo","payload":"...","flag":true}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetOpaqueOutput::Ok(got) = gen::execute(&mut svc, GetOpaque {}).await.unwrap();

    // The whole point of the issue: the body must not be silently
    // dropped to `{}`. Compare structurally — string compare would be
    // fragile across HashMap iteration orders if any layer were a map.
    let expected: serde_json::Value =
        serde_json::from_slice(wire).expect("test wire must be valid JSON");
    assert_eq!(serde_json::to_value(&got).unwrap(), expected);
}

/// The inline variant of the same shape. The original issue is written
/// against an inline response schema; mirror that exactly so a future
/// regression that fixes the named case but leaves the inline case
/// broken (or vice-versa) fails this test.
#[tokio::test]
async fn inline_response_surfaces_full_body() {
    let wire = br#"{"hello":"world","n":42,"arr":[1,2,3]}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(ok_json(wire))
    });
    let GetInlineOutput::Ok(got) = gen::execute(&mut svc, GetInline {}).await.unwrap();

    let expected: serde_json::Value =
        serde_json::from_slice(wire).expect("test wire must be valid JSON");
    assert_eq!(serde_json::to_value(&got).unwrap(), expected);
}
