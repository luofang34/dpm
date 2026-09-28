//! Retain map entries, including duplicates, so the derived domain decoder stays authoritative.

use serde::{
    Deserialize, Deserializer,
    de::{self, IntoDeserializer, Visitor},
};
use std::fmt;

#[derive(Clone)]
pub(super) enum Buffered {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
    Seq(Vec<Self>),
    Map(Vec<(String, Self)>),
}

impl<'de> Deserialize<'de> for Buffered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(BufferVisitor)
    }
}

struct BufferVisitor;

impl<'de> Visitor<'de> for BufferVisitor {
    type Value = Buffered;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a portable plan value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Buffered::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Buffered::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Buffered::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Buffered::Signed(value))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Buffered::Unsigned(value))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        Ok(Buffered::Float(value))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.visit_string(value.into())
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(Buffered::String(value))
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = access.next_element()? {
            values.push(value);
        }
        Ok(Buffered::Seq(values))
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut fields = Vec::new();
        while let Some(entry) = access.next_entry()? {
            fields.push(entry);
        }
        Ok(Buffered::Map(fields))
    }
}

impl<'de> IntoDeserializer<'de, de::value::Error> for Buffered {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

impl<'de> Deserializer<'de> for Buffered {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_unit(),
            Self::Bool(value) => visitor.visit_bool(value),
            Self::Signed(value) => visitor.visit_i64(value),
            Self::Unsigned(value) => visitor.visit_u64(value),
            Self::Float(value) => visitor.visit_f64(value),
            Self::String(value) => visitor.visit_string(value),
            Self::Seq(values) => {
                let mut access = de::value::SeqDeserializer::new(values.into_iter());
                let result = visitor.visit_seq(&mut access)?;
                access.end()?;
                Ok(result)
            }
            Self::Map(fields) => {
                let mut access = de::value::MapDeserializer::new(fields.into_iter());
                let result = visitor.visit_map(&mut access)?;
                access.end()?;
                Ok(result)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_none(),
            other => visitor.visit_some(other),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(value) => {
                de::value::StringDeserializer::new(value).deserialize_enum(name, variants, visitor)
            }
            Self::Map(fields) => {
                if fields.len() != 1 {
                    return Err(de::Error::invalid_length(fields.len(), &"one enum variant"));
                }
                let mut access = de::value::MapDeserializer::new(fields.into_iter());
                let result = de::value::MapAccessDeserializer::new(&mut access)
                    .deserialize_enum(name, variants, visitor)?;
                access.end()?;
                Ok(result)
            }
            other => other.deserialize_any(visitor),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}
