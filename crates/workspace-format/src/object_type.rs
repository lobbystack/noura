//! Object types that callers register with a property schema. The generic
//! managed-object envelope parses and serializes every type; a registered
//! schema adds validation, defaults and the canonical order of its properties.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    FormatError, WorkspaceObject, is_managed_property, rewrite_envelope, serialize_envelope,
    serialize_object, serialize_object_over, valid_date_value, valid_object_id, valid_object_type,
};

/// The properties an object type declares, in canonical order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertySchema {
    pub properties: Vec<PropertyDeclaration>,
}

/// One declared property: its name, the values it accepts, whether it
/// accepts `null`, and the value written when it is missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDeclaration {
    pub name: String,
    pub kind: PropertyKind,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
    /// `Some(Value::Null)` is a `null` default, which only a nullable
    /// property can have.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<Value>,
}

/// The values a declared property accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyKind {
    /// Any string.
    String,
    /// Any number.
    Number,
    /// `true` or `false`.
    Boolean,
    /// A civil date (`YYYY-MM-DD`) or an RFC 3339 instant with an explicit offset.
    Date,
    /// One of the listed strings.
    Enum(Vec<String>),
    /// A link, `"[[Title]]"` or `"[[Folder/Title]]"`, to an object of the
    /// named type, or of any type when the name is [`ANY_TYPE`]. The
    /// format checks the link's form; the application checks, when it
    /// writes, that the link leads to an object of that type.
    Reference(String),
    /// The stable ID of an object of the named type, for objects that
    /// belong to another one, as a chat message to its chat. Not a link:
    /// nobody reads it, and nothing rewrites it.
    Id(String),
    /// A list of values, each of the inner kind, as a list of links.
    List(Box<PropertyKind>),
}

/// The type name a reference uses to accept a link to an object of any
/// type.
pub const ANY_TYPE: &str = "*";

/// A link a property of a registered type holds, for the application to
/// check that it leads to an object of the declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredLink {
    /// The property's name.
    pub property: String,
    /// The link's target, between `[[` and `]]`, without a heading or a
    /// label.
    pub target: String,
    /// The type the link must lead to, or [`ANY_TYPE`].
    pub object_type: String,
}

/// Why an object type could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RegistrationError {
    #[error(
        "Object types start with a lowercase letter and use only lowercase letters and hyphens"
    )]
    InvalidTypeName { object_type: String },
    #[error("{object_type} is already registered")]
    TypeAlreadyRegistered { object_type: String },
    #[error("Every object has {property}, so a schema can't declare it")]
    ReservedProperty { property: String },
    #[error("{property} is declared more than once")]
    DuplicateProperty { property: String },
    #[error("The declaration of {property:?} needs a name, enum values or a valid reference type")]
    InvalidDeclaration { property: String },
    #[error("The default for {property} isn't a value it accepts")]
    InvalidDefault { property: String },
}

/// Why a property of a registered type doesn't fit its declaration.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PropertyError {
    #[error("{property} has a value of the wrong type")]
    WrongType { property: String },
    #[error("{property} isn't one of the allowed values")]
    NotAllowed { property: String },
    #[error("{property} must use YYYY-MM-DD or RFC 3339 with an explicit offset")]
    InvalidDate { property: String },
    #[error("{property} must link to an object of the declared type")]
    InvalidReference { property: String },
}

/// The object types a caller has registered, each with its property schema.
#[derive(Debug, Clone, Default)]
pub struct ObjectTypeRegistry {
    types: BTreeMap<String, PropertySchema>,
}

impl ObjectTypeRegistry {
    /// Creates a registry with no registered types.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an object type with its property schema.
    pub fn register(
        &mut self,
        object_type: &str,
        schema: PropertySchema,
    ) -> Result<(), RegistrationError> {
        if !valid_object_type(object_type) {
            return Err(RegistrationError::InvalidTypeName {
                object_type: object_type.into(),
            });
        }
        let mut names = BTreeSet::new();
        for declaration in &schema.properties {
            let property = declaration.name.clone();
            if is_managed_property(&declaration.name) {
                return Err(RegistrationError::ReservedProperty { property });
            }
            if !names.insert(declaration.name.as_str()) {
                return Err(RegistrationError::DuplicateProperty { property });
            }
            if !declaration.well_formed() {
                return Err(RegistrationError::InvalidDeclaration { property });
            }
            if declaration
                .default
                .as_ref()
                .is_some_and(|value| declaration.check(value).is_err())
            {
                return Err(RegistrationError::InvalidDefault { property });
            }
        }
        if self.types.contains_key(object_type) {
            return Err(RegistrationError::TypeAlreadyRegistered {
                object_type: object_type.into(),
            });
        }
        self.types.insert(object_type.into(), schema);
        Ok(())
    }

    /// Checks an object's properties against its type's schema and adds the
    /// default of each declared property that is missing. Unknown properties
    /// stay as they are. On an error the object is left unchanged.
    pub fn normalize(&self, object: &mut WorkspaceObject) -> Result<(), PropertyError> {
        let Some(schema) = self.types.get(&object.object_type) else {
            return Ok(());
        };
        let mut defaults = Vec::new();
        for declaration in &schema.properties {
            match object.properties.get(&declaration.name) {
                Some(value) => declaration.check(value)?,
                None => {
                    if let Some(default) = &declaration.default {
                        defaults.push((declaration.name.clone(), default.clone()));
                    }
                }
            }
        }
        object.properties.extend(defaults);
        Ok(())
    }

    /// Serializes an object through the envelope in canonical order: `id` and
    /// `type`, the properties its type declares in declared order, the other
    /// properties sorted by key, then `created` and `updated`. An object
    /// whose type has no registered schema gets `serialize_object`'s order,
    /// with every property sorted by key. It writes the properties as they
    /// are, without checking them or adding defaults.
    pub fn serialize(&self, object: &WorkspaceObject) -> Result<Vec<u8>, FormatError> {
        let Some(declared) = self.declared(&object.object_type) else {
            return serialize_object(object);
        };
        serialize_envelope(object, &declared)
    }

    /// Serializes `object` over `existing`, the bytes its file holds now,
    /// as `serialize_object_over` does, but a changed frontmatter takes
    /// [`Self::serialize`]'s canonical order. The parts nobody changed keep
    /// their bytes, and so do the file's byte-order mark and line endings.
    pub fn serialize_over(
        &self,
        existing: &[u8],
        object: &WorkspaceObject,
    ) -> Result<Vec<u8>, FormatError> {
        let Some(declared) = self.declared(&object.object_type) else {
            return serialize_object_over(existing, object);
        };
        rewrite_envelope(existing, object, &declared)
    }

    /// Whether `object_type` has a registered schema.
    pub fn contains(&self, object_type: &str) -> bool {
        self.types.contains_key(object_type)
    }

    /// The schema registered for `object_type`.
    pub fn schema(&self, object_type: &str) -> Option<&PropertySchema> {
        self.types.get(object_type)
    }

    /// The links that the reference properties of `object` hold, one for
    /// each link, in declared order. A property that isn't a link, or a
    /// type without a schema, gives none.
    pub fn declared_links(&self, object: &WorkspaceObject) -> Vec<DeclaredLink> {
        let Some(schema) = self.types.get(&object.object_type) else {
            return Vec::new();
        };
        let mut links = Vec::new();
        for declaration in &schema.properties {
            let Some(value) = object.properties.get(&declaration.name) else {
                continue;
            };
            let (kind, values): (&PropertyKind, Vec<&Value>) = match (&declaration.kind, value) {
                (PropertyKind::List(inner), Value::Array(items)) => (inner, items.iter().collect()),
                (kind, value) => (kind, vec![value]),
            };
            let PropertyKind::Reference(object_type) = kind else {
                continue;
            };
            for value in values {
                if let Some(target) = value.as_str().and_then(crate::links::property_link) {
                    links.push(DeclaredLink {
                        property: declaration.name.clone(),
                        target: target.to_owned(),
                        object_type: object_type.clone(),
                    });
                }
            }
        }
        links
    }

    /// The names a registered type declares, in declared order.
    fn declared(&self, object_type: &str) -> Option<Vec<&str>> {
        self.types.get(object_type).map(|schema| {
            schema
                .properties
                .iter()
                .map(|declaration| declaration.name.as_str())
                .collect()
        })
    }
}

impl PropertyKind {
    fn well_formed(&self) -> bool {
        match self {
            PropertyKind::Enum(values) => !values.is_empty(),
            PropertyKind::Reference(object_type) => {
                object_type == ANY_TYPE || valid_object_type(object_type)
            }
            PropertyKind::Id(object_type) => valid_object_type(object_type),
            PropertyKind::List(inner) => {
                !matches!(**inner, PropertyKind::List(_)) && inner.well_formed()
            }
            _ => true,
        }
    }
}

impl PropertyDeclaration {
    fn well_formed(&self) -> bool {
        !self.name.is_empty() && self.kind.well_formed()
    }

    fn check(&self, value: &Value) -> Result<(), PropertyError> {
        match (&self.kind, value) {
            (_, Value::Null) if self.nullable => Ok(()),
            (PropertyKind::List(inner), Value::Array(items)) => items
                .iter()
                .try_for_each(|item| check_kind(&self.name, inner, item)),
            (kind, value) => check_kind(&self.name, kind, value),
        }
    }
}

/// Whether `value` fits `kind`, for the property `property`.
fn check_kind(property: &str, kind: &PropertyKind, value: &Value) -> Result<(), PropertyError> {
    let property = property.to_owned();
    let fits = |valid: bool, error: PropertyError| if valid { Ok(()) } else { Err(error) };
    {
        match (kind, value) {
            (PropertyKind::String, Value::String(_))
            | (PropertyKind::Number, Value::Number(_))
            | (PropertyKind::Boolean, Value::Bool(_)) => Ok(()),
            (PropertyKind::Date, Value::String(value)) => fits(
                valid_date_value(value),
                PropertyError::InvalidDate { property },
            ),
            (PropertyKind::Enum(values), Value::String(value)) => fits(
                values.contains(value),
                PropertyError::NotAllowed { property },
            ),
            (PropertyKind::Reference(_), Value::String(value)) => fits(
                crate::links::property_link(value).is_some(),
                PropertyError::InvalidReference { property },
            ),
            (PropertyKind::Id(object_type), Value::String(value)) => fits(
                valid_object_id(value, object_type),
                PropertyError::InvalidReference { property },
            ),
            _ => Err(PropertyError::WrongType { property }),
        }
    }
}

/// Keeps an explicit `null` as `Some(Value::Null)`; only a missing key is `None`.
fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}
