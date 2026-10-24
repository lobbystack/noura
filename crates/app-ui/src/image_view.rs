//! The image tab: the image at its own size, or smaller to fit the tab,
//! labelled with its file name. `app-core` reads the file off the UI
//! thread, and the decoders in `image_decoders` draw its bytes, as they
//! draw the images in notes.

use std::collections::HashMap;

use app_core::{App, ImageState, TabId};
use egui::{
    Align, Layout, Spinner, TextureOptions, Ui,
    load::{SizeHint, SizedTexture, TexturePoll},
};

use crate::common::muted;

/// The address egui holds each image tab's bytes at.
#[derive(Default)]
pub(crate) struct ImageViews {
    tabs: HashMap<TabId, String>,
}

impl ImageViews {
    /// Forget the bytes of the tabs that closed.
    pub(crate) fn retain(&mut self, ctx: &egui::Context, app: &App) {
        self.tabs.retain(|tab, address| {
            let open = app.tabs().iter().any(|open| open.id() == *tab);
            if !open {
                ctx.forget_image(address);
            }
            open
        });
    }
}

/// Draw the image tab `tab`. Returns `false` when the tab shows no image.
pub(crate) fn show(ui: &mut Ui, app: &App, views: &mut ImageViews, tab: TabId) -> bool {
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
    if views.tabs.get(&tab) != Some(&address) {
        if let Some(old) = views.tabs.insert(tab, address.clone()) {
            ui.ctx().forget_image(&old);
        }
        ui.ctx().include_bytes(address.clone(), bytes);
    }
    match ui
        .ctx()
        .try_load_texture(&address, TextureOptions::LINEAR, SizeHint::default())
    {
        Ok(TexturePoll::Ready { texture }) => {
            let available = ui.available_size();
            let natural = texture.size;
            let scale = (available.x / natural.x.max(1.0))
                .min(available.y / natural.y.max(1.0))
                .min(1.0);
            let size = natural * scale;
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.add_space(((available.y - size.y) / 2.0).max(0.0));
                ui.add(
                    egui::Image::from_texture(SizedTexture::new(texture.id, size)).alt_text(&name),
                );
            });
        }
        Ok(TexturePoll::Pending { .. }) => waiting(ui, "Opening the image\u{2026}"),
        Err(_) => {
            ui.centered_and_justified(|ui| muted(ui, "Noura can\u{2019}t show this image."));
        }
    }
    true
}

fn waiting(ui: &mut Ui, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        ui.add(Spinner::new().size(20.0));
        muted(ui, text);
    });
}
