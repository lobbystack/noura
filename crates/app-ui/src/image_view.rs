//! The image tab: the image small enough to fit the tab, or at the zoom
//! you chose, labelled with its file name. A pinch zooms at the pointer,
//! Zoom In, Zoom Out and Actual Size zoom at the middle, and an image
//! larger than the tab scrolls, as in Preview; `app-core` keeps the zoom
//! and says where to scroll. The interface zoom leaves the image's size
//! on the screen alone. `app-core` also reads the file off the UI
//! thread, and the decoders in `image_decoders` measure its bytes and
//! draw them at the screen's pixels, as they do the images in notes.

use std::collections::HashMap;

use app_core::{App, ImageState, TabId};
use egui::{
    Rect, ScrollArea, Sense, Spinner, TextureOptions, Ui, Vec2, WidgetInfo,
    accesskit::Role,
    emath::GuiRounding as _,
    load::{SizeHint, SizedTexture, TexturePoll},
    vec2,
};

use crate::common::muted;

/// What each image tab's view keeps.
#[derive(Default)]
pub(crate) struct ImageViews {
    tabs: HashMap<TabId, ImageTab>,
}

#[derive(Default)]
struct ImageTab {
    /// The address egui holds the tab's bytes at.
    address: String,
    /// The last scroll `app-core` asked for that the view made.
    scrolled: u64,
}

impl ImageViews {
    /// Forget the bytes of the tabs that closed.
    pub(crate) fn retain(&mut self, ctx: &egui::Context, app: &App) {
        self.tabs.retain(|tab, view| {
            let open = app.tabs().iter().any(|open| open.id() == *tab);
            if !open {
                ctx.forget_image(&view.address);
            }
            open
        });
    }
}

/// Draw the image tab `tab`. Returns `false` when the tab shows no image.
pub(crate) fn show(ui: &mut Ui, app: &mut App, views: &mut ImageViews, tab: TabId) -> bool {
    let Some(image) = app.image(tab) else {
        return false;
    };
    crate::image_decoders::install(ui.ctx());
    let name = image.name().to_owned();
    let (bytes, generation) = match image.state() {
        ImageState::Reading => {
            waiting(ui, "Opening the image\u{2026}");
            return true;
        }
        ImageState::Failed { message } => {
            let message = message.clone();
            ui.centered_and_justified(|ui| muted(ui, &message));
            return true;
        }
        ImageState::Shown { bytes, generation } => (bytes.clone(), *generation),
    };
    // The decoders tell the format by the address's extension.
    let extension = name
        .rsplit_once('.')
        .map_or_else(String::new, |(_, extension)| extension.to_ascii_lowercase());
    let address = format!("bytes://noura-viewer/{generation}.{extension}");
    let view = views.tabs.entry(tab).or_default();
    if view.address != address {
        if !view.address.is_empty() {
            ui.ctx().forget_image(&view.address);
        }
        view.address.clone_from(&address);
        ui.ctx().include_bytes(address.clone(), bytes);
    }
    let size = match crate::image_decoders::natural_size(ui.ctx(), &address) {
        Ok(Some(size)) => size,
        Ok(None) => {
            waiting(ui, "Opening the image\u{2026}");
            return true;
        }
        Err(_) => {
            ui.centered_and_justified(|ui| muted(ui, CANT_SHOW));
            return true;
        }
    };
    // The interface zoom leaves the image's size on the screen alone, as
    // it does a PDF page's.
    let area = ui.available_size();
    app.size_image(tab, size.into(), area.into(), ui.ctx().zoom_factor());
    let Some(image) = app.image(tab) else {
        return true;
    };
    let drawn = Vec2::from(image.drawn_size().unwrap_or(size.into()));
    let origin = Vec2::from(image.image_origin());
    let scroll = image.scroll();

    let mut scroll_area = ScrollArea::both()
        .id_salt(("image", tab))
        .auto_shrink(false);
    let jump = scroll.filter(|scroll| scroll.seq != view.scrolled);
    if let Some(scroll) = jump {
        scroll_area = scroll_area.scroll_offset(Vec2::from(scroll.offset));
    }
    // An image that fits within half a point fills the view without
    // scrolling.
    let content = vec2(
        if drawn.x <= area.x + 0.5 {
            area.x
        } else {
            drawn.x
        },
        if drawn.y <= area.y + 0.5 {
            area.y
        } else {
            drawn.y
        },
    );
    let pixels_per_point = ui.ctx().pixels_per_point();
    let mut scope = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()));
    if jump.is_some() {
        // The area bounds its scroll by last frame's content, and springs
        // back from past them. A zoom's scroll fits this frame's content.
        scope.spacing_mut().scroll.kinetic.rubber_band = false;
    }
    let output = scroll_area.show_viewport(&mut scope, |ui, _viewport| {
        ui.set_min_size(content);
        // The image's top left on a screen pixel, and its size in whole
        // pixels, so at its own size each of its pixels lands on one.
        let corner = (ui.max_rect().min + origin).round_to_pixels(pixels_per_point);
        let pixels = (drawn * pixels_per_point).round().max(Vec2::splat(1.0));
        let rect = Rect::from_min_size(corner, pixels / pixels_per_point);
        let response = ui.interact(rect, ui.scope_id().with("image"), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(Role::Image, true, &name));
        let hint = SizeHint::Size {
            width: pixels.x as u32,
            height: pixels.y as u32,
            maintain_aspect_ratio: false,
        };
        match ui
            .ctx()
            .try_load_texture(&address, TextureOptions::LINEAR, hint)
        {
            Ok(TexturePoll::Ready { texture }) => {
                egui::Image::from_texture(SizedTexture::new(texture.id, rect.size()))
                    .paint_at(ui, rect);
            }
            Ok(TexturePoll::Pending { .. }) => {
                let spinner = Rect::from_center_size(rect.center(), Vec2::splat(20.0));
                Spinner::new().paint_at(ui, spinner.intersect(ui.clip_rect()));
            }
            Err(_) => {
                ui.put(rect.intersect(ui.clip_rect()), |ui: &mut Ui| {
                    ui.centered_and_justified(|ui| muted(ui, CANT_SHOW))
                        .response
                });
            }
        }
    });
    ui.advance_cursor_after_rect(scope.min_rect());
    if let Some(scroll) = scroll {
        view.scrolled = scroll.seq;
    }
    app.image_scrolled(tab, output.state.clamped_offset().into(), view.scrolled);
    // A pinch, or a scroll with Command held, zooms at the pointer.
    let zoom = ui.input(|input| input.zoom_delta());
    let pointer = ui.input(|input| input.pointer.hover_pos());
    if zoom != 1.0
        && let Some(pointer) = pointer.filter(|pointer| output.inner_rect.contains(*pointer))
    {
        app.image_zoom_by(tab, zoom, (pointer - output.inner_rect.min).into());
    }
    true
}

/// What the tab says when no decoder can draw the image.
const CANT_SHOW: &str = "Noura can\u{2019}t show this image.";

fn waiting(ui: &mut Ui, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        ui.add(Spinner::new().size(20.0));
        muted(ui, text);
    });
}
