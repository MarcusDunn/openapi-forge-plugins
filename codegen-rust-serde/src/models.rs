//! Emit `models.rs`: one `#[derive(Serialize, Deserialize)]` definition per
//! `NamedType` in the (filtered) IR. Objects become structs, string enums
//! become Rust enums, int enums become enums backed by `i64`. Primitive
//! aliases, top-level arrays, and modelable unions land as `pub type`.

use forge_plugin_sdk::ir;
use forge_plugin_sdk::peel_nullable;
use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use crate::naming;
use crate::types::{self, type_ref_to_rust, ModelsPath};

/// Sibling named types in `models.rs` resolve by bare name; pass an empty
/// prefix to `type_ref_to_rust`.
fn models_path_inside() -> ModelsPath {
    TokenStream::new()
}

pub fn render(spec: &ir::Ir) -> TokenStream {
    let mut items = TokenStream::new();
    for named in &spec.types {
        if named.id == ir::NULL_ID {
            continue;
        }
        items.extend(render_named(spec, named));
    }
    quote! {
        #![allow(non_snake_case, non_camel_case_types, clippy::all, clippy::pedantic, clippy::nursery, dead_code)]

        use serde::{Deserialize, Serialize};

        /// Serde `deserialize_with` for tri-state nullable fields
        /// (`Option<Option<T>>`). Without it, `null` on the wire would
        /// collapse to the outer `None` (because `Option<Option<T>>`'s
        /// default Deserialize treats `null` as the OUTER `None`), and
        /// generators would lose the missing-vs-explicit-null
        /// distinction the spec carries.
        ///
        /// With this helper: missing → outer `None` (via
        /// `#[serde(default)]`); `null` → `Some(None)`; value →
        /// `Some(Some(value))`. Inserted on every emitted models.rs
        /// even when no field uses it — `#![allow(dead_code)]` at the
        /// crate root suppresses the warning.
        fn deserialize_explicit_optional<'de, D, T>(
            deserializer: D,
        ) -> std::result::Result<Option<Option<T>>, D::Error>
        where
            D: serde::Deserializer<'de>,
            T: serde::Deserialize<'de>,
        {
            Option::<T>::deserialize(deserializer).map(Some)
        }

        #items
    }
}

fn render_named(spec: &ir::Ir, named: &ir::NamedType) -> TokenStream {
    // The parser hoists per-property bodies into named types (the
    // `Schemas<Type>Property<Field>` family). They never become useful Rust
    // aliases — they'd just be `pub type X = String;` namespace pollution
    // — and the field that birthed them inlines the primitive at the use
    // site via `type_ref_to_rust`. Skip them.
    if !should_emit_named(named) {
        return TokenStream::new();
    }
    let name = format_ident!("{}", naming::rust_type_name(named));
    let docs = doc_attrs(&named.documentation);
    match &named.definition {
        ir::TypeDef::Object(o) => render_struct(spec, &name, &docs, o),
        ir::TypeDef::EnumString(e) => render_string_enum(&name, &docs, e),
        ir::TypeDef::EnumInt(e) => render_int_enum(&name, &docs, e),
        ir::TypeDef::Primitive(_) | ir::TypeDef::Array(_) => {
            let rhs = type_ref_to_rust(spec, &named.id, &models_path_inside());
            quote! {
                #docs
                pub type #name = #rhs;
            }
        }
        ir::TypeDef::Union(u) => render_union(spec, &name, &docs, u),
        ir::TypeDef::Null => TokenStream::new(),
    }
}

/// Render a `TypeDef::Union` as either an `Option<T>` alias (the `T |
/// null` two-variant special case) or a `#[serde(untagged)]` enum whose
/// variants mirror the `oneOf` branches in declaration order.
///
/// Why untagged: the wire form of a hand-written `JsonValue` carries no
/// discriminator, so the serde representation must be untagged too.
/// Variant *names* never appear on the wire — they're for caller
/// ergonomics — so we derive them from the variant's type kind
/// (`String`, `Integer`, `Number`, `Bool`, `Array`, `Object`, `Null`) and
/// suffix duplicates with a positional index.
fn render_union(spec: &ir::Ir, name: &Ident, docs: &TokenStream, u: &ir::UnionType) -> TokenStream {
    if let Some(inner) = types::nullable_inner(spec, u, &models_path_inside()) {
        return quote! {
            #docs
            pub type #name = #inner;
        };
    }
    let names = unique_variant_names(spec, u);
    let mut variants = TokenStream::new();
    for (variant_name, v) in names.iter().zip(&u.variants) {
        let variant_ident = format_ident!("{}", variant_name);
        if v.r#type == ir::NULL_ID {
            // Unit variants in untagged enums serialize via
            // `serialize_unit` (→ JSON `null`) and deserialize from
            // `null`. No payload needed.
            variants.extend(quote! { #variant_ident, });
        } else {
            let inner = type_ref_to_rust(spec, &v.r#type, &models_path_inside());
            variants.extend(quote! { #variant_ident(#inner), });
        }
    }
    // No `PartialEq` derive: variants are arbitrary user-named types
    // (structs / arrays / maps) and the struct emission deliberately
    // doesn't derive `PartialEq` because we can't guarantee every
    // property's type supports it. Consumers that need `==` should
    // derive it themselves on the inner types (or on the union).
    quote! {
        #docs
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(untagged)]
        pub enum #name {
            #variants
        }
    }
}

/// Pick variant identifiers for a `#[serde(untagged)]` enum. Collisions
/// (e.g. two object-typed variants) get a `_N` suffix in declaration
/// order so they remain stable across edits to unrelated variants.
fn unique_variant_names(spec: &ir::Ir, u: &ir::UnionType) -> Vec<String> {
    let raw: Vec<String> = u
        .variants
        .iter()
        .map(|v| types::variant_ident_for(spec, &v.r#type))
        .collect();
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for n in &raw {
        *counts.entry(n.as_str()).or_insert(0) += 1;
    }
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    raw.iter()
        .map(|n| {
            if counts.get(n.as_str()).copied().unwrap_or(0) <= 1 {
                n.clone()
            } else {
                let idx = seen.entry(n.clone()).or_insert(0);
                *idx += 1;
                format!("{n}{idx}")
            }
        })
        .collect()
}

/// Decide whether a `NamedType` earns its keep as a definition in
/// `models.rs`. Synthesized per-property *primitive / array* aliases
/// (their `original_name` is `None`) are parser bookkeeping — the use
/// site inlines the same expression — so they get filtered out.
///
/// Unions are different: synthesized inline unions (e.g. anonymous
/// `oneOf` in a response body) still need a concrete `#[serde(untagged)]
/// pub enum` to land in `models.rs`, because use sites refer to them
/// by name (`type_ref_to_rust` emits `models::<rust_type_name>`).
/// Without that emission the use site would dangle.
fn should_emit_named(named: &ir::NamedType) -> bool {
    match &named.definition {
        ir::TypeDef::Object(o) => types::additional_properties_only(o).is_none(),
        ir::TypeDef::EnumString(_) | ir::TypeDef::EnumInt(_) => true,
        ir::TypeDef::Union(_) => true,
        ir::TypeDef::Primitive(_) | ir::TypeDef::Array(_) => named.original_name.is_some(),
        ir::TypeDef::Null => false,
    }
}

fn render_struct(
    spec: &ir::Ir,
    name: &Ident,
    docs: &TokenStream,
    o: &ir::ObjectType,
) -> TokenStream {
    if o.properties.is_empty()
        && matches!(o.additional_properties, ir::AdditionalProperties::Forbidden)
    {
        return quote! {
            #docs
            #[derive(Debug, Clone, Serialize, Deserialize)]
            pub struct #name {}
        };
    }
    let mut fields = TokenStream::new();
    for prop in &o.properties {
        let prop_docs = doc_attrs(&prop.documentation);
        let rust_name = naming::snake_case(&prop.name);
        let field_ident = types::ident(&rust_name);
        let rename_needed = strip_raw(&rust_name) != prop.name;
        // The IR canonicalises `T | null` as a two-variant union (see
        // SDK's `peel_nullable`). `type_ref_to_rust` already collapses
        // that to `Option<T>` at the use site. We need the *wire-level*
        // flag too, to decide between the three optional-field shapes:
        //   missing-or-value     → `Option<T>` + skip-if-none + default
        //   present-but-nullable → `Option<T>` (no skip, no default)
        //   tri-state            → `Option<Option<T>>` + skip + custom
        //                          `deserialize_with` so the outer
        //                          `None` survives a `null` on the wire
        //                          (without the custom path, `null`
        //                          deserialises as the outer `None`,
        //                          collapsing missing and explicit-null
        //                          back together).
        let nullable = peel_nullable(&spec.types, &prop.r#type).is_some();
        let inner_ty = type_ref_to_rust(spec, &prop.r#type, &models_path_inside());
        let (final_ty, serde_attr) = match (prop.required, nullable) {
            // Required, not nullable: field must be present, value of T.
            (true, false) => (inner_ty, TokenStream::new()),
            // Required, nullable: must be present; `null` → `None`, value
            // → `Some(value)`. `inner_ty` is already `Option<T>`.
            (true, true) => (inner_ty, TokenStream::new()),
            // Optional, not nullable: missing ≡ `None`; serde
            // round-trips both ways.
            (false, false) => (
                quote! { Option<#inner_ty> },
                quote! { #[serde(default, skip_serializing_if = "Option::is_none")] },
            ),
            // Optional, nullable (tri-state): outer `None` = missing,
            // `Some(None)` = explicit `null`, `Some(Some(v))` = value.
            // `inner_ty` is already `Option<T>`, so outer-wrap yields
            // `Option<Option<T>>`.
            (false, true) => (
                quote! { Option<#inner_ty> },
                quote! { #[serde(default, deserialize_with = "deserialize_explicit_optional", skip_serializing_if = "Option::is_none")] },
            ),
        };
        let rename_attr = if rename_needed {
            let wire = &prop.name;
            quote! { #[serde(rename = #wire)] }
        } else {
            TokenStream::new()
        };
        fields.extend(quote! {
            #prop_docs
            #rename_attr
            #serde_attr
            pub #field_ident: #final_ty,
        });
    }
    let additional = if let ir::AdditionalProperties::Typed { r#type } = &o.additional_properties {
        let inner = type_ref_to_rust(spec, r#type, &models_path_inside());
        quote! {
            #[serde(flatten)]
            pub additional: std::collections::HashMap<String, #inner>,
        }
    } else {
        TokenStream::new()
    };
    quote! {
        #docs
        #[derive(Debug, Clone, Serialize, Deserialize)]
        pub struct #name {
            #fields
            #additional
        }
    }
}

fn render_string_enum(name: &Ident, docs: &TokenStream, e: &ir::EnumStringType) -> TokenStream {
    // We always emit `#[serde(rename = "...")]` per variant. Trying to
    // detect "the Pascal-cased variant matches the wire value already" is
    // both clever and easy to get wrong (mixed-case wire values like
    // `inProgress` round-trip through `pascal_case` to `InProgress`, which
    // would *not* match `inProgress` by default).
    let mut variants = TokenStream::new();
    let mut display_arms = TokenStream::new();
    for value in &e.values {
        let variant_docs = doc_attrs(&value.documentation);
        let variant = format_ident!("{}", naming::pascal_case(&value.value));
        let wire = &value.value;
        variants.extend(quote! {
            #variant_docs
            #[serde(rename = #wire)]
            #variant,
        });
        display_arms.extend(quote! { Self::#variant => f.write_str(#wire), });
    }
    quote! {
        #docs
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum #name {
            #variants
        }

        // `Display` mirrors the serde wire value so the tower client's
        // `param.to_string()` query-param serialization round-trips
        // through the same representation `serde_json` would emit.
        impl std::fmt::Display for #name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #display_arms
                }
            }
        }
    }
}

fn render_int_enum(name: &Ident, docs: &TokenStream, e: &ir::EnumIntType) -> TokenStream {
    let mut variants = TokenStream::new();
    let mut to_arms = TokenStream::new();
    let mut from_arms = TokenStream::new();
    for value in &e.values {
        let variant_docs = doc_attrs(&value.documentation);
        let variant = format_ident!("{}", int_variant_name(value.value));
        let val_lit = Literal::i64_unsuffixed(value.value);
        variants.extend(quote! {
            #variant_docs
            #variant,
        });
        to_arms.extend(quote! { #name::#variant => #val_lit, });
        from_arms.extend(quote! { #val_lit => Ok(#name::#variant), });
    }
    let err_msg = format!("unknown {name} discriminant: {{}}");
    quote! {
        #docs
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(into = "i64", try_from = "i64")]
        pub enum #name {
            #variants
        }

        impl From<#name> for i64 {
            fn from(v: #name) -> i64 {
                match v {
                    #to_arms
                }
            }
        }

        impl std::convert::TryFrom<i64> for #name {
            type Error = String;
            fn try_from(v: i64) -> Result<Self, Self::Error> {
                match v {
                    #from_arms
                    other => Err(format!(#err_msg, other)),
                }
            }
        }

        // `Display` formats the discriminant integer. Same shape as the
        // serde wire form, so `param.to_string()` query-param
        // serialization matches what `serde_json` would emit.
        impl std::fmt::Display for #name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let v: i64 = self.clone().into();
                std::fmt::Display::fmt(&v, f)
            }
        }
    }
}

/// Rust variant name for an integer enum value. Sign-aware so `1` and
/// `-1` don't both collapse to `V1`; `0` is `V0`. `i64::MIN`'s absolute
/// value doesn't fit in `i64`, so we format from `unsigned_abs`.
fn int_variant_name(value: i64) -> String {
    if value < 0 {
        format!("VNeg{}", value.unsigned_abs())
    } else {
        format!("V{value}")
    }
}

/// Convert an `Option<String>` IR doc field to one `#[doc = " <line>"]`
/// attribute per line. prettyplease unparses these back to `///` form.
pub fn doc_attrs(doc: &Option<String>) -> TokenStream {
    let Some(doc) = doc else {
        return TokenStream::new();
    };
    let mut out = TokenStream::new();
    for line in doc.lines() {
        let line_str = format!(" {line}");
        out.extend(quote! { #[doc = #line_str] });
    }
    out
}

fn strip_raw(s: &str) -> &str {
    s.strip_prefix("r#").unwrap_or(s)
}
