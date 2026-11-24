//! The PDF tab: a toolbar that zooms and searches the PDF, the thumbnails
//! panel, the pages one under the other in a scroll area, and the password
//! form of a locked PDF. `app-core` reads, lays out, draws and searches the
//! PDF on the PDF's own thread; this view uploads the tiles it draws, puts
//! each where its pixels meet the screen's, and tells it how wide the tab
//! is and which part of the pages shows.

use std::collections::HashMap;

use app_core::{
    App, PAGE_GAP, PdfArea, PdfPicture, PdfState, PdfTileKey, PdfView, THUMBNAIL_MAX_HEIGHT,
    THUMBNAIL_WIDTH, TabId,
};
use egui::{
    Align, Button, Color32, ColorImage, EventFilter, Frame, Id, Key, Layout, Margin, Modifiers,
    Panel, Pos2, Rect, RichText, ScrollArea, Sense, Spinner, Stroke, StrokeKind, TextEdit,
    TextureHandle, TextureOptions, Ui, UiBuilder, Vec2, WidgetInfo, accesskit::Role,
    emath::GuiRounding, load::SizedTexture, pos2, text_edit::TextEditState, vec2,
};
use zeroize::Zeroize;

use crate::{
    common::{muted, primary_button},
    theme::{Palette, radius},
};

/// How wide the thumbnails panel is, in points.
const THUMBNAILS_WIDTH: f32 = THUMBNAIL_WIDTH + 32.0;

/// How far an arrow key scrolls the pages, in points.
const LINE_SCROLL: f32 = 40.0;

/// How much of the view Page Up, Page Down and Space scroll.
const PAGE_SCROLL: f32 = 0.9;

/// What each PDF tab keeps on this side: its fields and the pictures it
/// uploaded.
#[derive(Default)]
pub(crate) struct PdfViews {
    tabs: HashMap<TabId, PdfTab>,
}

/// The most characters the password field takes: a PDF password counts
/// at most 127 bytes, and the PDF ignores any past them.
const PASSWORD_CHARS: usize = 127;

/// The room the password field's text has before it moves, which would
/// leave a copy behind: four bytes for each character it takes.
const PASSWORD_ROOM: usize = PASSWORD_CHARS * 4;

#[derive(Default)]
struct PdfTab {
    /// What the search field holds.
    search: String,
    /// What the password field holds, until you unlock. It's wiped when
    /// the tab closes.
    password: String,
    /// The password field took the keyboard since the form showed.
    password_focused: bool,
    /// The tiles as uploaded, each with its picture's generation.
    tiles: HashMap<PdfTileKey, Option<(u64, TextureHandle)>>,
    thumbnails: HashMap<usize, Option<(u64, TextureHandle)>>,
    /// The newest jump the pages made.
    jumped: u64,
    /// Where the pages were scrolled to, and the size of the part that
    /// showed, last frame.
    offset: Vec2,
    shown: Vec2,
}

impl Drop for PdfTab {
    fn drop(&mut self) {
        self.password.zeroize();
    }
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
    GoTo(usize),
    ZoomIn,
    ZoomOut,
    FitWidth,
    Search(String),
    NextMatch,
    PreviousMatch,
    Unlock,
    Retry,
    /// The part of the pages that showed.
    Shown(PdfArea),
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
    if !matches!(pdf.state(), PdfState::Locked { .. }) {
        view.password_focused = false;
    }
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
            toolbar(ui, pdf, view, &mut actions);
            ui.add_space(8.0);
            if pdf.thumbnails_shown() {
                thumbnails(ui, pdf, view, tab, &mut actions);
            }
            // The pages lay out for the width that's left, before they
            // draw.
            let width = (ui.available_width() - 2.0 * PAGE_GAP).max(1.0);
            app.size_pdf(tab, width, ui.ctx().pixels_per_point());
            if let Some(pdf) = app.pdf(tab) {
                pages(ui, pdf, view, tab, &mut actions);
            }
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
        Action::GoTo(page) => app.pdf_go_to(tab, page),
        Action::ZoomIn => app.pdf_zoom_in(tab),
        Action::ZoomOut => app.pdf_zoom_out(tab),
        Action::FitWidth => app.pdf_fit_width(tab),
        Action::Search(query) => app.search_pdf(tab, &query),
        Action::NextMatch => app.pdf_next_match(tab),
        Action::PreviousMatch => app.pdf_previous_match(tab),
        Action::Unlock => {
            let password = std::mem::take(&mut view.password);
            // The form takes the keyboard again if the password is wrong,
            // however soon it comes back.
            view.password_focused = false;
            // The field's undo history holds what you typed too.
            ui.ctx()
                .data_mut(|data| data.remove::<TextEditState>(password_id(window, tab)));
            app.pdf_unlock(tab, password);
        }
        Action::Retry => app.retry_pdf(tab),
        Action::Shown(area) => app.show_pdf_area(tab, area),
        Action::Thumbnail(page, pixels_per_point) => {
            app.want_pdf_thumbnail(tab, page, pixels_per_point);
        }
    }
}

fn password_id(window: Id, tab: TabId) -> Id {
    window.with(("pdf-password", tab))
}

/// Ask for the password of a locked PDF. Enter or Unlock tries it. The
/// field takes the keyboard when the form shows, and again after a wrong
/// password.
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
        ui.add_space(8.0);
        if wrong {
            let builder = UiBuilder::new().accessibility_role(Role::Alert);
            ui.scope_builder(builder, |ui| {
                ui.label(RichText::new("Wrong password").color(palette.destructive));
            });
            ui.add_space(4.0);
        }
        make_room(&mut view.password);
        let field = ui
            .add(
                TextEdit::singleline(&mut view.password)
                    .id(password_id(window, tab))
                    .password(true)
                    .char_limit(PASSWORD_CHARS)
                    .hint_text("Password")
                    .desired_width(220.0),
            )
            .accessible_name("PDF password");
        if !view.password_focused {
            field.request_focus();
            view.password_focused = true;
        }
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        ui.add_space(8.0);
        let unlock = primary_button(ui, "Unlock", !view.password.is_empty()).clicked();
        if (entered || unlock) && !view.password.is_empty() {
            actions.push(Action::Unlock);
        }
    });
}

/// Give the password field's text [`PASSWORD_ROOM`] bytes, so typing
/// never moves it. An undo in the field hands it text with no room to
/// spare, so the text moves to a new string and Noura wipes the old one.
fn make_room(password: &mut String) {
    if password.capacity() >= PASSWORD_ROOM {
        return;
    }
    let mut roomy = String::with_capacity(PASSWORD_ROOM);
    roomy.push_str(password);
    std::mem::replace(password, roomy).zeroize();
}

/// A button that shows `icon` and reads as `label`.
fn icon_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, Button::new(icon).min_size(vec2(28.0, 0.0)))
        .accessible_name(label)
        .on_hover_text(label)
}

/// A button with a chevron pointing up or down, which reads as `label`.
/// egui's own fonts have no arrows, so it paints one.
fn chevron_button(ui: &mut Ui, up: bool, label: &str, enabled: bool) -> egui::Response {
    let response = icon_button(ui, " ", label, enabled);
    let color = if enabled {
        ui.style().interact(&response).fg_stroke.color
    } else {
        ui.visuals().widgets.noninteractive.fg_stroke.color
    };
    let center = response.rect.center();
    let rise = if up { -2.5 } else { 2.5 };
    ui.painter().line(
        vec![
            center + vec2(-4.0, -rise),
            center + vec2(0.0, rise),
            center + vec2(4.0, -rise),
        ],
        Stroke::new(1.5, color),
    );
    response
}

/// Zoom and search the PDF.
fn toolbar(ui: &mut Ui, pdf: &PdfView, view: &mut PdfTab, actions: &mut Vec<Action>) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Toolbar)
        .accessibility_label("PDF navigation");
    // Each group wraps to the next row whole when the tab is narrow.
    ui.scope_builder(builder, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(Button::selectable(pdf.thumbnails_shown(), "Thumbnails"))
                .clicked()
            {
                actions.push(Action::ToggleThumbnails);
            }
            ui.horizontal(|ui| {
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
            });
            ui.horizontal(|ui| search(ui, pdf, view, actions));
        });
    });
}

/// The search field, the buttons that move between matches and how many
/// there are. Enter goes to the next match, Shift-Enter to the one before.
fn search(ui: &mut Ui, pdf: &PdfView, view: &mut PdfTab, actions: &mut Vec<Action>) {
    let search = pdf.search();
    let response = ui
        .add(
            TextEdit::singleline(&mut view.search)
                .hint_text("Find in PDF")
                .desired_width(110.0),
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
    if chevron_button(ui, true, "Previous match", any).clicked() {
        actions.push(Action::PreviousMatch);
    }
    if chevron_button(ui, false, "Next match", any).clicked() {
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

/// `picture` as an image egui can upload, unless it has no pixels on a
/// side, which no texture can hold, or its bytes don't fill it.
pub(crate) fn picture_image(picture: &PdfPicture) -> Option<ColorImage> {
    let size = [picture.width as usize, picture.height as usize];
    let filled = size[0] > 0 && size[1] > 0 && picture.rgba.len() == size[0] * size[1] * 4;
    filled.then(|| ColorImage::from_rgba_premultiplied(size, &picture.rgba))
}

/// Upload `picture` unless `uploaded` holds it already, and return the
/// texture that shows it. `None` when the picture can't be uploaded.
fn upload(
    ui: &Ui,
    uploaded: &mut Option<(u64, TextureHandle)>,
    picture: &PdfPicture,
    name: &str,
) -> Option<TextureHandle> {
    if let Some((generation, texture)) = uploaded
        && *generation == picture.generation
    {
        return Some(texture.clone());
    }
    let texture = ui
        .ctx()
        .load_texture(name, picture_image(picture)?, TextureOptions::LINEAR);
    *uploaded = Some((picture.generation, texture.clone()));
    Some(texture)
}

/// `size` scaled to fit within `bounds`, keeping its shape, but at least a
/// point on each side so a sliver of a page still shows.
fn fit_within(size: [f32; 2], bounds: Vec2) -> Vec2 {
    let [width, height] = size.map(|side| side.max(f32::EPSILON));
    let scale = (bounds.x / width).min(bounds.y / height);
    vec2((width * scale).max(1.0), (height * scale).max(1.0))
}

/// One thumbnail per page, in a panel beside the pages. Picking one
/// scrolls to its page.
fn thumbnails(
    ui: &mut Ui,
    pdf: &PdfView,
    view: &mut PdfTab,
    tab: TabId,
    actions: &mut Vec<Action>,
) {
    let palette = Palette::of(ui.ctx());
    let pixels_per_point = ui.ctx().pixels_per_point();
    let size_of = |page: usize| pdf.page_size(page).unwrap_or([612.0, 792.0]);
    // Rows share one height so the panel scrolls without measuring every
    // page: the first page's, which most PDFs keep, within a half and
    // twice the thumbnail's width. Each page fits its row's box at its own
    // shape.
    let [first_width, first_height] = size_of(1);
    let box_height = (THUMBNAIL_WIDTH * first_height / first_width.max(1.0))
        .clamp(THUMBNAIL_WIDTH / 2.0, THUMBNAIL_MAX_HEIGHT);
    let bounds = vec2(THUMBNAIL_WIDTH, box_height);
    let row = box_height + 24.0;
    // The panel forgets the thumbnails `app-core` no longer keeps.
    view.thumbnails
        .retain(|page, _| pdf.thumbnail(*page).is_some());
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
                            let size = fit_within(size_of(page), bounds);
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
                            let frame = Rect::from_center_size(
                                rect.center_top() + vec2(0.0, 4.0 + box_height / 2.0),
                                size,
                            );
                            let painter = ui.painter();
                            painter.rect_filled(frame, radius(0.25), Color32::WHITE);
                            let slot = view.thumbnails.entry(page).or_default();
                            if let Some(texture) = pdf
                                .thumbnail(page)
                                .and_then(|picture| upload(ui, slot, picture, "pdf-thumbnail"))
                            {
                                egui::Image::from_texture(SizedTexture::new(texture.id(), size))
                                    .paint_at(ui, frame);
                            }
                            let stroke = if current || response.hovered() {
                                Stroke::new(2.0, palette.ring)
                            } else {
                                Stroke::new(1.0, palette.border)
                            };
                            painter.rect_stroke(frame, radius(0.25), stroke, StrokeKind::Outside);
                            painter.text(
                                pos2(rect.center().x, rect.top() + box_height + 8.0),
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

/// How the keyboard scrolls the pages.
#[derive(Debug, Clone, Copy, PartialEq)]
enum KeyScroll {
    By(f32),
    Top,
    Bottom,
}

/// How the keys pressed this frame scroll the pages, while the pages
/// `pages` or nothing has the keyboard: a line with the arrows, most of
/// the view with Page Up, Page Down and Space, to either end with Home and
/// End. Takes the keys it uses.
fn key_scroll(ui: &Ui, pages: Id, shown: f32) -> Option<KeyScroll> {
    if ui.memory(|memory| memory.focused().is_some_and(|focused| focused != pages)) {
        return None;
    }
    let page = shown * PAGE_SCROLL;
    ui.input_mut(|input| {
        let mut scroll = None;
        // Shift-Space first, which Space alone would take.
        for (modifiers, key, by) in [
            (Modifiers::SHIFT, Key::Space, KeyScroll::By(-page)),
            (Modifiers::NONE, Key::Space, KeyScroll::By(page)),
            (Modifiers::NONE, Key::PageDown, KeyScroll::By(page)),
            (Modifiers::NONE, Key::PageUp, KeyScroll::By(-page)),
            (Modifiers::NONE, Key::ArrowDown, KeyScroll::By(LINE_SCROLL)),
            (Modifiers::NONE, Key::ArrowUp, KeyScroll::By(-LINE_SCROLL)),
            (Modifiers::NONE, Key::Home, KeyScroll::Top),
            (Modifiers::NONE, Key::End, KeyScroll::Bottom),
        ] {
            if input.consume_key(modifiers, key) {
                scroll = Some(by);
            }
        }
        scroll
    })
}

/// The pages one under the other in a scroll area, as `app-core` lays
/// them out. Only the pages that show draw. Each page draws its tiles at
/// one of their pixels to each of the screen's, over tiles drawn for an
/// older zoom until the new ones arrive; a page with none yet says it's
/// rendering, or that it couldn't be drawn.
fn pages(ui: &mut Ui, pdf: &PdfView, view: &mut PdfTab, tab: TabId, actions: &mut Vec<Action>) {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let [width, height] = pdf.content_size();
    // A click on the pages gives them the keyboard, arrows included.
    let pages_id = ui.make_persistent_id(("pdf-pages-surface", tab));
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            pages_id,
            EventFilter {
                vertical_arrows: true,
                ..EventFilter::default()
            },
        );
    });
    let jump = pdf.jump().filter(|jump| jump.seq != view.jumped);
    let mut offset = view.offset;
    if let Some(jump) = jump {
        offset.y = jump.top;
        if let Some(left) = jump.left {
            offset.x = left;
        }
    } else if let Some(scroll) = key_scroll(ui, pages_id, view.shown.y) {
        offset.y = match scroll {
            KeyScroll::By(by) => offset.y + by,
            KeyScroll::Top => 0.0,
            KeyScroll::Bottom => height,
        };
    }
    let mut area = ScrollArea::both()
        .id_salt(("pdf-pages", tab))
        .auto_shrink(false)
        .content_margin(0.0);
    if offset != view.offset {
        // No further than the pages reach, which would bounce back.
        let reach = (vec2(width, height) - view.shown).max(Vec2::ZERO);
        area = area.scroll_offset(offset.clamp(Vec2::ZERO, reach));
    }
    let output = area.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(vec2(width, height));
        let surface = ui.interact(ui.clip_rect(), pages_id, Sense::click());
        surface.widget_info(|| {
            let mut info = WidgetInfo::new(Role::ScrollView);
            info.label = Some("Pages".to_owned());
            info
        });
        if surface.clicked() {
            surface.request_focus();
        }
        let origin = ui.max_rect().min;
        for page in pdf.pages_between(viewport.min.y, viewport.max.y) {
            draw_page(ui, pdf, view, origin, page, pixels_per_point);
        }
    });
    if let Some(jump) = jump {
        view.jumped = jump.seq;
    }
    view.offset = output.state.clamped_offset();
    view.shown = output.inner_rect.size();
    // Textures go with the tiles `app-core` forgot.
    view.tiles.retain(|key, _| pdf.tile(key).is_some());
    actions.push(Action::Shown(PdfArea {
        left: view.offset.x,
        top: view.offset.y,
        width: view.shown.x,
        height: view.shown.y,
        jump: view.jumped,
    }));
}

/// Draw the page `page`, whose layout starts at `origin`, on a screen with
/// `pixels_per_point`.
fn draw_page(
    ui: &mut Ui,
    pdf: &PdfView,
    view: &mut PdfTab,
    origin: Pos2,
    page: usize,
    pixels_per_point: f32,
) {
    let Some([left, top, width, height]) = pdf.page_rect(page) else {
        return;
    };
    // The page's corner sits on a pixel, so each of its tiles' pixels
    // falls on one of the screen's.
    let corner = (origin + vec2(left, top)).round_to_pixels(pixels_per_point);
    let rect = Rect::from_min_size(corner, vec2(width, height));
    let clip = ui.clip_rect();
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, Color32::WHITE);
    let density = pdf.density();
    let mut drawn = false;
    for newest in [false, true] {
        for tile in pdf
            .tiles(page)
            .filter(|tile| (tile.key.density() == density) == newest)
        {
            drawn = true;
            let points = if newest {
                1.0 / pixels_per_point
            } else {
                pdf.scale() / tile.key.density()
            };
            let place = Rect::from_min_size(
                corner + vec2(tile.x as f32, tile.y as f32) * points,
                vec2(tile.picture.width as f32, tile.picture.height as f32) * points,
            );
            if !clip.intersects(place) {
                continue;
            }
            let slot = view.tiles.entry(tile.key).or_default();
            if let Some(texture) = upload(ui, slot, &tile.picture, "pdf-tile") {
                painter.image(
                    texture.id(),
                    place,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
    }
    let palette = Palette::of(ui.ctx());
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, palette.border),
        StrokeKind::Outside,
    );
    let count = pdf.page_count();
    let response = ui.interact(rect, ui.scope_id().with(("pdf-page", page)), Sense::hover());
    if drawn {
        response.widget_info(|| {
            let mut info = WidgetInfo::new(Role::Image);
            info.label = Some(format!("Page {page} of {count}"));
            info
        });
        return;
    }
    let shows = rect.intersect(clip);
    if shows.height() < 40.0 || shows.width() < 40.0 {
        return;
    }
    let builder = UiBuilder::new()
        .max_rect(shows)
        .layout(Layout::top_down(Align::Center))
        .accessibility_role(Role::Status);
    ui.scope_builder(builder, |ui| {
        ui.add_space((shows.height() / 3.0).min(120.0));
        let text = if pdf.page_failed(page) {
            format!("Noura couldn\u{2019}t draw page {page}.")
        } else {
            ui.add(Spinner::new().size(16.0));
            format!("Rendering page {page}\u{2026}")
        };
        ui.label(RichText::new(text).color(Color32::from_gray(90)));
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use app_core::PdfPicture;
    use egui::vec2;

    use super::{PASSWORD_CHARS, PASSWORD_ROOM, fit_within, make_room, picture_image};

    fn picture(width: u32, height: u32, bytes: usize) -> PdfPicture {
        PdfPicture {
            page: 1,
            width,
            height,
            rgba: Arc::from(vec![255; bytes]),
            generation: 0,
        }
    }

    #[test]
    fn a_picture_with_no_pixels_on_a_side_is_never_uploaded() {
        assert!(picture_image(&picture(0, 1, 0)).is_none());
        assert!(picture_image(&picture(4096, 0, 0)).is_none());
        assert!(picture_image(&picture(2, 2, 12)).is_none(), "too few bytes");
        let image = picture_image(&picture(2, 1, 8)).unwrap();
        assert_eq!(image.size, [2, 1]);
    }

    #[test]
    fn a_thumbnail_keeps_its_pages_shape_inside_its_row() {
        let bounds = vec2(100.0, 129.0);
        // A page as tall as the row's first is as wide as the panel.
        let letter = fit_within([612.0, 792.0], vec2(100.0, 100.0 * 792.0 / 612.0));
        assert!((letter.x - 100.0).abs() < 0.01 && (letter.y - 129.41).abs() < 0.01);
        // A taller page fits the row's height and narrows to its shape.
        let tall = fit_within([612.0, 1584.0], bounds);
        assert!((tall.y - 129.0).abs() < 0.01);
        assert!((tall.x / tall.y - 612.0 / 1584.0).abs() < 0.001);
        // A wider page fits the panel's width.
        let wide = fit_within([1584.0, 612.0], bounds);
        assert!((wide.x - 100.0).abs() < 0.01);
        assert!((wide.x / wide.y - 1584.0 / 612.0).abs() < 0.001);
        // A sliver still shows a point.
        assert_eq!(fit_within([14400.0, 1.0], bounds).y, 1.0);
        assert_eq!(fit_within([1.0, 14400.0], bounds).x, 1.0);
    }

    #[test]
    fn the_password_moves_to_a_roomy_string_and_keeps_its_text() {
        // As an undo leaves it: no room past the text.
        let mut password = String::from("open sesame");
        password.shrink_to_fit();
        assert!(password.capacity() < PASSWORD_ROOM);

        make_room(&mut password);

        assert_eq!(password, "open sesame");
        assert!(password.capacity() >= PASSWORD_ROOM);
        // Typing up to the limit then fits where the text is.
        let before = password.as_ptr();
        password.push_str(&"\u{e9}".repeat(PASSWORD_CHARS - "open sesame".len()));
        make_room(&mut password);
        assert_eq!(password.as_ptr(), before);
    }
}
