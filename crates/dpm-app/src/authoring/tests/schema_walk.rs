//! Walks the traced model and `plan.schema.json` side by side from `Plan` and the schema root,
//! reporting every place where they disagree: a field or variant one side has and the other
//! lacks, a nullable schema for a required model value, a leaf of the wrong JSON type or format,
//! and a `$defs` entry the model never reaches.
//!
//! Fields of internally tagged variants are compared by name only; their values bypass the
//! type trace.

use super::model_record::TaggedFields;
use super::model_trace::{Container, Fields, Registry, Shape, Variant};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Every disagreement between the schema and the model; empty when they match.
pub(super) fn compare(schema: &Value, types: &Registry, tagged: &TaggedFields) -> Vec<String> {
    let mut walk = Walk {
        root: schema,
        types,
        tagged,
        visited: BTreeSet::new(),
        reached: BTreeSet::new(),
        gaps: Vec::new(),
    };
    walk.named("Plan", schema, "#".into());
    let defined = schema.get("$defs").and_then(Value::as_object);
    for name in defined.into_iter().flat_map(Map::keys) {
        if !walk.reached.contains(name) {
            walk.gaps.push(format!(
                "#/$defs/{name}: no model type reaches this definition"
            ));
        }
    }
    walk.gaps
}

struct Walk<'a> {
    root: &'a Value,
    types: &'a Registry,
    tagged: &'a TaggedFields,
    visited: BTreeSet<(String, String)>,
    reached: BTreeSet<String>,
    gaps: Vec<String>,
}

impl<'a> Walk<'a> {
    fn resolve(&mut self, mut node: &'a Value, mut at: String) -> (&'a Value, String) {
        while let Some(target) = node.get("$ref").and_then(Value::as_str) {
            if let Some(name) = target.strip_prefix("#/$defs/") {
                self.reached.insert(name.to_string());
            }
            match self.root.pointer(target.trim_start_matches('#')) {
                Some(resolved) => (node, at) = (resolved, target.to_string()),
                None => {
                    self.gaps.push(format!("{at}: unresolved {target}"));
                    break;
                }
            }
        }
        (node, at)
    }

    /// The non-null branch of `anyOf [T, null]`, or the node itself when it is not nullable.
    fn without_null(&mut self, node: &'a Value, at: String) -> Option<(&'a Value, String)> {
        let Some(branches) = node.get("anyOf").and_then(Value::as_array) else {
            return Some((node, at));
        };
        let mut others = branches
            .iter()
            .enumerate()
            .filter(|(_, b)| b.get("type").and_then(Value::as_str) != Some("null"));
        match (others.next(), others.next()) {
            (Some((i, branch)), None) => Some(self.resolve(branch, format!("{at}/anyOf/{i}"))),
            _ => None,
        }
    }

    fn shape(&mut self, shape: &Shape, node: &'a Value, at: String) {
        let (node, at) = self.resolve(node, at);
        if let Shape::Option(inner) = shape {
            match self.without_null(node, at.clone()) {
                Some((node, at)) => self.shape(inner, node, at),
                None => self.gaps.push(format!("{at}: not an optional value")),
            }
            return;
        }
        if nullable(node) {
            self.gaps.push(format!(
                "{at}: allows null, but the model value is required"
            ));
        }
        match shape {
            Shape::Seq(item) => match (json_type(node), node.get("items")) {
                (Some("array"), Some(items)) => self.shape(item, items, format!("{at}/items")),
                _ => self.gaps.push(format!("{at}: the model writes an array")),
            },
            Shape::Map(key, value) => self.map(key, value, node, at),
            Shape::Named(name) => self.named(name, node, at),
            leaf => self.leaf(leaf, node, &at),
        }
    }

    fn map(&mut self, key: &Shape, value: &Shape, node: &'a Value, at: String) {
        let names = node
            .pointer("/propertyNames/format")
            .and_then(Value::as_str);
        if (*key == Shape::Uuid) != (names == Some("uuid")) {
            self.gaps.push(format!(
                "{at}: map keys are {key:?}, schema key format {names:?}"
            ));
        }
        match (json_type(node), node.get("additionalProperties")) {
            (Some("object"), Some(entry)) if entry.is_object() => {
                self.shape(value, entry, format!("{at}/additionalProperties"));
            }
            _ => self.gaps.push(format!("{at}: the model writes a map")),
        }
    }

    fn named(&mut self, name: &str, node: &'a Value, at: String) {
        if !self.visited.insert((name.to_string(), at.clone())) {
            return;
        }
        match self.types.get(name) {
            Some(Container::Struct(fields)) => self.object(name, fields, node, &at),
            Some(Container::Enum(variants)) => self.external(name, variants, node, &at),
            Some(Container::Tagged { tag, variants }) => {
                self.internal(name, tag, variants, node, &at);
            }
            None => self.gaps.push(format!("{at}: {name} was never traced")),
        }
    }

    fn object(&mut self, name: &str, fields: &Fields, node: &'a Value, at: &str) {
        let Some(properties) = node.get("properties").and_then(Value::as_object) else {
            self.gaps
                .push(format!("{at}: declares no properties for {name}"));
            return;
        };
        if node.get("additionalProperties") != Some(&Value::Bool(false)) {
            self.gaps
                .push(format!("{at}: must reject fields {name} does not have"));
        }
        let model: BTreeSet<&str> = fields.iter().map(|(field, _)| *field).collect();
        self.names(name, &model, properties.keys().map(String::as_str), at);
        for (field, shape) in fields {
            if let Some(property) = properties.get(*field) {
                self.shape(shape, property, format!("{at}/properties/{field}"));
            }
        }
    }

    fn names<'n>(
        &mut self,
        name: &str,
        model: &BTreeSet<&str>,
        schema: impl Iterator<Item = &'n str>,
        at: &str,
    ) {
        let schema: BTreeSet<&str> = schema.collect();
        for missing in model.difference(&schema) {
            self.gaps.push(format!(
                "{at}: {name} writes `{missing}`, which the schema does not declare"
            ));
        }
        for extra in schema.difference(model) {
            self.gaps.push(format!(
                "{at}: declares `{extra}`, which {name} does not have"
            ));
        }
    }

    fn external(
        &mut self,
        name: &str,
        variants: &[(&'static str, Option<Variant>)],
        node: &'a Value,
        at: &str,
    ) {
        let mut units = BTreeSet::new();
        let mut payloads = BTreeMap::new();
        let branches = match node.get("oneOf").and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .enumerate()
                .map(|(i, b)| (b, format!("{at}/oneOf/{i}")))
                .collect(),
            None => vec![(node, at.to_string())],
        };
        for (branch, branch_at) in branches {
            let (branch, branch_at) = self.resolve(branch, branch_at);
            let listed = branch.get("enum").and_then(Value::as_array);
            units.extend(listed.into_iter().flatten().filter_map(Value::as_str));
            if let Some(properties) = branch.get("properties").and_then(Value::as_object) {
                for (key, payload) in properties {
                    payloads.insert(
                        key.as_str(),
                        (payload, format!("{branch_at}/properties/{key}")),
                    );
                }
            }
        }
        let schema = units.iter().copied().chain(payloads.keys().copied());
        let model: BTreeSet<&str> = variants.iter().map(|(v, _)| *v).collect();
        self.names(name, &model, schema, at);
        for (variant, payload) in variants {
            let declared = units.contains(variant) || payloads.contains_key(variant);
            match (payload, payloads.get(variant).cloned()) {
                (None, _) => self
                    .gaps
                    .push(format!("{at}: {name}::{variant} was never traced")),
                (Some(Variant::Unit), _) if units.contains(variant) => {}
                (Some(Variant::Newtype(shape)), Some((node, at))) => self.shape(shape, node, at),
                (Some(Variant::Struct(fields)), Some((node, at))) => {
                    let (node, at) = self.resolve(node, at);
                    self.object(&format!("{name}::{variant}"), fields, node, &at);
                }
                (Some(_), _) if declared => self.gaps.push(format!(
                    "{at}: {name}::{variant} is declared with the wrong payload"
                )),
                (Some(_), _) => {}
            }
        }
    }

    fn internal(
        &mut self,
        name: &str,
        tag: &str,
        variants: &[&'static str],
        node: &'a Value,
        at: &str,
    ) {
        let mut branches = BTreeMap::new();
        for (i, branch) in node
            .get("oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let (branch, _) = self.resolve(branch, format!("{at}/oneOf/{i}"));
            let properties = branch.get("properties").and_then(Value::as_object);
            let value = properties
                .and_then(|p| p.get(tag))
                .and_then(|t| t.get("const"))
                .and_then(Value::as_str);
            match (value, properties) {
                (Some(value), Some(properties)) => {
                    branches.insert(value, (properties, format!("{at}/oneOf/{i}")));
                }
                _ => self.gaps.push(format!(
                    "{at}/oneOf/{i}: {name} branch without a `{tag}` constant"
                )),
            }
        }
        let model: BTreeSet<&str> = variants.iter().copied().collect();
        self.names(name, &model, branches.keys().copied(), at);
        for variant in variants {
            let recorded = self.tagged.get(&(name.to_string(), variant.to_string()));
            match (recorded, branches.get(variant)) {
                (None, _) => self.gaps.push(format!(
                    "{at}: no sample document serializes {name}::{variant}; add one so its fields are compared"
                )),
                (Some(fields), Some((properties, branch_at))) => {
                    let declared = properties.keys().map(String::as_str).filter(|key| *key != tag);
                    let model: BTreeSet<&str> = fields.iter().copied().collect();
                    self.names(&format!("{name}::{variant}"), &model, declared, branch_at);
                }
                (Some(_), None) => {}
            }
        }
    }

    fn leaf(&mut self, shape: &Shape, node: &Value, at: &str) {
        let (kind, format) = match shape {
            Shape::Bool => ("boolean", None),
            Shape::Int => ("integer", None),
            Shape::Float => ("number", None),
            Shape::Str => ("string", None),
            Shape::Time => ("string", Some("date-time")),
            Shape::Uuid => ("string", Some("uuid")),
            other => {
                self.gaps
                    .push(format!("{at}: untraced model value {other:?}"));
                return;
            }
        };
        let declared = json_type(node).or_else(|| node.get("const").map(kind_of));
        let declared_format = node.get("format").and_then(Value::as_str);
        if declared != Some(kind) || declared_format != format {
            self.gaps.push(format!(
                "{at}: the model writes {kind} {format:?}, the schema declares {declared:?} {declared_format:?}"
            ));
        }
    }
}

fn json_type(node: &Value) -> Option<&str> {
    node.get("type").and_then(Value::as_str)
}

fn nullable(node: &Value) -> bool {
    json_type(node) == Some("null")
        || node
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|b| b.iter().any(|b| json_type(b) == Some("null")))
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "number",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    }
}
