//! Deserializes a `Plan` from its types alone, recording every struct field, enum variant and
//! value shape the model's `Deserialize` implementations request. No sample document is involved,
//! so a field that only appears in rare states is traced like any other.
//!
//! Internally tagged enums are deserialized from buffered content that bypasses this tracer, so
//! only their tag and variant names are traced here; `model_record` supplies their fields.

use dpm_model::Plan;
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

mod access;
use access::{FieldAccess, OneEntry, OneItem, OneVariant, TagOnly};

/// Serialized form of one value.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Shape {
    Bool,
    Int,
    Float,
    Str,
    Time,
    Date,
    Uuid,
    Option(Box<Shape>),
    Seq(Box<Shape>),
    Map(Box<Shape>, Box<Shape>),
    Named(String),
    Unknown,
}

pub(super) type Fields = Vec<(&'static str, Shape)>;

/// Payload of one externally tagged variant.
#[derive(Debug, Clone)]
pub(super) enum Variant {
    Unit,
    Newtype(Shape),
    Struct(Fields),
}

/// A named model type.
#[derive(Debug, Clone)]
pub(super) enum Container {
    Struct(Fields),
    /// Externally tagged; each variant's payload is `None` until a trace visits it.
    Enum(Vec<(&'static str, Option<Variant>)>),
    /// Internally tagged by the `tag` field.
    Tagged {
        tag: String,
        variants: Vec<&'static str>,
    },
}

pub(super) type Registry = BTreeMap<String, Container>;

#[derive(Default)]
struct State {
    registry: Registry,
    /// Variant each internally tagged enum tries next when deserialized from its tag alone.
    tagged_attempt: BTreeMap<String, usize>,
}

impl State {
    fn complete(&self) -> bool {
        self.registry.values().all(|container| match container {
            Container::Enum(variants) => variants.iter().all(|(_, v)| v.is_some()),
            _ => true,
        })
    }
}

/// Error type the traced `Deserialize` and `Serialize` implementations report through.
#[derive(Debug)]
pub(super) enum TraceError {
    /// The trace learned something that needs a fresh pass.
    Retry,
    /// Variant names captured from an unknown-variant rejection.
    Variants(Vec<&'static str>),
    Other(String),
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retry => f.write_str("retry"),
            Self::Variants(names) => write!(f, "variants {names:?}"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TraceError {}

impl de::Error for TraceError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::Other(message.to_string())
    }

    fn unknown_variant(_: &str, expected: &'static [&'static str]) -> Self {
        Self::Variants(expected.to_vec())
    }
}

impl serde::ser::Error for TraceError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::Other(message.to_string())
    }
}

/// Trace every type reachable from `Plan`, visiting each externally tagged variant at least once.
pub(super) fn trace_plan() -> Result<Registry, String> {
    let state = RefCell::new(State::default());
    for _ in 0..64 {
        let out = RefCell::new(Shape::Unknown);
        match Plan::deserialize(Tracer {
            state: &state,
            out: &out,
        }) {
            Ok(_) if state.borrow().complete() => return Ok(state.into_inner().registry),
            Ok(_) | Err(TraceError::Retry) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("the trace did not visit every variant".into())
}

fn expecting<'de, V: Visitor<'de>>(visitor: &V) -> String {
    struct Expecting<'v, 'de, V>(&'v V, PhantomData<&'de ()>);
    impl<'de, V: Visitor<'de>> fmt::Display for Expecting<'_, 'de, V> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.expecting(f)
        }
    }
    Expecting(visitor, PhantomData).to_string()
}

#[derive(Clone, Copy)]
struct Tracer<'a> {
    state: &'a RefCell<State>,
    out: &'a RefCell<Shape>,
}

impl<'a> Tracer<'a> {
    fn put(self, shape: Shape) {
        *self.out.borrow_mut() = shape;
    }

    fn into(self, out: &'a RefCell<Shape>) -> Self {
        Self { out, ..self }
    }

    fn structure<'de, V: Visitor<'de>>(
        self,
        name: &'static str,
        names: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        self.put(Shape::Named(name.into()));
        let mut fields = FieldAccess::new(self.state, names);
        let value = visitor.visit_map(&mut fields)?;
        self.state
            .borrow_mut()
            .registry
            .insert(name.into(), Container::Struct(fields.shapes));
        Ok(value)
    }

    fn external<'de, V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        self.put(Shape::Named(name.into()));
        let index = match self
            .state
            .borrow_mut()
            .registry
            .entry(name.into())
            .or_insert_with(|| Container::Enum(variants.iter().map(|v| (*v, None)).collect()))
        {
            Container::Enum(list) => list.iter().position(|(_, v)| v.is_none()).unwrap_or(0),
            _ => return Err(TraceError::Other(format!("{name} changed kind"))),
        };
        let variant = variants
            .get(index)
            .ok_or_else(|| TraceError::Other(format!("{name} has no variant {index}")))?;
        let payload = RefCell::new(None);
        let value = visitor.visit_enum(OneVariant {
            state: self.state,
            name: variant,
            out: &payload,
        })?;
        if let Some(Container::Enum(list)) = self.state.borrow_mut().registry.get_mut(name)
            && let Some(slot) = list.get_mut(index)
        {
            slot.1 = payload.into_inner();
        }
        Ok(value)
    }

    fn internal<'de, V: Visitor<'de>>(
        self,
        name: String,
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        self.put(Shape::Named(name.clone()));
        let attempt = self.state.borrow().tagged_attempt.get(&name).copied();
        let known = match self.state.borrow().registry.get(&name) {
            Some(Container::Tagged { variants, .. }) => Some(variants.clone()),
            _ => None,
        };
        let value = match (&known, attempt.unwrap_or(0)) {
            (Some(variants), index) => variants
                .get(index)
                .ok_or_else(|| TraceError::Other(format!("no {name} variant has a bare tag")))?
                .to_string(),
            (None, _) => "\u{0}unlisted".into(),
        };
        let tag = RefCell::new(None);
        let result = visitor.visit_map(TagOnly {
            tag: &tag,
            value,
            step: 0,
        });
        let mut state = self.state.borrow_mut();
        match (result, known) {
            (Ok(value), Some(_)) => Ok(value),
            (Err(TraceError::Variants(variants)), None) => {
                let tag = tag
                    .into_inner()
                    .ok_or_else(|| TraceError::Other(format!("{name} named no tag")))?;
                state
                    .registry
                    .insert(name, Container::Tagged { tag, variants });
                Err(TraceError::Retry)
            }
            (Err(_), Some(_)) => {
                state
                    .tagged_attempt
                    .insert(name, attempt.unwrap_or(0).wrapping_add(1));
                Err(TraceError::Retry)
            }
            (Ok(_), None) | (Err(_), None) => Err(TraceError::Other(format!(
                "{name} did not list its variants"
            ))),
        }
    }
}

macro_rules! integers {
    ($($method:ident => $visit:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
            self.put(Shape::Int);
            visitor.$visit(0)
        }
    )*};
}

impl<'de> Deserializer<'de> for Tracer<'_> {
    type Error = TraceError;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        let expects = expecting(&visitor);
        match expects.strip_prefix("internally tagged enum ") {
            Some(name) => self.internal(name.to_string(), visitor),
            None => Err(TraceError::Other(format!("untraceable shape: {expects}"))),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.put(Shape::Bool);
        visitor.visit_bool(false)
    }

    integers!(
        deserialize_i8 => visit_i8, deserialize_i16 => visit_i16,
        deserialize_i32 => visit_i32, deserialize_i64 => visit_i64,
        deserialize_u8 => visit_u8, deserialize_u16 => visit_u16,
        deserialize_u64 => visit_u64
    );

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.put(Shape::Int);
        // Format validation needs a supported representative value to complete the shape trace.
        visitor.visit_u32(Plan::empty("schema trace").format_version)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.put(Shape::Float);
        visitor.visit_f64(0.0)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        let expecting = expecting(&visitor);
        if expecting.contains("date and time") {
            self.put(Shape::Time);
            visitor.visit_str("2026-09-01T00:00:00Z")
        } else if expecting.contains("date string") {
            self.put(Shape::Date);
            visitor.visit_str("2026-09-01")
        } else if expecting.contains("local time") {
            self.put(Shape::Str);
            visitor.visit_str("08:00")
        } else {
            self.put(Shape::Str);
            visitor.visit_str("text")
        }
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.put(Shape::Uuid);
        visitor.visit_bytes(&[0x42; 16])
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        let inner = RefCell::new(Shape::Unknown);
        let value = visitor.visit_some(self.into(&inner))?;
        self.put(Shape::Option(Box::new(inner.into_inner())));
        Ok(value)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        let item = RefCell::new(Shape::Unknown);
        let value = visitor.visit_seq(OneItem {
            tracer: self.into(&item),
            done: false,
        })?;
        self.put(Shape::Seq(Box::new(item.into_inner())));
        Ok(value)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        let (key, entry) = (RefCell::new(Shape::Unknown), RefCell::new(Shape::Unknown));
        let value = visitor.visit_map(OneEntry {
            key: self.into(&key),
            value: self.into(&entry),
            done: false,
        })?;
        let (key, entry) = (key.into_inner(), entry.into_inner());
        self.put(Shape::Map(Box::new(key), Box::new(entry)));
        Ok(value)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        self.structure(name, fields, visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        self.external(name, variants, visitor)
    }

    serde::forward_to_deserialize_any! {
        i128 u128 char unit unit_struct tuple tuple_struct identifier ignored_any
    }
}
