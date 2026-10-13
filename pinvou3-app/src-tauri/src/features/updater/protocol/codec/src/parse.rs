use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

use crate::{CONTAINER_MEMBERS, Error, MAXIMUM_DEPTH, STRING_BYTES};

struct State {
    failure: Option<Error>,
}

impl State {
    fn reject<E: de::Error>(&mut self, error: Error) -> E {
        self.failure.get_or_insert(error);
        E::custom(error)
    }

    fn string<E: de::Error>(&mut self, text: &str) -> Result<(), E> {
        if text.len() > STRING_BYTES {
            return Err(self.reject(Error::StringLimit));
        }
        Ok(())
    }
}

struct Seed<'a> {
    state: &'a mut State,
    depth: usize,
    allowed: bool,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if !self.allowed {
            return Err(self.state.reject(Error::MemberLimit));
        }
        if self.depth > MAXIMUM_DEPTH {
            return Err(self.state.reject(Error::DepthLimit));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        self.visit_f64(value as f64)
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        self.visit_f64(value as f64)
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| self.state.reject(Error::InvalidNumber))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.state.string(value)?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        self.state.string(&value)?;
        Ok(Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed {
            state: &mut *self.state,
            depth: self.depth + 1,
            allowed: values.len() < CONTAINER_MEMBERS,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key_seed(KeySeed {
            state: &mut *self.state,
            allowed: values.len() < CONTAINER_MEMBERS,
        })? {
            if values.contains_key(&key) {
                return Err(self.state.reject(Error::DuplicateKey));
            }
            let value = object.next_value_seed(Seed {
                state: &mut *self.state,
                depth: self.depth + 1,
                allowed: true,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

struct KeySeed<'a> {
    state: &'a mut State,
    allowed: bool,
}

impl<'de> DeserializeSeed<'de> for KeySeed<'_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        if !self.allowed {
            return Err(self.state.reject(Error::MemberLimit));
        }
        deserializer.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for KeySeed<'_> {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON key")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        self.state.string(value)?;
        Ok(value.to_owned())
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<String, E> {
        self.state.string(&value)?;
        Ok(value)
    }
}

pub(crate) fn bounded(bytes: &[u8]) -> Result<Value, Error> {
    let mut state = State { failure: None };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Seed {
        state: &mut state,
        depth: 0,
        allowed: true,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| state.failure.unwrap_or(Error::InvalidJson))?;
    deserializer.end().map_err(|_| Error::InvalidJson)?;
    Ok(value)
}
