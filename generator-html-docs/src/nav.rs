//! Tag tree construction. Walks `ir.tags` (OAS 3.2 `parent` field)
//! into a rooted forest, attaches each operation to its tag, and adds
//! a synthetic `_default` tag for any operations the spec left
//! untagged.

use std::collections::BTreeMap;

use forge_plugin_sdk::ir::{Ir, Tag};
use serde::Serialize;

use crate::markdown;
use crate::paths;

/// The synthetic tag name we use for operations that declare no tags.
/// Lives at the root of the tree alongside real tags so untagged
/// endpoints stay discoverable from the sidebar.
pub const DEFAULT_TAG: &str = "Default";

#[derive(Debug, Clone, Serialize)]
pub struct NavOp {
    pub id: String,
    pub method: String,
    pub method_class: String,
    pub path_template: String,
    pub summary: Option<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavTag {
    pub name: String,
    /// Slug chain from root to this tag. The last element is this
    /// tag's own slug; `tags/<slug_chain>/index.html` is its page.
    pub slug_chain: Vec<String>,
    /// `slug_chain` joined with `/`. Precomputed because MiniJinja's
    /// HTML autoescape turns the joined string's slashes into `&#x2f;`
    /// — by passing a single safe string, templates can use this
    /// verbatim in `data-*` and `href` attributes.
    pub slug_path: String,
    /// Just the leaf slug (last element of `slug_chain`).
    pub slug: String,
    pub summary: Option<String>,
    pub description_html: Option<String>,
    pub external_docs_url: Option<String>,
    pub external_docs_description: Option<String>,
    pub operations: Vec<NavOp>,
    pub children: Vec<NavTag>,
    /// Total operation count including descendants — for the sidebar
    /// count badges.
    pub total_op_count: usize,
}

/// The full forest of tag roots in the order they should render.
#[derive(Debug, Clone, Serialize)]
pub struct Nav {
    pub roots: Vec<NavTag>,
}

impl Nav {
    pub fn walk(&self) -> impl Iterator<Item = &NavTag> {
        let mut out: Vec<&NavTag> = Vec::new();
        for root in &self.roots {
            collect(root, &mut out);
        }
        out.into_iter()
    }
}

fn collect<'a>(node: &'a NavTag, out: &mut Vec<&'a NavTag>) {
    out.push(node);
    for child in &node.children {
        collect(child, out);
    }
}

pub fn method_class(method: &str) -> &'static str {
    match method {
        "GET" => "method-get",
        "POST" => "method-post",
        "PUT" => "method-put",
        "PATCH" => "method-patch",
        "DELETE" => "method-delete",
        "HEAD" => "method-head",
        "OPTIONS" => "method-options",
        "TRACE" => "method-trace",
        _ => "method-other",
    }
}

pub fn build(spec: &Ir) -> Nav {
    // Index declared tags by name. Operations may reference tags that
    // aren't declared at the document root — we synthesize a bare entry
    // for those so the operation still has a place to live.
    let mut by_name: BTreeMap<String, &Tag> = BTreeMap::new();
    for t in &spec.tags {
        by_name.insert(t.name.clone(), t);
    }

    // First pass: figure out which tags any operation references but
    // which the document didn't declare. We don't mutate spec; we
    // remember the names.
    let mut synthesized: Vec<String> = Vec::new();
    for op in &spec.operations {
        for name in &op.tags {
            if !by_name.contains_key(name) && !synthesized.contains(name) {
                synthesized.push(name.clone());
            }
        }
    }

    // Untagged operations bucket under the synthetic DEFAULT_TAG.
    let has_untagged = spec.operations.iter().any(|op| op.tags.is_empty());
    if has_untagged
        && !by_name.contains_key(DEFAULT_TAG)
        && !synthesized.contains(&DEFAULT_TAG.to_string())
    {
        synthesized.push(DEFAULT_TAG.into());
    }

    // Pre-compute the slug-chain for every tag name. A tag's chain is
    // its ancestors' chain plus its own slug. Synthesized tags are
    // root-level (no parent).
    let mut chain_for: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut visiting: Vec<String> = Vec::new(); // cycle guard

    // All tag names, declared and synthesized:
    let all_names: Vec<String> = spec
        .tags
        .iter()
        .map(|t| t.name.clone())
        .chain(synthesized.iter().cloned())
        .collect();
    for name in &all_names {
        compute_chain(name, &by_name, &mut chain_for, &mut visiting);
    }

    // Build NavTag records for every tag, keyed by name, then assemble
    // the parent/child tree afterwards.
    let mut nodes: BTreeMap<String, NavTag> = BTreeMap::new();
    for name in &all_names {
        let chain = chain_for
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![paths::slugify(name)]);
        let (summary, description, ext) = match by_name.get(name) {
            Some(t) => (
                t.summary.clone(),
                t.description.as_deref().map(markdown::render),
                t.external_docs.clone(),
            ),
            None => (None, None, None),
        };
        let slug_path = chain.join("/");
        let slug = chain.last().cloned().unwrap_or_default();
        nodes.insert(
            name.clone(),
            NavTag {
                name: name.clone(),
                slug_chain: chain,
                slug_path,
                slug,
                summary,
                description_html: description,
                external_docs_url: ext.as_ref().map(|d| d.url.clone()),
                external_docs_description: ext.as_ref().and_then(|d| d.description.clone()),
                operations: Vec::new(),
                children: Vec::new(),
                total_op_count: 0,
            },
        );
    }

    // Attach operations. Each op surfaces under each tag it lists;
    // untagged ops attach to DEFAULT_TAG.
    for op in &spec.operations {
        let nav_op = NavOp {
            id: op.id.clone(),
            method: op.method.as_str().to_owned(),
            method_class: method_class(op.method.as_str()).to_owned(),
            path_template: op.path_template.clone(),
            summary: op.summary.clone(),
            deprecated: op.deprecated,
        };
        if op.tags.is_empty() {
            if let Some(n) = nodes.get_mut(DEFAULT_TAG) {
                n.operations.push(nav_op);
            }
        } else {
            for name in &op.tags {
                if let Some(n) = nodes.get_mut(name) {
                    n.operations.push(nav_op.clone());
                }
            }
        }
    }

    // Sort operations within each tag for deterministic output.
    for n in nodes.values_mut() {
        n.operations.sort_by(|a, b| {
            a.path_template
                .cmp(&b.path_template)
                .then_with(|| a.method.cmp(&b.method))
        });
    }

    // Resolve the parent map (name -> Option<parent name>). Synthesized
    // entries have no parent.
    let parent_of: BTreeMap<String, Option<String>> = all_names
        .iter()
        .map(|n| {
            let parent = by_name
                .get(n)
                .and_then(|t| t.parent.clone())
                .filter(|p| nodes.contains_key(p));
            (n.clone(), parent)
        })
        .collect();

    // Assemble the tree: iterate names depth-first so children are
    // attached only after their parent record is finalized.
    // We can't move out of `nodes` while still iterating it, so do a
    // separate take/insert dance keyed by name.
    let mut roots: Vec<String> = Vec::new();
    let mut child_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in &all_names {
        match parent_of.get(n).and_then(|p| p.as_ref()) {
            Some(parent) => child_of.entry(parent.clone()).or_default().push(n.clone()),
            None => roots.push(n.clone()),
        }
    }

    // Recursive assembly via post-order on the parent->children map.
    fn build_node(
        name: &str,
        nodes: &mut BTreeMap<String, NavTag>,
        child_of: &BTreeMap<String, Vec<String>>,
    ) -> NavTag {
        let children_names = child_of.get(name).cloned().unwrap_or_default();
        let mut children: Vec<NavTag> = children_names
            .iter()
            .map(|c| build_node(c, nodes, child_of))
            .collect();
        // Stable: declaration order from the spec, with synthesized
        // children landing at the end.
        children.sort_by(|a, b| a.name.cmp(&b.name));
        let mut node = nodes
            .remove(name)
            .expect("name was inserted in nodes map at population time");
        let descendant_ops: usize = children.iter().map(|c| c.total_op_count).sum();
        node.total_op_count = node.operations.len() + descendant_ops;
        node.children = children;
        node
    }

    let mut root_nodes: Vec<NavTag> = roots
        .iter()
        .map(|name| build_node(name, &mut nodes, &child_of))
        .collect();
    root_nodes.sort_by(|a, b| a.name.cmp(&b.name));
    Nav { roots: root_nodes }
}

fn compute_chain(
    name: &str,
    by_name: &BTreeMap<String, &Tag>,
    chain_for: &mut BTreeMap<String, Vec<String>>,
    visiting: &mut Vec<String>,
) -> Vec<String> {
    if let Some(c) = chain_for.get(name) {
        return c.clone();
    }
    if visiting.iter().any(|v| v == name) {
        // Cycle in `parent` declarations — degrade gracefully to a
        // root-level chain rather than recursing forever.
        let chain = vec![paths::slugify(name)];
        chain_for.insert(name.into(), chain.clone());
        return chain;
    }
    visiting.push(name.into());
    let chain = match by_name.get(name).and_then(|t| t.parent.as_ref()) {
        Some(parent) if by_name.contains_key(parent) => {
            let mut c = compute_chain(parent, by_name, chain_for, visiting);
            c.push(paths::slugify(name));
            c
        }
        _ => vec![paths::slugify(name)],
    };
    visiting.pop();
    chain_for.insert(name.into(), chain.clone());
    chain
}
