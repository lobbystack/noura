//! The object types running plugins registered. One registry serves every
//! plugin of a workspace: it keeps the kernel's own types, keeps each type
//! held for one plugin for that plugin, gives each other type to the first
//! running plugin that registers it, and holds each type's schema in
//! `workspace-format`'s declaration, which checks every write.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use workspace_format::{ObjectTypeRegistry, PropertySchema, RegistrationError, WorkspaceObject};

use crate::PropertyProblem;

/// The types the kernel keeps for itself. No plugin may register them.
/// Tasks belong to the Tasks plugin, projects to the Projects plugin, and
/// chats and their messages to the AI plugin.
pub const KERNEL_TYPES: [&str; 2] = ["note", "workspace"];

/// Types held for one plugin, by the plugin's ID, which no other plugin
/// may register even while that one is off. Files of these types already
/// fill workspaces, so a plugin that took one could rewrite every one of
/// them. The Tasks plugin, which ships with Noura, holds tasks, the
/// Projects plugin holds projects, and the AI plugin chats and their
/// messages; other plugins change them through their commands.
pub const PLUGIN_TYPES: [(&str, &str); 4] = [
    ("task", "tasks"),
    ("project", "projects"),
    ("chat", "ai"),
    ("chat-message", "ai"),
];

/// The most types one plugin may register.
const MAX_TYPES: usize = 64;

/// The longest schema declaration the host reads.
const MAX_SCHEMA_BYTES: usize = 64 << 10;

/// Names each running instance apart, so stopping one releases only the
/// types it registered.
static INSTANCES: AtomicU64 = AtomicU64::new(1);

/// A token for one running instance of a plugin.
pub(crate) fn next_instance() -> u64 {
    INSTANCES.fetch_add(1, Ordering::Relaxed)
}

/// Why a type wasn't registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TypeError {
    /// The kernel keeps this type.
    #[error("{object_type} is one of Noura's own types.")]
    KernelType { object_type: String },
    /// The type is held for another plugin.
    #[error("{object_type} belongs to the plugin {plugin}.")]
    HeldFor { object_type: String, plugin: String },
    /// Another running plugin registered the type first.
    #[error("The plugin {plugin} already holds the type {object_type}.")]
    HeldByPlugin { object_type: String, plugin: String },
    /// The schema isn't JSON in the workspace format's declaration.
    #[error("The schema of {object_type} isn't a schema declaration: {message}")]
    InvalidSchema {
        object_type: String,
        message: String,
    },
    /// The workspace format refused the type or its schema.
    #[error("{0}")]
    Refused(RegistrationError),
    /// The plugin registered more types than the host takes.
    #[error("A plugin may register at most {MAX_TYPES} types.")]
    TooMany,
}

impl TypeError {
    /// The error as a stable code a client can act on.
    pub fn code(&self) -> &'static str {
        match self {
            Self::KernelType { .. } => "kernel_type",
            Self::HeldFor { .. } => "type_reserved",
            Self::HeldByPlugin { .. } => "type_held",
            Self::InvalidSchema { .. } => "invalid_schema",
            Self::TooMany => "too_many_types",
            Self::Refused(error) => match error {
                RegistrationError::InvalidTypeName { .. } => "invalid_type_name",
                RegistrationError::TypeAlreadyRegistered { .. } => "type_already_registered",
                RegistrationError::ReservedProperty { .. } => "reserved_property",
                RegistrationError::DuplicateProperty { .. } => "duplicate_property",
                RegistrationError::InvalidDeclaration { .. } => "invalid_declaration",
                RegistrationError::InvalidDefault { .. } => "invalid_default",
            },
        }
    }
}

/// The object types running plugins hold, with their schemas. Clones share
/// one registry.
#[derive(Clone, Default)]
pub struct ObjectTypes {
    inner: Arc<RwLock<Table>>,
}

#[derive(Default)]
struct Table {
    held: BTreeMap<String, Held>,
    /// The schemas of `held`, as `workspace-format` checks them.
    schemas: ObjectTypeRegistry,
}

impl Table {
    /// Build `schemas` again from `held`, after types left it.
    fn rebuild_schemas(&mut self) {
        let mut schemas = ObjectTypeRegistry::new();
        for (object_type, held) in &self.held {
            // Each schema registered once already, so it registers again.
            let _ = schemas.register(object_type, held.schema.clone());
        }
        self.schemas = schemas;
    }
}

struct Held {
    plugin: String,
    instance: u64,
    schema: PropertySchema,
}

impl ObjectTypes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `object_type` for the plugin `plugin`, with `schema` in the
    /// workspace format's declaration, as JSON. `instance` names the running
    /// instance, whose stop releases it. A plugin that registers one of its
    /// own types again, as a new build or a new start of it does, replaces
    /// the type's schema and holds it under `instance`.
    pub fn register(
        &self,
        plugin: &str,
        instance: u64,
        object_type: &str,
        schema: &str,
    ) -> Result<(), TypeError> {
        if KERNEL_TYPES.contains(&object_type) {
            return Err(TypeError::KernelType {
                object_type: object_type.to_owned(),
            });
        }
        if let Some((_, owner)) = PLUGIN_TYPES
            .iter()
            .find(|(held, owner)| *held == object_type && *owner != plugin)
        {
            return Err(TypeError::HeldFor {
                object_type: object_type.to_owned(),
                plugin: (*owner).to_owned(),
            });
        }
        let invalid = |message: String| TypeError::InvalidSchema {
            object_type: object_type.to_owned(),
            message,
        };
        if schema.len() > MAX_SCHEMA_BYTES {
            return Err(invalid("it is larger than 64 KB".to_owned()));
        }
        let schema: PropertySchema =
            serde_json::from_str(schema).map_err(|error| invalid(error.to_string()))?;
        let mut table = self.write();
        if let Some(held) = table.held.get(object_type) {
            if held.plugin != plugin {
                return Err(TypeError::HeldByPlugin {
                    object_type: object_type.to_owned(),
                    plugin: held.plugin.clone(),
                });
            }
            table.held.remove(object_type);
            table.rebuild_schemas();
        }
        if table
            .held
            .values()
            .filter(|held| held.instance == instance)
            .count()
            >= MAX_TYPES
        {
            return Err(TypeError::TooMany);
        }
        table
            .schemas
            .register(object_type, schema.clone())
            .map_err(TypeError::Refused)?;
        table.held.insert(
            object_type.to_owned(),
            Held {
                plugin: plugin.to_owned(),
                instance,
                schema,
            },
        );
        Ok(())
    }

    /// Release every type the instance `instance` registered, as when its
    /// plugin stops.
    pub fn release(&self, instance: u64) {
        let mut table = self.write();
        if !table.held.values().any(|held| held.instance == instance) {
            return;
        }
        table.held.retain(|_, held| held.instance != instance);
        table.rebuild_schemas();
    }

    /// The plugin that holds `object_type`.
    pub fn owner(&self, object_type: &str) -> Option<String> {
        self.read()
            .held
            .get(object_type)
            .map(|held| held.plugin.clone())
    }

    /// Whether the instance `instance` holds `object_type`.
    pub(crate) fn held_by(&self, object_type: &str, instance: u64) -> bool {
        self.read()
            .held
            .get(object_type)
            .is_some_and(|held| held.instance == instance)
    }

    /// The types the plugin `plugin` holds, sorted.
    pub fn types_of(&self, plugin: &str) -> Vec<String> {
        self.read()
            .held
            .iter()
            .filter(|(_, held)| held.plugin == plugin)
            .map(|(object_type, _)| object_type.clone())
            .collect()
    }

    /// The registered schemas, for checking a write.
    pub fn schemas(&self) -> ObjectTypeRegistry {
        self.read().schemas.clone()
    }

    /// The first property of `object`, in its schema's order, that doesn't
    /// fit its type's schema. `None` when they all fit, or when no running
    /// plugin registered the type.
    pub fn problem(&self, object: &WorkspaceObject) -> Option<PropertyProblem> {
        let mut checked = object.clone();
        let error = self.read().schemas.normalize(&mut checked).err()?;
        let details = serde_json::to_value(&error).unwrap_or_default();
        Some(PropertyProblem {
            property: details["property"].as_str().unwrap_or_default().to_owned(),
            code: details["code"].as_str().unwrap_or_default().to_owned(),
        })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Table> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Table> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The type in a stable ID: everything before its first underscore. Types
/// hold no underscore, so a valid ID names exactly one type.
pub(crate) fn type_of_id(id: &str) -> Option<&str> {
    id.split_once('_').map(|(object_type, _)| object_type)
}
