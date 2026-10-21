//! Notes open in the editor: what each note's tab shows, the saves that
//! bring its edits to disk, its properties and its name, and the guard that
//! saves notes before a tab, the window, the workspace or the app closes.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use super::{App, Change, Job};
use crate::{
    AppError, TabId,
    files::remap,
    links::NoteLinks,
    notes::{
        ConflictChoice, FileCheck, Finished, NoteSession, NoteSync, PropertyValue, Restored,
        SaveProblem, is_markdown,
    },
};

/// How long the notes may take to save when the app ends without asking,
/// as when the system quits Noura at logout.
const EXIT_SAVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Something that waits for open notes to save before it happens.
#[derive(Clone)]
pub(super) enum Leave {
    /// Close these tabs. A tab you close while another waits joins it.
    CloseTabs(Vec<TabId>),
    CloseWindow,
    /// A quit or a switch to another workspace, which closes every
    /// window, or a restart to update: each window saves its notes for the
    /// departure `round`, and the window that asked does `then` once all
    /// of them have. The other windows have no `then`.
    Depart {
        round: u64,
        then: Option<Departure>,
    },
}

/// What a departure does once every window's notes are on disk.
#[derive(Clone)]
pub(super) enum Departure {
    Quit,
    /// Open another workspace, or create one.
    Open(Job),
    /// Install the downloaded update and restart.
    Restart,
}

/// The leave waiting for notes to save, and the prompt once a save failed,
/// with the tab of the note it names.
pub(super) struct Leaving {
    action: Leave,
    prompt: Option<(TabId, UnsavedPrompt)>,
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

    /// What the links and embeds in the note in the tab `tab` lead to, for
    /// its editor to ask as it draws them.
    pub fn note_links(&self, tab: TabId) -> Option<NoteLinks> {
        let note = self.notes.sessions.get(&tab)?;
        let workspace = self.workspace.as_ref()?;
        Some(NoteLinks::new(
            note.relative_path(),
            self.session.link_files.clone(),
            workspace.shared_engine(),
            self.session.remote_images.clone(),
        ))
    }

    /// Let remote images load from this computer, so tests can serve them
    /// from a local server.
    #[cfg(feature = "test-hooks")]
    pub fn allow_loopback_images_for_testing(&self) {
        self.session.remote_images.allow_loopback();
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
            self.edited_while_leaving();
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

    /// The text the editor in the tab `tab` shows in place of its own,
    /// after Discard changes dropped the edits that weren't on disk.
    pub fn take_note_reset(&mut self, tab: TabId) -> Option<String> {
        self.notes.sessions.get_mut(&tab)?.take_reset()
    }

    /// Wait at most `timeout` for a running save before a rename, a move
    /// or a drag out, instead of 30 seconds. Tests only.
    #[doc(hidden)]
    pub fn set_note_flush_timeout_for_testing(&mut self, timeout: Duration) {
        self.notes.flush_timeout = timeout;
    }

    /// Whether a note save is running.
    pub fn is_saving_notes(&self) -> bool {
        self.notes.sessions.values().any(NoteSession::is_saving)
    }

    /// Whether any open note has edits that aren't on disk yet.
    pub fn has_unsaved_notes(&self) -> bool {
        self.notes.sessions.values().any(NoteSession::is_unsaved)
    }

    /// Save every open note now and wait for the saves, as the app ends
    /// without asking, as when the system quits Noura at logout. Nothing
    /// can ask about a save that fails then: its edits stay off the disk.
    /// Returns whether every note is on disk.
    pub fn save_notes_before_exit(&mut self) -> bool {
        let deadline = Instant::now() + EXIT_SAVE_TIMEOUT;
        // A note's text, then its property edits: two writes at most, and
        // a write that failed before gets one more try.
        let mut started: HashMap<TabId, u8> = HashMap::new();
        loop {
            let next: Vec<TabId> = self
                .notes
                .sessions
                .iter()
                .filter(|(tab, session)| {
                    session.is_unsaved()
                        && !session.is_saving()
                        && started.get(*tab).copied().unwrap_or(0) < 2
                })
                .map(|(tab, _)| *tab)
                .collect();
            for tab in next {
                *started.entry(tab).or_default() += 1;
                if let Some(session) = self.notes.sessions.get_mut(&tab) {
                    session.save_soon();
                }
                self.start_note_write(tab);
            }
            if !self.is_saving_notes() {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.notes.results.recv_timeout(left) {
                Ok(result) => self.finish_note_write(result),
                Err(_) => {
                    tracing::warn!("a note save didn't finish before the app ended");
                    break;
                }
            }
        }
        let saved = !self.has_unsaved_notes();
        if !saved {
            tracing::warn!("the app ended with a note that couldn't save");
        }
        self.listeners.emit(Change::Note);
        saved
    }

    /// Read the Markdown file in the tab `tab` again, after it couldn't
    /// open, as the tab's Try again does.
    pub fn retry_note(&mut self, tab: TabId) {
        if self.notes.unreadable.remove(&tab).is_some() {
            self.sync_notes();
            self.listeners.emit(Change::Note);
        }
    }

    /// Read the files that couldn't open again, as when the files changed:
    /// a file that downloaded or became UTF-8 text opens in its tab.
    pub(super) fn retry_unreadable_notes(&mut self) {
        if !self.notes.unreadable.is_empty() {
            self.notes.unreadable.clear();
            self.sync_notes();
            self.listeners.emit(Change::Note);
        }
    }

    /// Read the files of the notes in `tabs` again, when they changed
    /// since Noura read them and the notes have nothing unsaved. Each
    /// editor takes its file's text.
    pub(super) fn reread_notes(&mut self, tabs: &[TabId]) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let mut changed = false;
        for tab in tabs {
            if let Some(session) = self.notes.sessions.get_mut(tab) {
                changed |= session.reread(workspace.engine());
            }
        }
        if changed {
            self.listeners.emit(Change::Note);
        }
    }

    /// Bring each open note in line with its file after the files changed:
    /// a note with nothing unsaved shows what another app wrote, as an
    /// outside edit that keeps the caret where it is. A note with unsaved
    /// edits merges the other app's edit at its next save, or finds that
    /// both changed the same text. A note whose file went away offers
    /// Restore, and one whose file came back, or changed again during a
    /// conflict, saves at once.
    pub(super) fn check_notes_on_disk(&mut self) {
        let tabs: Vec<TabId> = self.notes.sessions.keys().copied().collect();
        self.check_note_files(&tabs);
    }

    fn check_note_files(&mut self, tabs: &[TabId]) {
        let Some(engine) = self
            .workspace
            .as_deref()
            .map(crate::Workspace::shared_engine)
        else {
            return;
        };
        let mut shown = false;
        let mut changed = Vec::new();
        for tab in tabs {
            let Some(session) = self.notes.sessions.get_mut(tab) else {
                continue;
            };
            match session.file_changed(&engine) {
                FileCheck::Same => {}
                FileCheck::Shown => shown = true,
                FileCheck::Changed => {
                    session.save_soon();
                    changed.push(*tab);
                }
            }
        }
        for tab in &changed {
            self.start_note_write(*tab);
        }
        if shown || !changed.is_empty() {
            self.listeners.emit(Change::Note);
        }
    }

    /// Open the conflict review of the note in `tab`, as the banner's
    /// Compare does.
    pub fn review_conflict(&mut self, tab: TabId) {
        if let Some(session) = self.notes.sessions.get_mut(&tab)
            && session.set_reviewing(true)
        {
            self.listeners.emit(Change::Note);
        }
    }

    /// Close the conflict review of the note in `tab` and keep editing.
    /// Saves still wait, and a banner opens the review again.
    pub fn close_conflict_review(&mut self, tab: TabId) {
        if let Some(session) = self.notes.sessions.get_mut(&tab)
            && session.set_reviewing(false)
        {
            self.listeners.emit(Change::Note);
        }
    }

    /// Choose which version the conflict of the note in `tab` keeps.
    /// Nothing happens until you confirm with [`App::confirm_version`].
    pub fn choose_version(&mut self, tab: TabId, choice: ConflictChoice) {
        if let Some(session) = self.notes.sessions.get_mut(&tab)
            && session.set_confirming(Some(choice))
        {
            self.listeners.emit(Change::Note);
        }
    }

    /// Take back the version you chose, and keep reviewing.
    pub fn cancel_version_choice(&mut self, tab: TabId) {
        if let Some(session) = self.notes.sessions.get_mut(&tab)
            && session.set_confirming(None)
        {
            self.listeners.emit(Change::Note);
        }
    }

    /// Keep the version you chose for the conflict of the note in `tab`,
    /// once a running save attempt is in. The version it replaces goes to
    /// the workspace's history folder first.
    pub fn confirm_version(&mut self, tab: TabId) {
        if !self.wait_for_note_save(tab) {
            return;
        }
        let Some(engine) = self
            .workspace
            .as_deref()
            .map(crate::Workspace::shared_engine)
        else {
            return;
        };
        if let Some(session) = self.notes.sessions.get_mut(&tab) {
            session.resolve(&engine);
            self.listeners.emit(Change::Note);
            self.edited_while_leaving();
        }
    }

    /// Write the note in `tab` back where its file was, after another
    /// program deleted the file while the note had unsaved edits. When a
    /// file is there again, nothing is replaced: the note's typing meets
    /// that file as another app's edit would.
    pub fn restore_note(&mut self, tab: TabId) {
        if !self.wait_for_note_save(tab) {
            return;
        }
        let Some(engine) = self
            .workspace
            .as_deref()
            .map(crate::Workspace::shared_engine)
        else {
            return;
        };
        let Some(session) = self.notes.sessions.get_mut(&tab) else {
            return;
        };
        if !session.is_deleted() {
            return;
        }
        match session.restore(&engine) {
            Restored::Written => self.refresh_tree(None),
            Restored::FileBack => self.check_note_files(&[tab]),
            Restored::Failed => {}
        }
        self.listeners.emit(Change::Note);
        self.edited_while_leaving();
    }

    /// Wait at most the flush timeout for a running save of the note in
    /// `tab`. Returns whether none runs now.
    fn wait_for_note_save(&mut self, tab: TabId) -> bool {
        let deadline = Instant::now() + self.notes.flush_timeout;
        while self
            .notes
            .sessions
            .get(&tab)
            .is_some_and(NoteSession::is_saving)
        {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.notes.results.recv_timeout(left) {
                Ok(result) => self.finish_note_write(result),
                Err(_) => return false,
            }
        }
        true
    }

    /// The tabs whose note has edits that aren't on disk yet.
    pub(super) fn unsaved_note_tabs(&self) -> Vec<TabId> {
        self.notes
            .sessions
            .iter()
            .filter(|(_, session)| session.is_unsaved())
            .map(|(tab, _)| *tab)
            .collect()
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
        // Like typing, a property edit keeps a preview tab open.
        self.keep_tab(tab);
        self.start_note_write(tab);
        self.listeners.emit(Change::Note);
        self.edited_while_leaving();
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

    /// The prompt about a note that couldn't save before something closed.
    pub fn unsaved_prompt(&self) -> Option<&UnsavedPrompt> {
        self.leaving
            .as_ref()?
            .prompt
            .as_ref()
            .map(|(_, prompt)| prompt)
    }

    /// Whether something waits for notes to save before it happens: a tab
    /// or this window closing, or a quit or a switch to another workspace
    /// that any window asked for. A client keeps polling the window
    /// meanwhile, even while it draws no frame, as when it is minimized.
    pub fn is_leaving(&self) -> bool {
        self.leaving.is_some() || self.session.departure().is_some()
    }

    /// Try the failed saves again, and close once they succeed.
    pub fn retry_leaving(&mut self) {
        let Some(leaving) = &mut self.leaving else {
            return;
        };
        leaving.prompt = None;
        let action = leaving.action.clone();
        for tab in self.leaving_tabs(&action) {
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

    /// Close nothing and show the note that couldn't save. A quit or a
    /// workspace switch stops in every window.
    pub fn keep_editing(&mut self) {
        let Some(leaving) = self.leaving.take() else {
            return;
        };
        if let Leave::Depart { round, .. } = leaving.action {
            self.session.end_departure(self.key, round);
        }
        if let Some((tab, _)) = leaving.prompt {
            self.activate_tab(tab);
        }
        self.listeners.emit(Change::Note);
    }

    /// Drop the edits of the note the prompt names, leaving its file as it
    /// is on disk, and close once the other notes are saved.
    pub fn discard_and_leave(&mut self) {
        let Some((tab, _)) = self
            .leaving
            .as_mut()
            .and_then(|leaving| leaving.prompt.take())
        else {
            return;
        };
        if let Some(session) = self.notes.sessions.get_mut(&tab)
            && !session.is_saving()
        {
            session.discard();
        }
        self.listeners.emit(Change::Note);
        self.advance_leaving();
    }

    /// Do `action` once the notes it closes are on disk. Their saves start
    /// now; a failed one shows [`App::unsaved_prompt`]. While another leave
    /// waits, `action` joins it: another tab's close adds that tab, and
    /// closing the window, quitting or switching workspace takes its place.
    pub(super) fn leave(&mut self, action: Leave) {
        let (action, before) = match &self.leaving {
            None => (action, Vec::new()),
            Some(leaving) => match widen(&leaving.action, action) {
                Some(action) => (action, self.leaving_tabs(&leaving.action)),
                None => return,
            },
        };
        for tab in self.leaving_tabs(&action) {
            if before.contains(&tab) {
                continue;
            }
            if let Some(session) = self.notes.sessions.get_mut(&tab)
                && session.is_unsaved()
            {
                session.save_soon();
                self.start_note_write(tab);
            }
        }
        match &mut self.leaving {
            Some(leaving) => leaving.action = action,
            None => {
                self.leaving = Some(Leaving {
                    action,
                    prompt: None,
                });
            }
        }
        self.listeners.emit(Change::Note);
        self.advance_leaving();
    }

    /// Ask every window to save its notes, and then quit, open another
    /// workspace or restart to update as `then` says. A tab or this window that waits to close
    /// gives way to it. Nothing happens while this window already takes
    /// part in a departure, or while another window's departure waits.
    pub(super) fn depart(&mut self, then: Departure) {
        if matches!(
            &self.leaving,
            Some(Leaving {
                action: Leave::Depart { .. },
                ..
            })
        ) {
            return;
        }
        let Some(round) = self.session.begin_departure(self.key) else {
            return;
        };
        self.departure_seen = round;
        self.leave(Leave::Depart {
            round,
            then: Some(then),
        });
    }

    /// Take part in another window's quit or workspace switch, and go
    /// ahead with this window's own once every window is ready. Returns
    /// whether anything changed.
    pub(super) fn poll_departure(&mut self) -> bool {
        let current = self.session.departure();
        // Windows that wait for the client to show them take part too.
        if current.is_some() {
            for window in &mut self.opened_windows {
                window.poll();
            }
        }
        if let Some(Leaving {
            action: Leave::Depart { round, then },
            prompt,
        }) = &self.leaving
        {
            let (round, then, asking) = (*round, then.clone(), prompt.is_some());
            if current.map(|(current, _)| current) != Some(round) {
                // It went ahead, or a window kept editing.
                self.leaving = None;
                self.listeners.emit(Change::Note);
                return true;
            }
            if let Some(then) = then
                && !asking
                && self.session.all_ready(round)
            {
                self.go_ahead(round, then);
                return true;
            }
            return false;
        }
        let Some((round, by)) = current else {
            return false;
        };
        if by == self.key || round <= self.departure_seen {
            return false;
        }
        self.departure_seen = round;
        let action = Leave::Depart { round, then: None };
        match &mut self.leaving {
            // Closing a tab or this window waits too; the departure closes
            // them anyway.
            Some(leaving) => {
                leaving.action = action;
                self.advance_leaving();
            }
            None => self.leave(action),
        }
        true
    }

    /// The tabs whose notes `action` waits for.
    fn leaving_tabs(&self, action: &Leave) -> Vec<TabId> {
        match action {
            Leave::CloseTabs(tabs) => tabs.clone(),
            Leave::CloseWindow | Leave::Depart { .. } => {
                self.notes.sessions.keys().copied().collect()
            }
        }
    }

    /// Close once every note the leave waits for is on disk, or ask once a
    /// save failed and no save runs. A departure waits on until every
    /// window is ready, and an edit made meanwhile saves first.
    fn advance_leaving(&mut self) {
        let Some(leaving) = &self.leaving else {
            return;
        };
        if leaving.prompt.is_some() {
            return;
        }
        let action = leaving.action.clone();
        let mut failed: Option<(TabId, String, SaveProblem)> = None;
        let mut waiting = Vec::new();
        for tab in self.leaving_tabs(&action) {
            let Some(session) = self.notes.sessions.get(&tab) else {
                continue;
            };
            if !session.is_unsaved() {
                continue;
            }
            match session.save_problem() {
                _ if session.is_saving() => waiting.push(tab),
                Some(problem) => {
                    failed.get_or_insert((tab, session.name(), problem.clone()));
                }
                None => waiting.push(tab),
            }
        }
        if let Leave::Depart { round, .. } = action
            && (failed.is_some() || !waiting.is_empty())
        {
            self.session.set_ready(self.key, round, false);
        }
        if !waiting.is_empty() {
            for tab in waiting {
                if let Some(session) = self.notes.sessions.get_mut(&tab)
                    && !session.is_saving()
                {
                    session.save_soon();
                }
                self.start_note_write(tab);
            }
            return;
        }
        if let Some((tab, name, problem)) = failed {
            if let Some(leaving) = &mut self.leaving {
                leaving.prompt = Some((
                    tab,
                    UnsavedPrompt {
                        title: format!("Couldn\u{2019}t save \u{201c}{name}\u{201d}"),
                        message: format!("{} Your changes are still in the note.", problem.reason),
                    },
                ));
            }
            self.listeners.emit(Change::Note);
            return;
        }
        match action {
            Leave::Depart { round, then } => {
                let all = self.session.set_ready(self.key, round, true);
                if all && let Some(then) = then {
                    self.go_ahead(round, then);
                }
            }
            action => {
                self.leaving = None;
                self.carry_out(action);
            }
        }
    }

    /// An edit while something waits for the notes to save: it saves
    /// before anything closes.
    fn edited_while_leaving(&mut self) {
        if self.leaving.is_some() {
            self.advance_leaving();
        }
    }

    fn carry_out(&mut self, action: Leave) {
        match action {
            Leave::CloseTabs(tabs) => {
                for tab in tabs {
                    self.close_tab_now(tab);
                }
            }
            Leave::CloseWindow => self.close_window_now(),
            Leave::Depart { .. } => {}
        }
    }

    /// Every window's notes are on disk: quit, open the workspace, or
    /// install the update and restart.
    fn go_ahead(&mut self, round: u64, then: Departure) {
        self.leaving = None;
        self.session.end_departure(self.key, round);
        self.listeners.emit(Change::Note);
        match then {
            Departure::Quit => {
                self.quit = true;
                self.listeners.emit(Change::Window);
            }
            Departure::Open(job) => self.start(job),
            Departure::Restart => self.updates.restart(self.pending_saves.clone()),
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
        let merged = self
            .notes
            .sessions
            .get_mut(&finished.tab)
            .is_some_and(|session| session.finish_write(finished.written, Instant::now()));
        if merged {
            self.merged_notice(finished.tab);
        }
    }

    /// Say that a save merged another app's edit with your typing.
    fn merged_notice(&mut self, tab: TabId) {
        let name = self
            .notes
            .sessions
            .get(&tab)
            .map(NoteSession::name)
            .unwrap_or_default();
        self.show_done(
            "Merged changes from another app",
            format!(
                "Noura kept your typing in \u{201c}{name}\u{201d} and the other app\u{2019}s edits."
            ),
        );
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
        let mut look_again = Vec::new();
        for result in finished {
            let tab = result.tab;
            self.finish_note_write(result);
            if self
                .notes
                .sessions
                .get_mut(&tab)
                .is_some_and(NoteSession::take_look_again)
            {
                look_again.push(tab);
            }
        }
        self.check_note_files(&look_again);
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
        let Some(engine) = self
            .workspace
            .as_deref()
            .map(crate::Workspace::shared_engine)
        else {
            return Ok(());
        };
        for tab in tabs {
            // A running save finishes first.
            if !self.wait_for_note_save(tab) {
                // It may still land later, so nothing moves meanwhile.
                let name = self
                    .notes
                    .sessions
                    .get(&tab)
                    .map(NoteSession::name)
                    .unwrap_or_default();
                let problem = SaveProblem {
                    reason: "It's still saving. Try again in a moment.".to_owned(),
                };
                return Err((name, problem));
            }
            // The text, then any property edits.
            let mut merged = false;
            for _ in 0..2 {
                let Some(session) = self.notes.sessions.get_mut(&tab) else {
                    break;
                };
                let Some(write) = session.start_write() else {
                    break;
                };
                let written = write.run(&engine);
                merged |= session.finish_write(written, Instant::now());
            }
            if merged {
                self.merged_notice(tab);
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
        self.open_notes();
        self.session
            .set_has_notes(self.key, !self.notes.sessions.is_empty());
    }

    fn open_notes(&mut self) {
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

/// What waits once `action` joins the leave `pending`, or `None` when
/// `pending` already covers it: closing the window or a departure closes
/// every tab, and a departure takes the place of anything else.
fn widen(pending: &Leave, action: Leave) -> Option<Leave> {
    match (pending, action) {
        (Leave::Depart { .. }, _)
        | (Leave::CloseWindow, Leave::CloseTabs(_) | Leave::CloseWindow) => None,
        (Leave::CloseTabs(tabs), Leave::CloseTabs(more)) => {
            let mut tabs = tabs.clone();
            let before = tabs.len();
            for tab in more {
                if !tabs.contains(&tab) {
                    tabs.push(tab);
                }
            }
            (tabs.len() > before).then_some(Leave::CloseTabs(tabs))
        }
        (_, action) => Some(action),
    }
}
