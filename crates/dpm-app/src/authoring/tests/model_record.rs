//! Serializes plans and records the complete field list of every internally tagged variant they
//! contain. Derived `Serialize` reports each field through `serialize_field` or `skip_field`, so a
//! field omitted by `skip_serializing_if` is recorded as well.

use super::model_trace::TraceError;
use serde::Serialize;
use serde::ser::{self, Impossible, Serializer};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

/// Fields, without the tag, of each `(enum, variant)` seen.
pub(super) type TaggedFields = BTreeMap<(String, String), BTreeSet<&'static str>>;

/// Record the tagged variants in `value`; `tags` maps each internally tagged enum to its tag field.
pub(super) fn record(
    value: &impl Serialize,
    tags: &BTreeMap<String, String>,
    into: &mut TaggedFields,
) -> Result<(), TraceError> {
    let (fields, last) = (RefCell::new(std::mem::take(into)), RefCell::new(None));
    let result = value.serialize(Recorder {
        tags,
        fields: &fields,
        last: &last,
    });
    *into = fields.into_inner();
    result
}

#[derive(Clone, Copy)]
struct Recorder<'a> {
    tags: &'a BTreeMap<String, String>,
    fields: &'a RefCell<TaggedFields>,
    last: &'a RefCell<Option<String>>,
}

type Done = Result<(), TraceError>;

macro_rules! ignored {
    ($($method:ident: $ty:ty),*) => {$(
        fn $method(self, _: $ty) -> Done {
            Ok(())
        }
    )*};
}

impl<'a> Serializer for Recorder<'a> {
    type Ok = ();
    type Error = TraceError;
    type SerializeSeq = Self;
    type SerializeTuple = Impossible<(), TraceError>;
    type SerializeTupleStruct = Impossible<(), TraceError>;
    type SerializeTupleVariant = Impossible<(), TraceError>;
    type SerializeMap = Self;
    type SerializeStruct = StructRecord<'a>;
    type SerializeStructVariant = StructRecord<'a>;

    ignored!(serialize_bool: bool, serialize_i8: i8, serialize_i16: i16, serialize_i32: i32,
        serialize_i64: i64, serialize_u8: u8, serialize_u16: u16, serialize_u32: u32,
        serialize_u64: u64, serialize_f32: f32, serialize_f64: f64, serialize_char: char,
        serialize_bytes: &[u8], serialize_unit_struct: &'static str);

    fn serialize_str(self, value: &str) -> Done {
        *self.last.borrow_mut() = Some(value.to_string());
        Ok(())
    }

    fn serialize_none(self) -> Done {
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Done {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Done {
        Ok(())
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Done {
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, value: &T) -> Done {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Done {
        value.serialize(self)
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self, TraceError> {
        Ok(self)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, TraceError> {
        Err(TraceError::Other("the model has no tuples".into()))
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, TraceError> {
        Err(TraceError::Other(format!("tuple struct {name}")))
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, TraceError> {
        Err(TraceError::Other(format!("tuple variant of {name}")))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self, TraceError> {
        Ok(self)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        _: usize,
    ) -> Result<StructRecord<'a>, TraceError> {
        Ok(StructRecord {
            recorder: self,
            name,
            fields: BTreeSet::new(),
            variant: None,
        })
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        _: u32,
        _: &'static str,
        len: usize,
    ) -> Result<StructRecord<'a>, TraceError> {
        self.serialize_struct(name, len)
    }
}

impl ser::SerializeSeq for Recorder<'_> {
    type Ok = ();
    type Error = TraceError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Done {
        value.serialize(*self)
    }

    fn end(self) -> Done {
        Ok(())
    }
}

impl ser::SerializeMap for Recorder<'_> {
    type Ok = ();
    type Error = TraceError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Done {
        key.serialize(*self)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Done {
        value.serialize(*self)
    }

    fn end(self) -> Done {
        Ok(())
    }
}

pub(super) struct StructRecord<'a> {
    recorder: Recorder<'a>,
    name: &'static str,
    fields: BTreeSet<&'static str>,
    variant: Option<String>,
}

impl StructRecord<'_> {
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Done {
        value.serialize(self.recorder)?;
        if self.recorder.tags.get(self.name).map(String::as_str) == Some(key) {
            self.variant = self.recorder.last.borrow_mut().take();
        } else {
            self.fields.insert(key);
        }
        Ok(())
    }

    fn finish(self) -> Done {
        if let Some(variant) = self.variant {
            self.recorder
                .fields
                .borrow_mut()
                .entry((self.name.to_string(), variant))
                .or_default()
                .extend(self.fields);
        }
        Ok(())
    }
}

impl ser::SerializeStruct for StructRecord<'_> {
    type Ok = ();
    type Error = TraceError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Done {
        self.field(key, value)
    }

    fn skip_field(&mut self, key: &'static str) -> Done {
        self.fields.insert(key);
        Ok(())
    }

    fn end(self) -> Done {
        self.finish()
    }
}

impl ser::SerializeStructVariant for StructRecord<'_> {
    type Ok = ();
    type Error = TraceError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Done {
        self.field(key, value)
    }

    fn skip_field(&mut self, key: &'static str) -> Done {
        self.fields.insert(key);
        Ok(())
    }

    fn end(self) -> Done {
        self.finish()
    }
}
