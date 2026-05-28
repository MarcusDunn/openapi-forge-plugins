//! Top-level orchestration: walk `Ir::types`, parse `x-kotlin-source` on each
//! `NamedType`, group by destination file, hand each group to `render`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use std::collections::BTreeSet;

use forge_plugin_sdk::ir::{Diagnostic, Ir, NamedType, TypeDef, UnionType, NULL_ID};
use forge_plugin_sdk::output::OutputFile;
use forge_plugin_sdk::{diag, values_ext, GenerationOutput};

use crate::config::{Config, MissingExtensionPolicy};
use crate::naming::KotlinTarget;
use crate::render::{render_file, FileRender};

pub enum Outcome {
    Generated(GenerationOutput),
    Rejected(Vec<Diagnostic>),
}

/// Resolved Kotlin destination for one `NamedType`.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub target: KotlinTarget,
    /// `outputRoot` + `target.file`, normalised.
    pub output_path: String,
    /// Visibility modifier rendered before the class/interface/enum/typealias
    /// keyword. Default is `Internal` per project convention; an explicit
    /// `x-kotlin-visibility: "public"` overrides.
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Internal,
}

impl Visibility {
    pub fn modifier(self) -> &'static str {
        match self {
            Visibility::Public => "",
            Visibility::Internal => "internal ",
        }
    }
}

pub type FqcnIndex<'a> = BTreeMap<&'a str, ResolvedTarget>;

/// For each TypeRef that is a variant of a sealed-interface parent in Kotlin,
/// what the parent's FQCN is and (if the union is discriminated) the
/// discriminator property name + the wire tag. `discriminator` is `None` for
/// an untagged single-variant union — the variant inherits the sealed
/// interface but keeps every property and gets no `@SerialName`.
#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub parent_class: String,
    pub parent_package: String,
    pub discriminator: Option<String>,
    pub serial_name: Option<String>,
}

pub type VariantIndex<'a> = BTreeMap<&'a str, VariantInfo>;

/// For one output file: which annotated types are nested inside which other
/// annotated types, derived from FQCN-prefix relationships. If `Outer` and
/// `Outer.Inner` both live in the same file, `Inner` becomes a Kotlin nested
/// class inside `Outer`. This lets specs express idiomatic Kotlin nesting
/// (sealed parent + nested variants, response + nested status enum, …)
/// purely through `x-kotlin-source` annotations.
#[derive(Debug, Default)]
pub struct NestingTree<'a> {
    /// TypeRef -> direct children TypeRefs (in declared order).
    pub children: BTreeMap<&'a str, Vec<&'a str>>,
    /// Types in this file that are NOT nested inside any other type in the
    /// file — i.e. emit as top-level classes in the source.
    pub roots: Vec<&'a str>,
}

pub fn build_nesting_tree<'a>(
    index: &FqcnIndex<'a>,
    file_types: &[&'a NamedType],
) -> NestingTree<'a> {
    // Map every type's FQCN -> its IR id, so we can walk up the FQCN's dotted
    // segments to discover an enclosing type that shares this file.
    let mut fqcn_to_id: BTreeMap<String, &str> = BTreeMap::new();
    for t in file_types {
        if let Some(target) = index.get(t.id.as_str()) {
            fqcn_to_id.insert(target.target.fqcn(), t.id.as_str());
        }
    }

    let mut parent_of: BTreeMap<&'a str, &'a str> = BTreeMap::new();
    for t in file_types {
        let Some(target) = index.get(t.id.as_str()) else {
            continue;
        };
        let my_fqcn = target.target.fqcn();
        // Walk parent FQCNs from most-specific to least-specific. The first
        // ancestor that's also in this file is the nearest enclosing class.
        let parts: Vec<&str> = my_fqcn.split('.').collect();
        for prefix_len in (1..parts.len()).rev() {
            let prefix = parts[..prefix_len].join(".");
            if let Some(parent_id) = fqcn_to_id.get(&prefix).copied() {
                if parent_id != t.id.as_str() {
                    parent_of.insert(t.id.as_str(), parent_id);
                    break;
                }
            }
        }
    }

    let mut children: BTreeMap<&'a str, Vec<&'a str>> = BTreeMap::new();
    for t in file_types {
        if let Some(parent) = parent_of.get(t.id.as_str()) {
            children.entry(*parent).or_default().push(t.id.as_str());
        }
    }

    let roots: Vec<&'a str> = file_types
        .iter()
        .filter_map(|t| {
            if parent_of.contains_key(t.id.as_str()) {
                None
            } else {
                Some(t.id.as_str())
            }
        })
        .collect();

    NestingTree { children, roots }
}

pub fn all(spec: &Ir, cfg: &Config) -> Outcome {
    let (index, index_diagnostics) = build_index(spec, cfg);
    if index_diagnostics
        .iter()
        .any(|d| d.severity == forge_plugin_sdk::ir::Severity::Error)
    {
        return Outcome::Rejected(index_diagnostics);
    }
    let variants = build_variant_index(spec, &index);

    let mut by_file: BTreeMap<String, Vec<&NamedType>> = BTreeMap::new();
    for named in &spec.types {
        if named.id == NULL_ID {
            continue;
        }
        let resolved = match index.get(named.id.as_str()) {
            Some(r) => r,
            None => continue,
        };
        by_file
            .entry(resolved.output_path.clone())
            .or_default()
            .push(named);
    }

    let mut files: Vec<OutputFile> = Vec::new();
    // Carry index-time warnings (malformed x-kotlin-source values, etc.) into
    // the successful-generation output so the user sees them in the forge log.
    let mut diagnostics: Vec<Diagnostic> = index_diagnostics;

    for (path, types) in by_file {
        let nest = build_nesting_tree(&index, &types);
        match render_file(spec, &index, &variants, &nest, &path, &types) {
            Ok(FileRender { path, source }) => {
                files.push(OutputFile::text(path, source));
            }
            Err(e) => match cfg.missing_extension_policy() {
                MissingExtensionPolicy::Error => {
                    diagnostics.push(diag::error(
                        "generator-kotlin-types/render",
                        format!("failed to render {path}: {e}"),
                    ));
                    return Outcome::Rejected(diagnostics);
                }
                MissingExtensionPolicy::Warn => {
                    // Incremental rollout: log the failure and move on so the
                    // user gets every file they CAN generate. Common cause is
                    // referencing a user-facing type that hasn't been
                    // annotated yet — annotate it and the parent goes green.
                    diagnostics.push(diag::warning(
                        "generator-kotlin-types/render",
                        format!("skipped {path}: {e}"),
                    ));
                }
            },
        }
    }

    Outcome::Generated(GenerationOutput { files, diagnostics })
}

fn build_index<'a>(spec: &'a Ir, cfg: &Config) -> (FqcnIndex<'a>, Vec<Diagnostic>) {
    let mut index: FqcnIndex = BTreeMap::new();
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    // Track FQCN -> first IR id seen, so we can warn on duplicates rather
    // than silently emitting two `data class Foo` declarations in one file.
    let mut by_fqcn: BTreeMap<String, &'a str> = BTreeMap::new();

    let root = cfg.output_root();

    for named in &spec.types {
        if named.id == NULL_ID {
            continue;
        }
        let Some(raw) = lookup_extension(spec, named, "x-kotlin-source") else {
            // The forge IR explodes every property/parameter/response/variant
            // into its own synthetic NamedType so each can carry independent
            // docs (see generator-html-docs/src/schema_filter.rs). We don't
            // require annotations on those — references to them get inlined
            // as the underlying Kotlin type at the use site. Only user-facing
            // components (named schemas) demand x-kotlin-source.
            if !is_synthetic_id(&named.id) {
                missing.push(named.id.as_str());
            }
            continue;
        };
        let target = match KotlinTarget::parse(&raw) {
            Ok(t) => t,
            Err(e) => {
                // Annotation present but unparseable. Common during a rollout:
                // free-text placeholders ("(none — handled inline elsewhere)")
                // or partial object values. Don't fail the whole run — warn
                // and treat as un-annotated. Synthetic types stay synthetic
                // and get inlined; user-facing types fall into the
                // missing-annotation diagnostic so the real gap is visible.
                errors.push(diag::warning(
                    "generator-kotlin-types/x-kotlin-source-malformed",
                    format!(
                        "type '{}': x-kotlin-source is not a valid annotation ({e}); ignoring",
                        named.id
                    ),
                ));
                if !is_synthetic_id(&named.id) {
                    missing.push(named.id.as_str());
                }
                continue;
            }
        };
        let output_path = join_path(root, &target.file);
        let visibility = match lookup_extension(spec, named, "x-kotlin-visibility")
            .as_ref()
            .and_then(|v| v.as_str())
        {
            None => Visibility::Internal,
            Some("internal") => Visibility::Internal,
            Some("public") => Visibility::Public,
            Some(other) => {
                errors.push(diag::warning(
                    "generator-kotlin-types/x-kotlin-visibility-invalid",
                    format!(
                        "type '{}': x-kotlin-visibility value '{other}' is not one of \
                         'public' / 'internal'; defaulting to 'internal'",
                        named.id
                    ),
                ));
                Visibility::Internal
            }
        };
        let fqcn = target.fqcn();
        if let Some(existing_id) = by_fqcn.get(&fqcn) {
            errors.push(diag::warning(
                "generator-kotlin-types/x-kotlin-source-duplicate",
                format!(
                    "types '{}' and '{}' share x-kotlin-source FQCN '{fqcn}'; \
                     keeping the first, skipping the duplicate (would emit duplicate \
                     `data class` declarations otherwise)",
                    existing_id, named.id
                ),
            ));
            continue;
        }
        by_fqcn.insert(fqcn, named.id.as_str());
        index.insert(
            named.id.as_str(),
            ResolvedTarget {
                target,
                output_path,
                visibility,
            },
        );
    }

    if !missing.is_empty() {
        let mut list = missing.clone();
        list.sort_unstable();
        let msg = format!(
            "{} type(s) are missing an x-kotlin-source extension: {}",
            list.len(),
            list.join(", ")
        );
        let diag = match cfg.missing_extension_policy() {
            MissingExtensionPolicy::Error => diag::error(
                "generator-kotlin-types/x-kotlin-source-missing",
                msg,
            ),
            MissingExtensionPolicy::Warn => diag::warning(
                "generator-kotlin-types/x-kotlin-source-missing",
                msg,
            ),
        };
        errors.push(diag);
    }

    (index, errors)
}

fn lookup_extension(spec: &Ir, named: &NamedType, key: &str) -> Option<serde_json::Value> {
    let (_, vref) = named.extensions.iter().find(|(k, _)| k == key)?;
    Some(values_ext::resolve_to_serde(&spec.values, *vref))
}

fn build_variant_index<'a>(spec: &'a Ir, index: &FqcnIndex<'a>) -> VariantIndex<'a> {
    let mut out: VariantIndex = BTreeMap::new();
    for named in &spec.types {
        let TypeDef::Union(u) = &named.definition else {
            continue;
        };
        let Some(parent) = index.get(named.id.as_str()) else {
            continue;
        };
        if let Some(disc) = effective_discriminator(spec, u) {
            for (variant_ref, tag) in &disc.mapping {
                let id: &str = match spec.types.iter().find(|t| &t.id == variant_ref) {
                    Some(t) => t.id.as_str(),
                    None => continue,
                };
                out.insert(
                    id,
                    VariantInfo {
                        parent_class: parent.target.class_name.clone(),
                        parent_package: parent.target.package.clone(),
                        discriminator: Some(disc.property_name.clone()),
                        serial_name: Some(tag.clone()),
                    },
                );
            }
            continue;
        }
        // Untagged single-variant union: the lone variant inherits the sealed
        // interface (so consumers can pattern-match on the sealed parent) but
        // there's no discriminator wire tag and no property to drop.
        if u.variants.len() == 1 && u.discriminator.is_none() {
            let variant_ref = &u.variants[0].r#type;
            if let Some(t) = spec.types.iter().find(|t| &t.id == variant_ref) {
                if matches!(t.definition, TypeDef::Object(_)) {
                    out.insert(
                        t.id.as_str(),
                        VariantInfo {
                            parent_class: parent.target.class_name.clone(),
                            parent_package: parent.target.package.clone(),
                            discriminator: None,
                            serial_name: None,
                        },
                    );
                }
            }
        }
    }
    out
}

/// What kotlinx.serialization needs to model a union: the property name that
/// carries the discriminator tag, and each variant's wire-side tag value.
/// Mirrors the explicit OAS `discriminator` shape, but is also inferred from
/// the common idiom where each variant carries `type: const: "<tag>"` and the
/// union itself omits `discriminator`.
#[derive(Debug, Clone)]
pub struct EffectiveDiscriminator {
    pub property_name: String,
    /// `(variant_type_ref, wire_tag)` pairs.
    pub mapping: Vec<(String, String)>,
}

pub fn effective_discriminator(spec: &Ir, u: &UnionType) -> Option<EffectiveDiscriminator> {
    if let Some(disc) = &u.discriminator {
        // Explicit discriminator: build the variant_ref → tag mapping. OAS
        // `mapping` keys are tag → `$ref`/id; accept either the bare id or a
        // `.../Foo` suffix match against the variant's id.
        let mut mapping: Vec<(String, String)> = Vec::new();
        for variant in &u.variants {
            let tag = disc
                .mapping
                .iter()
                .find_map(|(tag, target)| {
                    if target == &variant.r#type
                        || target.ends_with(&format!("/{}", variant.r#type))
                    {
                        Some(tag.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| variant.r#type.clone());
            mapping.push((variant.r#type.clone(), tag));
        }
        return Some(EffectiveDiscriminator {
            property_name: disc.property_name.clone(),
            mapping,
        });
    }

    // No explicit discriminator: try to infer one. Every variant must be an
    // object whose properties include at least one single-value string enum
    // (the IR's spelling for `const: "<tag>"`). If all variants share exactly
    // one such property name, that's the discriminator and the per-variant
    // const values are the tags.
    let mut per_variant: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for variant in &u.variants {
        let named = spec.types.iter().find(|t| t.id == variant.r#type)?;
        let TypeDef::Object(obj) = &named.definition else {
            return None;
        };
        let mut candidates: BTreeMap<String, String> = BTreeMap::new();
        for prop in &obj.properties {
            let prop_type = spec.types.iter().find(|t| t.id == prop.r#type)?;
            if let TypeDef::EnumString(e) = &prop_type.definition {
                if e.values.len() == 1 {
                    candidates.insert(prop.name.clone(), e.values[0].value.clone());
                }
            }
        }
        if candidates.is_empty() {
            return None;
        }
        per_variant.push((variant.r#type.clone(), candidates));
    }
    if per_variant.is_empty() {
        return None;
    }

    // Intersect candidate property names across variants.
    let mut common: BTreeSet<String> = per_variant[0].1.keys().cloned().collect();
    for (_, c) in per_variant.iter().skip(1) {
        let here: BTreeSet<String> = c.keys().cloned().collect();
        common = common.intersection(&here).cloned().collect();
    }
    // Require unambiguity: exactly one shared single-value property across
    // every variant. Two would be ambiguous; zero means there's nothing to
    // infer from.
    if common.len() != 1 {
        return None;
    }
    let property_name = common.into_iter().next().unwrap();
    let mapping = per_variant
        .into_iter()
        .map(|(tr, c)| (tr, c[&property_name].clone()))
        .collect();
    Some(EffectiveDiscriminator {
        property_name,
        mapping,
    })
}

/// Mirrors `generator-html-docs/src/schema_filter.rs::is_synthetic_id`. The
/// forge IR's documented synthetic naming pattern for types it generated
/// from inline property / parameter / response shapes.
pub fn is_synthetic_id(id: &str) -> bool {
    const MARKERS: &[&str] = &[
        "_property_",
        "_param_",
        "_response_",
        "_request_",
        "_variant_",
        "_part_",
        "_items",
        "_fallback",
        "_switchBranch",
    ];
    MARKERS.iter().any(|m| id.contains(m))
}

fn join_path(root: &str, file: &str) -> String {
    if root == "." || root.is_empty() {
        return file.to_string();
    }
    let mut p = PathBuf::from(root);
    p.push(file);
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_path_default_root() {
        assert_eq!(join_path(".", "a/b.kt"), "a/b.kt");
        assert_eq!(join_path("", "a/b.kt"), "a/b.kt");
    }

    #[test]
    fn join_path_with_root() {
        assert_eq!(join_path("gen", "a/b.kt"), "gen/a/b.kt");
    }
}
