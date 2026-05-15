//! Tag tree construction. Pure logic over `forge_plugin_sdk::ir`; no
//! `wit_bindgen` types here. Drives the nested clap subcommand
//! emission in `emit::main_rs`.
//!
//! Algorithm:
//!   1. Index declared tags by name.
//!   2. Bucket each operation under its **first** declared tag, or
//!      treat as untagged. Multi-tag ops still get a single home.
//!   3. Tags referenced by an op but not declared become synthetic
//!      roots — never nested.
//!   4. Recurse from roots: a root is a declared tag with no parent,
//!      or a declared tag whose parent doesn't resolve, or a synthetic
//!      tag.
//!   5. Untagged ops collect under a root group named `_misc`.
//!
//! Output is deterministic: roots and children are sorted by name.

use std::collections::{BTreeMap, BTreeSet};

use forge_plugin_sdk::ir::{Ir, Operation, Tag};

pub struct TagTree<'a> {
    pub roots: Vec<TagGroup<'a>>,
}

pub struct TagGroup<'a> {
    pub name: String,
    pub tag: Option<&'a Tag>,
    pub direct_ops: Vec<&'a Operation>,
    pub children: Vec<TagGroup<'a>>,
}

impl<'a> TagGroup<'a> {
    /// `_misc` is the synthetic catch-all for operations with no tags.
    pub fn is_misc(&self) -> bool {
        self.tag.is_none() && self.children.is_empty() && self.name == MISC_NAME
    }
}

pub const MISC_NAME: &str = "_misc";

pub fn build<'a>(ir: &'a Ir) -> TagTree<'a> {
    let declared: BTreeMap<&str, &Tag> = ir.tags.iter().map(|t| (t.name.as_str(), t)).collect();

    // Resolve each declared tag's effective parent: only honored when the
    // parent name actually exists. Unresolved parents → root.
    let mut children_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut declared_roots: Vec<&str> = Vec::new();
    for tag in &ir.tags {
        if let Some(parent) = tag.parent.as_deref() {
            if declared.contains_key(parent) {
                children_of
                    .entry(parent)
                    .or_default()
                    .push(tag.name.as_str());
                continue;
            }
        }
        declared_roots.push(tag.name.as_str());
    }
    for v in children_of.values_mut() {
        v.sort_unstable();
    }
    declared_roots.sort_unstable();

    // Synthetic roots for tag names referenced by ops but not declared.
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for op in &ir.operations {
        for t in &op.tags {
            referenced.insert(t.as_str());
        }
    }
    let mut synthetic_roots: Vec<&str> = referenced
        .iter()
        .copied()
        .filter(|name| !declared.contains_key(name))
        .collect();
    synthetic_roots.sort_unstable();

    // Bucket ops by their first tag.
    let mut by_tag: BTreeMap<&str, Vec<&Operation>> = BTreeMap::new();
    let mut untagged: Vec<&Operation> = Vec::new();
    for op in &ir.operations {
        match op.tags.first() {
            Some(tag) => by_tag.entry(tag.as_str()).or_default().push(op),
            None => untagged.push(op),
        }
    }

    // Recursive walk.
    let mut roots: Vec<TagGroup<'a>> = Vec::new();
    for name in declared_roots.iter().chain(synthetic_roots.iter()) {
        roots.push(build_group(name, &declared, &children_of, &by_tag));
    }
    if !untagged.is_empty() {
        roots.push(TagGroup {
            name: MISC_NAME.into(),
            tag: None,
            direct_ops: untagged,
            children: vec![],
        });
    }

    TagTree { roots }
}

fn build_group<'a>(
    name: &str,
    declared: &BTreeMap<&str, &'a Tag>,
    children_of: &BTreeMap<&str, Vec<&str>>,
    by_tag: &BTreeMap<&str, Vec<&'a Operation>>,
) -> TagGroup<'a> {
    let tag = declared.get(name).copied();
    let direct_ops = by_tag.get(name).cloned().unwrap_or_default();
    let children: Vec<TagGroup<'a>> = children_of
        .get(name)
        .into_iter()
        .flatten()
        .map(|child| build_group(child, declared, children_of, by_tag))
        .collect();
    TagGroup {
        name: name.to_string(),
        tag,
        direct_ops,
        children,
    }
}
