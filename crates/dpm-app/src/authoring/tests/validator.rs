//! The JSON Schema keywords `plan.schema.json` uses, enough to check documents against it and to
//! record which declared properties those documents exercise.

use serde_json::Value;
use std::collections::BTreeSet;

pub(super) struct Validator<'a> {
    root: &'a Value,
    declared: BTreeSet<String>,
    seen: BTreeSet<String>,
}

type Checked = Result<BTreeSet<String>, String>;

impl<'a> Validator<'a> {
    pub(super) fn new(root: &'a Value) -> Self {
        let mut declared = BTreeSet::new();
        declare(root, "#", &mut declared);
        Self {
            root,
            declared,
            seen: BTreeSet::new(),
        }
    }

    pub(super) fn declared(&self) -> &BTreeSet<String> {
        &self.declared
    }

    pub(super) fn seen(&self) -> &BTreeSet<String> {
        &self.seen
    }

    pub(super) fn validate(&mut self, instance: &Value) -> Result<(), String> {
        let seen = self.check(self.root, "#", instance, "$")?;
        self.seen.extend(seen);
        Ok(())
    }

    fn check(&self, schema: &Value, at: &str, value: &Value, path: &str) -> Checked {
        let mut seen = BTreeSet::new();
        if let Some(target) = schema.get("$ref").and_then(Value::as_str) {
            let pointer = target.trim_start_matches('#');
            let resolved = self
                .root
                .pointer(pointer)
                .ok_or(format!("unresolved {target}"))?;
            seen.extend(self.check(resolved, target, value, path)?);
        }
        for keyword in ["anyOf", "oneOf"] {
            if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
                let passing: Vec<_> = branches
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| {
                        self.check(b, &format!("{at}/{keyword}/{i}"), value, path)
                            .ok()
                    })
                    .collect();
                if passing.is_empty() || (keyword == "oneOf" && passing.len() > 1) {
                    return Err(format!(
                        "{path}: {} of {keyword} match {value}",
                        passing.len()
                    ));
                }
                passing.into_iter().for_each(|s| seen.extend(s));
            }
        }
        scalar(schema, value, path)?;
        if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
            for (i, item) in array.iter().enumerate() {
                seen.extend(self.check(
                    items,
                    &format!("{at}/items"),
                    item,
                    &format!("{path}[{i}]"),
                )?);
            }
        }
        if let Some(object) = value.as_object() {
            seen.extend(self.object(schema, at, object, path)?);
        }
        Ok(seen)
    }

    fn object(
        &self,
        schema: &Value,
        at: &str,
        object: &serde_json::Map<String, Value>,
        path: &str,
    ) -> Checked {
        let mut seen = BTreeSet::new();
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = name.as_str().unwrap_or_default();
            if !object.contains_key(name) {
                return Err(format!("{path}: missing {name}"));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (name, child) in object {
            let child_path = format!("{path}.{name}");
            if let Some(names) = schema.get("propertyNames") {
                scalar(names, &Value::from(name.as_str()), &child_path)?;
            }
            match (
                properties.and_then(|p| p.get(name)),
                schema.get("additionalProperties"),
            ) {
                (Some(declared), _) => {
                    seen.insert(format!("{at}/properties/{name}"));
                    seen.extend(self.check(
                        declared,
                        &format!("{at}/properties/{name}"),
                        child,
                        &child_path,
                    )?);
                }
                (None, Some(Value::Bool(false))) => {
                    return Err(format!("{child_path}: not in the schema"));
                }
                (None, Some(extra)) if extra.is_object() => {
                    seen.extend(self.check(
                        extra,
                        &format!("{at}/additionalProperties"),
                        child,
                        &child_path,
                    )?);
                }
                (None, _) => {}
            }
        }
        Ok(seen)
    }
}

fn scalar(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let fail = |what: &str| Err(format!("{path}: {what} ({value})"));
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        return fail(&format!("expected {expected}"));
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        return fail("not an allowed value");
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str)
        && !has_type(kind, value)
    {
        return fail(&format!("expected {kind}"));
    }
    let number = value.as_f64();
    if let (Some(minimum), Some(number)) = (schema.get("minimum").and_then(Value::as_f64), number)
        && number < minimum
    {
        return fail("below minimum");
    }
    if let (Some(maximum), Some(number)) = (schema.get("maximum").and_then(Value::as_f64), number)
        && number > maximum
    {
        return fail("above maximum");
    }
    let text = value.as_str();
    if let (Some(length), Some(text)) = (schema.get("minLength").and_then(Value::as_u64), text)
        && (text.chars().count() as u64) < length
    {
        return fail("too short");
    }
    match (schema.get("format").and_then(Value::as_str), text) {
        (Some("uuid"), Some(text)) if !is_uuid(text) => fail("not a UUID"),
        (Some("date-time"), Some(text)) if chrono::DateTime::parse_from_rfc3339(text).is_err() => {
            fail("not RFC 3339")
        }
        _ => Ok(()),
    }
}

fn has_type(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_u64() || value.is_i64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn declare(schema: &Value, at: &str, declared: &mut BTreeSet<String>) {
    let Some(object) = schema.as_object() else {
        return;
    };
    for (keyword, child) in object {
        let here = format!("{at}/{keyword}");
        match (keyword.as_str(), child) {
            ("properties", Value::Object(properties)) => {
                for (name, property) in properties {
                    declared.insert(format!("{here}/{name}"));
                    declare(property, &format!("{here}/{name}"), declared);
                }
            }
            ("$defs", Value::Object(definitions)) => {
                for (name, definition) in definitions {
                    declare(definition, &format!("{here}/{name}"), declared);
                }
            }
            (_, Value::Array(branches)) => {
                for (i, branch) in branches.iter().enumerate() {
                    declare(branch, &format!("{here}/{i}"), declared);
                }
            }
            (_, Value::Object(_)) => declare(child, &here, declared),
            _ => {}
        }
    }
}
