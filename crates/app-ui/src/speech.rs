//! Settings > Speech, laid out as Handy's Models page: a search and two
//! filters over one table of models, On this computer then For your
//! languages, each row with its mark, name, grades, speed, size and
//! delete; then the plugins' shortcuts. `app-core` sorts, filters and
//! grades the rows; this file only draws them.

use app_core::{
    App, SPEECH_LANGUAGES, SpeechModelFilter, SpeechModelRow, SpeechModelSection, keys_from_label,
    speech_language_name,
};
use egui::{
    Align, Color32, Frame, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui, UiBuilder,
    WidgetInfo, accesskit::Role, vec2,
};

use crate::{
    ai::{alert, text_field},
    common::{Btn, Size, Variant, badge, field_label, focus_ring, muted, primary_button},
    icons::{self, Icon},
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, Weight, font, radius},
};

/// What the Speech section remembers between frames.
#[derive(Default)]
pub(crate) struct SpeechSettingsView {
    /// Keys being typed for a shortcut, by `<plugin>/<shortcut>`.
    keys: std::collections::BTreeMap<String, String>,
    /// Why the last change didn't go in.
    problem: Option<String>,
    /// The search and the filters over the models.
    filter: SpeechModelFilter,
}

const ROW_HEIGHT: f32 = 56.0;
const MARK: f32 = 28.0;
const GRADE: f32 = 30.0;
const SPEED: f32 = 40.0;
const SIZE: f32 = 64.0;
const ACTION: f32 = 28.0;
const GAP: f32 = 8.0;

fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// How a screen reader names `bars` of speed out of four.
fn speed_name(bars: u8) -> &'static str {
    match bars {
        0 => "Very slow",
        1 => "Slow",
        2 => "Moderate",
        3 => "Fast",
        _ => "Very fast",
    }
}

enum Action {
    Select(String),
    Download(String),
    Cancel(String),
    Delete(String),
}

/// A box of `size` that reads as `role` named `label` with `value`, or
/// as just `value` when `label` is `None`.
fn read_as(
    ui: &mut Ui,
    size: egui::Vec2,
    role: Role,
    label: Option<&str>,
    value: String,
) -> egui::Response {
    let (_, response) = ui.allocate_exact_size(size, Sense::hover());
    let label = label.map(str::to_owned);
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(role);
        if let Some(label) = label {
            node.set_label(label);
        }
        node.set_value(value);
    });
    response
}

/// A grade chip: filled for an A, toned for a B, outlined below.
fn grade_chip(ui: &mut Ui, language: &str, grade: Option<&str>) {
    let palette = Palette::of(ui.ctx());
    let name = speech_language_name(language);
    let read = match grade {
        Some(grade) => format!("{name} {grade}"),
        None => format!("No {name}"),
    };
    let response = read_as(ui, vec2(GRADE, 22.0), Role::Label, None, read);
    if !ui.is_rect_visible(response.rect) {
        return;
    }
    let painter = ui.painter();
    let rect = response.rect;
    let (fill, stroke, text) = match grade {
        Some("A") => (palette.primary, None, palette.primary_foreground),
        Some("B") => (palette.secondary, None, palette.foreground),
        Some(_) => (
            Color32::TRANSPARENT,
            Some(palette.border),
            palette.muted_foreground,
        ),
        None => (Color32::TRANSPARENT, None, palette.muted_foreground),
    };
    painter.rect(
        rect,
        radius(0.6),
        fill,
        stroke.map_or(Stroke::NONE, |color| Stroke::new(1.0, color)),
        egui::StrokeKind::Inside,
    );
    let galley = painter.layout_no_wrap(
        grade.unwrap_or("–").to_owned(),
        font(ui.ctx(), BODY_SIZE, Weight::Medium),
        text,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, text);
}

/// Four bars, filled up to the model's speed.
fn speed_meter(ui: &mut Ui, speed: Option<u8>) {
    let palette = Palette::of(ui.ctx());
    let value = speed.map_or("Not measured", speed_name);
    let response = read_as(
        ui,
        vec2(SPEED, 22.0),
        Role::Meter,
        Some("Speed"),
        value.to_owned(),
    );
    if !ui.is_rect_visible(response.rect) {
        return;
    }
    let painter = ui.painter();
    let bottom = response.rect.center().y + 7.0;
    let left = response.rect.center().x - (4.0 * 4.0 + 3.0 * 2.0) / 2.0;
    for (index, height) in [5.0, 8.0, 11.0, 14.0].into_iter().enumerate() {
        let x = left + index as f32 * 6.0;
        let bar =
            egui::Rect::from_min_max(egui::pos2(x, bottom - height), egui::pos2(x + 4.0, bottom));
        let color = if speed.is_some_and(|speed| (index as u8) < speed) {
            palette.muted_foreground
        } else {
            palette.border
        };
        painter.rect_filled(bar, 1.0, color);
    }
}

/// A small icon that reads as `label`, with `label` on hover.
fn tag(ui: &mut Ui, icon: Icon, label: &str) {
    let palette = Palette::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, label));
    icons::paint(ui.painter(), icon, rect, palette.muted_foreground, false);
    response.on_hover_text(label);
}

/// The radio of a downloaded model, checked for the one in use.
fn radio(ui: &mut Ui, row: &SpeechModelRow) -> bool {
    let palette = Palette::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(MARK, MARK), Sense::click());
    let selected = row.selected;
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, &row.name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let circle = egui::Rect::from_center_size(rect.center(), vec2(18.0, 18.0));
        if response.has_focus() {
            focus_ring(painter, circle, radius(1.0), &palette);
        }
        if selected {
            painter.circle_filled(circle.center(), 9.0, palette.primary);
            let check = circle.shrink(4.0);
            icons::paint(
                painter,
                icons::CHECK,
                check,
                palette.primary_foreground,
                false,
            );
        } else {
            let color = if response.hovered() {
                palette.muted_foreground
            } else {
                palette.input
            };
            painter.circle_stroke(circle.center(), 8.5, Stroke::new(1.5, color));
        }
    }
    response.clicked() && !selected
}

/// One model's row.
fn model_row(ui: &mut Ui, row: &SpeechModelRow, languages: &[String], action: &mut Option<Action>) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(&row.name)
        .sense(if row.downloaded {
            Sense::click()
        } else {
            Sense::hover()
        });
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope_builder(builder, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ROW_HEIGHT);
            ui.spacing_mut().item_spacing.x = GAP;
            let room = ui.available_width()
                - 2.0 * 12.0
                - MARK
                - languages.len() as f32 * (GRADE + GAP)
                - (SPEED + GAP)
                - (SIZE + GAP)
                - ACTION
                - 2.0 * GAP;
            ui.horizontal_centered(|ui| {
                ui.add_space(12.0 - GAP);
                match (row.downloaded, row.downloading) {
                    (_, Some(_)) => {
                        ui.allocate_ui(vec2(MARK, MARK), |ui| {
                            ui.add(egui::Spinner::new().size(16.0));
                        });
                    }
                    (true, None) => {
                        if radio(ui, row) {
                            *action = Some(Action::Select(row.id.clone()));
                        }
                    }
                    (false, None) => {
                        let label = format!("Download {}", row.name);
                        if Btn::icon_only(icons::DOWNLOAD, &label)
                            .size(Size::Sm)
                            .show(ui)
                            .on_hover_text("Download")
                            .clicked()
                        {
                            *action = Some(Action::Download(row.id.clone()));
                        }
                    }
                }
                ui.allocate_ui_with_layout(
                    vec2(room.max(80.0), ROW_HEIGHT),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(room.max(80.0));
                        ui.add_space(9.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add(
                                egui::Label::new(RichText::new(&row.name).font(font(
                                    ui.ctx(),
                                    BODY_SIZE,
                                    Weight::Medium,
                                )))
                                .truncate(),
                            );
                            if row.recommended {
                                badge(ui, "Recommended");
                            }
                            if row.streaming {
                                tag(ui, icons::BROADCAST, "Live preview");
                            }
                            if row.translation {
                                tag(ui, icons::TRANSLATE, "Translates");
                            }
                        });
                        let (line, color) = match (row.downloading, &row.problem) {
                            (Some((received, total)), _) => {
                                let percent = (received * 100).checked_div(total).unwrap_or(0);
                                (
                                    format!("Downloading {percent}% of {}", size(row.size)),
                                    palette.muted_foreground,
                                )
                            }
                            (None, Some(problem)) => (problem.clone(), palette.destructive),
                            (None, None) => (row.description.clone(), palette.muted_foreground),
                        };
                        if !line.is_empty() {
                            let label = ui.add(
                                egui::Label::new(
                                    RichText::new(&line).size(SUPPORTING_SIZE).color(color),
                                )
                                .truncate(),
                            );
                            if row.problem.is_some() && row.downloading.is_none() {
                                ui.ctx().accesskit_node_builder(label.id, |node| {
                                    node.set_role(Role::Alert);
                                });
                            }
                        }
                        if let Some((received, total)) = row.downloading {
                            let share = if total == 0 {
                                0.0
                            } else {
                                received as f32 / total as f32
                            };
                            let (bar, _) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 3.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(bar, 1.5, palette.border);
                            let done = egui::Rect::from_min_size(
                                bar.min,
                                vec2(bar.width() * share.clamp(0.0, 1.0), bar.height()),
                            );
                            ui.painter().rect_filled(done, 1.5, palette.primary);
                        }
                    },
                );
                for (language, grade) in languages.iter().zip(&row.grades) {
                    grade_chip(ui, language, *grade);
                }
                speed_meter(ui, row.speed);
                let size_text = size(row.size);
                ui.allocate_ui_with_layout(
                    vec2(SIZE, ROW_HEIGHT),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        ui.set_width(SIZE);
                        muted(ui, &size_text);
                    },
                );
                ui.allocate_ui(vec2(ACTION, ACTION), |ui| {
                    match (row.downloaded, row.downloading) {
                        (_, Some(_)) => {
                            let label = format!("Cancel {}", row.name);
                            if Btn::icon_only(icons::CLOSE, &label)
                                .size(Size::Sm)
                                .show(ui)
                                .on_hover_text("Cancel")
                                .clicked()
                            {
                                *action = Some(Action::Cancel(row.id.clone()));
                            }
                        }
                        (true, None) => {
                            let label = format!("Delete {}", row.name);
                            if Btn::icon_only(icons::TRASH, &label)
                                .size(Size::Sm)
                                .show(ui)
                                .on_hover_text("Delete")
                                .clicked()
                            {
                                *action = Some(Action::Delete(row.id.clone()));
                            }
                        }
                        (false, None) => {
                            ui.allocate_exact_size(vec2(ACTION, ACTION), Sense::hover());
                        }
                    }
                });
            });
        })
        .response;
    if row.downloaded && !row.selected && response.clicked() && action.is_none() {
        *action = Some(Action::Select(row.id.clone()));
    }
    let fill = if row.selected {
        palette.muted
    } else if row.downloaded && response.hovered() {
        palette.muted.gamma_multiply(0.6)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().set(
        background,
        egui::Shape::rect_filled(response.rect, radius(1.0), fill),
    );
}

/// A section's name, with the column heads across from it on the first.
fn section_head(ui: &mut Ui, title: &str, columns: Option<&[String]>) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.spacing_mut().item_spacing.x = GAP;
        ui.add_space(12.0 + MARK);
        let head = |text: &str| {
            RichText::new(text)
                .size(SUPPORTING_SIZE)
                .color(palette.muted_foreground)
        };
        ui.label(head(title));
        let Some(columns) = columns else {
            return;
        };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(12.0 - GAP);
            ui.allocate_exact_size(vec2(ACTION, 1.0), Sense::hover());
            for (text, width) in [("Size", SIZE), ("Speed", SPEED)] {
                ui.allocate_ui_with_layout(
                    vec2(width, 20.0),
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        ui.label(head(text));
                    },
                );
            }
            for code in columns.iter().rev() {
                ui.allocate_ui_with_layout(
                    vec2(GRADE, 20.0),
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        ui.label(head(&code.to_uppercase()))
                            .on_hover_text(speech_language_name(code));
                    },
                );
            }
        });
    });
}

/// The search field and the two filters.
fn search_bar(ui: &mut Ui, filter: &mut SpeechModelFilter) {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(28.0);
                ui.spacing_mut().item_spacing.x = GAP;
                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(
                    ui.painter(),
                    icons::SEARCH,
                    rect,
                    palette.muted_foreground,
                    false,
                );
                let filters = Btn::new("Live preview").measure(ui)
                    + Btn::new("Translates").measure(ui)
                    + 64.0;
                ui.add(
                    TextEdit::singleline(&mut filter.search)
                        .hint_text("Search models")
                        .frame(egui::Frame::NONE)
                        .align(egui::Align2::LEFT_CENTER)
                        .min_size(vec2(0.0, 28.0))
                        .desired_width((ui.available_width() - filters).max(80.0)),
                )
                .accessible_name("Search models");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for (label, icon, on) in [
                        ("Translates", icons::TRANSLATE, &mut filter.translates),
                        ("Live preview", icons::BROADCAST, &mut filter.live_preview),
                    ] {
                        let variant = if *on {
                            Variant::Secondary
                        } else {
                            Variant::Ghost
                        };
                        if Btn::new(label)
                            .icon(icon)
                            .variant(variant)
                            .selected(*on)
                            .size(Size::Sm)
                            .show(ui)
                            .clicked()
                        {
                            *on = !*on;
                        }
                    }
                });
            });
        });
}

/// The title and, across from it, the language the grades are for.
fn title_row(ui: &mut Ui, app: &mut App) -> Option<Result<(), app_core::AppError>> {
    let language = app.speech_language().unwrap_or_else(|| "en".to_owned());
    let mut chosen = None;
    ui.horizontal(|ui| {
        field_label(ui, "Models");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            egui::ComboBox::from_id_salt("speech-language")
                .selected_text(speech_language_name(&language))
                .show_ui(ui, |ui| {
                    for (code, name) in SPEECH_LANGUAGES {
                        if ui.selectable_label(code == language, name).clicked() {
                            chosen = Some(code);
                        }
                    }
                })
                .response
                .accessible_name("Language");
        });
    });
    chosen.map(|code| app.set_speech_language(Some(code)))
}

/// The Speech section of Settings.
pub(crate) fn settings(ui: &mut Ui, app: &mut App, view: &mut SpeechSettingsView) {
    // Downloads move on outside any frame.
    let _ = app.speech_version();
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
    let palette = Palette::of(ui.ctx());
    if let Some(problem) = &view.problem {
        alert(ui, problem);
    }
    let mut action = None;
    let mut changed = title_row(ui, app);
    ui.add_space(8.0);

    let page = app.speech_models(&view.filter);
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Models");
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                search_bar(ui, &mut view.filter);
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0, palette.border),
                );
                Frame::new()
                    .inner_margin(Margin::symmetric(6, 8))
                    .show(ui, |ui| {
                        if page.sections.is_empty() {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add_space(12.0 + MARK + GAP);
                                muted(ui, "No models match");
                            });
                            ui.add_space(8.0);
                        }
                        for (index, (section, rows)) in page.sections.iter().enumerate() {
                            if index > 0 {
                                ui.add_space(12.0);
                            }
                            let title = section.title();
                            let group = UiBuilder::new()
                                .accessibility_role(Role::Group)
                                .accessibility_label(title);
                            ui.scope_builder(group, |ui| {
                                let columns = (index == 0).then_some(page.languages.as_slice());
                                section_head(ui, title, columns);
                                if rows.is_empty() && *section == SpeechModelSection::OnThisComputer
                                {
                                    ui.horizontal(|ui| {
                                        ui.set_height(ROW_HEIGHT * 0.75);
                                        ui.add_space(12.0 + MARK + GAP);
                                        muted(ui, "No models yet");
                                    });
                                }
                                for row in rows {
                                    model_row(ui, row, &page.languages, &mut action);
                                }
                            });
                        }
                    });
            });
    });
    ui.add_space(24.0);

    let shortcuts = app.global_shortcuts();
    if !shortcuts.is_empty() {
        crate::common::settings_group(ui, "Shortcuts", |ui| {
            for shortcut in shortcuts {
                let key = format!("{}/{}", shortcut.plugin, shortcut.id);
                let typed = view
                    .keys
                    .entry(key.clone())
                    .or_insert_with(|| app.keys_label(&shortcut.keys));
                text_field(ui, &shortcut.title, typed, false);
                ui.horizontal(|ui| {
                    let saved = keys_from_label(typed).as_deref() == Some(shortcut.keys.as_str());
                    let ready = !saved && !typed.trim().is_empty();
                    if primary_button(ui, "Save", ready).clicked() {
                        let saved = app.set_shortcut_keys(
                            &shortcut.plugin,
                            &shortcut.id,
                            Some(typed.trim()),
                        );
                        // The field shows the keys as this system writes
                        // them, whichever way you typed them.
                        if saved.is_ok()
                            && let Some(keys) = keys_from_label(typed)
                        {
                            *typed = app.keys_label(&keys);
                        }
                        changed = Some(saved);
                    }
                    if shortcut.keys != shortcut.declared && ui.button("Reset").clicked() {
                        changed = Some(app.set_shortcut_keys(&shortcut.plugin, &shortcut.id, None));
                        *typed = app.keys_label(&shortcut.declared);
                    }
                });
            }
        });
    }

    if let Some(action) = action {
        changed = Some(match action {
            Action::Select(id) => app.select_speech_model(&id),
            Action::Download(id) => app.download_speech_model(&id),
            Action::Cancel(id) => {
                app.cancel_speech_download(&id);
                Ok(())
            }
            Action::Delete(id) => app.delete_speech_model(&id),
        });
    }
    if let Some(changed) = changed {
        view.problem = changed.err().map(|error| error.message);
    }
}
