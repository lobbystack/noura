//! Settings: opening it, its sections and what each section does.

use std::{collections::BTreeMap, sync::Arc};

use super::{App, Change};
use crate::{
    LoginItem, Notice, NoticeKind, OpenAtLogin, Settings, SettingsGroup, SettingsSection,
    diagnostics::{self, WorkspaceHealth},
    settings::{is_web_link, settings_section},
};

/// The folder and file names in a path, split at both kinds of slash.
fn path_parts(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
}

impl App {
    /// Open Settings on its first section, or leave it on the section it
    /// shows when it's open already.
    pub fn open_settings(&mut self) {
        if self.settings.is_none() {
            self.open_settings_at(settings_section::GENERAL);
        }
    }

    /// Open Settings on the section `id`, or on its first section when
    /// nothing registered `id`.
    pub fn open_settings_at(&mut self, id: &str) {
        let section = if self.settings_section(id).is_some() {
            id.to_owned()
        } else {
            settings_section::GENERAL.to_owned()
        };
        let manifest_revision = self.read_manifest();
        self.settings = Some(Settings {
            section,
            open_at_login: OpenAtLogin::read(self.login_item.as_ref()),
            notice: None,
            manifest_revision,
        });
        self.listeners.emit(Change::Settings);
    }

    /// Settings, while it is open.
    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }

    pub fn close_settings(&mut self) {
        if self.settings.take().is_some() {
            self.listeners.emit(Change::Settings);
        }
    }

    /// Show the section `id` in the open Settings.
    pub fn select_settings_section(&mut self, id: &str) {
        if self.settings_section(id).is_none() {
            return;
        }
        if let Some(settings) = &mut self.settings
            && settings.section != id
        {
            settings.section = id.to_owned();
            settings.notice = None;
            self.listeners.emit(Change::Settings);
        }
    }

    /// Add a section to Settings, after the sections already in `group`.
    /// Registering a section again renames it or moves it to `group`.
    pub fn register_settings_section(&mut self, id: &str, title: &str, group: SettingsGroup) {
        match self
            .settings_sections
            .iter_mut()
            .find(|section| section.id == id)
        {
            Some(section) => {
                title.clone_into(&mut section.title);
                section.group = group;
            }
            None => self.settings_sections.push(SettingsSection {
                id: id.to_owned(),
                title: title.to_owned(),
                group,
            }),
        }
        self.listeners.emit(Change::Settings);
    }

    /// Settings' sections by group, in the order Settings lists them.
    /// Groups without a section are left out.
    pub fn settings_sections(&self) -> Vec<(SettingsGroup, Vec<&SettingsSection>)> {
        SettingsGroup::ALL
            .into_iter()
            .map(|group| {
                let sections = self
                    .settings_sections
                    .iter()
                    .filter(|section| section.group == group)
                    .collect::<Vec<_>>();
                (group, sections)
            })
            .filter(|(_, sections)| !sections.is_empty())
            .collect()
    }

    /// Use `item` for Open at login. Until a client calls this, the
    /// switch is off limits.
    pub fn set_login_item(&mut self, item: Arc<dyn LoginItem>) {
        self.login_item = item;
    }

    /// Add Noura to the apps that open at login, or remove it. When the
    /// system refuses, the switch stays as it was and says why.
    pub fn set_open_at_login(&mut self, enabled: bool) {
        if let Some(settings) = &mut self.settings {
            settings
                .open_at_login
                .set(self.login_item.as_ref(), enabled);
            self.listeners.emit(Change::Settings);
        }
    }

    /// Give the open workspace the display name `name`, trimmed, in its
    /// manifest. Returns whether the manifest on disk now holds it. The
    /// folder keeps its name, and the recent list shows the new one. An
    /// empty name keeps the old one. When the manifest changed on disk
    /// since Settings read it, nothing is written and Settings says so,
    /// then shows the manifest as it is now.
    pub fn rename_workspace(&mut self, name: &str) -> bool {
        let name = name.trim();
        let Some(workspace) = &self.workspace else {
            return false;
        };
        if name.is_empty() {
            return false;
        }
        if name == workspace.name() {
            return true;
        }
        let revision = match self
            .settings
            .as_ref()
            .and_then(|settings| settings.manifest_revision.clone())
        {
            Some(revision) => Ok(revision),
            None => workspace.manifest_revision().map(|(revision, _)| revision),
        };
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        match revision.and_then(|revision| workspace.save_name(name, &revision)) {
            Ok(revision) => {
                if let Some(settings) = &mut self.settings {
                    settings.manifest_revision = Some(revision);
                    settings.notice = None;
                }
                match self.service.remember(workspace) {
                    Ok(recents) => self.recents = recents,
                    Err(error) => {
                        tracing::warn!(code = %error.code, "the renamed workspace's recent entry wasn't saved");
                    }
                }
                self.listeners.emit(Change::Recents);
                self.listeners.emit(Change::Settings);
                true
            }
            Err(error) => {
                // Show the manifest as it is now, so trying again starts
                // from the file.
                let manifest_revision = self.read_manifest();
                if let Some(settings) = &mut self.settings {
                    settings.manifest_revision = manifest_revision;
                }
                self.settings_notice(
                    "Couldn't rename the workspace",
                    &error.message,
                    NoticeKind::Problem,
                );
                false
            }
        }
    }

    /// Read the open workspace's manifest again: show the name in it, and
    /// return its revision.
    fn read_manifest(&mut self) -> Option<String> {
        let workspace = self.workspace.as_mut()?;
        match workspace.manifest_revision() {
            Ok((revision, name)) => {
                if name != workspace.name() {
                    workspace.set_name(name);
                    self.listeners.emit(Change::Recents);
                }
                Some(revision)
            }
            Err(error) => {
                tracing::warn!(code = %error.code, "Settings couldn't read the workspace manifest");
                None
            }
        }
    }

    /// Put a diagnostics report on the clipboard: the app version, the
    /// system, the open workspace's indexed file count and problem counts
    /// by kind, and the last 200 lines of the local diagnostic log with
    /// paths, file and folder names, and quoted text replaced.
    pub fn copy_diagnostics(&mut self) {
        let mut names: Vec<String> = Vec::new();
        for recent in &self.recents {
            names.push(recent.name.clone());
            names.extend(path_parts(&recent.path));
        }
        let health = self.workspace.as_ref().map(|workspace| {
            names.push(workspace.name().to_owned());
            names.extend(path_parts(workspace.root()));
            let state = workspace.engine().state();
            let mut problems = BTreeMap::new();
            for problem in state.diagnostics {
                names.extend(
                    problem
                        .relative_path
                        .as_deref()
                        .map(path_parts)
                        .into_iter()
                        .flatten(),
                );
                *problems.entry(problem.code).or_insert(0) += 1;
            }
            WorkspaceHealth {
                indexed_files: state.indexed_files,
                problems,
            }
        });
        if let Some(tree) = &self.tree {
            names.extend(tree.names().map(str::to_owned));
        }
        let report = diagnostics::report(&self.log_folder, health.as_ref(), names);
        self.system.copy_text(&report);
        self.settings_notice(
            "Diagnostics copied",
            "Noura left out file names, paths and quoted text.",
            NoticeKind::Done,
        );
    }

    /// Open the web page at `url`, such as one of the About section's
    /// links, in the default browser. Only `http` and `https` links go.
    pub fn open_link(&mut self, url: &str) {
        if !is_web_link(url) {
            tracing::warn!("a link that isn't a web page was not opened");
            self.settings_notice(
                "Couldn't open the link",
                "Noura only opens links to web pages.",
                NoticeKind::Problem,
            );
            return;
        }
        if let Err(error) = self.system.open_url(url) {
            tracing::warn!(%error, "the browser didn't open");
            self.settings_notice(
                "Couldn't open the link",
                "Noura couldn't open your web browser.",
                NoticeKind::Problem,
            );
        }
    }

    /// Show what an action in Settings did, in Settings when it is open
    /// and as the window's notice otherwise.
    pub(super) fn settings_notice(&mut self, title: &str, message: &str, kind: NoticeKind) {
        let notice = Notice {
            title: title.to_owned(),
            message: message.to_owned(),
            remove_recent: None,
            kind,
        };
        match &mut self.settings {
            Some(settings) => {
                settings.notice = Some(notice);
                self.listeners.emit(Change::Settings);
            }
            None => {
                self.notice = Some(notice);
                self.listeners.emit(Change::Notice);
            }
        }
    }

    fn settings_section(&self, id: &str) -> Option<&SettingsSection> {
        self.settings_sections
            .iter()
            .find(|section| section.id == id)
    }
}
