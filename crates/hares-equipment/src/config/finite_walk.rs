//! The non-finite guard of `EquipmentConfig::from_typed`.
//!
//! serde_json maps non-finite floats to `null` (JSON cannot represent
//! them), so a `Some(NaN)` in an `Option<f64>` field of a typed config
//! silently becomes `None` in the stored payload — the field's
//! documented default then applies with no error anywhere, the exact
//! violation of the constitution's loud-errors-at-boundaries clause
//! ("never silently substitute a default for an unrecognised value")
//! that the I-07 Python boundary measured end to end: a NaN
//! `battery_temp_c` silently attached a differently-configured vehicle
//! (the ambient-cascade temperature and the default 5 kW heater instead
//! of the user's value, no exception anywhere). The walk runs inside
//! `from_typed` before serialization — the one choke point every typed
//! config crosses: every equipment type, every field, every entry path
//! (the Python binding, Rust-side construction, any future boundary).
//!
//! The walk is a full `serde::Serializer` that visits the same structure
//! `serde_json::to_value` would, rejecting non-finite `f32`/`f64` leaves
//! with their dotted field path and passing everything else through.

use serde::Serialize;
use serde::ser;
use std::fmt;

/// The walk's outcome: either a non-finite leaf (with its field path) or
/// a nested `Serialize` failure (surfaced by serde_json's own
/// serialization, which fails the same impl — the walk does not
/// reinterpret it).
#[derive(Debug)]
pub(crate) enum WalkError {
    NonFinite { path: String },
    Nested(String),
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite { path } => {
                write!(f, "non-finite float at config field '{path}'")
            }
            Self::Nested(msg) => write!(f, "nested serialization error: {msg}"),
        }
    }
}

impl ser::Error for WalkError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Nested(msg.to_string())
    }
}

impl std::error::Error for WalkError {}

/// The walking serializer. `path` is the dotted field path of the value
/// being visited.
pub(crate) struct Walker {
    path: String,
}

/// Joins a path segment onto a (possibly empty) parent path.
fn join(parent: &str, segment: &str) -> String {
    if parent.is_empty() {
        segment.to_string()
    } else {
        format!("{parent}.{segment}")
    }
}

impl Walker {
    /// The walk entry point: the config root.
    pub(crate) fn root() -> Self {
        Self {
            path: String::new(),
        }
    }
}

impl ser::Serializer for Walker {
    type Ok = ();
    type Error = WalkError;
    type SerializeSeq = SeqPath;
    type SerializeTuple = SeqPath;
    type SerializeTupleStruct = SeqPath;
    type SerializeTupleVariant = SeqPath;
    type SerializeMap = MapPath;
    type SerializeStruct = StructPath;
    type SerializeStructVariant = StructPath;

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        if v.is_finite() {
            Ok(())
        } else {
            Err(WalkError::NonFinite { path: self.path })
        }
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        if v.is_finite() {
            Ok(())
        } else {
            Err(WalkError::NonFinite { path: self.path })
        }
    }

    fn serialize_bool(self, _: bool) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_char(self, _: char) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_str(self, _: &str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        // `Option` is path-transparent: `Some(v)` carries the field's path.
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(Walker {
            path: join(&self.path, variant),
        })
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(SeqPath {
            path: self.path,
            idx: 0,
        })
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(SeqPath {
            path: self.path,
            idx: 0,
        })
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(SeqPath {
            path: self.path,
            idx: 0,
        })
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(SeqPath {
            path: join(&self.path, variant),
            idx: 0,
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(MapPath {
            path: self.path,
            key: String::new(),
        })
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(StructPath { path: self.path })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(StructPath {
            path: join(&self.path, variant),
        })
    }
}

/// Sequence-like containers (seq, tuple, tuple struct, tuple variant):
/// elements are addressed by index on the parent path.
pub(crate) struct SeqPath {
    path: String,
    idx: usize,
}

impl SeqPath {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), WalkError> {
        let walker = Walker {
            path: format!("{}[{}]", self.path, self.idx),
        };
        self.idx += 1;
        value.serialize(walker)
    }
}

impl ser::SerializeSeq for SeqPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.element(value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl ser::SerializeTuple for SeqPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.element(value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for SeqPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.element(value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for SeqPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.element(value)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

/// Maps: values are addressed by their (string-rendered) key on the
/// parent path. serde_json requires string map keys, so a non-finite
/// float key cannot round-trip through `to_value` at all; the key is
/// rendered only for the value's path.
pub(crate) struct MapPath {
    path: String,
    key: String,
}

impl ser::SerializeMap for MapPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.key = serde_json::to_string(key)
            .map(|rendered| rendered.trim_matches('"').to_string())
            .unwrap_or_else(|_| "<key>".to_string());
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        value.serialize(Walker {
            path: join(&self.path, &self.key),
        })
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

/// Structs: fields are addressed by their field name on the parent path.
pub(crate) struct StructPath {
    path: String,
}

impl ser::SerializeStruct for StructPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(Walker {
            path: join(&self.path, key),
        })
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for StructPath {
    type Ok = ();
    type Error = WalkError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(Walker {
            path: join(&self.path, key),
        })
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Leaf {
        plain: Option<f64>,
        nested: Option<Nested>,
    }

    #[derive(Serialize)]
    struct Nested {
        list: Vec<f64>,
    }

    fn walk_error<T: Serialize>(value: &T) -> Option<WalkError> {
        value.serialize(Walker::root()).err()
    }

    #[test]
    fn finite_config_walks_clean() {
        let value = Leaf {
            plain: Some(1.5),
            nested: Some(Nested {
                list: vec![0.0, -2.25, 1e300],
            }),
        };
        assert!(walk_error(&value).is_none(), "finite floats must pass");
    }

    #[test]
    fn nan_leaf_is_rejected_with_its_field_path() {
        let value = Leaf {
            plain: Some(f64::NAN),
            nested: None,
        };
        match walk_error(&value) {
            Some(WalkError::NonFinite { path }) => assert_eq!(path, "plain"),
            other => panic!("expected a NonFinite error at 'plain', got {other:?}"),
        }
    }

    #[test]
    fn non_finite_inside_nested_containers_is_rejected_with_the_full_path() {
        let value = Leaf {
            plain: None,
            nested: Some(Nested {
                list: vec![1.0, f64::INFINITY],
            }),
        };
        match walk_error(&value) {
            Some(WalkError::NonFinite { path }) => {
                assert_eq!(path, "nested.list[1]");
            }
            other => panic!("expected a NonFinite error at 'nested.list[1]', got {other:?}"),
        }
    }

    #[test]
    fn absent_fields_are_path_transparent() {
        let value = Leaf {
            plain: None,
            nested: None,
        };
        assert!(walk_error(&value).is_none(), "absent fields must pass");
    }

    #[test]
    fn negative_infinity_is_rejected_like_nan() {
        let value = Leaf {
            plain: Some(f64::NEG_INFINITY),
            nested: None,
        };
        match walk_error(&value) {
            Some(WalkError::NonFinite { path }) => assert_eq!(path, "plain"),
            other => panic!("expected a NonFinite error at 'plain', got {other:?}"),
        }
    }
}
