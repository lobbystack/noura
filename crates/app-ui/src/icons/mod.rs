//! Noura's interface icons: Phosphor's outlines, filled for the page you
//! are on. Each icon renders once per size and stays as a texture, tinted
//! where it draws.

// The generated table holds icons the interface doesn't draw yet.
#[allow(dead_code)]
mod phosphor;

use std::{collections::HashMap, sync::Arc};

use egui::{Color32, ColorImage, Context, Id, Painter, Rect, TextureHandle, TextureOptions, pos2};
use resvg::{tiny_skia, usvg};

/// An icon, as its outline and its filled form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Icon {
    name: &'static str,
    regular: &'static [&'static str],
    fill: &'static [&'static str],
}

macro_rules! icons {
    ($($name:ident => $regular:ident, $fill:ident;)*) => {
        $(pub(crate) const $name: Icon = Icon {
            name: stringify!($name),
            regular: phosphor::$regular,
            fill: phosphor::$fill,
        };)*
    };
}

icons! {
    INBOX => TRAY_REGULAR, TRAY_FILL;
    FILES => FILES_REGULAR, FILES_FILL;
    TASKS => CHECKS_REGULAR, CHECKS_FILL;
    CALENDAR => CALENDAR_REGULAR, CALENDAR_FILL;
    AI => SPARKLE_REGULAR, SPARKLE_FILL;
    PROJECTS => FOLDER_OPEN_REGULAR, FOLDER_OPEN_FILL;
    SEARCH => MAGNIFYING_GLASS_REGULAR, MAGNIFYING_GLASS_FILL;
    SETTINGS => GEAR_SIX_REGULAR, GEAR_SIX_FILL;
    FOLDER => FOLDER_REGULAR, FOLDER_FILL;
    FOLDER_OPEN => FOLDER_OPEN_REGULAR, FOLDER_OPEN_FILL;
    NOTE => FILE_TEXT_REGULAR, FILE_TEXT_FILL;
    PDF => FILE_PDF_REGULAR, FILE_PDF_FILL;
    CARET_RIGHT => CARET_RIGHT_REGULAR, CARET_RIGHT_FILL;
    CARET_DOWN => CARET_DOWN_REGULAR, CARET_DOWN_FILL;
    NEW_NOTE => NOTE_PENCIL_REGULAR, NOTE_PENCIL_FILL;
    NEW_FOLDER => FOLDER_SIMPLE_PLUS_REGULAR, FOLDER_SIMPLE_PLUS_FILL;
    COLLAPSE => ARROWS_IN_LINE_VERTICAL_REGULAR, ARROWS_IN_LINE_VERTICAL_FILL;
}

/// Draw `icon` in `rect`, in `color`, filled when `filled`.
pub(crate) fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32, filled: bool) {
    let side = (rect.width().min(rect.height()) * painter.pixels_per_point()).round() as u32;
    let Some(texture) = texture(painter.ctx(), icon, filled, side) else {
        return;
    };
    let square = Rect::from_center_size(
        rect.center(),
        egui::Vec2::splat(rect.width().min(rect.height())),
    );
    painter.image(
        texture.id(),
        square,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        color,
    );
}

type Cache = Arc<egui::mutex::Mutex<HashMap<(&'static str, bool, u32), TextureHandle>>>;

fn texture(ctx: &Context, icon: Icon, filled: bool, side: u32) -> Option<TextureHandle> {
    if side == 0 {
        return None;
    }
    let cache: Cache = ctx.data_mut(|data| data.get_temp_mut_or_default::<Cache>(Id::NULL).clone());
    let key = (icon.name, filled, side);
    if let Some(texture) = cache.lock().get(&key) {
        return Some(texture.clone());
    }
    let image = render(if filled { icon.fill } else { icon.regular }, side)?;
    let texture = ctx.load_texture(
        format!("icon-{}-{filled}-{side}", icon.name),
        image,
        TextureOptions::LINEAR,
    );
    cache.lock().insert(key, texture.clone());
    Some(texture)
}

/// Render `paths` white on a transparent square `side` pixels wide.
fn render(paths: &[&str], side: u32) -> Option<ColorImage> {
    let body: String = paths
        .iter()
        .map(|d| format!(r#"<path d="{d}"/>"#))
        .collect();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="white">{body}</svg>"#
    );
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(side, side)?;
    let scale = side as f32 / 256.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(ColorImage::from_rgba_premultiplied(
        [side as usize, side as usize],
        pixmap.data(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_renders_something_in_both_forms() {
        for icon in [
            INBOX,
            FILES,
            TASKS,
            CALENDAR,
            AI,
            PROJECTS,
            SEARCH,
            SETTINGS,
            FOLDER,
            FOLDER_OPEN,
            NOTE,
            PDF,
            CARET_RIGHT,
            CARET_DOWN,
            NEW_NOTE,
            NEW_FOLDER,
            COLLAPSE,
        ] {
            for paths in [icon.regular, icon.fill] {
                let image = render(paths, 24).expect(icon.name);
                assert!(
                    image.pixels.iter().any(|pixel| pixel.a() > 0),
                    "{}",
                    icon.name
                );
            }
        }
    }
}
