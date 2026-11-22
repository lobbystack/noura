//! Orbs, the workspace's personal agents, for kernel screens and tests.
//! Each call reaches the session's AI service, the same one the AI plugin
//! reaches through the `ai` interface, so no window holds Orb logic.

use plugin_host::{AiModel, AiOrb, AiOrbChanges, AiOrbRun, AiRun};

use super::App;
use crate::AppError;

impl App {
    /// The default Orb, then the open workspace's Orbs by title.
    pub fn orbs(&self) -> Result<Vec<AiOrb>, AppError> {
        self.session.ai.orbs().map_err(orb_error)
    }

    /// The models of the providers you turned on, each provider's own
    /// first, with the efforts each takes.
    pub fn ai_models(&self) -> Result<Vec<AiModel>, AppError> {
        self.session.ai.models().map_err(orb_error)
    }

    /// Create an Orb named `title` in `Orbs/<title>/`.
    pub fn create_orb(&mut self, title: &str) -> Result<AiOrb, AppError> {
        self.session.ai.create_orb(title).map_err(orb_error)
    }

    /// The Orb `id`'s runs, the newest first, at most `limit`.
    pub fn orb_runs(&self, id: &str, limit: u32) -> Result<Vec<AiOrbRun>, AppError> {
        self.session.ai.orb_runs(id, limit).map_err(orb_error)
    }

    /// Send `message` to the Orb `id` in its thread. Its tool calls run
    /// without asking.
    pub fn send_to_orb(&mut self, id: &str, message: &str) -> Result<AiRun, AppError> {
        self.session
            .ai
            .send_to_orb(crate::ai::orbs::ORB_PLUGIN, id, message)
            .map_err(orb_error)
    }

    /// Change the Orb `id`'s file, when it still holds `expected_revision`.
    pub fn update_orb(
        &mut self,
        id: &str,
        changes: AiOrbChanges,
        expected_revision: &str,
    ) -> Result<AiOrb, AppError> {
        self.session
            .ai
            .update_orb(id, changes, expected_revision)
            .map_err(orb_error)
    }

    /// Pause the Orb `id`'s triggers, or let them fire again.
    pub fn set_orb_paused(
        &mut self,
        id: &str,
        paused: bool,
        expected_revision: &str,
    ) -> Result<AiOrb, AppError> {
        self.session
            .ai
            .set_orb_paused(id, paused, expected_revision)
            .map_err(orb_error)
    }

    /// Check every Orb's triggers now: take up the runs a quit left,
    /// catch up on missed schedules, and start the Orbs that waiting events
    /// name. The runner checks by itself every 30 seconds.
    pub fn check_orbs(&mut self) {
        self.session.ai.check_orbs();
    }

    /// Whether Noura holds every Orb's triggers because you haven't used
    /// Noura on this device in a while.
    pub fn orbs_held(&self) -> bool {
        self.session.ai.orbs_held()
    }

    /// Keep the Orbs' triggers running after a while away, or pause every
    /// Orb that has triggers.
    pub fn keep_orbs_running(&mut self, keep: bool) -> Result<(), AppError> {
        self.session.ai.keep_orbs_running(keep).map_err(orb_error)
    }
}

fn orb_error(error: plugin_host::ServiceError) -> AppError {
    use plugin_host::ServiceError;
    match error {
        ServiceError::NotFound(id) => {
            AppError::new("orb_not_found", format!("No Orb has the ID {id}."))
        }
        ServiceError::RevisionConflict(_) => AppError::new(
            "revision_conflict",
            "The Orb's file changed elsewhere. Showing the latest version.",
        ),
        ServiceError::IdentityConflict(_) => AppError::new(
            "identity_conflict",
            "More than one file holds this Orb's ID, so it doesn't run until only one does.",
        ),
        ServiceError::InvalidProperty(problem) => AppError::new(
            "invalid_property",
            format!(
                "The Orb's {} isn't valid: {}.",
                problem.property, problem.code
            ),
        ),
        ServiceError::AlreadyExists(path) => AppError::new(
            "already_exists",
            format!("{path} already exists. Choose another name."),
        ),
        ServiceError::Failed(message) => AppError::new("orb_failed", message),
        other => AppError::new("orb_failed", format!("{other:?}")),
    }
}
