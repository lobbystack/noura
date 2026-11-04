//! Pieces more than one screen uses: the loading state, notices, buttons
//! and the New workspace form.

use app_core::NoticeKind;
use egui::{
    Button, Frame, Key, Margin, Response, RichText, Spinner, Stroke, TextEdit, Ui, UiBuilder,
    Widget, accesskit::Role,
};

use crate::{
    NouraUi,
    theme::{Palette, SUPPORTING_SIZE, radius},
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
    let palette = Palette::of(ui.ctx());
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(text).color(palette.primary_foreground))
            .fill(palette.primary)
            .stroke(Stroke::NONE),
    )
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

/// The current notice, if any, with Remove from recents when the notice
/// offers it.
pub(crate) fn notice(state: &mut NouraUi, ui: &mut Ui) {
    let Some(notice) = state.app.notice().cloned() else {
        return;
    };
    let busy = state.app.is_busy();
    let palette = Palette::of(ui.ctx());
    // A problem interrupts a screen reader; a finished action waits its
    // turn.
    let (role, title_color) = match notice.kind {
        NoticeKind::Problem => (Role::Alert, palette.destructive),
        NoticeKind::Done => (Role::Status, palette.foreground),
    };
    let builder = UiBuilder::new().accessibility_role(role);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&notice.title).strong().color(title_color));
                if !notice.message.is_empty() {
                    ui.label(&notice.message);
                }
                ui.horizontal(|ui| {
                    if let Some(workspace_id) = &notice.remove_recent
                        && ui
                            .add_enabled(!busy, Button::new("Remove from recents"))
                            .clicked()
                    {
                        state.app.remove_recent(workspace_id);
                    }
                    if ui.button("Dismiss").clicked() {
                        state.app.dismiss_notice();
                    }
                });
            });
    });
}

/// The New workspace form: a name, then a folder.
pub(crate) struct NewWorkspace {
    name: String,
    focus: bool,
}

impl NewWorkspace {
    pub(crate) fn new() -> Self {
        Self {
            name: String::new(),
            focus: true,
        }
    }
}

/// Draw the open New workspace form. Choose folder… asks for a folder and
/// creates the workspace there; Cancel or Escape closes the form.
pub(crate) fn new_workspace_form(state: &mut NouraUi, ui: &mut Ui) {
    let busy = state.app.is_busy();
    let Some(form) = &mut state.new_workspace else {
        return;
    };
    let field = ui.add(
        TextEdit::singleline(&mut form.name)
            .hint_text("Name")
            .desired_width(f32::INFINITY),
    );
    let field = field.accessible_name("Workspace name");
    if form.focus {
        form.focus = false;
        field.request_focus();
    }
    let submitted = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
    let escaped = ui.input(|input| input.key_pressed(Key::Escape));
    let named = !form.name.trim().is_empty();
    let mut choose = false;
    let mut cancel = escaped;
    ui.horizontal(|ui| {
        choose = primary_button(ui, "Choose folder…", named && !busy).clicked();
        cancel |= ui.button("Cancel").clicked();
    });
    if cancel {
        state.new_workspace = None;
        return;
    }
    if (choose || submitted) && named && !busy {
        let name = form.name.trim().to_owned();
        if let Some(folder) = state
            .picker
            .pick_folder("Choose a folder for the new workspace")
        {
            state.new_workspace = None;
            state.app.create_workspace(&name, folder);
        }
    }
}

/// A row of joined buttons, one per option, with the chosen one raised:
/// a choice of a few short options. Each is a radio button to screen
/// readers. Returns whether you picked another option.
pub(crate) fn segmented<T: Clone + PartialEq>(
    ui: &mut Ui,
    options: &[(T, &str)],
    chosen: &mut T,
) -> bool {
    let palette = Palette::of(ui.ctx());
    let mut picked = None;
    Frame::new()
        .fill(palette.muted)
        .corner_radius(radius(1.0))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (value, label) in options {
                    let selected = *chosen == *value;
                    let galley = egui::WidgetText::from(*label).into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Button,
                    );
                    let size = egui::vec2(galley.size().x + 20.0, 26.0);
                    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            Role::RadioButton,
                            ui.is_enabled(),
                            selected,
                            *label,
                        )
                    });
                    if ui.is_rect_visible(rect) {
                        let painter = ui.painter();
                        if selected {
                            painter.rect_filled(rect, radius(0.8), palette.background);
                        } else if response.hovered() {
                            painter.rect_filled(rect, radius(0.8), palette.accent);
                        }
                        if response.has_focus() {
                            painter.rect_stroke(
                                rect,
                                radius(0.8),
                                Stroke::new(1.0, palette.ring),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let color = if selected {
                            palette.foreground
                        } else {
                            palette.muted_foreground
                        };
                        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
                    }
                    if response.clicked() && !selected {
                        picked = Some(value.clone());
                    }
                }
            });
        });
    match picked {
        Some(value) => {
            *chosen = value;
            true
        }
        None => false,
    }
}
