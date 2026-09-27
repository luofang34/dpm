//! Map, sequence and enum access that feeds the tracer one representative value of each kind.

use super::{Fields, Shape, State, TraceError, Tracer, Variant, expecting};
use serde::de::value::{BorrowedStrDeserializer, StrDeserializer};
use serde::de::{
    DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use std::cell::RefCell;

pub(super) struct FieldAccess<'a> {
    pub(super) state: &'a RefCell<State>,
    pub(super) names: &'static [&'static str],
    pub(super) next: usize,
    pub(super) shapes: Fields,
}

impl<'a> FieldAccess<'a> {
    pub(super) fn new(state: &'a RefCell<State>, names: &'static [&'static str]) -> Self {
        Self {
            state,
            names,
            next: 0,
            shapes: Vec::new(),
        }
    }
}

impl<'de> MapAccess<'de> for FieldAccess<'_> {
    type Error = TraceError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceError> {
        self.names
            .get(self.next)
            .map(|name| seed.deserialize(BorrowedStrDeserializer::new(name)))
            .transpose()
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, TraceError> {
        let name = *self
            .names
            .get(self.next)
            .ok_or_else(|| TraceError::Other("value without a field".into()))?;
        self.next = self.next.wrapping_add(1);
        let out = RefCell::new(Shape::Unknown);
        let value = seed.deserialize(Tracer {
            state: self.state,
            out: &out,
        })?;
        self.shapes.push((name, out.into_inner()));
        Ok(value)
    }
}

pub(super) struct OneItem<'a> {
    pub(super) tracer: Tracer<'a>,
    pub(super) done: bool,
}

impl<'de> SeqAccess<'de> for OneItem<'_> {
    type Error = TraceError;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, TraceError> {
        if std::mem::replace(&mut self.done, true) {
            return Ok(None);
        }
        seed.deserialize(self.tracer).map(Some)
    }
}

pub(super) struct OneEntry<'a> {
    pub(super) key: Tracer<'a>,
    pub(super) value: Tracer<'a>,
    pub(super) done: bool,
}

impl<'de> MapAccess<'de> for OneEntry<'_> {
    type Error = TraceError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceError> {
        if self.done {
            return Ok(None);
        }
        seed.deserialize(self.key).map(Some)
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, TraceError> {
        self.done = true;
        seed.deserialize(self.value)
    }
}

pub(super) struct OneVariant<'a> {
    pub(super) state: &'a RefCell<State>,
    pub(super) name: &'static str,
    pub(super) out: &'a RefCell<Option<Variant>>,
}

impl<'de, 'a> EnumAccess<'de> for OneVariant<'a> {
    type Error = TraceError;
    type Variant = Self;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self), TraceError> {
        let variant = seed.deserialize(BorrowedStrDeserializer::new(self.name))?;
        Ok((variant, self))
    }
}

impl<'de> VariantAccess<'de> for OneVariant<'_> {
    type Error = TraceError;

    fn unit_variant(self) -> Result<(), TraceError> {
        *self.out.borrow_mut() = Some(Variant::Unit);
        Ok(())
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, TraceError> {
        let inner = RefCell::new(Shape::Unknown);
        let value = seed.deserialize(Tracer {
            state: self.state,
            out: &inner,
        })?;
        *self.out.borrow_mut() = Some(Variant::Newtype(inner.into_inner()));
        Ok(value)
    }

    fn tuple_variant<V: Visitor<'de>>(self, _: usize, _: V) -> Result<V::Value, TraceError> {
        Err(TraceError::Other(format!("tuple variant {}", self.name)))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        names: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, TraceError> {
        let mut fields = FieldAccess::new(self.state, names);
        let value = visitor.visit_map(&mut fields)?;
        *self.out.borrow_mut() = Some(Variant::Struct(fields.shapes));
        Ok(value)
    }
}

/// Content of an internally tagged enum holding only its tag, whose name the key visitor reveals.
pub(super) struct TagOnly<'a> {
    pub(super) tag: &'a RefCell<Option<String>>,
    pub(super) value: String,
    pub(super) step: u8,
}

impl<'de> MapAccess<'de> for TagOnly<'_> {
    type Error = TraceError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, TraceError> {
        if self.step > 0 {
            return Ok(None);
        }
        self.step = 1;
        seed.deserialize(TagKey { tag: self.tag }).map(Some)
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, TraceError> {
        seed.deserialize(StrDeserializer::new(&self.value))
    }
}

pub(super) struct TagKey<'a> {
    pub(super) tag: &'a RefCell<Option<String>>,
}

impl<'de> Deserializer<'de> for TagKey<'_> {
    type Error = TraceError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, TraceError> {
        let expects = expecting(&visitor);
        let tag = expects
            .split('`')
            .nth(1)
            .ok_or_else(|| TraceError::Other(format!("no tag name in {expects:?}")))?
            .to_string();
        let value = visitor.visit_str(&tag);
        *self.tag.borrow_mut() = Some(tag);
        value
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option
        unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier
        ignored_any
    }
}
