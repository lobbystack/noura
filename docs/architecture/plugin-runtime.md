# Plugin runtime architecture

Noura's tasks, projects, calendar and AI features are first-party plugins. They build to WebAssembly components against the same `noura:plugin` interface as third-party plugins, and they run under the same capability checks. This page describes how the desktop app loads and checks them. To write a plugin, read the [TypeScript author guide](../plugins/README.md) or the [Rust author guide](../plugins/rust.md).

```text
crates/app-ui        draws plugin views with the widget kit
   │
crates/app-core      plugin service: installs, approvals, enabled list, one thread per plugin
   │
crates/plugin-host   Wasmtime: compiles, starts and checks each call
   │
plugin component     built against crates/plugin-sdk/wit/plugin.wit
```

## Load and run a plugin

A plugin is a folder that holds `plugin.toml` and a WebAssembly component. You install one from a folder, which is the dev mode that reloads each new build, or from a release on GitHub. `crates/plugin-host` compiles the component on your device and starts it with the capabilities you approved.

Each call runs under these limits:

- **Memory**: 256 MiB per plugin
- **Time**: 1 second of the plugin's own work, and 120 seconds in all
- **Imports**: only `noura:plugin` interfaces. WASI imports trap, so a plugin gets no filesystem or sockets of its own.
- **Failures**: a failed call restarts the plugin's store, and three failures in a short window stop the plugin until Noura restarts

`crates/app-core` runs each plugin's calls on a thread of its own, so a plugin that loops or crashes never holds up a window. The four plugins that ship with Noura, `ai`, `calendar`, `projects` and `tasks`, live in `crates/plugins/` and wait for one approval on each device's first-run screen.

## Turn plugins on and off

`enabled_plugins` in `.noura/workspace.yaml` lists the plugins a workspace turns on. When you open a workspace, `app-core` stops every running plugin, then starts each enabled plugin that this device installed and approved. A plugin that the workspace enables but this device lacks appears in Settings > Plugins and runs nothing.

Turning a plugin on or off in Settings writes `enabled_plugins` through the engine's revision-checked `manifest_update`, then starts or stops the plugin. The engine adopts an outside edit of the manifest and emits `workspace:manifest-updated`. Running plugins change only when you open a workspace, turn a plugin on or off, or install or update one. A tool call checks the manifest again and refuses to run a plugin the workspace turned off.

Uninstalling a plugin removes its code and approvals from the device and leaves the workspace's `enabled_plugins` alone.

## Capabilities

`plugin.toml` declares capabilities as `[[capabilities]]` tables. The names are `workspace-read`, `views`, `object-types`, `network` with `hosts`, `commands` with optional `targets`, `inbox-sections`, `settings`, `tools` and `ai`. The [author guide](../plugins/README.md#declare-what-the-plugin-needs) says what each one allows.

You approve each capability, network host and command target when you install a plugin, and the install dialog lists them before any code runs. Approvals live in this device's app data, never in workspace files, so a workspace that lists a plugin runs none of its code on a device where you haven't approved it. An update that asks for more waits for your approval, and you can revoke any grant in Settings > Plugins.

`plugin-host` checks the grant on every import a plugin calls and before Noura calls one of the plugin's exports. A call without a grant fails with `capability-not-declared` and names the capability. The `objects` interface also reaches only the object types the plugin registered.

## Storage contract

Durable plugin data lives in workspace files. A plugin writes objects through the `objects` interface as Markdown files with a stable ID, frontmatter checked against the type's schema, and revision-checked durable writes. Plugins can't write under `.noura`, `.git`, `node_modules` or `target`. A plugin keeps its settings in workspace files too.

The index still has a `plugin_state` table for disposable cache state, but no plugin interface reaches it.

## UI contributions

Plugins describe their UI as a tree of nodes, and `crates/app-ui` draws the nodes with the widget kit and the theme's tokens. Through the `view` interface a plugin adds pages to the rail and a sidebar section above the file tree. Through `settings`, `inbox`, `object-types` and `commands` it adds a Settings page, Inbox sections, a properties panel and palette commands. `plugin-host` checks each tree before Noura draws it, with at most 10,000 nodes and 64 levels.

The `navigation` interface opens pages and objects only in answer to something you did.

## MCP

`crates/mcp-server` offers the kernel's tools plus `<plugin>.<tool>` for each enabled plugin's tools, such as `tasks.list`. It starts the plugins through `app-core`'s plugin service with this device's approvals, and drops a plugin's tools when the workspace turns the plugin off.

## Legacy sync switch

The legacy app kept a `sync` plugin that turned v1 sync on per workspace. `crates/local-core` still holds its gate: `SYNC_PLUGIN_ID`, `require_sync_plugin`, the `sync_plugin_disabled` error and the `.noura/sync/plugin.json` marker. The desktop app doesn't call it, and no Rust plugin has the ID `sync`. You turn on v2 sync in Settings > Sync and devices.
