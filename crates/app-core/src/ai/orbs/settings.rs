//! The default Orb's provider, model and effort. The default Orb has no
//! file, and provider IDs belong to each device, so this device keeps
//! them in its app data beside the providers.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use workspace_format::orb::Effort;

/// Where the default Orb's settings live in the app data.
const FILE: &str = "ai/default-orb.json";

/// What the default Orb asks with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DefaultOrb {
    /// A provider ID on this device, or none for your first provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<String>,
    /// A model in place of the provider's own, or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// How hard that model reasons, as an Orb's file names it, or none
    /// for the model's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

impl DefaultOrb {
    pub(crate) fn effort(&self) -> Option<Effort> {
        self.effort.as_deref().and_then(Effort::parse)
    }

    pub(crate) fn set_effort(&mut self, effort: Option<Effort>) {
        self.effort = effort.map(|effort| effort.as_str().to_owned());
    }
}

/// The default Orb's settings and the file that holds them.
pub(crate) struct Settings {
    file: PathBuf,
    current: DefaultOrb,
}

impl Settings {
    /// The settings `app_data` holds, or none chosen when it holds none or
    /// they can't be read.
    pub(crate) fn load(app_data: &Path) -> Self {
        let file = app_data.join(FILE);
        let current = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { file, current }
    }

    pub(crate) fn current(&self) -> &DefaultOrb {
        &self.current
    }

    /// Keep `next`, once its bytes are on disk.
    pub(crate) fn save(&mut self, next: DefaultOrb) -> std::io::Result<()> {
        if let Some(folder) = self.file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let bytes = serde_json::to_vec_pretty(&next).map_err(std::io::Error::other)?;
        let partial = self.file.with_extension("json.partial");
        let mut written = std::fs::File::create(&partial)?;
        written.write_all(&bytes)?;
        written.sync_all()?;
        drop(written);
        std::fs::rename(&partial, &self.file)?;
        self.current = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_reopen_as_they_were_saved() {
        let app_data = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(app_data.path());
        assert_eq!(settings.current(), &DefaultOrb::default());
        let mut next = DefaultOrb {
            provider: Some("claude".into()),
            model: Some("claude-opus-5-5".into()),
            ..DefaultOrb::default()
        };
        next.set_effort(Some(Effort::High));
        settings.save(next.clone()).unwrap();
        let reopened = Settings::load(app_data.path());
        assert_eq!(reopened.current(), &next);
        assert_eq!(reopened.current().effort(), Some(Effort::High));
    }
}
