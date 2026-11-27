//! Settings: opening it, its sections and what each section does.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use super::{App, Change};
use crate::{
    AboutLink, LoginItem, Notice, NoticeKind, OpenAtLogin, Settings, SettingsGroup,
    SettingsSection, THIRD_PARTY_LICENSES,
    diagnostics::{self, WorkspaceHealth},
    settings::{
        AI_PLUGIN, ai_search_entry, is_web_link, section_keywords, section_rank, section_showing,
        settings_section, shows_ai_settings,
    },
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
    /// nothing registered `id`. When Settings is open already, it shows
    /// the section `id`.
    pub fn open_settings_at(&mut self, id: &str) {
        let plugin = self.plugin_showing(id);
        if self.settings.is_some() {
            self.select_settings_section(id);
            return;
        }
        let id = section_showing(id);
        let section = if self.settings_section(id).is_some() {
            id.to_owned()
        } else {
            settings_section::GENERAL.to_owned()
        };
        self.settings = Some(Settings {
            section,
            open_at_login: OpenAtLogin::read(self.login_item.as_ref()),
            notice: None,
            query: String::new(),
            plugin_details: plugin,
        });
        self.follow_settings_section();
        if self.mcp_test != crate::McpTest::Testing {
            self.mcp_test = crate::McpTest::NotRun;
        }
        self.listeners.emit(Change::Settings);
    }

    /// Settings, while it is open.
    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }

    pub fn close_settings(&mut self) {
        if self.settings.take().is_some() {
            self.close_ignored_files();
            self.listeners.emit(Change::Settings);
        }
    }

    /// Search settings for `query`: the section list keeps only the
    /// sections whose title or keywords hold it.
    pub fn set_settings_query(&mut self, query: &str) {
        if let Some(settings) = &mut self.settings
            && settings.query != query
        {
            query.clone_into(&mut settings.query);
            self.listeners.emit(Change::Settings);
        }
    }

    /// The plugin whose page in Plugins holds the AI settings: the AI
    /// plugin while it's installed, on or off, or else the first plugin
    /// installed that uses the AI providers, such as Transcription. None
    /// when no plugin installed uses them.
    pub fn ai_settings_plugin(&self) -> Option<String> {
        let rows = self.plugins();
        let installed = || rows.iter().filter(|row| row.installed);
        installed()
            .find(|row| row.id == AI_PLUGIN)
            .or_else(|| installed().find(|row| row.uses_ai()))
            .map(|row| row.id.clone())
    }

    /// The plugin page Plugins opens on for the section `id`: the AI
    /// settings open [`Self::ai_settings_plugin`].
    fn plugin_showing(&self, id: &str) -> Option<String> {
        shows_ai_settings(id)
            .then(|| self.ai_settings_plugin())
            .flatten()
    }

    /// Show the details of the plugin `id` in Plugins, in place of the
    /// list, or the list again with `None`.
    pub fn show_plugin_details(&mut self, id: Option<&str>) {
        if let Some(settings) = &mut self.settings
            && settings.plugin_details.as_deref() != id
        {
            settings.plugin_details = id.map(str::to_owned);
            self.listeners.emit(Change::Settings);
        }
    }

    /// Open the page the shown section draws, and close the one it no
    /// longer shows: Ignored files reads its list when it shows.
    fn follow_settings_section(&mut self) {
        let showing = self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.section == settings_section::IGNORED_FILES);
        if showing && self.ignored_files.is_none() {
            self.open_ignored_files();
        } else if !showing {
            self.close_ignored_files();
        }
    }

    /// Show the section `id` in the open Settings.
    /// The AI settings and Web search open the AI plugin's page in
    /// Plugins.
    pub fn select_settings_section(&mut self, id: &str) {
        let plugin = self.plugin_showing(id);
        let id = section_showing(id);
        if self.settings_section(id).is_none() {
            return;
        }
        if let Some(settings) = &mut self.settings
            && (settings.section != id || settings.plugin_details != plugin)
        {
            if settings.section != id {
                settings.notice = None;
            }
            settings.section = id.to_owned();
            settings.plugin_details = plugin;
            self.follow_settings_section();
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
            None => {
                let section = SettingsSection {
                    id: id.to_owned(),
                    title: title.to_owned(),
                    group,
                    keywords: section_keywords(id).to_owned(),
                };
                // A known section goes before the first known section
                // that follows it; any other goes last.
                let index = section_rank(id)
                    .and_then(|rank| {
                        self.settings_sections.iter().position(|known| {
                            section_rank(&known.id).is_some_and(|known| known > rank)
                        })
                    })
                    .unwrap_or(self.settings_sections.len());
                self.settings_sections.insert(index, section);
            }
        }
        self.listeners.emit(Change::Settings);
    }

    /// The sections Search settings shows for what it holds, by group:
    /// every section while it is empty.
    /// While it holds something, the AI plugin's page shows as AI when
    /// it matches, beside Plugins, which holds it.
    pub fn matching_settings_sections(&self) -> Vec<(SettingsGroup, Vec<&SettingsSection>)> {
        let query = self
            .settings
            .as_ref()
            .map(|settings| settings.query.as_str())
            .unwrap_or_default();
        let ai = ai_search_entry();
        let ai_found =
            !query.trim().is_empty() && ai.matches(query) && self.ai_settings_plugin().is_some();
        self.settings_sections()
            .into_iter()
            .map(|(group, sections)| {
                let mut sections = sections
                    .into_iter()
                    .filter(|section| section.matches(query))
                    .collect::<Vec<_>>();
                if ai_found && group == ai.group {
                    let at = sections
                        .iter()
                        .position(|section| section.id == settings_section::PLUGINS)
                        .map_or(0, |index| index + 1);
                    sections.insert(at, ai);
                }
                (group, sections)
            })
            .filter(|(_, sections)| !sections.is_empty())
            .collect()
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
            names.push(workspace.name());
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

    /// Use the third-party license list at `path`, which this copy's
    /// package ships, for About's Third-party licenses link.
    pub fn set_third_party_licenses(&mut self, path: PathBuf) {
        self.third_party_licenses = Some(path);
    }

    /// Open one of About's links. Third-party licenses opens the list the
    /// package ships when there is one; every other link goes to the
    /// browser through [`App::open_link`].
    pub fn open_about_link(&mut self, link: &AboutLink) {
        let bundled = (*link == THIRD_PARTY_LICENSES)
            .then(|| self.third_party_licenses.clone())
            .flatten();
        let Some(path) = bundled else {
            self.open_link(link.url);
            return;
        };
        if let Err(error) = self.system.open_file(&path) {
            tracing::warn!(%error, "the third-party license list didn't open");
            self.settings_notice(
                "Couldn't open the license list",
                "Couldn't open the third-party licenses.",
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
