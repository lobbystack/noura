//! Notes open in the editor: what each note's tab shows, the saves that
//! bring its edits to disk, its properties and its name, and the guard that
//! saves notes before a tab, the window, the workspace or the app closes.

use std::time::{Duration, Instant};

use super::{App, Change, Job};
use crate::{
    AppError, LineWidth, TabId,
    files::remap,
    notes::{Finished, NoteSession, NoteSync, PropertyValue, SaveProblem, is_markdown},
};

/// How long a save that must finish first, as before a rename, may take.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

/// Something that waits for open notes to save before it happens.
#[derive(Clone)]
pub(super) enum Leave {
    CloseTab(TabId),
    CloseWindow,
    Quit,
    /// Open another workspace, or create one.
    Open(Job),
}

/// The leave waiting for notes to save, and the prompt once a save failed.
pub(super) struct Leaving {
    action: Leave,
    tabs: Vec<TabId>,
    prompt: Option<UnsavedPrompt>,
}

/// What Noura asks when a note couldn't save before a tab, the window, the
/// workspace or the app closed. Nothing closes until you choose Try again,
/// Keep editing or Discard changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsavedPrompt {
    pub title: String,
    pub message: String,
}

impl App {
    /// The note open in the tab `tab`, when the tab shows a Markdown file
    /// Noura could read.
    pub fn note(&self, tab: TabId) -> Option<&NoteSession> {
        self.notes.sessions.get(&tab)
    }

    /// Why the Markdown file in the tab `tab` couldn't open in the editor.
    pub fn note_unreadable(&self, tab: TabId) -> Option<&AppError> {
        self.notes.unreadable.get(&tab)
    }

    /// The editor in the tab `tab` holds `text` at buffer version
    /// `version`, as of `at`. The first edit keeps a preview tab open, and
    /// a save follows once typing pauses.
    pub fn edit_note(&mut self, tab: TabId, version: usize, text: &str, at: Instant) {
        let Some(session) = self.notes.sessions.get_mut(&tab) else {
            return;
        };
        if session.edited(version, text, at) {
            self.keep_tab(tab);
            self.listeners.emit(Change::Note);
        }
    }

    /// Start the saves due at `now`. Clients call this every frame, and
    /// again at [`App::next_note_save`].
    pub fn save_due_notes(&mut self, now: Instant) {
        let due: Vec<TabId> = self
            .notes
            .sessions
            .iter()
            .filter(|(_, session)| session.is_due(now))
            .map(|(tab, _)| *tab)
            .collect();
        for tab in due {
            self.start_note_write(tab);
        }
    }

    /// When typing makes the next save due.
    pub fn next_note_save(&self) -> Option<Instant> {
        self.notes
            .sessions
            .values()
            .filter_map(NoteSession::next_due)
            .min()
    }

    /// Save the note in the tab `tab` now, as Mod-S and Try again do.
    pub fn save_note_now(&mut self, tab: TabId) {
        if let Some(session) = self.notes.sessions.get_mut(&tab) {
            session.save_soon();
            self.start_note_write(tab);
            self.listeners.emit(Change::Note);
        }
    }

    /// Start saving the note the window shows, when it has unsaved edits,
    /// as you switch to another tab.
    pub(super) fn save_leaving_note(&mut self) {
        let Some(tab) = self.tabs.active().map(crate::Tab::id) else {
            return;
        };
        if self
            .notes
            .sessions
            .get(&tab)
            .is_some_and(|session| session.is_unsaved() && !session.is_saving())
        {
            self.save_note_now(tab);
        }
    }

    /// Save the note the window shows now.
    pub(super) fn save_active_note(&mut self) {
        if let Some(tab) = self.tabs.active().map(crate::Tab::id) {
            self.save_note_now(tab);
        }
    }

    /// What the editor in the tab `tab` has to catch up on after a save.
    pub fn take_note_sync(&mut self, tab: TabId) -> Option<NoteSync> {
        self.notes.sessions.get_mut(&tab)?.take_sync()
    }

    /// Whether a note save is running.
    pub fn is_saving_notes(&self) -> bool {
        self.notes.sessions.values().any(NoteSession::is_saving)
    }

    /// Whether any open note has edits that aren't on disk yet.
    pub fn has_unsaved_notes(&self) -> bool {
        self.notes.sessions.values().any(NoteSession::is_unsaved)
    }

    /// Set the property `key` of the managed note in the tab `tab`. The
    /// write waits for a running save, and only a file still as Noura read
    /// it takes it.
    pub fn set_note_property(&mut self, tab: TabId, key: &str, value: PropertyValue) {
        let Some(session) = self.notes.sessions.get_mut(&tab) else {
            return;
        };
        let Some(json) = value.to_json() else {
            return;
        };
        if !session.has_property(key) {
            return;
        }
        session.queue_property(key.to_owned(), json);
        self.start_note_write(tab);
        self.listeners.emit(Change::Note);
    }

    /// Rename the note in the tab `tab` from its name field to `typed`. The
    /// name loses what a file name can't hold; an empty or unchanged name
    /// keeps the old one, and so does a taken one, which the note's
    /// [`NoteSession::name_problem`] then explains. The note keeps its ID.
    pub fn rename_note(&mut self, tab: TabId, typed: &str) {
        let Some(session) = self.notes.sessions.get_mut(&tab) else {
            return;
        };
        session.set_name_problem(None);
        let name = local_core::normalize_file_name(typed);
        let from = session.relative_path().to_owned();
        if name.is_empty() || name == session.name() {
            self.listeners.emit(Change::Note);
            return;
        }
        if let Err((_, problem)) = self.flush_notes_inside(&from) {
            self.set_name_problem(
                tab,
                format!("Noura couldn't save the note first. {}", problem.reason),
            );
            return;
        }
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.rename(&from, &name) {
            Ok(to) => {
                self.dismiss_notice();
                self.follow_move(&from, &to);
            }
            Err(error) => {
                let problem = if error.code == "name_taken" {
                    "A file with that name already exists in this folder.".to_owned()
                } else {
                    error.message
                };
                let to = crate::operations::renamed_path(&from, &name, false);
                self.follow_failed_change(&from, Some(&to));
                self.set_name_problem(tab, problem);
            }
        }
        self.listeners.emit(Change::Note);
    }

    fn set_name_problem(&mut self, tab: TabId, problem: String) {
        if let Some(session) = self.notes.sessions.get_mut(&tab) {
            session.set_name_problem(Some(problem));
        }
        self.listeners.emit(Change::Note);
    }

    /// The line width every open note uses.
    pub fn line_width(&self) -> LineWidth {
        self.layout.line_width
    }

    /// Use `width` for every open note, on this device, from now on.
    pub fn set_line_width(&mut self, width: LineWidth) {
        if self.layout.line_width != width {
            self.layout.line_width = width;
            self.layout_changed();
        }
    }

    /// The prompt about a note that couldn't save before something closed.
    pub fn unsaved_prompt(&self) -> Option<&UnsavedPrompt> {
        self.leaving.as_ref()?.prompt.as_ref()
    }

    /// Try the failed saves again, and close once they succeed.
    pub fn retry_leaving(&mut self) {
        let Some(leaving) = &mut self.leaving else {
            return;
        };
        leaving.prompt = None;
        let tabs = leaving.tabs.clone();
        for tab in tabs {
            if let Some(session) = self.notes.sessions.get_mut(&tab)
                && session.is_unsaved()
            {
                session.save_soon();
                self.start_note_write(tab);
            }
        }
        self.listeners.emit(Change::Note);
        self.advance_leaving();
    }

    /// Close nothing and show the note that couldn't save.
    pub fn keep_editing(&mut self) {
        let Some(leaving) = self.leaving.take() else {
            return;
        };
        let failed = leaving.tabs.iter().copied().find(|tab| {
            self.notes
                .sessions
                .get(tab)
                .is_some_and(|session| session.save_problem().is_some())
        });
        if let Some(tab) = failed {
            self.activate_tab(tab);
        }
        self.listeners.emit(Change::Note);
    }

    /// Drop the edits that couldn't save, leaving each file as it is on
    /// disk, and close.
    pub fn discard_and_leave(&mut self) {
        let Some(leaving) = self.leaving.take() else {
            return;
        };
        for tab in &leaving.tabs {
            if let Some(session) = self.notes.sessions.get_mut(tab)
                && !session.is_saving()
            {
                session.discard();
            }
        }
        self.listeners.emit(Change::Note);
        self.carry_out(leaving.action);
    }

    /// Do `action` once the notes it closes are on disk. Their saves start
    /// now; a failed one shows [`App::unsaved_prompt`].
    pub(super) fn leave(&mut self, action: Leave) {
        if self.leaving.is_some() {
            return;
        }
        let tabs: Vec<TabId> = match &action {
            Leave::CloseTab(tab) => vec![*tab],
            _ => self.notes.sessions.keys().copied().collect(),
        };
        let unsaved: Vec<TabId> = tabs
            .into_iter()
            .filter(|tab| {
                self.notes
                    .sessions
                    .get(tab)
                    .is_some_and(NoteSession::is_unsaved)
            })
            .collect();
        if unsaved.is_empty() {
            self.carry_out(action);
            return;
        }
        for tab in &unsaved {
            if let Some(session) = self.notes.sessions.get_mut(tab) {
                session.save_soon();
            }
            self.start_note_write(*tab);
        }
        self.leaving = Some(Leaving {
            action,
            tabs: unsaved,
            prompt: None,
        });
        self.listeners.emit(Change::Note);
        self.advance_leaving();
    }

    /// Close once every note the leave waits for is on disk, or ask once a
    /// save failed and no save runs.
    fn advance_leaving(&mut self) {
        let Some(leaving) = &self.leaving else {
            return;
        };
        if leaving.prompt.is_some() {
            return;
        }
        let mut failed: Option<(String, SaveProblem)> = None;
        let mut waiting = Vec::new();
        for tab in &leaving.tabs {
            let Some(session) = self.notes.sessions.get(tab) else {
                continue;
            };
            if !session.is_unsaved() {
                continue;
            }
            match session.save_problem() {
                _ if session.is_saving() => waiting.push(*tab),
                Some(problem) => {
                    failed.get_or_insert((session.name(), problem.clone()));
                }
                None => waiting.push(*tab),
            }
        }
        if !waiting.is_empty() {
            for tab in waiting {
                self.start_note_write(tab);
            }
            return;
        }
        match failed {
            Some((name, problem)) => {
                if let Some(leaving) = &mut self.leaving {
                    leaving.prompt = Some(UnsavedPrompt {
                        title: format!("Couldn\u{2019}t save \u{201c}{name}\u{201d}"),
                        message: format!("{} Your changes are still in the note.", problem.reason),
                    });
                }
                self.listeners.emit(Change::Note);
            }
            None => {
                if let Some(leaving) = self.leaving.take() {
                    self.carry_out(leaving.action);
                }
            }
        }
    }

    fn carry_out(&mut self, action: Leave) {
        match action {
            Leave::CloseTab(tab) => self.close_tab_now(tab),
            Leave::CloseWindow => {
                self.close_window = true;
                self.listeners.emit(Change::Window);
            }
            Leave::Quit => {
                self.quit = true;
                self.listeners.emit(Change::Window);
            }
            Leave::Open(job) => self.start(job),
        }
    }

    /// Hand the next write of the note in `tab` to a worker thread.
    fn start_note_write(&mut self, tab: TabId) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let engine = workspace.shared_engine();
        let Some(write) = self
            .notes
            .sessions
            .get_mut(&tab)
            .and_then(NoteSession::start_write)
        else {
            return;
        };
        let listeners = self.listeners.clone();
        let spawned = self.notes.spawn(tab, write, engine.clone(), move || {
            listeners.emit(Change::WorkFinished);
        });
        if let Some(write) = spawned {
            // No thread to run it on: write here instead.
            let written = write.run(&engine);
            self.finish_note_write(Finished { tab, written });
        }
    }

    fn finish_note_write(&mut self, finished: Finished) {
        if let Some(session) = self.notes.sessions.get_mut(&finished.tab) {
            session.finish_write(finished.written, Instant::now());
        }
    }

    /// Take the results of the saves that finished. Returns whether any
    /// did.
    pub(super) fn collect_note_writes(&mut self) -> bool {
        let mut finished = Vec::new();
        while let Ok(result) = self.notes.results.try_recv() {
            finished.push(result);
        }
        if finished.is_empty() {
            return false;
        }
        for result in finished {
            self.finish_note_write(result);
        }
        self.listeners.emit(Change::Note);
        self.advance_leaving();
        true
    }

    /// Save every note at `relative_path`, or inside the folder there, and
    /// return once the saves are on disk, as before the file moves or goes
    /// to another app. Fails with the note's name and the reason when a
    /// save fails.
    pub(super) fn flush_notes_inside(
        &mut self,
        relative_path: &str,
    ) -> Result<(), (String, SaveProblem)> {
        let tabs: Vec<TabId> = self
            .notes
            .sessions
            .iter()
            .filter(|(_, session)| {
                remap(session.relative_path(), relative_path, relative_path).is_some()
            })
            .map(|(tab, _)| *tab)
            .collect();
        let Some(engine) = self.workspace.as_ref().map(crate::Workspace::shared_engine) else {
            return Ok(());
        };
        for tab in tabs {
            // A running save finishes first.
            while self
                .notes
                .sessions
                .get(&tab)
                .is_some_and(NoteSession::is_saving)
            {
                match self.notes.results.recv_timeout(FLUSH_TIMEOUT) {
                    Ok(result) => self.finish_note_write(result),
                    Err(_) => break,
                }
            }
            // The text, then any property edits.
            for _ in 0..2 {
                let Some(session) = self.notes.sessions.get_mut(&tab) else {
                    break;
                };
                let Some(write) = session.start_write() else {
                    break;
                };
                let written = write.run(&engine);
                session.finish_write(written, Instant::now());
            }
            if let Some(session) = self.notes.sessions.get(&tab)
                && session.is_unsaved()
                && let Some(problem) = session.save_problem()
            {
                return Err((session.name(), problem.clone()));
            }
        }
        self.listeners.emit(Change::Note);
        Ok(())
    }

    /// Save the notes at `relative_path`, or inside the folder there, before
    /// `doing` something to the item there. Returns whether they're on
    /// disk; a notice says why when one isn't.
    pub(super) fn save_before(&mut self, relative_path: &str, doing: &str) -> bool {
        match self.flush_notes_inside(relative_path) {
            Ok(()) => true,
            Err((name, problem)) => {
                let title = format!("Couldn\u{2019}t save \u{201c}{name}\u{201d} before {doing}");
                self.show_notice(&title, AppError::new("save_failed", problem.reason), None);
                false
            }
        }
    }

    /// Bring the note sessions in line with the tabs: open each Markdown
    /// file a tab shows, follow tabs whose file moved, and drop the
    /// sessions of closed tabs.
    pub(super) fn sync_notes(&mut self) {
        let Some(workspace) = &self.workspace else {
            self.notes.sessions.clear();
            self.notes.unreadable.clear();
            return;
        };
        let files: Vec<(TabId, String)> = self
            .tabs
            .list()
            .iter()
            .filter_map(|tab| Some((tab.id(), tab.file()?.relative_path.clone())))
            .filter(|(_, path)| is_markdown(path))
            .collect();
        let open = |tab: &TabId| files.iter().any(|(id, _)| id == tab);
        self.notes.sessions.retain(|tab, _| open(tab));
        self.notes.unreadable.retain(|tab, _| open(tab));
        for (tab, path) in &files {
            if let Some(session) = self.notes.sessions.get_mut(tab) {
                if session.relative_path() != path {
                    session.moved_to(path);
                }
                continue;
            }
            if self.notes.unreadable.contains_key(tab) {
                continue;
            }
            match NoteSession::open(workspace.engine(), path) {
                Ok(session) => {
                    self.notes.sessions.insert(*tab, session);
                }
                Err(error) => {
                    self.notes.unreadable.insert(*tab, error);
                }
            }
        }
    }
}
