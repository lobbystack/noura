# Write a Noura plugin in Rust

A Rust plugin is a `cdylib` crate that you build for `wasm32-wasip2`, and it runs in the same sandbox as a TypeScript plugin. `plugin-sdk` holds the interface and the Rust library. The [TypeScript guide](README.md) explains capabilities, limits, release archives and interface versions, and those apply to Rust too.

You need Rust 1.99 or later and the WebAssembly target:

```sh
rustup target add wasm32-wasip2
```

## Start a plugin

`plugin-sdk` isn't on crates.io yet, so your plugin depends on a clone of this repository.

1. Clone this repository next to your plugin, then copy the template:

   ```sh
   git clone https://github.com/lobbystack/noura
   cp -r noura/crates/plugin-sdk/template word-count
   cd word-count
   mv gitignore .gitignore
   ```

2. Rename the plugin. In `Cargo.toml`, set `name`. In `plugin.toml`, set `id` and `name`. Build it again whenever you change either file.

3. Build the plugin and copy the component to the path that `entry` names. Cargo turns the hyphen in the crate's name into an underscore:

   ```sh
   cargo build --release --target wasm32-wasip2
   mkdir -p dist
   cp target/wasm32-wasip2/release/word_count.wasm dist/plugin.wasm
   ```

4. In Noura, open Settings > Plugins, choose Install from folder and pick the `word-count` folder. Noura lists the capabilities the plugin asks for before it runs any of its code. Approve them, and the plugin's page appears in the rail.

Noura watches the folder you installed from, so it reloads the plugin after you copy a new build.

## What the template holds

`Cargo.toml` builds a `cdylib` and keeps debug info out of the component:

```toml
[package]
name = "word-count"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
plugin-sdk = { path = "../noura/crates/plugin-sdk" }

[profile.release]
opt-level = "s"
strip = "debuginfo"

# Keeps this project out of any Cargo workspace above it.
[workspace]
```

`plugin.toml` is the manifest. The [TypeScript guide](README.md#declare-what-the-plugin-needs) lists each field and capability:

```toml
id = "word-count"
name = "Word count"
version = "0.1.0"
interface-version = "0.1.0"
entry = "dist/plugin.wasm"
platforms = ["desktop"]

[[capabilities]]
name = "workspace-read"

[[capabilities]]
name = "views"
```

`src/lib.rs` is the plugin. It implements `Plugin` and names its type once with `export!`:

```rust
use std::sync::atomic::{AtomicU32, Ordering};

use plugin_sdk::{Event, Node, Page, Plugin, View, workspace};

static PRESSES: AtomicU32 = AtomicU32::new(0);

struct WordCount;

impl Plugin for WordCount {
    fn pages() -> Vec<Page> {
        vec![Page {
            id: "main".into(),
            title: "Word count".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        let files = workspace::list_entries().map_or(0, |entries| entries.len());
        let presses = PRESSES.load(Ordering::Relaxed);
        let mut view = View::panel("root", Some("Word count"));
        view.text(View::ROOT, "files", &format!("Files: {files}"));
        view.text(View::ROOT, "presses", &format!("Presses: {presses}"));
        view.button(View::ROOT, "press", "Press me");
        view.into_nodes()
    }

    fn handle_event(_page: String, event: Event) {
        if matches!(event, Event::Pressed(node) if node == "press") {
            PRESSES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

plugin_sdk::export!(WordCount);
```

Each method of `Plugin` has a default, so you write only the ones for the capabilities in your manifest. Each capability interface you call has a module, such as `workspace`, `objects` and `network`. The interface pages under [`interfaces/`](interfaces/) describe each one, and their examples use TypeScript names. Run `cargo doc --open -p plugin-sdk` in the clone to read the Rust names.

## Package it for release

Zip `plugin.toml` and the component into `<id>-<version>.noura-plugin.zip`, with `plugin.toml` at the root and the component at the path `entry` names:

```sh
zip word-count-0.1.0.noura-plugin.zip plugin.toml dist/plugin.wasm
```

Attach the archive to a GitHub release, then paste the release's address under Install from GitHub in Settings > Plugins.

CI copies this template to a folder outside the repository, builds it, and runs it in Noura's plugin host.
