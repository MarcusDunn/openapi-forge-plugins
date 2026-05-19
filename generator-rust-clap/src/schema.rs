//! JSON Schema 2020-12 rendering from the IR's type pool.
//!
//! Used to embed body / response schemas as `&'static str` constants in
//! the generated CLI so users can run `<op> --body-schema` /
//! `--response-schema` to discover the expected shape without bloating
//! `--help`.

use std::collections::{BTreeMap, HashMap, HashSet};

use forge_plugin_sdk::ir::{
    AdditionalProperties, Body, BodyContent, EnumIntType, EnumStringType, NamedType, ObjectType,
    PrimitiveKind, PrimitiveType, Response, ResponseStatus, TypeDef, TypeRef, UnionKind, UnionType,
    Value as IrValue,
};
use forge_plugin_sdk::serde_json::{self, json, Map, Value};
use forge_plugin_sdk::values_ext;

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// JSON Schema for the request body. Picks `application/json` if
/// present, falls back to the first content entry. Returns `None` if
/// the body has no content list (shouldn't happen in well-formed IR).
pub fn render_body_schema(types: &[NamedType], values: &[IrValue], body: &Body) -> Option<String> {
    let content = pick_content(&body.content)?;
    let mut r = Renderer::new(types, values);
    let root = r.render_ref(&content.r#type);
    Some(serialize_with_meta(root, r.into_defs(), Some(content)))
}

/// JSON Schemas for the response bodies, keyed by status (`"200"`,
/// `"4XX"`, `"default"`). Skips responses with no content. Returns
/// `None` if no response has any content.
pub fn render_response_schemas(
    types: &[NamedType],
    values: &[IrValue],
    responses: &[Response],
) -> Option<String> {
    let mut r = Renderer::new(types, values);
    let mut by_status: BTreeMap<String, Value> = BTreeMap::new();
    for resp in responses {
        let Some(content) = pick_content(&resp.content) else {
            continue;
        };
        let key = status_key(&resp.status);
        by_status.insert(key, r.render_ref(&content.r#type));
    }
    if by_status.is_empty() {
        return None;
    }
    let defs = r.into_defs();

    // Flat status→schema map. $defs sits at the document root so all
    // `$ref: "#/$defs/<id>"` pointers under any status resolve. The
    // top-level object is intentionally not itself a JSON Schema —
    // it's a discoverability index. Pipe through
    // `jq '."201"'` to get a single status' schema.
    let mut top: Map<String, Value> = by_status.into_iter().collect();
    if !defs.is_empty() {
        top.insert("$defs".into(), Value::Object(defs.into_iter().collect()));
    }
    Some(serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_else(|_| "{}".into()))
}

fn pick_content(content: &[BodyContent]) -> Option<&BodyContent> {
    content
        .iter()
        .find(|c| c.media_type.eq_ignore_ascii_case("application/json"))
        .or_else(|| content.first())
}

fn status_key(s: &ResponseStatus) -> String {
    match s {
        ResponseStatus::Explicit { code } => code.to_string(),
        ResponseStatus::Default => "default".into(),
        ResponseStatus::Range { class } => format!("{class}XX"),
    }
}

fn serialize_with_meta(
    mut root: Value,
    defs: BTreeMap<String, Value>,
    content: Option<&BodyContent>,
) -> String {
    // Promote root to top-level $schema-bearing object. If `root` is
    // a `$ref`, keep it as a sibling — JSON Schema 2020-12 permits
    // sibling keywords next to `$ref`.
    let obj = match &mut root {
        Value::Object(m) => m,
        _ => {
            // Non-object roots (e.g. `true`/`false` boolean schema)
            // are theoretically possible; wrap them.
            let mut wrap = Map::new();
            wrap.insert("$schema".into(), Value::String(DIALECT.into()));
            wrap.insert("schema".into(), root);
            return serde_json::to_string_pretty(&Value::Object(wrap))
                .unwrap_or_else(|_| "{}".into());
        }
    };
    obj.insert("$schema".into(), Value::String(DIALECT.into()));
    if let Some(c) = content {
        if !c.media_type.is_empty() {
            obj.insert(
                "contentMediaType".into(),
                Value::String(c.media_type.clone()),
            );
        }
    }
    if !defs.is_empty() {
        obj.insert("$defs".into(), Value::Object(defs.into_iter().collect()));
    }
    serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".into())
}

struct Renderer<'a> {
    types_by_id: HashMap<&'a str, &'a NamedType>,
    values: &'a [IrValue],
    on_stack: HashSet<String>,
    promoted: HashSet<String>,
    defs: BTreeMap<String, Value>,
}

impl<'a> Renderer<'a> {
    fn new(types: &'a [NamedType], values: &'a [IrValue]) -> Self {
        let types_by_id = types.iter().map(|t| (t.id.as_str(), t)).collect();
        Self {
            types_by_id,
            values,
            on_stack: HashSet::new(),
            promoted: HashSet::new(),
            defs: BTreeMap::new(),
        }
    }

    fn into_defs(self) -> BTreeMap<String, Value> {
        self.defs
    }

    /// Render a reference to a named type. Inlines the type unless it
    /// participates in a cycle, in which case we promote it to
    /// `$defs` and emit a `$ref`.
    fn render_ref(&mut self, id: &TypeRef) -> Value {
        if self.promoted.contains(id) {
            return ref_to(id);
        }
        if self.on_stack.contains(id) {
            // Back-edge: this is recursion. Promote and return $ref.
            self.promote(id.clone());
            return ref_to(id);
        }
        let Some(named) = self.types_by_id.get(id.as_str()).copied() else {
            // Dangling ref — emit an empty schema with a comment-ish
            // description rather than crashing the generator.
            return json!({ "description": format!("(unresolved type: {id})") });
        };

        self.on_stack.insert(id.clone());
        let mut schema = self.render_named(named);
        self.on_stack.remove(id);
        decorate(&mut schema, named, self.values);

        if self.promoted.contains(id) {
            // Promotion happened during this call (self-recursion).
            // Stash the rendered schema in defs and return $ref.
            self.defs.insert(id.clone(), schema);
            ref_to(id)
        } else {
            schema
        }
    }

    fn promote(&mut self, id: String) {
        // Inserting an empty placeholder so callers see it as
        // "already in defs" before we finish rendering.
        self.defs.entry(id.clone()).or_insert(Value::Null);
        self.promoted.insert(id);
    }

    fn render_named(&mut self, named: &NamedType) -> Value {
        match &named.definition {
            TypeDef::Primitive(p) => render_primitive(p, self.values),
            TypeDef::Array(a) => {
                let mut m = Map::new();
                m.insert("type".into(), Value::String("array".into()));
                m.insert("items".into(), self.render_ref(&a.items));
                if let Some(v) = a.constraints.min_items {
                    m.insert("minItems".into(), json!(v));
                }
                if let Some(v) = a.constraints.max_items {
                    m.insert("maxItems".into(), json!(v));
                }
                if a.constraints.unique_items {
                    m.insert("uniqueItems".into(), Value::Bool(true));
                }
                Value::Object(m)
            }
            TypeDef::Object(o) => self.render_object(o),
            TypeDef::EnumString(EnumStringType { values }) => {
                let arr: Vec<Value> = values
                    .iter()
                    .map(|v| Value::String(v.value.clone()))
                    .collect();
                json!({ "type": "string", "enum": arr })
            }
            TypeDef::EnumInt(EnumIntType { values, .. }) => {
                let arr: Vec<Value> = values.iter().map(|v| json!(v.value)).collect();
                json!({ "type": "integer", "enum": arr })
            }
            TypeDef::Union(u) => self.render_union(u),
            TypeDef::Null => json!({ "type": "null" }),
        }
    }

    fn render_object(&mut self, o: &ObjectType) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String("object".into()));

        let mut props = Map::new();
        let mut required: Vec<Value> = Vec::new();
        for p in &o.properties {
            let mut child = self.render_ref(&p.r#type);
            if let Value::Object(cm) = &mut child {
                if let Some(doc) = &p.documentation {
                    cm.entry("description".to_string())
                        .or_insert(Value::String(doc.clone()));
                }
                if p.deprecated {
                    cm.entry("deprecated".to_string())
                        .or_insert(Value::Bool(true));
                }
                if p.read_only {
                    cm.entry("readOnly".to_string())
                        .or_insert(Value::Bool(true));
                }
                if p.write_only {
                    cm.entry("writeOnly".to_string())
                        .or_insert(Value::Bool(true));
                }
                if let Some(d) = p.default {
                    cm.entry("default".to_string())
                        .or_insert_with(|| values_ext::resolve_to_serde(self.values, d));
                }
            }
            props.insert(p.name.clone(), child);
            if p.required {
                required.push(Value::String(p.name.clone()));
            }
        }
        m.insert("properties".into(), Value::Object(props));
        if !required.is_empty() {
            m.insert("required".into(), Value::Array(required));
        }
        match &o.additional_properties {
            AdditionalProperties::Forbidden => {
                m.insert("additionalProperties".into(), Value::Bool(false));
            }
            AdditionalProperties::Any => {
                // 2020-12 default; omit for compactness.
            }
            AdditionalProperties::Typed { r#type } => {
                let v = self.render_ref(r#type);
                m.insert("additionalProperties".into(), v);
            }
        }
        if let Some(v) = o.constraints.min_properties {
            m.insert("minProperties".into(), json!(v));
        }
        if let Some(v) = o.constraints.max_properties {
            m.insert("maxProperties".into(), json!(v));
        }
        Value::Object(m)
    }

    fn render_union(&mut self, u: &UnionType) -> Value {
        let variants: Vec<Value> = u
            .variants
            .iter()
            .map(|v| self.render_ref(&v.r#type))
            .collect();
        let key = match u.kind {
            UnionKind::OneOf => "oneOf",
            UnionKind::AnyOf => "anyOf",
        };
        let mut m = Map::new();
        m.insert(key.into(), Value::Array(variants));
        if let Some(d) = &u.discriminator {
            let mapping: Map<String, Value> = d
                .mapping
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(format!("#/$defs/{v}"))))
                .collect();
            m.insert(
                "discriminator".into(),
                json!({
                    "propertyName": d.property_name.clone(),
                    "mapping": mapping,
                }),
            );
        }
        Value::Object(m)
    }
}

fn ref_to(id: &str) -> Value {
    json!({ "$ref": format!("#/$defs/{id}") })
}

fn render_primitive(p: &PrimitiveType, values: &[IrValue]) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), Value::String(prim_type(p.kind).into()));
    let c = &p.constraints;
    if let Some(fmt) = &c.format_extension {
        m.insert("format".into(), Value::String(fmt.clone()));
    }
    if let Some(r) = c.minimum {
        m.insert("minimum".into(), values_ext::resolve_to_serde(values, r));
    }
    if let Some(r) = c.maximum {
        m.insert("maximum".into(), values_ext::resolve_to_serde(values, r));
    }
    if let Some(r) = c.exclusive_minimum {
        m.insert(
            "exclusiveMinimum".into(),
            values_ext::resolve_to_serde(values, r),
        );
    }
    if let Some(r) = c.exclusive_maximum {
        m.insert(
            "exclusiveMaximum".into(),
            values_ext::resolve_to_serde(values, r),
        );
    }
    if let Some(r) = c.multiple_of {
        m.insert("multipleOf".into(), values_ext::resolve_to_serde(values, r));
    }
    if let Some(v) = c.min_length {
        m.insert("minLength".into(), json!(v));
    }
    if let Some(v) = c.max_length {
        m.insert("maxLength".into(), json!(v));
    }
    if let Some(s) = &c.pattern {
        m.insert("pattern".into(), Value::String(s.clone()));
    }
    if let Some(s) = &c.content_encoding {
        m.insert("contentEncoding".into(), Value::String(s.clone()));
    }
    if let Some(s) = &c.content_media_type {
        m.insert("contentMediaType".into(), Value::String(s.clone()));
    }
    Value::Object(m)
}

fn prim_type(k: PrimitiveKind) -> &'static str {
    match k {
        PrimitiveKind::String => "string",
        PrimitiveKind::Integer => "integer",
        PrimitiveKind::Number => "number",
        PrimitiveKind::Bool => "boolean",
    }
}

/// Add title / description / default from a NamedType onto an inline
/// schema. Doesn't touch `$ref` schemas — those keep their pointer.
fn decorate(schema: &mut Value, named: &NamedType, values: &[IrValue]) {
    let Value::Object(m) = schema else {
        return;
    };
    if m.contains_key("$ref") {
        return;
    }
    if let Some(t) = &named.title {
        m.entry("title".to_string())
            .or_insert(Value::String(t.clone()));
    }
    if let Some(d) = &named.documentation {
        m.entry("description".to_string())
            .or_insert(Value::String(d.clone()));
    }
    if let Some(d) = named.default {
        m.entry("default".to_string())
            .or_insert_with(|| values_ext::resolve_to_serde(values, d));
    }
    if named.read_only {
        m.entry("readOnly".to_string()).or_insert(Value::Bool(true));
    }
    if named.write_only {
        m.entry("writeOnly".to_string())
            .or_insert(Value::Bool(true));
    }
}
