//! A test plugin for seams 1 and 3 that owns the `recipe` object type.
//!
//! It registers `recipe`, and also `note`, which Noura keeps for itself,
//! so a test sees that registration refused. Its `recipes` page runs the
//! call a pressed node ID names and shows the answer, one text node per
//! line, with tabs between the fields:
//!
//! - `create|<type>|<title>|<path or ->|<properties>|<body>`
//! - `get|<id>`
//! - `list|<type>`
//! - `update|<id>|<revision>|<properties>`
//! - `body|<id>|<revision>|<body>`
//! - `remove|<id>|<revision>|<property>`
//! - `move|<id>|<revision>|<path>`
//! - `delete|<id>|<revision>`
//!
//! An object comes back as `object`, its ID, revision, path, first schema
//! problem as `property:code` or `-`, its properties as JSON and its body
//! as a JSON string. A deleted one comes back as `deleted` and its ID, and
//! an error as `error`, its kind and what it holds.
//!
//! A recipe's properties panel shows its course, servings and whether it
//! is vegetarian, with a button that switches the last.

use std::sync::Mutex;

use plugin_sdk::{
    Error, Event, Node, ObjectType, Page, Plugin, View,
    objects::{self, Changes, NewObject, Object},
};

/// What the last call on the `recipes` page answered.
static ANSWER: Mutex<Vec<String>> = Mutex::new(Vec::new());

const RECIPE_SCHEMA: &str = r#"{"properties":[
  {"name":"course","kind":{"enum":["starter","main","dessert"]},"default":"main"},
  {"name":"servings","kind":"number","default":2},
  {"name":"vegetarian","kind":"boolean","default":false},
  {"name":"cooked","kind":"date","nullable":true},
  {"name":"source","kind":"string"}
]}"#;

struct Recipes;

impl Plugin for Recipes {
    fn pages() -> Vec<Page> {
        vec![Page {
            id: "recipes".into(),
            title: "Recipes".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        let mut view = View::panel("root", Some("Recipes"));
        let answer = ANSWER.lock().unwrap();
        if answer.is_empty() {
            view.text(View::ROOT, "line-0", "No calls yet.");
        }
        for (index, line) in answer.iter().enumerate() {
            view.text(View::ROOT, &format!("line-{index}"), line);
        }
        view.into_nodes()
    }

    fn handle_event(_page: String, event: Event) {
        let Event::Pressed(call) = event;
        *ANSWER.lock().unwrap() = run(&call);
    }

    fn object_types() -> Vec<ObjectType> {
        vec![
            ObjectType {
                name: "recipe".into(),
                schema: RECIPE_SCHEMA.into(),
            },
            ObjectType {
                name: "note".into(),
                schema: r#"{"properties":[]}"#.into(),
            },
        ]
    }

    fn render_panel(object: String) -> Vec<Node> {
        let mut view = View::panel("recipe", Some("Recipe"));
        match objects::get(&object) {
            Ok(recipe) => {
                let properties = properties(&recipe);
                let course = properties["course"].as_str().unwrap_or("-");
                let servings = properties["servings"].to_string();
                let vegetarian = properties["vegetarian"].as_bool().unwrap_or(false);
                view.text(View::ROOT, "course", &format!("Course: {course}"));
                view.text(View::ROOT, "servings", &format!("Servings: {servings}"));
                view.text(
                    View::ROOT,
                    "vegetarian-now",
                    if vegetarian {
                        "Vegetarian: yes"
                    } else {
                        "Vegetarian: no"
                    },
                );
                if let Some(problem) = &recipe.problem {
                    view.text(
                        View::ROOT,
                        "problem",
                        &format!("{} doesn't fit: {}", problem.property, problem.code),
                    );
                }
                view.button(
                    View::ROOT,
                    "vegetarian",
                    if vegetarian {
                        "Mark not vegetarian"
                    } else {
                        "Mark vegetarian"
                    },
                );
            }
            Err(error) => {
                view.text(View::ROOT, "error", &describe(&error));
            }
        }
        view.into_nodes()
    }

    fn handle_panel_event(object: String, event: Event) {
        let Event::Pressed(node) = event;
        if node != "vegetarian" {
            return;
        }
        let answer = objects::get(&object).and_then(|recipe| {
            let vegetarian = properties(&recipe)["vegetarian"].as_bool().unwrap_or(false);
            objects::update(
                &recipe.id,
                &recipe.revision,
                &Changes {
                    body: None,
                    properties: serde_json::json!({ "vegetarian": !vegetarian }).to_string(),
                    remove: Vec::new(),
                },
            )
        });
        *ANSWER.lock().unwrap() = vec![answered(answer)];
    }
}

fn properties(object: &Object) -> serde_json::Value {
    serde_json::from_str(&object.properties).unwrap_or_default()
}

/// Run the call `call` names, and say what it answered.
fn run(call: &str) -> Vec<String> {
    let parts: Vec<&str> = call.split('|').collect();
    let changes = |properties: &str, body: Option<&str>, remove: Vec<String>| Changes {
        body: body.map(str::to_owned),
        properties: properties.to_owned(),
        remove,
    };
    match parts.as_slice() {
        ["create", object_type, title, path, properties, body] => {
            vec![answered(objects::create(&NewObject {
                object_type: (*object_type).to_owned(),
                title: (*title).to_owned(),
                path: (*path != "-").then(|| (*path).to_owned()),
                body: (*body).to_owned(),
                properties: (*properties).to_owned(),
            }))]
        }
        ["get", id] => vec![answered(objects::get(id))],
        ["list", object_type] => match objects::list(object_type) {
            Ok(objects) => objects.iter().map(line).collect(),
            Err(error) => vec![describe(&error)],
        },
        ["update", id, revision, properties] => vec![answered(objects::update(
            id,
            revision,
            &changes(properties, None, Vec::new()),
        ))],
        ["body", id, revision, body] => vec![answered(objects::update(
            id,
            revision,
            &changes("{}", Some(body), Vec::new()),
        ))],
        ["remove", id, revision, property] => vec![answered(objects::update(
            id,
            revision,
            &changes("{}", None, vec![(*property).to_owned()]),
        ))],
        ["move", id, revision, path] => vec![answered(objects::move_to(id, revision, path))],
        ["delete", id, revision] => vec![match objects::delete(id, revision) {
            Ok(()) => format!("deleted\t{id}"),
            Err(error) => describe(&error),
        }],
        _ => vec![format!("unknown call {call}")],
    }
}

fn answered(result: Result<Object, Error>) -> String {
    match result {
        Ok(object) => line(&object),
        Err(error) => describe(&error),
    }
}

fn line(object: &Object) -> String {
    let problem = object.problem.as_ref().map_or_else(
        || "-".to_owned(),
        |problem| format!("{}:{}", problem.property, problem.code),
    );
    format!(
        "object\t{}\t{}\t{}\t{}\t{}\t{}",
        object.id,
        object.revision,
        object.path,
        problem,
        object.properties,
        serde_json::Value::String(object.body.clone()),
    )
}

fn describe(error: &Error) -> String {
    match error {
        Error::CapabilityNotDeclared(capability) => {
            format!("error\tcapability-not-declared\t{capability}")
        }
        Error::UnsupportedPath => "error\tunsupported-path\t-".to_owned(),
        Error::InvalidPath(path) => format!("error\tinvalid-path\t{path}"),
        Error::NotFound(what) => format!("error\tnot-found\t{what}"),
        Error::Failed(message) => format!("error\tfailed\t{message}"),
        Error::RevisionConflict(id) => format!("error\trevision-conflict\t{id}"),
        Error::IdentityConflict(id) => format!("error\tidentity-conflict\t{id}"),
        Error::InvalidProperty(problem) => format!(
            "error\tinvalid-property\t{}:{}",
            problem.property, problem.code
        ),
        Error::AlreadyExists(path) => format!("error\talready-exists\t{path}"),
    }
}

plugin_sdk::export!(Recipes);
