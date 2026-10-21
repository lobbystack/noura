//! Changes that haven't reached disk yet, which Noura saves before it
//! hands a file to another app.

use crate::AppError;

/// Changes still waiting to be saved, such as typing the editor writes
/// after a pause.
pub trait PendingSaves: Send + Sync {
    /// Save every change still waiting for the file at `relative_path`, or
    /// for the files inside the folder there, and return once they are on
    /// disk.
    fn save_now(&self, relative_path: &str) -> Result<(), AppError>;
}

/// Nothing waits to be saved: every change reaches disk before its call
/// returns.
pub struct NoPendingSaves;

impl PendingSaves for NoPendingSaves {
    fn save_now(&self, _relative_path: &str) -> Result<(), AppError> {
        Ok(())
    }
}
