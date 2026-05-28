//! Structural assertions on the Kotlin source emitted by
//! `generator-kotlin-types` for the `kotlin-petstore` fixture.
//!
//! Same pattern as `fixtures/html-petstore/compile-test/tests/html_shape.rs`
//! and `fixtures/clap-petstore/compile-test/tests/codegen.rs`: read the
//! generated files off disk with `std::fs`, then assert on substring and
//! data-shape properties. No third-party Kotlin parser — the generator emits
//! deterministic, well-formatted Kotlin and these tests pin the exact
//! patterns documented in the plan (see /home/marcus/.claude/plans/).
//!
//! The fixture's spec exercises:
//! - unconstrained string alias (`CustomerId`) → values4k wrapper, no validator
//! - constrained string with pattern (`AccountNumber`)
//! - multi-constraint string (`Email`: minLength + maxLength + regex)
//! - integer with min/max (`Quantity`)
//! - date-time (`CreatedAt` → java.time.Instant + inlined serializer)
//! - string enum (`Status`)
//! - explicit OAS discriminator (`Animal` → Dog/Cat)
//! - inferred discriminator from `const`-style variant property (`Shape` → Circle/Square)
//! - array typealias (`Pets`)
//! - cross-package reference (`Pet` in ca.dialai.pet → `Owner` in ca.dialai.account)
//! - data-class field ordering / nullability / `@SerialName` for snake_case

use std::fs;
use std::path::{Path, PathBuf};

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compile-test dir has a parent fixture dir")
        .join("out")
}

fn read(rel: &str) -> String {
    let p = out_dir().join(rel);
    fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("expected emitted file {} ({})", p.display(), e))
}

fn exists(rel: &str) -> bool {
    out_dir().join(rel).exists()
}

// All generated files live under this prefix. The new object-shaped
// x-kotlin-source derives `backend/<module>/src/main/kotlin/<pkg>` from
// `{class, module}` — see naming.rs.
const PKG_PATH: &str = "backend/pet/src/main/kotlin/ca/dialai/pet";
const ACCOUNT_PATH: &str = "backend/account/src/main/kotlin/ca/dialai/account";

fn pet(rel: &str) -> String {
    read(&format!("{PKG_PATH}/{rel}"))
}

// ---- file presence ----

#[test]
fn every_expected_file_exists() {
    for name in [
        "CustomerId.kt",
        "AccountNumber.kt",
        "Email.kt",
        "Quantity.kt",
        "CreatedAt.kt",
        "Status.kt",
        "Pet.kt",
        "Pets.kt",
        "Animal.kt",
        "Dog.kt",
        "Cat.kt",
        "Shape.kt",
        "Circle.kt",
        "Square.kt",
        "AuditBase.kt",
        "AuditedNote.kt",
        "Avatar.kt",
        "JsonValue.kt",
        "AgentContent.kt",
        "AgentTextContent.kt",
        "FunctionCallSuccess.kt",
    ] {
        let rel = format!("{PKG_PATH}/{name}");
        assert!(exists(&rel), "missing generated file: {rel}");
    }
    let owner = format!("{ACCOUNT_PATH}/Owner.kt");
    assert!(exists(&owner), "missing cross-package file: {owner}");
}

#[test]
fn every_file_declares_its_package() {
    for name in [
        "CustomerId.kt",
        "Pet.kt",
        "Animal.kt",
        "Shape.kt",
        "Pets.kt",
    ] {
        let body = pet(name);
        assert!(
            body.contains("package ca.dialai.pet\n"),
            "{name} missing or wrong package declaration:\n{body}"
        );
    }
    let owner = read(&format!("{ACCOUNT_PATH}/Owner.kt"));
    assert!(
        owner.contains("package ca.dialai.account\n"),
        "Owner.kt missing cross-package declaration"
    );
}

// ---- values4k wrappers ----

#[test]
fn unconstrained_string_alias_is_value_wrapper_with_no_validator() {
    let body = pet("CustomerId.kt");
    assert!(body.contains("import dev.forkhandles.values.StringValue"));
    assert!(body.contains("import dev.forkhandles.values.StringValueFactory"));
    assert!(body.contains(
        "internal data class CustomerId private constructor(val value: String) : StringValue(value)"
    ));
    // Single-arg factory — no validation block at all.
    assert!(body.contains("companion object : StringValueFactory<CustomerId>(::CustomerId)"));
    // No values4k validation extension was imported.
    assert!(
        !body.contains("import dev.forkhandles.values.regex"),
        "unconstrained alias should not import .regex"
    );
    assert!(
        !body.contains("import dev.forkhandles.values.minLength"),
        "unconstrained alias should not import .minLength"
    );
}

#[test]
fn pattern_constrained_string_uses_regex_extension() {
    let body = pet("AccountNumber.kt");
    assert!(body.contains("import dev.forkhandles.values.regex"));
    // Backslashes in the OAS pattern (`\d{8}`) must be escaped for the Kotlin
    // string literal, hence the doubled `\\` in the rendered source.
    assert!(body.contains(
        r#"companion object : StringValueFactory<AccountNumber>(::AccountNumber, "\\d{8}".regex)"#
    ));
}

#[test]
fn multi_constraint_string_chains_with_and() {
    let body = pet("Email.kt");
    for needle in [
        "import dev.forkhandles.values.minLength",
        "import dev.forkhandles.values.maxLength",
        "import dev.forkhandles.values.regex",
    ] {
        assert!(body.contains(needle), "Email.kt missing import {needle}");
    }
    // Right-associative .and(...) chain — the exact joiner the renderer emits.
    assert!(
        body.contains(r#"3.minLength.and(254.maxLength.and("^.+@.+\$".regex))"#),
        "Email.kt validator chain not in expected shape:\n{body}"
    );
}

#[test]
fn int_with_range_uses_int_value_and_min_max_value() {
    let body = pet("Quantity.kt");
    assert!(body.contains("import dev.forkhandles.values.IntValue"));
    assert!(body.contains("import dev.forkhandles.values.IntValueFactory"));
    assert!(body.contains("import dev.forkhandles.values.minValue"));
    assert!(body.contains("import dev.forkhandles.values.maxValue"));
    assert!(body.contains(
        "data class Quantity private constructor(val value: Int) : IntValue(value)"
    ));
    assert!(
        body.contains("companion object : IntValueFactory<Quantity>(::Quantity, 1.minValue.and(100.maxValue))"),
        "Quantity.kt factory not in expected shape:\n{body}"
    );
}

// ---- custom KSerializer (the whole point of values4k + JSON primitive roundtrip) ----

#[test]
fn every_value_wrapper_has_a_primitive_kserializer() {
    let cases = &[
        ("CustomerId.kt", "CustomerId", "STRING", "encodeString", "decodeString"),
        ("AccountNumber.kt", "AccountNumber", "STRING", "encodeString", "decodeString"),
        ("Email.kt", "Email", "STRING", "encodeString", "decodeString"),
        ("Quantity.kt", "Quantity", "INT", "encodeInt", "decodeInt"),
    ];
    for (file, cls, kind, enc, dec) in cases {
        let body = pet(file);
        assert!(
            body.contains(&format!("@Serializable(with = {cls}.Serializer::class)")),
            "{file} missing @Serializable(with = {cls}.Serializer::class)"
        );
        assert!(
            body.contains(&format!("internal object Serializer : KSerializer<{cls}>")),
            "{file} missing Serializer object"
        );
        assert!(
            body.contains(&format!(
                r#"PrimitiveSerialDescriptor("{cls}", PrimitiveKind.{kind})"#
            )),
            "{file} wrong descriptor kind, expected {kind}"
        );
        assert!(
            body.contains(&format!("encoder.{enc}(value.value)")),
            "{file} not encoding via {enc}"
        );
        assert!(
            body.contains(&format!("{cls}.of(decoder.{dec}())")),
            "{file} not decoding via {cls}.of(decoder.{dec}())"
        );
    }
}

// ---- date-time ----

#[test]
fn instant_format_uses_java_time_instant_and_separate_serializer() {
    // `format: instant` is the explicit-Instant escape hatch for fields that
    // should drop the offset (UTC-only). Distinct from `format: date-time`
    // which uses OffsetDateTime.
    let body = pet("EventTimestamp.kt");
    assert!(body.contains("import java.time.Instant"));
    assert!(body.contains("data class EventTimestamp(val value: Instant)"));
    assert!(body.contains(
        "internal object Serializer : KSerializer<EventTimestamp> by InstantSerializer.map(EventTimestamp::value, ::EventTimestamp)"
    ));
    assert!(body.contains("private object InstantSerializer : KSerializer<java.time.Instant>"));
    // OffsetDateTime must not leak in when only Instant is asked for.
    assert!(!body.contains("OffsetDateTime"));
}

#[test]
fn date_time_uses_java_time_and_inlines_serializer() {
    let body = pet("CreatedAt.kt");
    assert!(body.contains("import java.time.OffsetDateTime"));
    assert!(body.contains("data class CreatedAt(val value: OffsetDateTime)"));
    // Delegation to the inlined helper.
    assert!(body.contains(
        "internal object Serializer : KSerializer<CreatedAt> by OffsetDateTimeSerializer.map(CreatedAt::value, ::CreatedAt)"
    ));
    // The helper itself is inlined into the same file (single-file goal).
    assert!(
        body.contains("private object OffsetDateTimeSerializer : KSerializer<java.time.OffsetDateTime>"),
        "CreatedAt.kt missing inlined OffsetDateTimeSerializer:\n{body}"
    );
    assert!(body.contains("java.time.OffsetDateTime.parse(decoder.decodeString())"));
}

// ---- enums ----

#[test]
fn string_enum_uses_serial_name_per_variant() {
    let body = pet("Status.kt");
    assert!(body.contains("@Serializable\ninternal enum class Status"));
    assert!(body.contains(r#"@SerialName("active") ACTIVE"#));
    assert!(body.contains(r#"@SerialName("archived") ARCHIVED"#));
    // Snake_case OAS value → SCREAMING_SNAKE Kotlin variant.
    assert!(body.contains(r#"@SerialName("pending_review") PENDING_REVIEW"#));
}

// ---- objects ----

#[test]
fn defaults_to_internal_visibility() {
    // Every generated top-level class/interface/enum should carry the
    // `internal` modifier unless x-kotlin-visibility overrides.
    assert!(pet("Pet.kt").contains("internal data class Pet("));
    assert!(pet("Status.kt").contains("internal enum class Status"));
    assert!(pet("Animal.kt").contains("internal sealed interface Animal"));
    assert!(pet("Pets.kt").contains("internal typealias Pets = List<Pet>"));
    assert!(pet("AgentContent.kt").contains("internal sealed interface AgentContent"));
    assert!(pet("JsonValue.kt").contains("internal typealias JsonValue = JsonElement"));
}

#[test]
fn x_kotlin_visibility_public_drops_internal_modifier() {
    let body = pet("PublicHealthcheck.kt");
    assert!(
        body.contains("@Serializable\ndata class PublicHealthcheck("),
        "PublicHealthcheck.kt should be public (no `internal`):\n{body}"
    );
    assert!(
        !body.contains("internal data class PublicHealthcheck"),
        "PublicHealthcheck.kt should NOT carry `internal`:\n{body}"
    );
}

#[test]
fn object_uses_serial_name_only_for_snake_case_fields() {
    let body = pet("Pet.kt");
    assert!(body.contains("@Serializable\ninternal data class Pet("));
    // Required field, no default.
    assert!(body.contains("val id: CustomerId,"));
    // Required cross-package ref renders as bare class name + import (asserted
    // separately in cross_package_imports).
    assert!(body.contains("val owner: Owner,"));
    // snake_case → @SerialName.
    assert!(body.contains("@SerialName(\"created_at\")"));
    assert!(body.contains("val createdAt: CreatedAt? = null,"));
    assert!(body.contains("@SerialName(\"owner_email\")"));
    assert!(body.contains("val ownerEmail: Email? = null,"));
    // camelCase field with no rename gets no @SerialName.
    let id_line_idx = body.find("val id:").expect("Pet has an id field");
    let preceding_50 = &body[id_line_idx.saturating_sub(60)..id_line_idx];
    assert!(
        !preceding_50.contains("@SerialName(\"id\")"),
        "Pet.id should not carry a redundant @SerialName"
    );
}

#[test]
fn cross_package_imports_are_emitted() {
    let pet = pet("Pet.kt");
    assert!(
        pet.contains("import ca.dialai.account.Owner"),
        "Pet.kt missing cross-package import for Owner"
    );
    let owner = read(&format!("{ACCOUNT_PATH}/Owner.kt"));
    assert!(
        owner.contains("import ca.dialai.pet.CustomerId"),
        "Owner.kt missing cross-package import for CustomerId"
    );
    // Same-package refs do NOT import.
    let cat = pet_no_owner_import(&pet);
    assert!(
        !cat.lines().any(|l| l.trim() == "import ca.dialai.pet.Pet"),
        "Pet.kt should not import itself"
    );
}

fn pet_no_owner_import(body: &str) -> String {
    body.lines()
        .filter(|l| !l.starts_with("import ca.dialai.account."))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- explicit discriminator ----

#[test]
fn explicit_discriminator_emits_sealed_interface_and_drops_field_from_variants() {
    let animal = pet("Animal.kt");
    assert!(animal.contains("import kotlinx.serialization.json.JsonClassDiscriminator"));
    assert!(animal.contains(r#"@JsonClassDiscriminator("type")"#));
    assert!(animal.contains("sealed interface Animal"));

    let dog = pet("Dog.kt");
    assert!(dog.contains(r#"@SerialName("dog")"#));
    assert!(dog.contains("data class Dog("));
    assert!(dog.trim_end().ends_with(") : Animal"));
    // The `type` discriminator MUST be dropped (kotlinx.serialization owns it).
    assert!(
        !dog.contains("val type:"),
        "Dog.kt should not carry the discriminator property:\n{dog}"
    );

    let cat = pet("Cat.kt");
    assert!(cat.contains(r#"@SerialName("cat")"#));
    assert!(cat.trim_end().ends_with(") : Animal"));
    assert!(!cat.contains("val type:"));
}

// ---- inferred discriminator (the feature from the latest user ask) ----

#[test]
fn inferred_discriminator_works_without_oas_discriminator_keyword() {
    // Shape has no `discriminator` block in the spec — each variant carries
    // `kind: { type: string, enum: ["<tag>"] }`. The generator must infer
    // `kind` as the discriminator property.
    let shape = pet("Shape.kt");
    assert!(
        shape.contains(r#"@JsonClassDiscriminator("kind")"#),
        "Shape.kt missing inferred discriminator:\n{shape}"
    );
    assert!(shape.contains("sealed interface Shape"));

    let circle = pet("Circle.kt");
    assert!(circle.contains(r#"@SerialName("circle")"#));
    assert!(circle.trim_end().ends_with(") : Shape"));
    // Inferred discriminator must also be dropped from the variant body.
    assert!(
        !circle.contains("val kind:"),
        "Circle.kt should not carry the inferred discriminator property:\n{circle}"
    );

    let square = pet("Square.kt");
    assert!(square.contains(r#"@SerialName("square")"#));
    assert!(square.trim_end().ends_with(") : Shape"));
    assert!(!square.contains("val kind:"));
}

// ---- array typealias ----

#[test]
fn array_component_emits_typealias() {
    let body = pet("Pets.kt");
    assert!(
        body.contains("typealias Pets = List<Pet>"),
        "Pets.kt should be a typealias to List<Pet>:\n{body}"
    );
    // No @Serializable on a typealias — kotlinx.serialization handles
    // List<Pet> automatically via Pet's @Serializable.
    assert!(!body.contains("@Serializable"));
}

// ---- top-of-file invariants ----

#[test]
fn every_file_carries_the_do_not_edit_banner() {
    let dir = out_dir();
    let mut count = 0;
    visit_kt(&dir, &mut |path, body| {
        count += 1;
        assert!(
            body.starts_with("// Generated by generator-kotlin-types — do not edit.\n"),
            "{} missing do-not-edit banner",
            path.display()
        );
    });
    assert!(count >= 15, "expected at least 15 .kt files, got {count}");
}

// ---- allOf flattening (forge IR pre-flattens; we just assert the result) ----

#[test]
fn allof_components_are_flattened_into_a_single_data_class() {
    let body = pet("AuditedNote.kt");
    // All 4 fields from Base + extension must show up on one data class.
    assert!(body.contains("@Serializable\ninternal data class AuditedNote("));
    for field in ["val id:", "val createdAt:", "val text:", "val tag:"] {
        assert!(
            body.contains(field),
            "AuditedNote.kt missing {field} (allOf branches must be flattened):\n{body}"
        );
    }
    // The base type is still emitted on its own — allOf flattening copies, not moves.
    assert!(exists(&format!("{PKG_PATH}/AuditBase.kt")));
}

// ---- byte format → ByteArray + Base64 serializer ----

#[test]
fn named_byte_primitive_renders_as_bytearray_wrapper() {
    let body = pet("Avatar.kt");
    assert!(body.contains("@Serializable(with = Avatar.Serializer::class)"));
    assert!(body.contains("data class Avatar private constructor(val value: ByteArray)"));
    assert!(body.contains("fun of(value: ByteArray): Avatar = Avatar(value)"));
    assert!(body.contains("java.util.Base64.getEncoder().encodeToString(value.value)"));
    assert!(body.contains("java.util.Base64.getDecoder().decode(decoder.decodeString())"));
    // ByteArray is built-in — no values4k base class on the wrapper.
    assert!(
        !body.contains(": StringValue"),
        "byte wrapper should not inherit StringValue (values4k has no ByteArrayValue):\n{body}"
    );
}

#[test]
fn inline_byte_property_is_bytearray_with_file_level_serializer() {
    // AuditBase has an inline `thumbnail: { type: string, format: byte }` —
    // it should round-trip via the file-level @file:UseSerializers + an
    // inlined Base64ByteArraySerializer object at the end of the file.
    let body = pet("AuditBase.kt");
    assert!(body.contains("@file:UseSerializers("));
    assert!(body.contains("Base64ByteArraySerializer::class"));
    assert!(body.contains("import kotlinx.serialization.UseSerializers"));
    assert!(body.contains("val thumbnail: ByteArray? = null"));
    assert!(body.contains("private object Base64ByteArraySerializer : KSerializer<ByteArray>"));
}

// ---- inline date-time → @file:UseSerializers + inlined Instant serializer ----

#[test]
fn inline_date_time_property_uses_file_level_offset_date_time_serializer() {
    let body = pet("AuditBase.kt");
    assert!(body.contains("val createdAt: OffsetDateTime,"));
    assert!(body.contains("@file:UseSerializers("));
    assert!(
        body.contains("OffsetDateTimeSerializer::class"),
        "AuditBase.kt should wire OffsetDateTimeSerializer via @file:UseSerializers:\n{body}"
    );
    assert!(body.contains("private object OffsetDateTimeSerializer : KSerializer<java.time.OffsetDateTime>"));
}

// ---- untagged union with primitives → JsonElement ----

#[test]
fn untagged_primitive_union_typealiases_to_json_element() {
    let body = pet("JsonValue.kt");
    assert!(body.contains("import kotlinx.serialization.json.JsonElement"));
    assert!(
        body.contains("typealias JsonValue = JsonElement"),
        "JsonValue.kt should typealias to JsonElement:\n{body}"
    );
    // No sealed-interface machinery — just the alias.
    assert!(!body.contains("sealed interface"));
    assert!(!body.contains("@Serializable"));
}

// ---- single-variant untagged union → sealed interface delegating to variant ----

#[test]
fn single_variant_untagged_union_delegates_to_its_sole_member() {
    let parent = pet("AgentContent.kt");
    assert!(parent.contains("@Serializable(with = AgentContent.Serializer::class)"));
    assert!(parent.contains("sealed interface AgentContent {"));
    // The delegating serializer routes to the lone variant's auto-generated
    // serializer (no JsonClassDiscriminator at all).
    assert!(parent.contains("private val delegate = AgentTextContent.serializer()"));
    assert!(parent.contains("override val descriptor = delegate.descriptor"));
    assert!(parent.contains("delegate.serialize(encoder, value as AgentTextContent)"));
    assert!(parent.contains("override fun deserialize(decoder: Decoder): AgentContent =\n            delegate.deserialize(decoder)"));
    assert!(
        !parent.contains("@JsonClassDiscriminator"),
        "AgentContent has no discriminator; @JsonClassDiscriminator must not be emitted"
    );

    let variant = pet("AgentTextContent.kt");
    // Variant declares the parent — but keeps every property (nothing to drop)
    // and gets no @SerialName (there's no discriminator tag).
    assert!(variant.contains("data class AgentTextContent(\n    val text: String,\n) : AgentContent"));
    assert!(!variant.contains("@SerialName"));
}

// ---- inline-object promotion → nested class ----

#[test]
fn inline_object_property_promotes_to_nested_class() {
    let body = pet("FunctionCallSuccess.kt");
    // Outer's constructor refers to the nested class by its simple name —
    // Kotlin's name resolution scope makes this resolve to Outer.Response.
    assert!(body.contains("val response: Response,"));
    // Nested data class lives inside the outer's body.
    assert!(body.contains("@Serializable\n    data class Response("));
    // Multi-level nesting: Response itself promotes `audit` to an Audit
    // nested-nested class, which in turn promotes `level` to a Level enum.
    assert!(
        body.contains("data class Audit(") && body.contains("enum class Level"),
        "FunctionCallSuccess.kt should nest Response → Audit → Level:\n{body}"
    );
    // The deepest nested enum still gets its @SerialName tags.
    assert!(body.contains(r#"@SerialName("info") INFO"#));
    assert!(body.contains(r#"@SerialName("warn") WARN"#));
    assert!(body.contains(r#"@SerialName("error") ERROR"#));
    // Sanity: indentation grows with depth — Level lines are deeper indented
    // than Audit lines, which are deeper than Response lines.
    let response_indent = body
        .find("    data class Response(")
        .expect("Response declaration");
    let audit_indent = body
        .find("        data class Audit(")
        .expect("Audit declaration");
    let level_indent = body
        .find("            @Serializable\n            enum class Level")
        .expect("Level declaration");
    assert!(response_indent < audit_indent && audit_indent < level_indent);
}

// ---- existing tree walker ----

fn visit_kt(dir: &Path, on_file: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dirent");
        let p = entry.path();
        if p.is_dir() {
            visit_kt(&p, on_file);
        } else if p.extension().and_then(|s| s.to_str()) == Some("kt") {
            let body = fs::read_to_string(&p).expect("read_to_string");
            on_file(&p, &body);
        }
    }
}
