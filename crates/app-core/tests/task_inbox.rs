//! Seam 1: the Tasks plugin's Due today section in the Inbox, with the
//! clock stopped. It shows how many tasks not marked done are due by the
//! end of today, overdue included, and up to six of them in the Tasks
//! list order, each with its due label, the overdue warning and a
//! checkbox. Selecting one opens it, its checkbox completes it against the
//! revision the section drew, and a field adds a task due on the device's
//! date. Turning Tasks off takes the section away and leaves the files.

mod support;

use app_core::{Field, InputValue, OpenAs, PluginCheck, PluginEvent, Submission, Target, ViewKind};
use support::{
    FakeSystem, Fixture,
    tasks::{
        A, B, C, D, E, F, G, H, Row, due_labels, due_rows, due_text, due_view, find, inbox_section,
        property, send_due, task_file, with_tasks,
    },
};

const J: &str = "01jbcdefghjkmnpqrstvwxyz0j";
const K: &str = "01jbcdefghjkmnpqrstvwxyz0k";

fn task(suffix: &str, properties: &str) -> Vec<u8> {
    task_file(suffix, properties, "2026-10-01T10:00:00Z")
}

/// A workspace whose clock reads 11:00 on 21 October 2026 in Toronto, with
/// seven tasks not done that are due by the end of today, one due
/// tomorrow, one done and one without a due date.
fn busy_day() -> (Fixture, std::sync::Arc<FakeSystem>) {
    let fixture = Fixture::new(&[
        (
            "tasks/Water the beds.md",
            &task(A, "status: todo\npriority: medium\ndue: 2026-10-19\n"),
        ),
        (
            "tasks/Pay rent.md",
            &task(B, "status: todo\npriority: urgent\ndue: 2026-10-21\n"),
        ),
        (
            // 23:30 in Toronto is already the 22nd in UTC.
            "tasks/Call the vet.md",
            &task(
                C,
                "status: in-progress\npriority: low\ndue: 2026-10-21T23:30:00-04:00\n",
            ),
        ),
        (
            "tasks/Order seeds.md",
            &task(D, "status: todo\npriority: medium\ndue: 2026-10-22\n"),
        ),
        (
            "tasks/Mulch.md",
            &task(E, "status: done\npriority: medium\ndue: 2026-10-20\n"),
        ),
        (
            "tasks/Read the manual.md",
            &task(F, "status: todo\npriority: medium\n"),
        ),
        (
            "tasks/Sweep.md",
            &task(G, "status: todo\npriority: high\ndue: 2026-10-21\n"),
        ),
        (
            "tasks/Rake.md",
            &task(H, "status: todo\npriority: medium\ndue: 2026-10-21\n"),
        ),
        (
            "Shed/Oil the hinges.md",
            &task(J, "status: todo\npriority: low\ndue: 2026-10-21\n"),
        ),
        (
            "tasks/Prune.md",
            &task(K, "status: cancelled\npriority: medium\ndue: 2026-10-01\n"),
        ),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    (fixture, system)
}

#[test]
fn due_today_counts_the_tasks_due_by_tonight_and_shows_six_in_list_order() {
    let (fixture, system) = busy_day();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    let section = inbox_section(&app).expect("the Inbox shows Due today");
    assert_eq!(section.title, "Due today");
    assert_eq!(due_text(&app, "due-count").as_deref(), Some("7 tasks"));
    // Overdue first, then by due date and priority, as the Tasks lists
    // order them. Done, undated and later tasks stay out.
    assert_eq!(
        due_labels(&app),
        [
            "Prune",
            "Water the beds",
            "Pay rent",
            "Sweep",
            "Rake",
            "Oil the hinges",
        ]
    );
}

#[test]
fn turning_tasks_off_takes_due_today_away_and_leaves_every_task_file() {
    let (fixture, system) = busy_day();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    assert!(inbox_section(&app).is_some());
    let before = fixture.snapshot();

    assert!(app.set_plugin_enabled("tasks", false));
    support::settle(&mut app);

    assert!(inbox_section(&app).is_none());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn each_row_shows_the_tasks_name_and_due_label_and_marks_overdue_tasks() {
    let (fixture, system) = busy_day();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    let row = |suffix: &str, label: &str, due: &str, overdue: bool| Row {
        id: format!("task_{suffix}"),
        label: label.to_owned(),
        detail: Some(due.to_owned()),
        checked: Some(false),
        warning: overdue.then(|| "Overdue".to_owned()),
    };
    assert_eq!(
        due_rows(&app),
        [
            row(K, "Prune", "Oct 1", true),
            row(A, "Water the beds", "Oct 19", true),
            row(B, "Pay rent", "Today", false),
            row(G, "Sweep", "Today", false),
            row(H, "Rake", "Today", false),
            row(J, "Oil the hinges", "Today", false),
        ]
    );
}

#[test]
fn a_click_on_a_row_opens_its_task() {
    let (fixture, system) = busy_day();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(app_core::INBOX_PAGE, OpenAs::Preview);
    // One click opens, as a link does: the list sends the click as an
    // activation.
    let view = due_view(&app);
    assert!(matches!(
        find(view.root(), "due").map(|list| &list.kind),
        Some(ViewKind::List {
            opens_on_click: true,
            ..
        })
    ));

    send_due(
        &mut app,
        PluginEvent::Activated(Target {
            node: "due".to_owned(),
            item: format!("task_{B}"),
        }),
    );

    support::plugins::wait_until(&mut app, "Pay rent opens", |app| {
        app.active_tab()
            .and_then(|tab| tab.file())
            .is_some_and(|file| file.relative_path == "tasks/Pay rent.md")
    });
}

fn check(id: &str) -> PluginEvent {
    PluginEvent::Checked(PluginCheck {
        node: "due".to_owned(),
        item: id.to_owned(),
        checked: true,
    })
}

#[test]
fn a_rows_checkbox_completes_its_task_and_the_task_leaves_due_today() {
    let (fixture, system) = busy_day();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    send_due(&mut app, check(&format!("task_{B}")));

    assert_eq!(
        property(&fixture.read("tasks/Pay rent.md"), "status"),
        Some("done".into())
    );
    assert_eq!(due_text(&app, "due-count").as_deref(), Some("6 tasks"));
    assert!(!due_labels(&app).contains(&"Pay rent".to_owned()));
    assert_eq!(due_text(&app, "due-notice"), None);
}

#[test]
fn a_check_against_a_stale_revision_leaves_the_file_and_reloads_the_section() {
    let (fixture, system) = busy_day();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    // Another app moves the due date back after the section drew.
    let outside = task(B, "status: todo\npriority: urgent\ndue: 2026-10-20\n");
    std::fs::write(fixture.root.join("tasks/Pay rent.md"), &outside).unwrap();

    send_due(&mut app, check(&format!("task_{B}")));

    assert_eq!(fixture.read("tasks/Pay rent.md"), outside);
    assert_eq!(
        due_text(&app, "due-notice").as_deref(),
        Some("It changed elsewhere. Showing the latest version.")
    );
    let pay = due_rows(&app)
        .into_iter()
        .find(|row| row.label == "Pay rent")
        .expect("Pay rent still shows");
    assert_eq!(pay.detail.as_deref(), Some("Yesterday"));
    assert_eq!(pay.warning.as_deref(), Some("Overdue"));
    assert_eq!(pay.checked, Some(false));
}

/// The add field's form as Due today draws it.
struct AddForm {
    label: String,
    submit: String,
    /// The inputs that need a name before Add works.
    required: Vec<String>,
    /// The input's node ID and the text it holds.
    input: String,
    text: String,
}

fn add_form(app: &app_core::App) -> AddForm {
    let view = due_view(app);
    let form = find(view.root(), "add-today").expect("Due today has the add field");
    let ViewKind::Form {
        label,
        submit,
        required,
    } = &form.kind
    else {
        panic!("the add field isn't a form: {:?}", form.kind);
    };
    let [input] = form.children.as_slice() else {
        panic!("the add form holds {} nodes", form.children.len());
    };
    let ViewKind::Input {
        label: input_label,
        value: InputValue::Text(text),
        ..
    } = &input.kind
    else {
        panic!("the add form's input isn't text: {:?}", input.kind);
    };
    assert_eq!(input_label, "Task");
    AddForm {
        label: label.clone(),
        submit: submit.clone(),
        required: required.clone(),
        input: input.id.clone(),
        text: text.clone(),
    }
}

fn add(app: &mut app_core::App, input: &str, name: &str) {
    send_due(
        app,
        PluginEvent::Submitted(Submission {
            node: "add-today".to_owned(),
            fields: vec![Field {
                node: input.to_owned(),
                value: InputValue::Text(name.to_owned()),
            }],
        }),
    );
}

#[test]
fn the_add_field_creates_a_task_due_on_the_devices_date_then_clears() {
    let fixture = Fixture::new(&[(
        "tasks/Read the manual.md",
        &task(F, "status: todo\npriority: medium\n"),
    )]);
    // 23:30 on 21 October in Toronto is already the 22nd in UTC.
    let system = FakeSystem::new();
    system.set_clock("2026-10-22T03:30:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    assert_eq!(
        due_text(&app, "nothing-due").as_deref(),
        Some("Nothing due today.")
    );

    let form = add_form(&app);
    assert_eq!(form.label, "Add a task for today");
    assert_eq!(form.submit, "Add");
    // Add waits for a name.
    assert_eq!(form.required, std::slice::from_ref(&form.input));
    assert_eq!(form.text, "");

    add(&mut app, &form.input, "Buy seeds");

    let bytes = fixture.read("tasks/Buy seeds.md");
    assert_eq!(property(&bytes, "due"), Some("2026-10-21".into()));
    assert_eq!(property(&bytes, "status"), Some("todo".into()));
    assert_eq!(due_labels(&app), ["Buy seeds"]);
    assert_eq!(due_text(&app, "due-count").as_deref(), Some("1 task"));
    // The field starts again, empty.
    let next = add_form(&app);
    assert_ne!(next.input, form.input);
    assert_eq!(next.required, std::slice::from_ref(&next.input));
    assert_eq!(next.text, "");
}

#[test]
fn an_add_that_fails_keeps_the_field_and_says_why() {
    // A file named `tasks` stands where the tasks folder goes.
    let fixture = Fixture::new(&[("tasks", b"in the way")]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let before = fixture.snapshot();
    let form = add_form(&app);

    add(&mut app, &form.input, "Buy seeds");

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(add_form(&app).input, form.input);
    let problem = due_text(&app, "add-problem").expect("Due today says why");
    assert!(
        problem.starts_with("Couldn't add \u{201c}Buy seeds\u{201d}."),
        "{problem}"
    );
}
