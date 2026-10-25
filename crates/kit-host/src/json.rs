//! Views and events as JSON, in the shape TypeScript plugins see them: a
//! WIT variant is `{"tag": ..., "val": ...}`, a record's fields are in
//! camel case, and an empty option is `null`. A view is its tree, each node
//! holding its children.

use app_core::{
    CalendarMode, Choice, Column, ColumnPress, Field, InputValue, PluginCheck, PluginEvent,
    PluginMove, PluginNaming, SortOrder, Submission, Target, ViewKind, ViewNode,
};
use serde_json::{Value, json};

/// A checked view's node and everything under it.
pub(crate) fn view_node(node: &ViewNode) -> Value {
    json!({
        "id": node.id,
        "kind": kind(&node.kind),
        "children": node.children.iter().map(view_node).collect::<Vec<_>>(),
    })
}

fn tagged(tag: &str, val: Value) -> Value {
    json!({ "tag": tag, "val": val })
}

fn kind(kind: &ViewKind) -> Value {
    match kind {
        ViewKind::Panel { label } => tagged("panel", json!(label)),
        ViewKind::Text(text) => tagged("text", json!(text)),
        ViewKind::Markdown(markdown) => tagged("markdown", json!(markdown)),
        ViewKind::Button { label } => tagged("button", json!(label)),
        ViewKind::List {
            label,
            reorder,
            selected,
            opens_on_click,
        } => tagged(
            "list",
            json!({
                "label": label,
                "reorder": reorder,
                "selected": selected,
                "opensOnClick": opens_on_click,
            }),
        ),
        ViewKind::Item {
            label,
            detail,
            checked,
            warning,
        } => tagged(
            "item",
            json!({ "label": label, "detail": detail, "checked": checked, "warning": warning }),
        ),
        ViewKind::VirtualList { label, rows } => {
            tagged("virtual-list", json!({ "label": label, "rows": rows }))
        }
        ViewKind::Table { label, columns } => tagged(
            "table",
            json!({ "label": label, "columns": columns.iter().map(column).collect::<Vec<_>>() }),
        ),
        ViewKind::Row { cells } => tagged("row", json!(cells)),
        ViewKind::Form {
            label,
            submit,
            required,
        } => tagged(
            "form",
            json!({ "label": label, "submit": submit, "required": required }),
        ),
        ViewKind::Input {
            label,
            value,
            choices,
        } => tagged(
            "input",
            json!({
                "label": label,
                "value": input_value(value),
                "choices": choices.iter().map(choice).collect::<Vec<_>>(),
            }),
        ),
        ViewKind::NameField { label, value } => {
            tagged("name-field", json!({ "label": label, "value": value }))
        }
        ViewKind::Menu { label } => tagged("menu", json!(label)),
        ViewKind::MenuItem { label, enabled } => {
            tagged("menu-item", json!({ "label": label, "enabled": enabled }))
        }
        ViewKind::Separator => json!({ "tag": "separator" }),
        ViewKind::Board { label } => tagged("board", json!(label)),
        ViewKind::BoardColumn { label } => tagged("board-column", json!(label)),
        ViewKind::Card { label, detail } => {
            tagged("card", json!({ "label": label, "detail": detail }))
        }
        ViewKind::Calendar { label, mode, date } => tagged(
            "calendar",
            json!({ "label": label, "mode": calendar_mode(*mode), "date": date }),
        ),
        ViewKind::Entry { title, start, end } => tagged(
            "entry",
            json!({ "title": title, "start": start, "end": end }),
        ),
    }
}

fn column(column: &Column) -> Value {
    json!({
        "id": column.id,
        "title": column.title,
        "sorted": column.sorted.map(|order| match order {
            SortOrder::Ascending => "ascending",
            SortOrder::Descending => "descending",
        }),
    })
}

fn choice(choice: &Choice) -> Value {
    json!({ "id": choice.id, "label": choice.label })
}

fn calendar_mode(mode: CalendarMode) -> &'static str {
    match mode {
        CalendarMode::Month => "month",
        CalendarMode::Week => "week",
        CalendarMode::Day => "day",
    }
}

fn input_value(value: &InputValue) -> Value {
    match value {
        InputValue::Text(text) => tagged("text", json!(text)),
        InputValue::Multiline(text) => tagged("multiline", json!(text)),
        InputValue::Checkbox(checked) => tagged("checkbox", json!(checked)),
        InputValue::Choice(choice) => tagged("choice", json!(choice)),
        InputValue::Date(date) => tagged("date", json!(date)),
        InputValue::DateTime(date_time) => tagged("date-time", json!(date_time)),
    }
}

/// The event `value` describes.
pub(crate) fn event(value: &Value) -> Result<PluginEvent, String> {
    let tag = value
        .get("tag")
        .and_then(Value::as_str)
        .ok_or("An event needs a tag.")?;
    let val = value.get("val").unwrap_or(&Value::Null);
    Ok(match tag {
        "pressed" => PluginEvent::Pressed(string(val)?),
        "selected" => PluginEvent::Selected(target(val)?),
        "activated" => PluginEvent::Activated(target(val)?),
        "sorted" => PluginEvent::Sorted(ColumnPress {
            node: field(val, "node")?,
            column: field(val, "column")?,
        }),
        "changed" => PluginEvent::Changed(input_field(val)?),
        "submitted" => PluginEvent::Submitted(Submission {
            node: field(val, "node")?,
            fields: val
                .get("fields")
                .and_then(Value::as_array)
                .ok_or("A submission needs its fields.")?
                .iter()
                .map(input_field)
                .collect::<Result<_, _>>()?,
        }),
        "chosen" => PluginEvent::Chosen(string(val)?),
        "moved" => PluginEvent::Moved(PluginMove {
            node: field(val, "node")?,
            item: field(val, "item")?,
            to: field(val, "to")?,
            position: val
                .get("position")
                .and_then(Value::as_u64)
                .and_then(|position| u32::try_from(position).ok())
                .ok_or("A move needs a position.")?,
        }),
        "checked" => PluginEvent::Checked(PluginCheck {
            node: field(val, "node")?,
            item: field(val, "item")?,
            checked: val
                .get("checked")
                .and_then(Value::as_bool)
                .ok_or("A check needs checked.")?,
        }),
        "named" => PluginEvent::Named(PluginNaming {
            node: field(val, "node")?,
            name: field(val, "name")?,
        }),
        "cancelled" => PluginEvent::Cancelled(string(val)?),
        other => return Err(format!("There's no {other} event.")),
    })
}

fn string(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "The event needs text.".to_owned())
}

fn field(value: &Value, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("The event needs {name} as text."))
}

fn target(value: &Value) -> Result<Target, String> {
    Ok(Target {
        node: field(value, "node")?,
        item: field(value, "item")?,
    })
}

fn input_field(field_value: &Value) -> Result<Field, String> {
    let input = field_value.get("value").ok_or("A field needs its value.")?;
    let tag = input
        .get("tag")
        .and_then(Value::as_str)
        .ok_or("A value needs a tag.")?;
    let val = input.get("val").unwrap_or(&Value::Null);
    let optional = || val.as_str().map(str::to_owned);
    let value = match tag {
        "text" => InputValue::Text(string(val)?),
        "multiline" => InputValue::Multiline(string(val)?),
        "checkbox" => InputValue::Checkbox(val.as_bool().ok_or("A checkbox needs true or false.")?),
        "choice" => InputValue::Choice(optional()),
        "date" => InputValue::Date(optional()),
        "date-time" => InputValue::DateTime(optional()),
        other => return Err(format!("There's no {other} value.")),
    };
    Ok(Field {
        node: field(field_value, "node")?,
        value,
    })
}
