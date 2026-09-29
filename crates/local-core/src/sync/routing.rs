//! One routing rule for object mutations, shared by the desktop host and the
//! MCP server. A mutation goes through the collaboration coordinator only
//! while the sync plugin is on and the workspace or object collaborates;
//! everything else is an ordinary local file write. The device connection is
//! requested only on the collaboration path, so local writes never read the
//! credential store.

use super::{DeviceConnection, SyncCredentials, WorkspaceSyncCoordinator};
use crate::{
    CoreError, CreateObjectInput, ErrorCategory, MutationResult, ObjectPatch, Result,
    WorkspaceEngine, WorkspaceObject,
};

fn require_connection(
    connection: impl FnOnce() -> Result<Option<DeviceConnection>>,
    operation: &str,
) -> Result<DeviceConnection> {
    // Matches the desktop host's `sync_sign_in_required` error.
    connection()?.ok_or_else(|| {
        CoreError::new(
            "sync_sign_in_required",
            ErrorCategory::Credential,
            "Sign in to use sync.",
            operation,
        )
    })
}

/// Whether an existing object collaborates. Answers from the in-memory
/// manifest and the sync journal only.
fn object_collaborates(engine: &WorkspaceEngine, id: &str) -> Result<bool> {
    Ok(engine.sync_plugin_enabled() && engine.collaboration_object_is_active(id)?)
}

pub fn route_create_object(
    engine: &WorkspaceEngine,
    connection: impl FnOnce() -> Result<Option<DeviceConnection>>,
    store: &impl SyncCredentials,
    input: CreateObjectInput,
) -> Result<MutationResult<WorkspaceObject>> {
    if !engine.sync_plugin_enabled() || engine.sync_configuration()?.is_none() {
        return engine.create_object(input);
    }
    let connection = require_connection(connection, "object_create")?;
    WorkspaceSyncCoordinator::collaboration_create_object(engine, &connection, store, input)
}

/// `expected_type`, when set, rejects a local update to an object of another type.
pub fn route_update_object(
    engine: &WorkspaceEngine,
    connection: impl FnOnce() -> Result<Option<DeviceConnection>>,
    store: &impl SyncCredentials,
    id: &str,
    patch: ObjectPatch,
    expected_type: Option<&str>,
) -> Result<MutationResult<WorkspaceObject>> {
    if !object_collaborates(engine, id)? {
        return match expected_type {
            Some(object_type) => engine.update_object_typed(id, object_type, patch),
            None => engine.update_object(id, patch),
        };
    }
    let connection = require_connection(connection, "object_update")?;
    WorkspaceSyncCoordinator::collaboration_update_object(engine, &connection, store, id, patch)
}

pub fn route_move_object(
    engine: &WorkspaceEngine,
    connection: impl FnOnce() -> Result<Option<DeviceConnection>>,
    store: &impl SyncCredentials,
    id: &str,
    destination: &str,
    expected_revision: &str,
) -> Result<MutationResult<WorkspaceObject>> {
    if !object_collaborates(engine, id)? {
        return engine.move_object(id, destination, expected_revision);
    }
    let connection = require_connection(connection, "object_move")?;
    WorkspaceSyncCoordinator::collaboration_move_object(
        engine,
        &connection,
        store,
        id,
        destination,
        expected_revision,
    )
}

pub fn route_delete_object(
    engine: &WorkspaceEngine,
    connection: impl FnOnce() -> Result<Option<DeviceConnection>>,
    store: &impl SyncCredentials,
    id: &str,
    expected_revision: &str,
) -> Result<MutationResult<WorkspaceObject>> {
    if !object_collaborates(engine, id)? {
        return engine.delete_object(id, expected_revision);
    }
    let connection = require_connection(connection, "object_delete")?;
    WorkspaceSyncCoordinator::collaboration_delete_object(
        engine,
        &connection,
        store,
        id,
        expected_revision,
    )
}
