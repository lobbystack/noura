//! Pieces more than one screen uses: the loading state, notices, buttons
//! and the New workspace form.

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
    let builder = UiBuilder::new().accessibility_role(Role::Alert);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(&notice.title)
                        .strong()
                        .color(palette.destructive),
                );
                ui.label(&notice.message);
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
