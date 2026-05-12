//! Property tests: the generated `JsonValue` must be a drop-in
//! functional substitute for `serde_json::Value`.
//!
//! The strong claim: for any wire form `W` that `serde_json::Value`
//! accepts, `JsonValue` accepts it and re-emits a wire form that
//! canonicalizes (re-parse + re-serialize through `serde_json::Value`)
//! to the *same bytes* as `serde_json::Value`'s own round-trip. Any
//! number-precision quirks in serde_json's own parser (e.g. the
//! one-ULP drift that bites some large decimals) hit both sides
//! identically, so the comparison cancels them out and isolates real
//! codegen divergences.

use compile_test::gen::models::JsonValue;
use proptest::prelude::*;
use serde_json::{Number, Value};

/// Re-parse and re-serialize through `serde_json::Value`. That uses a
/// `BTreeMap` for objects, so map keys come out in alphabetic order
/// regardless of what the input used. Any number-precision behaviour
/// in serde_json's parser is also exercised identically on both sides.
fn canonicalize(wire: &str) -> String {
    let v: Value = serde_json::from_str(wire).unwrap();
    serde_json::to_string(&v).unwrap()
}

/// Arbitrary `serde_json::Value` tree. Number leaves go through
/// `Number::from_f64` so we don't tickle the int-vs-float fidelity loss
/// documented in the codegen tests (the spec only has a `number`
/// variant, no `integer`).
fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| Value::Number(Number::from_f64(f).expect("finite f64"))),
        any::<String>().prop_map(Value::String),
    ];
    leaf.prop_recursive(
        4,  // max depth
        32, // max total nodes
        4,  // max items per collection
        |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::hash_map(any::<String>(), inner, 0..4)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        },
    )
}

proptest! {
    /// `from_str::<JsonValue>` accepts every wire that
    /// `from_str::<Value>` does, and the round trip through `JsonValue`
    /// canonicalizes to the same wire as the round trip through
    /// `Value`. This is the "drop-in substitute" claim.
    #[test]
    fn jsonvalue_matches_serde_value_on_arbitrary_input(v in arb_value()) {
        let wire = serde_json::to_string(&v).unwrap();

        let jv: JsonValue = serde_json::from_str(&wire)
            .unwrap_or_else(|e| panic!("JsonValue rejected wire {}: {}", wire, e));
        let sv: Value = serde_json::from_str(&wire).unwrap();

        let jv_canonical = canonicalize(&serde_json::to_string(&jv).unwrap());
        let sv_canonical = canonicalize(&serde_json::to_string(&sv).unwrap());

        prop_assert_eq!(jv_canonical, sv_canonical);
    }

    /// In-memory path: build a `JsonValue` directly via `from_value`
    /// (the API a caller uses when handing off a `serde_json::Value`),
    /// then round-trip back. Verifies serialization is consistent
    /// regardless of how the `JsonValue` got built.
    #[test]
    fn jsonvalue_via_from_value_matches_serde_value(v in arb_value()) {
        let jv: JsonValue = serde_json::from_value(v.clone()).unwrap();
        let jv_canonical = canonicalize(&serde_json::to_string(&jv).unwrap());
        let sv_canonical = canonicalize(&serde_json::to_string(&v).unwrap());
        prop_assert_eq!(jv_canonical, sv_canonical);
    }

    /// Round-tripping through `JsonValue` and through `Value` must
    /// remain in lockstep no matter how many passes you do. serde_json's
    /// own float parser drifts ~1 ULP per parse at some magnitudes — so
    /// `wire → Value → wire → Value → wire` is *not* byte-stable across
    /// rounds. The property that *is* stable: each successive pass
    /// through `JsonValue` matches the corresponding pass through
    /// `Value`. If `JsonValue` drifted faster (or in a different
    /// direction) than `Value`, this test would catch it.
    #[test]
    fn jsonvalue_drifts_in_lockstep_with_serde_value(v in arb_value()) {
        let mut jv_wire = serde_json::to_string(&v).unwrap();
        let mut sv_wire = jv_wire.clone();
        for _ in 0..3 {
            let jv: JsonValue = serde_json::from_str(&jv_wire).unwrap();
            let sv: Value = serde_json::from_str(&sv_wire).unwrap();
            jv_wire = canonicalize(&serde_json::to_string(&jv).unwrap());
            sv_wire = canonicalize(&serde_json::to_string(&sv).unwrap());
            prop_assert_eq!(&jv_wire, &sv_wire);
        }
    }
}

/// Explicit edge cases. Property tests exercise the variant
/// cross-product, but a curated set is faster to skim and a clearer
/// regression signal when one specific case breaks. Each entry is a
/// wire form that must canonicalize identically through `JsonValue` and
/// `serde_json::Value`.
#[test]
fn edge_case_wires_match_serde_value() {
    for wire in [
        "null",
        "true",
        "false",
        "0.0",
        "-1.5",
        "1.7976931348623157e308",
        "1e-10",
        r#""""#,
        r#""contains \"quotes\" and \n newlines""#,
        r#"" ""#,
        "[]",
        "{}",
        "[null,true,false,0.0,\"\",[],{}]",
        r#"{"k1":null,"k2":[1.0,2.0],"k3":{"nested":"value"}}"#,
    ] {
        let jv: JsonValue = serde_json::from_str(wire)
            .unwrap_or_else(|e| panic!("JsonValue rejected {wire}: {e}"));
        let sv: Value = serde_json::from_str(wire).unwrap();
        let jv_canonical = canonicalize(&serde_json::to_string(&jv).unwrap());
        let sv_canonical = canonicalize(&serde_json::to_string(&sv).unwrap());
        assert_eq!(jv_canonical, sv_canonical, "wire={wire}");
    }
}
