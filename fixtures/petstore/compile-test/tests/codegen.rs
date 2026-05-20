//! Runtime tests targeting the *generated* petstore client.
//!
//! Each module covers one bug class. They use `tower::service_fn` to stand
//! in for the real HTTP service — every emitted Operation routes through
//! `runtime::execute(&mut svc, op).await`, so a closure that inspects the
//! `http::Request` and returns a synthetic `http::Response` exercises both
//! `into_http_request` and `parse_response`.

use compile_test::gen;
use compile_test::gen::models::{Error, NullabilityShowcase, Pet, PetMood, PetStatus, Pets};
use compile_test::gen::operations::{
    DownloadPetPhoto, DownloadPetPhotoOutput, FindPetsByTag, FindPetsByTagOutput, GetPetProblem,
    GetPetProblemOutput, ListPets, ListPetsOutput, ReplacePet, ReplacePetOutput,
};
use compile_test::gen::Operation;
use http_body_util::{BodyExt, Full};
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

fn status_json(status: u16, body: &[u8]) -> http::Response<RespBody> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(bytes::Bytes::copy_from_slice(body)))
        .unwrap()
}

/// Bug #1: mixed-case string enum variants round-trip via explicit
/// `#[serde(rename)]` annotations.
#[test]
fn string_enum_mixed_case_roundtrip() {
    let enc = serde_json::to_string(&PetStatus::InProgress).unwrap();
    assert_eq!(enc, r#""inProgress""#);
    let dec: PetStatus = serde_json::from_str(r#""inProgress""#).unwrap();
    assert_eq!(dec, PetStatus::InProgress);

    // Sanity: every wire value decodes to its expected variant.
    for (wire, variant) in [
        ("available", PetStatus::Available),
        ("inProgress", PetStatus::InProgress),
        ("sold", PetStatus::Sold),
    ] {
        let json = format!(r#""{wire}""#);
        let got: PetStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(got, variant);
    }
}

/// Bug #2: integer enums round-trip across positive, zero, and negative
/// discriminants without collision. (The pre-fix `.abs()` naming collapsed
/// `1` and `-1` to the same `V1` variant.)
#[test]
fn int_enum_negative_roundtrip() {
    for (wire, variant) in [(-1_i64, PetMood::VNeg1), (0, PetMood::V0), (1, PetMood::V1)] {
        let enc = serde_json::to_string(&variant).unwrap();
        assert_eq!(enc, wire.to_string());
        let dec: PetMood = serde_json::from_str(&wire.to_string()).unwrap();
        assert_eq!(dec, variant);
    }
}

/// Bug #5: array-shaped query parameters encode as repeated `key=value`
/// pairs (OpenAPI 3 `style=form, explode=true` default).
#[tokio::test]
async fn array_query_param_repeats_key() {
    let mut svc = tower::service_fn(|req: http::Request<ReqBody>| async move {
        let uri = req.uri().to_string();
        // Three pairs, in spec order: tag=cat, tag=dog, tag=fish.
        assert_eq!(uri, "/pets/findByTag?tag=cat&tag=dog&tag=fish");
        Ok::<_, Infallible>(ok_json(b"[]"))
    });
    let op = FindPetsByTag {
        tag: vec!["cat".into(), "dog".into(), "fish".into()],
    };
    let out = gen::execute(&mut svc, op).await.unwrap();
    assert!(matches!(out, FindPetsByTagOutput::Ok(ref pets) if pets.is_empty()));
}

/// Bug #3: spec declares both `400` and `4XX`. The generator must emit
/// match arms with `400` ahead of `400..=499`, otherwise `400` would be
/// shadowed and never reachable. Verify the specific arm wins.
#[tokio::test]
async fn response_explicit_400_beats_4xx_range() {
    let body = br#"{"code":400,"message":"specific"}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(status_json(400, body))
    });
    let op = ReplacePet {
        pet_id: "42".into(),
        body: Pet {
            id: 42,
            name: "Mittens".into(),
            mood: None,
            status: None,
            tag: None,
        },
    };
    let out = gen::execute(&mut svc, op).await.unwrap();
    let ReplacePetOutput::BadRequest(err) = out else {
        panic!("expected the BadRequest arm to win over 4XX, got {out:?}");
    };
    assert_eq!(err.code, 400);
    assert_eq!(err.message, "specific");
}

/// Same operation, a 4xx that *isn't* 400 — falls through to the range arm.
#[tokio::test]
async fn response_other_4xx_falls_to_range() {
    let body = br#"{"code":418,"message":"teapot"}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(status_json(418, body))
    });
    let op = ReplacePet {
        pet_id: "42".into(),
        body: Pet {
            id: 42,
            name: "Mittens".into(),
            mood: None,
            status: None,
            tag: None,
        },
    };
    let out = gen::execute(&mut svc, op).await.unwrap();
    let ReplacePetOutput::FourXx(err) = out else {
        panic!("expected the 4XX range arm, got {out:?}");
    };
    assert_eq!(err.code, 418);
}

/// Bug #11: per-operation types are re-exported at `gen::operations` root,
/// so callers don't have to know the snake-case submodule path. The
/// imports at the top of this file already exercise this — assert here
/// that the types are reachable through that path (a compile-time check
/// disguised as a runtime test).
#[test]
fn operations_root_reexports() {
    fn assert_op<O: Operation>() {}
    assert_op::<ListPets>();
    assert_op::<FindPetsByTag>();
    assert_op::<ReplacePet>();
    assert_op::<GetPetProblem>();
}

/// Bug #13: the response is `application/problem+json`. The decoder
/// doesn't actually inspect headers — `parse_response` matches on
/// `status.as_u16()` — so this test serves more as a smoke check that the
/// generator picked up the `+json` content-type at codegen time and gave
/// the 200 arm a structured body type. If the matcher had stayed strict,
/// the 200 variant would have ended up as a bare unit `Ok` with no body.
#[tokio::test]
async fn problem_plus_json_content_type_decoded() {
    let body = br#"{"code":404,"message":"not found"}"#;
    let mut svc = tower::service_fn(|req: http::Request<ReqBody>| async move {
        assert_eq!(req.uri().path(), "/pets/42/problem");
        Ok::<_, Infallible>(
            http::Response::builder()
                .status(200)
                .header(http::header::CONTENT_TYPE, "application/problem+json")
                .body(Full::new(bytes::Bytes::copy_from_slice(body)))
                .unwrap(),
        )
    });
    let op = GetPetProblem {
        pet_id: "42".into(),
    };
    let out = gen::execute(&mut svc, op).await.unwrap();
    let GetPetProblemOutput::Ok(err) = out;
    assert_eq!(err.code, 404);
}

/// Sanity: the JSON `application/json` request path still works (the
/// `is_json_media_type` change shouldn't have regressed plain JSON).
#[tokio::test]
async fn create_pet_serializes_json_body() {
    use compile_test::gen::operations::{CreatePet, CreatePetOutput};

    let mut svc = tower::service_fn(|req: http::Request<ReqBody>| async move {
        assert_eq!(req.method(), http::Method::POST);
        assert_eq!(
            req.headers().get(http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let pet: Pet = serde_json::from_slice(&body).unwrap();
        assert_eq!(pet.name, "Buddy");
        Ok::<_, Infallible>(
            http::Response::builder()
                .status(201)
                .body(Full::new(bytes::Bytes::new()))
                .unwrap(),
        )
    });
    let op = CreatePet {
        body: Pet {
            id: 1,
            name: "Buddy".into(),
            mood: Some(PetMood::V1),
            status: Some(PetStatus::Available),
            tag: None,
        },
    };
    let out = gen::execute(&mut svc, op).await.unwrap();
    assert!(matches!(out, CreatePetOutput::Created));
}

/// `Pets` (a top-level array alias) should round-trip too.
#[test]
fn pets_array_alias_roundtrip() {
    let pets: Pets = vec![Pet {
        id: 1,
        name: "Rex".into(),
        mood: None,
        status: None,
        tag: Some("dog".into()),
    }];
    let json = serde_json::to_string(&pets).unwrap();
    let back: Pets = serde_json::from_str(&json).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].name, "Rex");
}

/// `ListPets` returns either the `Ok` page or the `Default` error. Verify
/// the default arm is reachable for a status not in the explicit list.
#[tokio::test]
async fn list_pets_default_response_for_unexpected_status() {
    let body = br#"{"code":500,"message":"boom"}"#;
    let mut svc = tower::service_fn(|_req: http::Request<ReqBody>| async move {
        Ok::<_, Infallible>(status_json(500, body))
    });
    let op = ListPets { limit: Some(10) };
    let out = gen::execute(&mut svc, op).await.unwrap();
    let ListPetsOutput::Default(err) = out else {
        panic!("expected Default arm, got {out:?}");
    };
    assert_eq!(err.code, 500);
    assert_eq!(err.message, "boom");
}

/// Bug #19: a binary-body 2xx response (`image/png`) must skip the
/// `JsonOperation` path the rest of the petstore takes — otherwise the
/// generated `parse_status` would attempt to `serde_json::from_slice`
/// the bytes and reject anything that isn't valid JSON. The body of
/// this test is non-JSON bytes; if the streaming-op heuristic misfires
/// and emits `JsonOperation`, this test fails at decode time.
///
/// This also exercises the compile-time invariant: `DownloadPetPhoto`
/// implements `Operation` *without* `JsonOperation`, so the blanket
/// impl in `runtime.rs` doesn't apply and the body stays out of
/// JSON-decode paths.
#[tokio::test]
async fn streaming_op_skips_json_decode() {
    let png_signature = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let body = png_signature.to_vec();
    let mut svc = tower::service_fn(move |_req: http::Request<ReqBody>| {
        let body = body.clone();
        async move {
            Ok::<_, Infallible>(
                http::Response::builder()
                    .status(200)
                    .header(http::header::CONTENT_TYPE, "image/png")
                    .body(Full::new(bytes::Bytes::from(body)))
                    .unwrap(),
            )
        }
    });
    let op = DownloadPetPhoto {
        pet_id: "42".into(),
    };
    let out = gen::execute(&mut svc, op).await.expect(
        "non-JSON 200 body must not be JSON-decoded; if this errored with a serde \
         decode failure the streaming-op heuristic regressed and the op took the \
         JsonOperation path",
    );
    assert!(matches!(out, DownloadPetPhotoOutput::Ok));
}

/// Bug #20: `nullable: true` on an *optional* property must preserve the
/// missing-vs-explicit-null distinction. The generator emits
/// `Option<Option<T>>` for that case, gated by a `deserialize_with`
/// helper, so:
///   - field missing from wire → outer `None`
///   - field present as `null` → `Some(None)`
///   - field present with value → `Some(Some(value))`
///
/// The other three (required × nullable) cross-products keep their
/// natural shapes (`T`, `Option<T>` no-skip, `Option<T>` with skip).
/// All four exercised below.
#[test]
fn nullability_field_shapes_roundtrip() {
    // ---- deserialize: every wire form decodes to the right state ----

    // All three optional+nullable variants present.
    let v: NullabilityShowcase = serde_json::from_str(
        r#"{"always_present":"x","missing_allowed":"y","nullable_required":"z","tristate":"q"}"#,
    )
    .unwrap();
    assert_eq!(v.always_present, "x");
    assert_eq!(v.missing_allowed.as_deref(), Some("y"));
    assert_eq!(v.nullable_required.as_deref(), Some("z"));
    assert!(matches!(v.tristate, Some(Some(ref s)) if s == "q"));

    // Optional fields missing; required nullable is null; tristate
    // explicit-null (distinct from missing!).
    let v: NullabilityShowcase = serde_json::from_str(
        r#"{"always_present":"x","nullable_required":null,"tristate":null}"#,
    )
    .unwrap();
    assert_eq!(v.missing_allowed, None);
    assert_eq!(v.nullable_required, None);
    assert!(
        matches!(v.tristate, Some(None)),
        "explicit `null` must decode as Some(None), not the missing-case None; got {:?}",
        v.tristate
    );

    // tristate missing entirely (vs explicit null above).
    let v: NullabilityShowcase = serde_json::from_str(
        r#"{"always_present":"x","nullable_required":null}"#,
    )
    .unwrap();
    assert_eq!(
        v.tristate, None,
        "missing tristate must decode as the outer None"
    );

    // Required field missing → decode error.
    serde_json::from_str::<NullabilityShowcase>(
        r#"{"missing_allowed":"y","nullable_required":null}"#,
    )
    .expect_err("`always_present` is required; missing it must fail decode");

    // ---- serialize: each Rust state writes the right wire shape ----

    // missing_allowed=None → field omitted;
    // nullable_required=Some("z") → "z";
    // tristate=Some(None) → null.
    let v = NullabilityShowcase {
        always_present: "x".into(),
        missing_allowed: None,
        nullable_required: Some("z".into()),
        tristate: Some(None),
    };
    let s = serde_json::to_string(&v).unwrap();
    assert!(s.contains(r#""always_present":"x""#), "{s}");
    assert!(
        !s.contains("missing_allowed"),
        "optional+non-nullable None must be omitted: {s}"
    );
    assert!(s.contains(r#""nullable_required":"z""#), "{s}");
    assert!(
        s.contains(r#""tristate":null"#),
        "tristate=Some(None) must serialize as explicit null: {s}"
    );

    // tristate=None → field omitted; nullable_required=None → null.
    let v = NullabilityShowcase {
        always_present: "x".into(),
        missing_allowed: None,
        nullable_required: None,
        tristate: None,
    };
    let s = serde_json::to_string(&v).unwrap();
    assert!(
        !s.contains("tristate"),
        "tristate=None must be omitted (missing on the wire): {s}"
    );
    assert!(
        s.contains(r#""nullable_required":null"#),
        "required+nullable None must serialize as explicit null: {s}"
    );
}

#[allow(dead_code)]
fn _unused(_e: Error) {}
