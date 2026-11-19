//! The recipes test plugin in a running app: it owns the `recipe` type, and
//! its page runs the object call a pressed node ID names, as the plugin's
//! crate documentation lists them.

use app_core::App;

use super::plugins::{self, PluginFolder};

/// The recipes plugin's page.
pub const PAGE: &str = "plugin:recipes:recipes";

/// The ID suffixes the tests' recipes use.
pub const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
pub const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";

/// The answer when a plugin calls about a type it doesn't hold.
pub const REFUSED: &str = "error\tcapability-not-declared\tobject-types";

/// A recipe's file, as another app or an earlier session wrote it.
pub fn recipe_file(suffix: &str, properties: &str, body: &str) -> Vec<u8> {
    format!(
        "---\nid: recipe_{suffix}\ntype: recipe\n{properties}created: 2026-10-20T10:00:00Z\n\
         updated: 2026-10-20T10:00:00Z\n---\n{body}"
    )
    .into_bytes()
}

/// One object as the plugin's page reports it.
#[derive(Debug, Clone)]
pub struct Answer {
    pub id: String,
    pub revision: String,
    pub path: String,
    /// The first schema problem as `property:code`, or `-`.
    pub problem: String,
    pub properties: serde_json::Value,
    pub body: String,
}

/// The recipes plugin installed and turned on in `app`'s workspace, with
/// its types registered, or `None` when it isn't built. The folder lasts
/// as long as the guard.
pub fn with_recipes(app: &mut App) -> Option<PluginFolder> {
    let folder = PluginFolder::recipes("recipes")?;
    plugins::install(app, folder.path());
    assert!(app.set_plugin_enabled("recipes", true));
    super::settle(app);
    assert!(
        app.plugins()
            .iter()
            .any(|row| row.id == "recipes" && row.running),
        "{:?}",
        app.plugins()
    );
    Some(folder)
}

/// Run `call` on the recipes page, wait for the plugin to answer on its
/// thread, and return the answer's lines.
pub fn call(app: &mut App, call: &str) -> Vec<String> {
    app.press_plugin_button(PAGE, call);
    super::settle(app);
    plugins::texts(app, PAGE).expect("the page draws")
}

/// Run `request`, which answers with one object.
pub fn object(app: &mut App, request: &str) -> Answer {
    let lines = call(app, request);
    assert_eq!(lines.len(), 1, "{request}: {lines:?}");
    parse(&lines[0]).unwrap_or_else(|| panic!("{request}: {}", lines[0]))
}

/// The objects `list|recipe` answers, by path.
pub fn listed(app: &mut App) -> Vec<Answer> {
    let mut listed: Vec<Answer> = call(app, "list|recipe")
        .iter()
        .map(|line| parse(line).unwrap_or_else(|| panic!("{line}")))
        .collect();
    listed.sort_by(|a, b| a.path.cmp(&b.path));
    listed
}

pub fn parse(line: &str) -> Option<Answer> {
    let fields: Vec<&str> = line.split('\t').collect();
    let ["object", id, revision, path, problem, properties, body] = fields.as_slice() else {
        return None;
    };
    Some(Answer {
        id: (*id).to_owned(),
        revision: (*revision).to_owned(),
        path: (*path).to_owned(),
        problem: (*problem).to_owned(),
        properties: serde_json::from_str(properties).unwrap(),
        body: serde_json::from_str(body).unwrap(),
    })
}

/// The content revision of a managed file's bytes.
pub fn revision(bytes: &[u8]) -> String {
    match local_core::parse_markdown("x.md", bytes) {
        local_core::ParsedMarkdown::Managed(object) => object.revision,
        other => panic!("not a managed file: {other:?}"),
    }
}
