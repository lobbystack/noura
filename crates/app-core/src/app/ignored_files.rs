//! The Ignored files page's view state. Settings can host the page once
//! it exists; for now the workspace switcher opens it.

use super::{App, Change};
use crate::ignored::IgnoreList;

/// The Ignored files list while its page is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredFiles {
    /// The workspace's patterns, in the manifest's order.
    pub patterns: Vec<String>,
    /// Why the last change didn't save.
    pub problem: Option<String>,
    /// The revision of the manifest the patterns came from.
    revision: String,
}

impl App {
    /// Open the Ignored files page with the list from the manifest on disk.
    pub fn open_ignored_files(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        self.ignored_files = Some(match workspace.ignore_list() {
            Ok(list) => IgnoredFiles::from(list),
            Err(error) => IgnoredFiles {
                patterns: Vec::new(),
                problem: Some(error.message),
                revision: String::new(),
            },
        });
        self.listeners.emit(Change::Settings);
    }

    /// The Ignored files page, while it is open.
    pub fn ignored_files(&self) -> Option<&IgnoredFiles> {
        self.ignored_files.as_ref()
    }

    pub fn close_ignored_files(&mut self) {
        if self.ignored_files.take().is_some() {
            self.listeners.emit(Change::Settings);
        }
    }

    /// Add `pattern` to the list and save it. Returns whether the list on
    /// disk now holds the pattern. A blank or invalid pattern, or a
    /// manifest changed on disk, saves nothing and sets
    /// [`IgnoredFiles::problem`].
    pub fn add_ignored(&mut self, pattern: &str) -> bool {
        let pattern = pattern.trim();
        let Some(page) = &mut self.ignored_files else {
            return false;
        };
        if pattern.is_empty() {
            page.problem = Some("Enter a pattern.".to_owned());
            self.listeners.emit(Change::Settings);
            return false;
        }
        if page.patterns.iter().any(|known| known == pattern) {
            page.problem = None;
            self.listeners.emit(Change::Settings);
            return true;
        }
        let mut patterns = page.patterns.clone();
        patterns.push(pattern.to_owned());
        self.save_ignored(patterns)
    }

    /// Remove `pattern` from the list and save it.
    pub fn remove_ignored(&mut self, pattern: &str) -> bool {
        let Some(page) = &self.ignored_files else {
            return false;
        };
        let patterns = page
            .patterns
            .iter()
            .filter(|known| *known != pattern)
            .cloned()
            .collect();
        self.save_ignored(patterns)
    }

    fn save_ignored(&mut self, patterns: Vec<String>) -> bool {
        let (Some(workspace), Some(page)) = (&self.workspace, &self.ignored_files) else {
            return false;
        };
        let saved = workspace.save_ignore_list(patterns, &page.revision);
        // Show what the manifest holds now, whether or not the save worked,
        // so trying again starts from the file.
        let reloaded = workspace.ignore_list();
        let problem = saved.as_ref().err().map(|error| error.message.clone());
        self.ignored_files = Some(match reloaded {
            Ok(list) => IgnoredFiles {
                problem,
                ..IgnoredFiles::from(list)
            },
            Err(error) => IgnoredFiles {
                patterns: Vec::new(),
                problem: Some(error.message),
                revision: String::new(),
            },
        });
        self.listeners.emit(Change::Settings);
        if saved.is_ok() {
            self.refresh_tree(None);
        }
        saved.is_ok()
    }
}

impl From<IgnoreList> for IgnoredFiles {
    fn from(list: IgnoreList) -> Self {
        Self {
            patterns: list.patterns,
            problem: None,
            revision: list.revision,
        }
    }
}
