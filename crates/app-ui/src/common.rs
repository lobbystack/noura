//! Pieces more than one screen uses: the loading state, notices, buttons
//! and the New workspace form.

use app_core::NoticeKind;
use egui::{
    Color32, Frame, Id, Margin, Response, RichText, Sense, Spinner, Stroke, Ui, UiBuilder, Widget,
    WidgetInfo, accesskit::Role, vec2,
};

use crate::{
    NouraUi,
    icons::{self, Icon},
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, Weight, font, pill, radius},
};

pub(crate) fn loading(ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        Spinner::new()
            .size(24.0)
            .ui(ui)
            .accessible_name("Opening workspace");
    });
}

/// The legacy app's primary button: dark on light, light on dark.
pub(crate) fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    Btn::new(text)
        .variant(Variant::Default)
        .enabled(enabled)
        .show(ui)
}

/// Supporting text in the muted color.
pub(crate) fn muted(ui: &mut Ui, text: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    ui.label(
        RichText::new(text)
            .size(SUPPORTING_SIZE)
            .color(palette.muted_foreground),
    )
}

/// The current notice, if any, as the legacy toast in the window's
/// bottom right corner: its icon, title and message, with Remove from
/// recents when the notice offers it, and Dismiss.
pub(crate) fn notice(state: &mut NouraUi, ui: &mut Ui) {
    let Some(notice) = state.app.notice().cloned() else {
        return;
    };
    let busy = state.app.is_busy();
    let palette = Palette::of(ui.ctx());
    // A problem interrupts a screen reader; a finished action waits its
    // turn.
    let (role, icon) = match notice.kind {
        NoticeKind::Problem => (Role::Alert, icons::FAILED),
        NoticeKind::Done => (Role::Status, icons::DONE),
    };
    egui::Area::new(Id::unique("notice"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-24.0, -24.0])
        .show(ui.ctx(), |ui| {
            let builder = UiBuilder::new().accessibility_role(role);
            ui.scope_builder(builder, |ui| {
                Frame::new()
                    .fill(palette.popover)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(radius(0.8))
                    .inner_margin(Margin::same(16))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 12,
                        spread: 0,
                        color: Color32::from_black_alpha(26),
                    })
                    .show(ui, |ui| {
                        ui.set_width(356.0 - 32.0);
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
                            let square =
                                egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
                            icons::paint(ui.painter(), icon, square, palette.foreground, false);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(&notice.title).font(font(
                                    ui.ctx(),
                                    BODY_SIZE,
                                    Weight::Medium,
                                )));
                                if !notice.message.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&notice.message).size(SUPPORTING_SIZE),
                                        )
                                        .wrap(),
                                    );
                                }
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if let Some(workspace_id) = &notice.remove_recent
                                        && Btn::new("Remove from recents")
                                            .variant(Variant::Default)
                                            .size(Size::Sm)
                                            .enabled(!busy)
                                            .show(ui)
                                            .clicked()
                                    {
                                        state.app.remove_recent(workspace_id);
                                    }
                                    if Btn::new("Dismiss")
                                        .variant(Variant::Ghost)
                                        .size(Size::Sm)
                                        .show(ui)
                                        .clicked()
                                    {
                                        state.app.dismiss_notice();
                                    }
                                });
                            });
                        });
                    });
            });
        });
}

/// What a [`Btn`] looks like: the legacy app's button variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Dark on light, light on dark: the main action.
    Default,
    /// A hairline border, filled on hover.
    Outline,
    /// A light fill.
    Secondary,
    /// No fill until hovered.
    Ghost,
    /// A tinted fill in the destructive color.
    Destructive,
}

/// How big a [`Btn`] is: the legacy app's button sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Size {
    /// 28 px high.
    Sm,
    /// 32 px high.
    Default,
}

impl Size {
    fn height(self) -> f32 {
        match self {
            Self::Sm => 28.0,
            Self::Default => 32.0,
        }
    }

    /// The side padding without and with a leading icon, and the gap
    /// between icon and text.
    fn padding(self) -> (f32, f32, f32) {
        match self {
            Self::Sm => (12.0, 8.0, 4.0),
            Self::Default => (12.0, 10.0, 6.0),
        }
    }

    fn text_size(self) -> f32 {
        BODY_SIZE
    }

    fn icon_size(self) -> f32 {
        16.0
    }
}

/// A legacy button: a pill with an optional leading icon, or an icon
/// alone with its text as its accessible name.
pub(crate) struct Btn<'a> {
    text: &'a str,
    icon: Option<Icon>,
    variant: Variant,
    size: Size,
    enabled: bool,
    icon_only: bool,
    width: Option<f32>,
    selected: Option<bool>,
    current: bool,
    fill: Option<Color32>,
}

impl<'a> Btn<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            text,
            icon: None,
            variant: Variant::Outline,
            size: Size::Default,
            enabled: true,
            icon_only: false,
            width: None,
            selected: None,
            current: false,
            fill: None,
        }
    }

    /// A square button that shows `icon` and reads as `label`.
    pub(crate) fn icon_only(icon: Icon, label: &'a str) -> Self {
        Self {
            icon: Some(icon),
            icon_only: true,
            variant: Variant::Ghost,
            ..Self::new(label)
        }
    }

    pub(crate) fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub(crate) fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }

    pub(crate) fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Stretch to `width` with the icon and text at the start, as a nav
    /// item does.
    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Tell screen readers this button stands for the page that shows,
    /// as a nav item does.
    pub(crate) fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// Read as a toggle that is on when `on`, as a filter does.
    pub(crate) fn selected(mut self, on: bool) -> Self {
        self.selected = Some(on);
        self
    }

    /// Fill the button with `color` while it isn't hovered.
    pub(crate) fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self
    }

    /// What's left of the row for a field before this button, so the two
    /// end exactly at the row's end.
    pub(crate) fn room_before(&self, ui: &Ui) -> f32 {
        (ui.available_width() - self.measure(ui) - ui.spacing().item_spacing.x).max(0.0)
    }

    /// How wide the button draws, so a row can leave it room.
    pub(crate) fn measure(&self, ui: &Ui) -> f32 {
        let height = self.size.height();
        let (pad, pad_icon, gap) = self.size.padding();
        if self.icon_only {
            return self.width.unwrap_or(height);
        }
        let font = font(ui.ctx(), self.size.text_size(), Weight::Medium);
        let text = ui
            .painter()
            .layout_no_wrap(self.text.to_owned(), font, Color32::PLACEHOLDER)
            .size()
            .x;
        let natural = match self.icon {
            Some(_) => pad_icon + self.size.icon_size() + gap + text + pad,
            None => pad * 2.0 + text,
        };
        self.width.unwrap_or(natural).max(natural.min(height))
    }

    pub(crate) fn show(self, ui: &mut Ui) -> Response {
        let palette = Palette::of(ui.ctx());
        let dark = ui.visuals().dark_mode;
        let enabled = self.enabled && ui.is_enabled();
        let height = self.size.height();
        let (pad, pad_icon, gap) = self.size.padding();
        let icon_size = self.size.icon_size();
        let font = font(ui.ctx(), self.size.text_size(), Weight::Medium);
        let galley = (!self.icon_only).then(|| match self.width {
            Some(width) => {
                let room = width
                    - pad
                    - if self.icon.is_some() {
                        pad_icon + icon_size + gap
                    } else {
                        pad
                    };
                egui::WidgetText::from(RichText::new(self.text).font(font.clone())).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    room.max(0.0),
                    egui::TextStyle::Button,
                )
            }
            None => ui.painter().layout_no_wrap(
                self.text.to_owned(),
                font.clone(),
                Color32::PLACEHOLDER,
            ),
        });
        let natural = match (&galley, self.icon) {
            (None, _) => height,
            (Some(galley), Some(_)) => pad_icon + icon_size + gap + galley.size().x + pad,
            (Some(galley), None) => pad * 2.0 + galley.size().x,
        };
        let width = self.width.unwrap_or(natural).max(natural.min(height));
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
        let label = self.text;
        let selected = self.selected;
        response.widget_info(|| match selected {
            Some(selected) => WidgetInfo::selected(Role::Button, enabled, selected, label),
            None => WidgetInfo::labeled(Role::Button, enabled, label),
        });
        if self.current {
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_aria_current(egui::accesskit::AriaCurrent::Page);
            });
        }
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let hovered = enabled && (response.hovered() || response.is_pointer_button_down_on());
        let (mut fill, mut text, stroke) = match self.variant {
            Variant::Default => (
                if hovered {
                    palette.primary.gamma_multiply(0.8)
                } else {
                    palette.primary
                },
                palette.primary_foreground,
                None,
            ),
            Variant::Outline => (
                match (hovered, dark) {
                    (true, true) => palette.input.gamma_multiply(0.3),
                    (true, false) => palette.muted,
                    (false, true) => Color32::TRANSPARENT,
                    (false, false) => palette.background,
                },
                palette.foreground,
                Some(palette.border),
            ),
            Variant::Secondary => (
                if hovered {
                    mix(palette.secondary, palette.foreground, 0.05)
                } else {
                    palette.secondary
                },
                palette.foreground,
                None,
            ),
            Variant::Ghost => (
                match (hovered, dark) {
                    (true, true) => palette.muted.gamma_multiply(0.5),
                    (true, false) => palette.muted,
                    (false, _) => Color32::TRANSPARENT,
                },
                palette.foreground,
                None,
            ),
            Variant::Destructive => (
                palette.destructive.gamma_multiply(match (hovered, dark) {
                    (false, false) => 0.1,
                    (true, false) | (false, true) => 0.2,
                    (true, true) => 0.3,
                }),
                palette.destructive,
                None,
            ),
        };
        if let Some(color) = self.fill
            && !hovered
        {
            fill = color;
        }
        let mut stroke = stroke.map(|color| Stroke::new(1.0, color));
        if !enabled {
            fill = fill.gamma_multiply(0.5);
            text = text.gamma_multiply(0.5);
            stroke = stroke.map(|stroke| Stroke::new(1.0, stroke.color.gamma_multiply(0.5)));
        }
        let painter = ui.painter();
        if response.has_focus() {
            focus_ring(painter, rect, pill(), &palette);
        }
        painter.rect(
            rect,
            pill(),
            fill,
            stroke.unwrap_or(Stroke::NONE),
            egui::StrokeKind::Inside,
        );
        let mut x = if self.icon_only {
            rect.center().x - icon_size / 2.0
        } else if self.icon.is_some() {
            rect.left() + pad_icon
        } else if self.width.is_some() {
            rect.left() + pad
        } else {
            rect.center().x - galley.as_ref().map_or(0.0, |galley| galley.size().x) / 2.0
        };
        if let Some(icon) = self.icon {
            let square = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - icon_size / 2.0),
                vec2(icon_size, icon_size),
            );
            icons::paint(painter, icon, square, text, false);
            x += icon_size + gap;
        }
        if let Some(galley) = galley {
            let y = rect.center().y - galley.size().y / 2.0;
            painter.galley(egui::pos2(x, y), galley, text);
        }
        response
    }
}

/// Whether focus shows, as CSS's `:focus-visible` decides: after the
/// keyboard moved, until the pointer presses.
pub(crate) fn focus_visible(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(focus_visible_id()))
        .unwrap_or(false)
}

fn focus_visible_id() -> egui::Id {
    egui::Id::unique("focus-visible")
}

/// Note this frame's input for [`focus_visible`].
pub(crate) fn track_focus_visible(ctx: &egui::Context) {
    let change = ctx.input(|input| {
        input.events.iter().rev().find_map(|event| match event {
            egui::Event::Key { pressed: true, .. } => Some(true),
            egui::Event::PointerButton { pressed: true, .. } => Some(false),
            _ => None,
        })
    });
    if let Some(visible) = change {
        ctx.data_mut(|data| data.insert_temp(focus_visible_id(), visible));
    }
}

/// The legacy focus ring: a `--ring` border with a soft 3 px halo. It
/// draws only while focus shows (see [`focus_visible`]).
pub(crate) fn focus_ring(
    painter: &egui::Painter,
    rect: egui::Rect,
    corner: egui::CornerRadius,
    palette: &Palette,
) {
    if !focus_visible(painter.ctx()) {
        return;
    }
    painter.rect_stroke(
        rect.expand(1.5),
        corner,
        Stroke::new(3.0, palette.ring.gamma_multiply(0.3)),
        egui::StrokeKind::Middle,
    );
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(1.0, palette.ring),
        egui::StrokeKind::Inside,
    );
}

/// `a` with `amount` of `b` mixed in.
pub(crate) fn mix(a: Color32, b: Color32, amount: f32) -> Color32 {
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, _] = b.to_array();
    let blend = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * amount).round() as u8;
    Color32::from_rgba_premultiplied(blend(ar, br), blend(ag, bg), blend(ab, bb), aa)
}

/// The legacy switch: a 32 by 20 track with a sliding thumb. It reads as
/// a switch named `label`. Returns whether you flipped it.
pub(crate) fn switch(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> Response {
    let palette = Palette::of(ui.ctx());
    let dark = ui.visuals().dark_mode;
    let enabled = enabled && ui.is_enabled();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(vec2(32.0, 20.0), sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let checked = *on;
    response.widget_info(|| WidgetInfo::selected(Role::Switch, enabled, checked, label));
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, checked);
        let painter = ui.painter();
        let track = if checked {
            palette.primary
        } else {
            palette.input.gamma_multiply(0.9)
        };
        let thumb = match (dark, checked) {
            (true, true) => palette.primary_foreground,
            (true, false) => palette.foreground,
            (false, _) => palette.background,
        };
        let fade = |color: Color32| {
            if enabled {
                color
            } else {
                color.gamma_multiply(0.5)
            }
        };
        if response.has_focus() {
            focus_ring(painter, rect, pill(), &palette);
        }
        painter.rect_filled(rect, pill(), fade(track));
        let inner = rect.shrink(2.0);
        let x = egui::lerp(inner.left() + 8.0..=inner.right() - 8.0, how_on);
        painter.circle_filled(egui::pos2(x, inner.center().y), 8.0, fade(thumb));
    }
    response
}

/// A row of joined buttons, one per option, with the chosen one filled:
/// the legacy outline toggle group, for a choice of a few short options.
/// Each is a radio button to screen readers. Returns whether you picked
/// another option.
pub(crate) fn segmented<T: Clone + PartialEq>(
    ui: &mut Ui,
    options: &[(T, &str)],
    chosen: &mut T,
) -> bool {
    let options: Vec<_> = options
        .iter()
        .map(|(value, label)| (value.clone(), *label, None))
        .collect();
    segmented_with_icons(ui, &options, chosen)
}

/// [`segmented`] with an icon before each option's label.
pub(crate) fn segmented_with_icons<T: Clone + PartialEq>(
    ui: &mut Ui,
    options: &[(T, &str, Option<Icon>)],
    chosen: &mut T,
) -> bool {
    let palette = Palette::of(ui.ctx());
    let font = font(ui.ctx(), BODY_SIZE, Weight::Medium);
    let mut picked = None;
    let last = options.len().saturating_sub(1);
    // Measure first, so the row lays out left to right even inside a
    // right-to-left layout, such as a heading's end.
    let width: f32 = options
        .iter()
        .map(|(_, label, icon)| {
            let text = ui
                .painter()
                .layout_no_wrap((*label).to_owned(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x;
            let icon_width = if icon.is_some() { 20.0 } else { 0.0 };
            (text + icon_width + 16.0).max(28.0) - 1.0
        })
        .sum::<f32>()
        + 1.0;
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(vec2(width, 28.0), layout, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (index, (value, label, icon)) in options.iter().enumerate() {
            let selected = *chosen == *value;
            let galley = ui.painter().layout_no_wrap(
                (*label).to_owned(),
                font.clone(),
                Color32::PLACEHOLDER,
            );
            let icon_width = if icon.is_some() { 16.0 + 4.0 } else { 0.0 };
            let size = vec2((galley.size().x + icon_width + 16.0).max(28.0), 28.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            response.widget_info(|| {
                WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, *label)
            });
            if ui.is_rect_visible(rect) {
                let painter = ui.painter();
                let round = 14;
                let corner = egui::CornerRadius {
                    nw: if index == 0 { round } else { 0 },
                    sw: if index == 0 { round } else { 0 },
                    ne: if index == last { round } else { 0 },
                    se: if index == last { round } else { 0 },
                };
                if selected || response.hovered() {
                    painter.rect_filled(rect, corner, palette.muted);
                }
                // Neighbours share a border: each draws its own left
                // edge, and the last one also its right.
                let stroke = Stroke::new(1.0, palette.input);
                painter.rect_stroke(rect, corner, stroke, egui::StrokeKind::Inside);
                if response.has_focus() {
                    focus_ring(painter, rect, corner, &palette);
                }
                let mut x = rect.center().x - (galley.size().x + icon_width) / 2.0;
                if let Some(icon) = icon {
                    let square = egui::Rect::from_min_size(
                        egui::pos2(x, rect.center().y - 8.0),
                        vec2(16.0, 16.0),
                    );
                    icons::paint(painter, *icon, square, palette.foreground, false);
                    x += icon_width;
                }
                let y = rect.center().y - galley.size().y / 2.0;
                painter.galley(egui::pos2(x, y), galley, palette.foreground);
            }
            if response.clicked() && !selected {
                picked = Some(value.clone());
            }
            if index != last {
                // Overlap the next border with this one.
                ui.add_space(-1.0);
            }
        }
    });
    match picked {
        Some(value) => {
            *chosen = value;
            true
        }
        None => false,
    }
}

/// A Settings field's label: 14 px medium.
pub(crate) fn field_label(ui: &mut Ui, text: &str) -> Response {
    let font = font(ui.ctx(), BODY_SIZE, Weight::Medium);
    ui.label(RichText::new(text).font(font))
}

/// A Settings field's description: 14 px in the muted color.
pub(crate) fn description(ui: &mut Ui, text: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(BODY_SIZE)
                .color(palette.muted_foreground),
        )
        .wrap(),
    )
}

/// The legacy empty state: an optional icon in a 40 px tile, a title and
/// a description, centered with room around them.
pub(crate) fn empty_state(ui: &mut Ui, icon: Option<Icon>, title: &str, text: Option<&str>) {
    empty_state_with(ui, icon, title, text, None::<fn(&mut Ui)>);
}

/// [`empty_state`] with controls under the description, such as a
/// plugin's buttons. The gap before them shows only when there are some.
pub(crate) fn empty_state_with(
    ui: &mut Ui,
    icon: Option<Icon>,
    title: &str,
    text: Option<&str>,
    controls: Option<impl FnOnce(&mut Ui)>,
) {
    let palette = Palette::of(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        if let Some(icon) = icon {
            let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
            ui.painter().rect_filled(rect, radius(1.4), palette.muted);
            let square = egui::Rect::from_center_size(rect.center(), vec2(20.0, 20.0));
            icons::paint(ui.painter(), icon, square, palette.foreground, false);
            ui.add_space(8.0);
        }
        ui.label(RichText::new(title).font(font(
            ui.ctx(),
            crate::theme::TITLE_SIZE,
            Weight::Semibold,
        )));
        if let Some(text) = text {
            ui.scope(|ui| {
                ui.set_max_width(384.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .size(BODY_SIZE)
                            .color(palette.muted_foreground),
                    )
                    .wrap(),
                );
            });
        }
        if let Some(controls) = controls {
            ui.add_space(8.0);
            controls(ui);
        }
        ui.add_space(48.0);
    });
}

/// The legacy alert: a bordered box with an icon, a title and a
/// description, in the destructive color when `destructive`.
pub(crate) fn alert(ui: &mut Ui, icon: Icon, title: &str, text: &str, destructive: bool) {
    let palette = Palette::of(ui.ctx());
    let (color, body) = if destructive {
        (palette.destructive, palette.destructive.gamma_multiply(0.9))
    } else {
        (palette.foreground, palette.muted_foreground)
    };
    let role = if destructive { Role::Alert } else { Role::Note };
    let builder = UiBuilder::new()
        .accessibility_role(role)
        .accessibility_label(title);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .fill(palette.popover)
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(pill())
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
                    let square = egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
                    icons::paint(ui.painter(), icon, square, color, false);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(
                            RichText::new(title)
                                .font(font(ui.ctx(), BODY_SIZE, Weight::Medium))
                                .color(color),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(text).size(BODY_SIZE).color(body))
                                .wrap(),
                        );
                    });
                });
            });
    });
}

/// A bordered row that opens `url`: its title and an arrow pointing out.
/// It reads as a link named `title`.
pub(crate) fn link_item(ui: &mut Ui, title: &str, url: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    let font = font(ui.ctx(), BODY_SIZE, Weight::Medium);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Link, true, title));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if response.hovered() {
            palette.muted
        } else {
            Color32::TRANSPARENT
        };
        painter.rect(
            rect,
            pill(),
            fill,
            Stroke::new(1.0, palette.border),
            egui::StrokeKind::Inside,
        );
        if response.has_focus() {
            focus_ring(painter, rect, pill(), &palette);
        }
        let galley = painter.layout_no_wrap(title.to_owned(), font, palette.foreground);
        painter.galley(
            egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.foreground,
        );
        let square = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 14.0 - 8.0, rect.center().y),
            vec2(16.0, 16.0),
        );
        icons::paint(
            painter,
            icons::ARROW_UP_RIGHT,
            square,
            palette.foreground,
            false,
        );
    }
    response.on_hover_text(url)
}

/// A titled group of controls in a Settings section, as the legacy field
/// drew one: its 14 px medium label, its controls 12 px below, then 24 px
/// to the next group.
pub(crate) fn settings_group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(title);
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        field_label(ui, title);
        add(ui);
    });
    ui.add_space(24.0);
}

/// The legacy menu's frame: a rounded popover with a hairline ring and a
/// soft shadow, 4 px around its items.
pub(crate) fn menu_frame(ctx: &egui::Context) -> Frame {
    let palette = Palette::of(ctx);
    let dark = ctx.theme() == egui::Theme::Dark;
    Frame::new()
        .fill(palette.popover)
        .corner_radius(pill())
        .inner_margin(Margin::same(4))
        .stroke(Stroke::new(
            1.0,
            palette
                .foreground
                .gamma_multiply(if dark { 0.1 } else { 0.05 }),
        ))
        .shadow(egui::Shadow {
            offset: [0, 10],
            blur: 15,
            spread: 0,
            color: Color32::from_black_alpha(26),
        })
}

/// One item of a legacy menu: an optional icon, its text with an optional
/// second line, and a check when `checked`. It fills on hover.
pub(crate) fn menu_item(
    ui: &mut Ui,
    icon: Option<Icon>,
    text: &str,
    detail: Option<&str>,
    checked: bool,
    enabled: bool,
) -> Response {
    let palette = Palette::of(ui.ctx());
    let enabled = enabled && ui.is_enabled();
    let width = ui.available_width();
    let title_font = font(
        ui.ctx(),
        BODY_SIZE,
        if detail.is_some() {
            Weight::Medium
        } else {
            Weight::Regular
        },
    );
    let text_width = width - 16.0 - if icon.is_some() { 24.0 } else { 0.0 } - 24.0;
    let title = egui::WidgetText::from(RichText::new(text).font(title_font)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_width,
        egui::TextStyle::Body,
    );
    let detail = detail.map(|detail| {
        egui::WidgetText::from(RichText::new(detail).size(SUPPORTING_SIZE)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            text_width,
            egui::TextStyle::Small,
        )
    });
    let height = match &detail {
        Some(detail) => 12.0 + title.size().y + 2.0 + detail.size().y,
        None => crate::theme::MENU_ROW,
    };
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    response.widget_info(|| WidgetInfo::labeled(Role::MenuItem, enabled, text));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if enabled && (response.hovered() || response.has_focus()) {
            painter.rect_filled(rect, radius(1.4), palette.accent);
        }
        let fade = |color: Color32| {
            if enabled {
                color
            } else {
                color.gamma_multiply(0.5)
            }
        };
        let mut x = rect.left() + 8.0;
        if let Some(icon) = icon {
            let square = egui::Rect::from_min_size(
                egui::pos2(
                    x,
                    rect.top()
                        + (if detail.is_some() {
                            8.0
                        } else {
                            height / 2.0 - 8.0
                        }),
                ),
                vec2(16.0, 16.0),
            );
            icons::paint(painter, icon, square, fade(palette.muted_foreground), false);
            x += 24.0;
        }
        match detail {
            Some(detail) => {
                let y = rect.top() + 6.0;
                let below = y + title.size().y + 2.0;
                painter.galley(egui::pos2(x, y), title, fade(palette.foreground));
                painter.galley(egui::pos2(x, below), detail, fade(palette.muted_foreground));
            }
            None => {
                let y = rect.center().y - title.size().y / 2.0;
                painter.galley(egui::pos2(x, y), title, fade(palette.foreground));
            }
        }
        if checked {
            let square = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 16.0, rect.center().y),
                vec2(16.0, 16.0),
            );
            icons::paint(painter, icons::CHECK, square, palette.foreground, false);
        }
    }
    response
}

/// A label over a menu's items, in the muted color.
pub(crate) fn menu_label(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.add_space(8.0);
        ui.label(
            RichText::new(text)
                .size(SUPPORTING_SIZE)
                .color(palette.muted_foreground),
        );
    });
}

/// A line between a menu's groups, edge to edge.
pub(crate) fn menu_separator(ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    ui.add_space(4.0);
    let rect = ui.max_rect().expand2(vec2(4.0, 0.0));
    let y = ui.cursor().top();
    ui.painter().hline(
        rect.x_range(),
        y,
        Stroke::new(1.0, palette.border.gamma_multiply(0.5)),
    );
    ui.add_space(5.0);
}

/// The height of the row the window's title bar takes.
pub(crate) const TITLE_BAR_HEIGHT: f32 = 36.0;

/// Let `rect`, an empty part of the title bar row, move the window when
/// dragged, do what the system's setting says when double-clicked and ask
/// for the window menu when right-clicked, as a title bar does.
pub(crate) fn title_bar_area(ui: &mut Ui, rect: egui::Rect, id: &str) {
    let response = ui.interact(rect, Id::unique(("title-bar", id)), Sense::click_and_drag());
    if response.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        let key = double_click_key(ui.ctx().viewport_id());
        ui.ctx().data_mut(|data| data.insert_temp(key, true));
        ui.ctx().request_repaint();
    }
    if response.secondary_clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        let key = window_menu_key(ui.ctx().viewport_id());
        ui.ctx().data_mut(|data| data.insert_temp(key, at));
        ui.ctx().request_repaint();
    }
}

fn window_menu_key(viewport: egui::ViewportId) -> Id {
    Id::unique(("window-menu", viewport))
}

fn double_click_key(viewport: egui::ViewportId) -> Id {
    Id::unique(("title-bar-double-click", viewport))
}

/// Whether a double-click on the title bar row of the window `viewport`
/// asked for the title bar's action since the last call. What that does,
/// zoom, fill, minimize or nothing, is the system's setting, which the
/// desktop shell reads.
pub fn take_title_bar_double_click(ctx: &egui::Context, viewport: egui::ViewportId) -> bool {
    let key = double_click_key(viewport);
    ctx.data_mut(|data| {
        let asked = data.get_temp::<bool>(key).unwrap_or(false);
        data.remove::<bool>(key);
        asked
    })
}

/// Where a right click on the title bar row of the window `viewport` asked
/// for the window menu since the last call, in points from the window's
/// top-left corner. egui has no command for the menu, so the desktop shell
/// shows it.
pub fn take_window_menu_request(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
) -> Option<egui::Pos2> {
    let key = window_menu_key(viewport);
    ctx.data_mut(|data| {
        let at = data.get_temp::<egui::Pos2>(key);
        data.remove::<egui::Pos2>(key);
        at
    })
}

/// The legacy dialog's frame: a 24 px rounded popover with a hairline
/// ring and a large shadow, 24 px around its contents.
pub(crate) fn dialog_frame(ctx: &egui::Context) -> Frame {
    let palette = Palette::of(ctx);
    let dark = ctx.theme() == egui::Theme::Dark;
    Frame::new()
        .fill(palette.popover)
        .corner_radius(egui::CornerRadius::same(24))
        .inner_margin(Margin::same(24))
        .stroke(Stroke::new(
            1.0,
            palette
                .foreground
                .gamma_multiply(if dark { 0.1 } else { 0.05 }),
        ))
        .shadow(egui::Shadow {
            offset: [0, 20],
            blur: 25,
            spread: 0,
            color: Color32::from_black_alpha(26),
        })
}

/// The dim behind a legacy dialog.
pub(crate) const BACKDROP: Color32 = Color32::from_black_alpha(77);

/// A dialog's title: 16 px semibold, so it leads what the dialog says.
pub(crate) fn dialog_title(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).font(font(ui.ctx(), crate::theme::TITLE_SIZE, Weight::Semibold)))
}

/// The legacy dialog footer: 24 points under what the dialog says, its
/// buttons in a row at its end, 8 points apart, in the order given, so the
/// keyboard meets them left to right. Returns each button's response.
pub(crate) fn dialog_footer(ui: &mut Ui, buttons: Vec<Btn<'_>>) -> Vec<Response> {
    ui.add_space((24.0 - ui.spacing().item_spacing.y).max(0.0));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let widths: f32 = buttons.iter().map(|button| button.measure(ui)).sum();
        let gaps = 8.0 * buttons.len().saturating_sub(1) as f32;
        // The space before the first button is followed by one more gap.
        ui.add_space((ui.available_width() - widths - gaps - 8.0).max(0.0));
        buttons.into_iter().map(|button| button.show(ui)).collect()
    })
    .inner
}

/// The legacy badge: a 20 px pill of the light fill with 12 px medium
/// text.
pub(crate) fn badge(ui: &mut Ui, text: &str) -> Response {
    let palette = Palette::of(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        font(ui.ctx(), SUPPORTING_SIZE, Weight::Medium),
        palette.foreground,
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x + 16.0, 20.0), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, pill(), palette.secondary);
        painter.galley(
            rect.center() - galley.size() / 2.0,
            galley,
            palette.foreground,
        );
    }
    response
}

/// A legacy dialog: centered over the dimmed window in the legacy
/// dialog frame.
pub(crate) fn dialog(ctx: &egui::Context, id: Id) -> egui::Modal {
    egui::Modal::new(id)
        .frame(dialog_frame(ctx))
        .backdrop_color(BACKDROP)
}
