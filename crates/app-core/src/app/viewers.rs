//! The file viewers' view state: what each PDF and image tab shows, and
//! what you do in a PDF tab. The work runs on the viewers' own threads;
//! [`App::poll`] takes in what they found.

use std::sync::Arc;

use super::{App, Change};
use crate::{
    AppError, ImageView, OpenAs, PdfView, Platform, TabId, files::shown_name, pdf::PdfSession,
    viewers::could_run_code,
};

impl App {
    /// What the PDF tab `tab` shows, when it shows a PDF.
    pub fn pdf(&self, tab: TabId) -> Option<&PdfView> {
        self.viewers.pdfs.get(&tab).map(PdfSession::view)
    }

    /// What the image tab `tab` shows, when it shows an image.
    pub fn image(&self, tab: TabId) -> Option<&ImageView> {
        self.viewers.images.get(&tab).map(|session| session.view())
    }

    /// Change the PDF session of `tab` with `change`, and announce it when
    /// `change` says it changed what the tab shows.
    fn change_pdf(&mut self, tab: TabId, change: impl FnOnce(&mut PdfSession) -> bool) {
        if let Some(session) = self.viewers.pdfs.get_mut(&tab)
            && change(session)
        {
            self.listeners.emit(Change::Viewer);
        }
    }

    /// Show the page `page` of the PDF in `tab`, from 1, or the nearest
    /// page it has, as the page number field does.
    pub fn pdf_go_to(&mut self, tab: TabId, page: usize) {
        self.change_pdf(tab, |session| session.go_to(page));
    }

    pub fn pdf_next_page(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| session.step_page(true));
    }

    pub fn pdf_previous_page(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| session.step_page(false));
    }

    /// Draw the PDF in `tab` one zoom step larger.
    pub fn pdf_zoom_in(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| session.zoom(true));
    }

    /// Draw the PDF in `tab` one zoom step smaller.
    pub fn pdf_zoom_out(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| session.zoom(false));
    }

    /// Draw the PDF in `tab` as wide as the tab.
    pub fn pdf_fit_width(&mut self, tab: TabId) {
        self.change_pdf(tab, PdfSession::fit_width);
    }

    /// The PDF tab `tab` draws its page at most `width` points wide, on a
    /// screen with `pixels_per_point`. Its client says so each frame, and
    /// the page draws again on the PDF's thread when it needs to.
    pub fn size_pdf(&mut self, tab: TabId, width: f32, pixels_per_point: f32) {
        if let Some(session) = self.viewers.pdfs.get_mut(&tab) {
            session.size(width, pixels_per_point);
        }
    }

    /// Show or hide the thumbnails of the PDF in `tab`.
    pub fn pdf_toggle_thumbnails(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| {
            session.toggle_thumbnails();
            true
        });
    }

    /// Draw the thumbnail of the page `page` of the PDF in `tab`, for a
    /// screen with `pixels_per_point`, as the thumbnails panel shows it.
    pub fn want_pdf_thumbnail(&mut self, tab: TabId, page: usize, pixels_per_point: f32) {
        if let Some(session) = self.viewers.pdfs.get_mut(&tab) {
            session.want_thumbnail(page, pixels_per_point);
        }
    }

    /// Try `password` on the locked PDF in `tab`. Noura writes it nowhere
    /// and wipes its copy once the PDF opens with it or refuses it.
    pub fn pdf_unlock(&mut self, tab: TabId, password: String) {
        self.change_pdf(tab, |session| {
            session.unlock(password);
            true
        });
    }

    /// Search the text of the PDF in `tab` for `query`. The first match
    /// shows once the search ends. An empty query ends the search.
    pub fn search_pdf(&mut self, tab: TabId, query: &str) {
        self.change_pdf(tab, |session| {
            session.search(query);
            true
        });
    }

    pub fn pdf_next_match(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| session.step_match(true));
    }

    pub fn pdf_previous_match(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| session.step_match(false));
    }

    /// Read the PDF in `tab` again after it couldn't open, as its Try
    /// again does.
    pub fn retry_pdf(&mut self, tab: TabId) {
        self.change_pdf(tab, |session| {
            session.retry();
            true
        });
    }

    /// Hold every PDF thread before it draws a page, until the hold drops,
    /// so tests can watch the window keep drawing meanwhile.
    #[cfg(feature = "test-hooks")]
    pub fn hold_pdf_rendering_for_testing(&self) -> crate::pdf::PdfRenderHold {
        self.viewers.gate.set(true);
        crate::pdf::PdfRenderHold(self.viewers.gate.clone())
    }

    /// Make every drawing of a PDF page fail, until the guard drops, so
    /// tests can see what a page that can't be drawn shows.
    #[cfg(feature = "test-hooks")]
    pub fn fail_pdf_drawing_for_testing(&self) -> crate::pdf::PdfDrawFailure {
        self.viewers.gate.fail(true);
        crate::pdf::PdfDrawFailure(self.viewers.gate.clone())
    }

    /// Open the PDF at `relative_path` as `how` says, at the page `page`
    /// names: page 1 for none, the last page for one past the end.
    pub fn open_pdf_page(&mut self, relative_path: &str, page: Option<u32>, how: OpenAs) {
        self.open_file(relative_path, how);
        let Some(tab) = self
            .tabs
            .active()
            .filter(|tab| {
                tab.file()
                    .is_some_and(|file| file.relative_path == relative_path)
            })
            .map(crate::Tab::id)
        else {
            return;
        };
        if let Some(session) = self.viewers.pdfs.get_mut(&tab) {
            session.go_to_link_page(page);
            self.listeners.emit(Change::Viewer);
        }
    }

    /// Hand the file at `relative_path`, which opens in no tab, to the
    /// system's default app.
    pub(super) fn open_with_system(&mut self, relative_path: &str) {
        self.dismiss_notice();
        let Some(workspace) = &self.workspace else {
            return;
        };
        if let Err(error) = workspace.open_with_system(relative_path) {
            let name = relative_path.rsplit('/').next().unwrap_or(relative_path);
            let title = format!(
                "Couldn't open \u{201c}{}\u{201d}",
                shown_name(name, crate::EntryKind::File)
            );
            self.show_notice(&title, error, None);
        }
    }

    /// Bring the viewers in line with the tabs: open one for each PDF or
    /// image tab, follow tabs whose file moved, and drop the viewers of
    /// closed tabs.
    pub(super) fn sync_viewers(&mut self) {
        let Some(workspace) = &self.workspace else {
            self.viewers.clear();
            return;
        };
        let files: Vec<(TabId, String)> = self
            .tabs
            .list()
            .iter()
            .filter_map(|tab| Some((tab.id(), tab.file()?.relative_path.clone())))
            .collect();
        let listeners = self.listeners.clone();
        let wake: Arc<dyn Fn() + Send + Sync> =
            Arc::new(move || listeners.emit(Change::WorkFinished));
        self.viewers.sync(&files, &workspace.shared_engine(), &wake);
    }

    /// Take in what the viewers' threads found. Returns whether any view
    /// changed.
    pub(super) fn poll_viewers(&mut self) -> bool {
        let changed = self.viewers.poll();
        if changed {
            self.listeners.emit(Change::Viewer);
        }
        changed
    }

    /// The files changed: each viewer reads its file again if another app
    /// replaced it.
    pub(super) fn check_viewers_on_disk(&mut self) {
        self.viewers.check();
    }
}

impl crate::Workspace {
    /// Hand the file at `relative_path` to the app the system opens its
    /// type with. The file must be in the workspace, off links, and must
    /// not be an app, script, installer or shortcut, since its default
    /// app would run code someone else wrote.
    pub(crate) fn open_with_system(&self, relative_path: &str) -> Result<(), AppError> {
        let path = self
            .engine()
            .resolve_managed_path(relative_path, "file_open")
            .map_err(|error| AppError::from_change(error, relative_path))?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                if could_run_code(&path, &metadata) {
                    let file_manager = match self.system().platform() {
                        Platform::MacOs => "Finder",
                        Platform::Windows => "Explorer",
                        Platform::Linux => "your file manager",
                    };
                    return Err(AppError::new(
                        "open_not_allowed",
                        format!(
                            "Noura doesn't open apps or scripts. Show the file in \
                             {file_manager} and open it there."
                        ),
                    ));
                }
            }
            _ => {
                return Err(AppError::new(
                    "file_missing",
                    format!("{relative_path} isn't in the workspace anymore."),
                ));
            }
        }
        self.system().open_file(&path).map_err(|error| {
            tracing::warn!(%error, "the default app didn't open");
            AppError::new(
                "open_failed",
                "Noura couldn't find an app on this computer that opens this file.",
            )
        })
    }
}
