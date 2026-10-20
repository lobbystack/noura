//! The layering rules, checked against `cargo metadata` so CI fails when a
//! change breaks them:
//!
//! - `app-core` depends on no UI crate, directly or through another crate,
//!   so a mobile client can reuse it.
//! - `app-ui` reaches workspace behavior only through `app-core`: it names
//!   no other workspace crate as a dependency.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

/// Workspace crates `app-ui` may depend on.
const APP_UI_MAY_USE: &[&str] = &["app-core"];

fn is_ui_crate(name: &str) -> bool {
    name.starts_with("egui")
        || name.starts_with("eframe")
        || name.starts_with("epaint")
        || name.starts_with("accesskit")
        || name.starts_with("winit")
        || name.starts_with("wgpu")
        || matches!(
            name,
            "app-ui" | "editor-egui" | "desktop" | "glow" | "kittest" | "muda" | "rfd"
        )
}

struct Graph {
    names: HashMap<String, String>,
    members: HashSet<String>,
    /// Each package's dependencies with whether any use is a normal or
    /// build dependency (as opposed to dev-only).
    deps: HashMap<String, Vec<(String, bool)>>,
}

fn metadata() -> Graph {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
            manifest,
        ])
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            (
                package["id"].as_str().unwrap().to_owned(),
                package["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let members = value["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    let deps = value["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            let deps = node["deps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dep| {
                    let shipped = dep["dep_kinds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|kind| kind["kind"].as_str() != Some("dev"));
                    (dep["pkg"].as_str().unwrap().to_owned(), shipped)
                })
                .collect();
            (node["id"].as_str().unwrap().to_owned(), deps)
        })
        .collect();
    Graph {
        names,
        members,
        deps,
    }
}

impl Graph {
    fn member(&self, name: &str) -> &str {
        self.members
            .iter()
            .find(|id| self.names[*id] == name)
            .unwrap_or_else(|| panic!("{name} is a workspace member"))
    }

    /// The path from `from` to the first shipped dependency matching
    /// `forbidden`, if any.
    fn path_to(&self, from: &str, forbidden: impl Fn(&str) -> bool) -> Option<Vec<String>> {
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        let mut seen = HashSet::from([from]);
        while let Some(id) = queue.pop_front() {
            for (dep, shipped) in &self.deps[id] {
                if !shipped || !seen.insert(dep.as_str()) {
                    continue;
                }
                parent.insert(dep.as_str(), id);
                if forbidden(&self.names[dep]) {
                    let mut path = vec![self.names[dep].clone()];
                    let mut cursor = id;
                    while cursor != from {
                        path.push(self.names[cursor].clone());
                        cursor = parent[cursor];
                    }
                    path.push(self.names[from].clone());
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(dep.as_str());
            }
        }
        None
    }
}

#[test]
fn app_core_depends_on_no_ui_crate() {
    let graph = metadata();
    let app_core = graph.member("app-core");

    let path = graph.path_to(app_core, is_ui_crate);

    assert!(
        path.is_none(),
        "app-core must not depend on a UI crate, but it reaches one: {}",
        path.unwrap_or_default().join(" -> ")
    );
}

#[test]
fn app_ui_uses_no_workspace_crate_but_app_core() {
    let graph = metadata();
    let app_ui = graph.member("app-ui");

    let direct: Vec<&str> = graph.deps[app_ui]
        .iter()
        .filter(|(dep, _)| graph.members.contains(dep))
        .map(|(dep, _)| graph.names[dep].as_str())
        .filter(|name| !APP_UI_MAY_USE.contains(name))
        .collect();

    assert!(
        direct.is_empty(),
        "app-ui reaches workspace behavior only through app-core, but it depends on {direct:?}"
    );
}

#[test]
fn the_rules_catch_a_ui_crate_in_the_graph() {
    let graph = metadata();
    let desktop = graph.member("desktop");

    let path = graph.path_to(desktop, |name| name == "egui").unwrap();

    assert_eq!(path.first().map(String::as_str), Some("desktop"));
    assert_eq!(path.last().map(String::as_str), Some("egui"));
}
