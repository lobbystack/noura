//! The PDF tab: a toolbar that pages through the PDF, zooms it and
//! searches it, the thumbnails panel, the page itself, and the password
//! form of a locked PDF. `app-core` reads, draws and searches the PDF on
//! the PDF's own thread; this view uploads what it draws and tells it how
//! wide the tab shows the page.

use std::collections::HashMap;

use app_core::{App, PdfPicture, PdfState, PdfView, THUMBNAIL_WIDTH, TabId};
use egui::{
    Align, Button, ColorImage, Frame, Id, Key, Layout, Margin, Panel, RichText, ScrollArea, Sense,
    Spinner, Stroke, TextEdit, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2, accesskit::Role,
    load::SizedTexture, text_edit::TextEditState, vec2,
};

use crate::{
    common::{muted, primary_button},
    theme::{Palette, radius},
};

/// How wide the thumbnails panel is, in points.
const THUMBNAILS_WIDTH: f32 = THUMBNAIL_WIDTH + 32.0;

/// The space around the page, in points.
const PAGE_MARGIN: f32 = 16.0;

/// What each PDF tab keeps on this side: its fields and the pictures it
/// uploaded.
#[derive(Default)]
pub(crate) struct PdfViews {
    tabs: HashMap<TabId, PdfTab>,
}

#[derive(Default)]
struct PdfTab {
    /// What the page number field holds while you type in it.
    page_field: Option<String>,
    /// What the search field holds.
    search: String,
    /// What the password field holds, until you unlock.
    password: String,
    /// The page's picture as uploaded, by its generation.
    page: Option<(u64, TextureHandle)>,
    thumbnails: HashMap<usize, Option<(u64, TextureHandle)>>,
}

impl PdfViews {
    /// Forget the tabs that closed, with their pictures.
    pub(crate) fn retain(&mut self, app: &App) {
        self.tabs
            .retain(|tab, _| app.tabs().iter().any(|open| open.id() == *tab));
    }
}

/// What you did in the tab this frame, for after it draws.
enum Action {
    ToggleThumbnails,
    PreviousPage,
    NextPage,
    GoTo(usize),
    ZoomIn,
    ZoomOut,
    FitWidth,
    Search(String),
    NextMatch,
    PreviousMatch,
    Unlock,
    Retry,
    /// The page shows this many points wide, at these pixels per point.
    Size(f32, f32),
    Thumbnail(usize, f32),
}

/// Draw the PDF tab `tab`. Returns `false` when the tab shows no PDF.
pub(crate) fn show(
    ui: &mut Ui,
    app: &mut App,
    views: &mut PdfViews,
    window: Id,
    tab: TabId,
) -> bool {
    let Some(pdf) = app.pdf(tab) else {
        return false;
    };
    let view = views.tabs.entry(tab).or_default();
    let mut actions = Vec::new();
    match pdf.state() {
        PdfState::Opening | PdfState::Unlocking => {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.add(Spinner::new().size(20.0));
                muted(ui, "Opening the PDF\u{2026}");
            });
        }
        PdfState::Locked { wrong } => password_form(ui, view, window, tab, *wrong, &mut actions),
        PdfState::Failed { message } => {
            let message = message.clone();
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                muted(ui, &message);
                ui.add_space(8.0);
                if ui.button("Try again").clicked() {
                    actions.push(Action::Retry);
                }
            });
        }
        PdfState::Ready => {
            toolbar(ui, pdf, view, window, tab, &mut actions);
            ui.add_space(8.0);
            if pdf.thumbnails_shown() {
                thumbnails(ui, pdf, view, tab, &mut actions);
            }
            page(ui, pdf, view, tab, &mut actions);
        }
    }
    for action in actions {
        apply(ui, app, view, window, tab, action);
    }
    true
}

fn apply(ui: &Ui, app: &mut App, view: &mut PdfTab, window: Id, tab: TabId, action: Action) {
    match action {
        Action::ToggleThumbnails => app.pdf_toggle_thumbnails(tab),
        Action::PreviousPage => app.pdf_previous_page(tab),
        Action::NextPage => app.pdf_next_page(tab),
        Action::GoTo(page) => app.pdf_go_to(tab, page),
        Action::ZoomIn => app.pdf_zoom_in(tab),
        Action::ZoomOut => app.pdf_zoom_out(tab),
        Action::FitWidth => app.pdf_fit_width(tab),
        Action::Search(query) => app.search_pdf(tab, &query),
        Action::NextMatch => app.pdf_next_match(tab),
        Action::PreviousMatch => app.pdf_previous_match(tab),
        Action::Unlock => {
            let password = std::mem::take(&mut view.password);
            // The field's undo history holds what you typed too.
            ui.ctx()
                .data_mut(|data| data.remove::<TextEditState>(password_id(window, tab)));
            app.pdf_unlock(tab, password);
        }
        Action::Retry => app.retry_pdf(tab),
        Action::Size(width, pixels_per_point) => app.size_pdf(tab, width, pixels_per_point),
        Action::Thumbnail(page, pixels_per_point) => {
            app.want_pdf_thumbnail(tab, page, pixels_per_point);
        }
    }
}

fn password_id(window: Id, tab: TabId) -> Id {
    window.with(("pdf-password", tab))
}

/// Ask for the password of a locked PDF. Enter or Unlock tries it.
fn password_form(
    ui: &mut Ui,
    view: &mut PdfTab,
    window: Id,
    tab: TabId,
    wrong: bool,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 4.0);
        ui.label(RichText::new("This PDF is locked").strong());
        muted(ui, "Enter its password to read it.");
        ui.add_space(8.0);
        if wrong {
            let builder = UiBuilder::new().accessibility_role(Role::Alert);
            ui.scope_builder(builder, |ui| {
                ui.label(
                    RichText::new("That password didn\u{2019}t open the PDF. Try again.")
                        .color(palette.destructive),
                );
            });
            ui.add_space(4.0);
        }
        let field = ui
            .add(
                TextEdit::singleline(&mut view.password)
                    .id(password_id(window, tab))
                    .password(true)
                    .hint_text("Password")
                    .desired_width(220.0),
            )
            .accessible_name("PDF password");
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        ui.add_space(8.0);
        let unlock = primary_button(ui, "Unlock", !view.password.is_empty()).clicked();
        if (entered || unlock) && !view.password.is_empty() {
            actions.push(Action::Unlock);
        }
    });
}

/// A button that shows `icon` and reads as `label`.
fn icon_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, Button::new(icon).min_size(vec2(28.0, 0.0)))
        .accessible_name(label)
        .on_hover_text(label)
}

/// Page through, zoom and search the PDF.
fn toolbar(
    ui: &mut Ui,
    pdf: &PdfView,
    view: &mut PdfTab,
    window: Id,
    tab: TabId,
    actions: &mut Vec<Action>,
) {
    let page = pdf.page();
    let count = pdf.page_count();
    let builder = UiBuilder::new()
        .accessibility_role(Role::Toolbar)
        .accessibility_label("PDF navigation");
    ui.scope_builder(builder, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(Button::selectable(pdf.thumbnails_shown(), "Thumbnails"))
                .clicked()
            {
                actions.push(Action::ToggleThumbnails);
            }
            ui.separator();
            if icon_button(ui, "\u{2039}", "Previous page", page > 1).clicked() {
                actions.push(Action::PreviousPage);
            }
            page_field(ui, view, window.with(("pdf-page", tab)), page, actions);
            ui.label(format!("of {count}"));
            if icon_button(ui, "\u{203a}", "Next page", page < count).clicked() {
                actions.push(Action::NextPage);
            }
            ui.separator();
            if icon_button(ui, "\u{2212}", "Zoom out", true).clicked() {
                actions.push(Action::ZoomOut);
            }
            ui.label(format!("{:.0}%", pdf.scale() * 100.0));
            if icon_button(ui, "+", "Zoom in", true).clicked() {
                actions.push(Action::ZoomIn);
            }
            if ui.button("Fit width").clicked() {
                actions.push(Action::FitWidth);
            }
            ui.separator();
            search(ui, pdf, view, actions);
        });
    });
}

/// The page number field: Enter or leaving it shows the page typed, and
/// anything but a number puts the page back.
fn page_field(ui: &mut Ui, view: &mut PdfTab, id: Id, page: usize, actions: &mut Vec<Action>) {
    let mut text = view.page_field.clone().unwrap_or_else(|| page.to_string());
    let response = ui
        .add(
            TextEdit::singleline(&mut text)
                .id(id)
                .desired_width(36.0)
                .align(egui::Align2::CENTER_CENTER),
        )
        .accessible_name("Page number");
    if response.changed() {
        view.page_field = Some(text.clone());
    }
    if response.lost_focus() {
        if let Ok(number) = text.trim().parse::<usize>() {
            actions.push(Action::GoTo(number));
        }
        view.page_field = None;
    }
}

/// The search field, the buttons that move between matches and how many
/// there are. Enter goes to the next match, Shift-Enter to the one before.
fn search(ui: &mut Ui, pdf: &PdfView, view: &mut PdfTab, actions: &mut Vec<Action>) {
    let search = pdf.search();
    let response = ui
        .add(
            TextEdit::singleline(&mut view.search)
                .hint_text("Find in PDF")
                .desired_width(140.0),
        )
        .accessible_name("Search PDF");
    let query = view.search.trim();
    let entered = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    if response.changed() || (entered && query != search.query) {
        actions.push(Action::Search(query.to_owned()));
    } else if entered {
        if ui.input(|input| input.modifiers.shift) {
            actions.push(Action::PreviousMatch);
        } else {
            actions.push(Action::NextMatch);
        }
    }
    if entered {
        // The field keeps the keyboard, so Enter moves on again.
        response.request_focus();
    }
    let any = !search.matches.is_empty();
    if icon_button(ui, "\u{2191}", "Previous match", any).clicked() {
        actions.push(Action::PreviousMatch);
    }
    if icon_button(ui, "\u{2193}", "Next match", any).clicked() {
        actions.push(Action::NextMatch);
    }
    let status = if search.searching {
        "Searching\u{2026}".to_owned()
    } else if search.query.is_empty() {
        String::new()
    } else if let Some(current) = search.current {
        format!("{} of {}", current + 1, search.matches.len())
    } else {
        "No matches".to_owned()
    };
    if !status.is_empty() {
        let builder = UiBuilder::new().accessibility_role(Role::Status);
        ui.scope_builder(builder, |ui| muted(ui, &status));
    }
}

/// Upload `picture` unless `uploaded` holds it already, and return the
/// texture that shows it.
fn upload(
    ui: &Ui,
    uploaded: &mut Option<(u64, TextureHandle)>,
    picture: &PdfPicture,
    name: &str,
) -> TextureHandle {
    if let Some((generation, texture)) = uploaded
        && *generation == picture.generation
    {
        return texture.clone();
    }
    let size = [picture.width as usize, picture.height as usize];
    let image = ColorImage::from_rgba_premultiplied(size, &picture.rgba);
    let texture = ui.ctx().load_texture(name, image, TextureOptions::LINEAR);
    *uploaded = Some((picture.generation, texture.clone()));
    texture
}

/// One thumbnail per page, in a panel beside the page. Picking one shows
/// its page.
fn thumbnails(
    ui: &mut Ui,
    pdf: &PdfView,
    view: &mut PdfTab,
    tab: TabId,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let pixels_per_point = ui.ctx().pixels_per_point();
    let height = |page: usize| {
        pdf.page_size(page)
            .map_or(THUMBNAIL_WIDTH, |[width, height]| {
                THUMBNAIL_WIDTH * height / width.max(1.0)
            })
    };
    // Rows share one height so the panel scrolls without measuring every
    // page: the first page's, which most PDFs keep.
    let row = height(1).min(THUMBNAIL_WIDTH * 2.0) + 24.0;
    Panel::left(("pdf-thumbnails", tab))
        .resizable(false)
        .exact_size(THUMBNAILS_WIDTH)
        .frame(Frame::new().inner_margin(Margin::symmetric(4, 0)))
        .show(ui, |ui| {
            let builder = UiBuilder::new()
                .accessibility_role(Role::List)
                .accessibility_label("Page thumbnails");
            ui.scope_builder(builder, |ui| {
                ScrollArea::vertical()
                    .id_salt(("pdf-thumbnails", tab))
                    .auto_shrink(false)
                    .show_rows(ui, row, pdf.page_count(), |ui, rows| {
                        for page in rows.start + 1..=rows.end {
                            actions.push(Action::Thumbnail(page, pixels_per_point));
                            let size = vec2(THUMBNAIL_WIDTH, height(page).min(row - 24.0));
                            let current = page == pdf.page();
                            let (rect, response) = ui.allocate_exact_size(
                                vec2(ui.available_width(), row),
                                Sense::click(),
                            );
                            let response = response.accessible_name(format!("Go to page {page}"));
                            ui.ctx().accesskit_node_builder(response.id, |node| {
                                node.set_role(Role::Button);
                                node.set_selected(current);
                            });
                            let frame = egui::Rect::from_min_size(
                                rect.center_top() + vec2(-size.x / 2.0, 4.0),
                                size,
                            );
                            let painter = ui.painter();
                            painter.rect_filled(frame, radius(0.25), egui::Color32::WHITE);
                            if let Some(picture) = pdf.thumbnail(page) {
                                let slot = view.thumbnails.entry(page).or_default();
                                let texture = upload(ui, slot, picture, "pdf-thumbnail");
                                egui::Image::from_texture(SizedTexture::new(texture.id(), size))
                                    .paint_at(ui, frame);
                            }
                            let stroke = if current || response.hovered() {
                                Stroke::new(2.0, palette.ring)
                            } else {
                                Stroke::new(1.0, palette.border)
                            };
                            painter.rect_stroke(
                                frame,
                                radius(0.25),
                                stroke,
                                egui::StrokeKind::Outside,
                            );
                            painter.text(
                                egui::pos2(rect.center().x, frame.bottom() + 4.0),
                                egui::Align2::CENTER_TOP,
                                page.to_string(),
                                egui::FontId::proportional(12.0),
                                palette.muted_foreground,
                            );
                            if response.clicked() {
                                actions.push(Action::GoTo(page));
                            }
                        }
                    });
            });
        });
}

/// The page that shows, at the zoom's size, in a scroll area. Until its
/// picture arrives, a blank page says it's rendering.
fn page(ui: &mut Ui, pdf: &PdfView, view: &mut PdfTab, tab: TabId, actions: &mut Vec<Action>) {
    let number = pdf.page();
    let count = pdf.page_count();
    let [width, height] = pdf.page_size(number).unwrap_or([612.0, 792.0]);
    let available = (ui.available_width() - 2.0 * PAGE_MARGIN).max(1.0);
    actions.push(Action::Size(available, ui.ctx().pixels_per_point()));
    let scale = pdf.scale();
    let size = vec2(width * scale, height * scale);
    let picture = pdf.picture().filter(|picture| picture.page == number);
    ScrollArea::both()
        .id_salt(("pdf-page", tab))
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.add_space(PAGE_MARGIN);
                let label = format!("Page {number} of {count}");
                match picture {
                    Some(picture) => {
                        let texture = upload(ui, &mut view.page, picture, "pdf-page");
                        ui.add(
                            egui::Image::from_texture(SizedTexture::new(texture.id(), size))
                                .alt_text(label),
                        );
                    }
                    None => blank_page(ui, size, number),
                }
                ui.add_space(PAGE_MARGIN);
            });
        });
}

/// A white page of `size` that says page `number` is rendering.
fn blank_page(ui: &mut Ui, size: Vec2, number: usize) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let palette = Palette::of(ui.ctx());
    ui.painter()
        .rect_filled(rect, radius(0.25), egui::Color32::WHITE);
    ui.painter().rect_stroke(
        rect,
        radius(0.25),
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Outside,
    );
    let builder = UiBuilder::new()
        .max_rect(rect)
        .layout(Layout::top_down(Align::Center))
        .accessibility_role(Role::Status);
    ui.scope_builder(builder, |ui| {
        ui.add_space((rect.height() / 3.0).min(120.0));
        ui.add(Spinner::new().size(16.0));
        ui.label(
            RichText::new(format!("Rendering page {number}\u{2026}"))
                .color(egui::Color32::from_gray(90)),
        );
    });
}
