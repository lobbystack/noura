# Plugin runtime architecture

The plugin platform layers the workspace on one kernel: `packages/plugin-sdk` defines capability contracts, `packages/workspace` adapts them to the typed client, and `apps/app` constructs the runtime inside the webview. First-party domains (`plugins/notes`, `tasks`, `calendar`, `projects`, `folders`) dogfood the same public surface a future ecosystem plugin receives.

```text
apps/app state
   │
PluginRuntime (packages/workspace)
   │  syncWithManifest(): reads .noura/workspace.yaml on disk
PluginHost (packages/plugin-sdk)
   │  activate / deactivate, capability guards
PluginContext ── capability-gated facade over NouraClient
```

## Activation lifecycle

- Similar to any other file, `.noura/workspace.yaml` is authoritative. The runtime never caches plugin state; `syncWithManifest` re-reads the manifest and reconciles the active set.
- A plugin activates only when its manifest id appears in `enabled_plugins`. Unknown ids in the manifest are ignored, so a workspace carrying ecosystem plugins opens on older builds.
- Plugin definitions may implement `deactivate(context)`. The host passes the same context instance the plugin saw during activation, so handlers and disposers captured then stay valid. Deactivation runs commands unregistering, AI tool/context removal, and event unsubscription through the disposers the capabilities already return.
- The desktop app re-syncs on startup, after `workspace:ready`, after `workspace:manifest-updated`, and on `file:changed`. The engine observes external edits to `.noura/workspace.yaml` through its watcher: the atomic-write journal suppresses the engine's own writes, and a genuine external change adopts the file into the engine snapshot and emits `workspace:manifest-updated` with source `external`, so plugin state follows the file even while the app is open. The Settings panel writes `enabled_plugins` through the engine's `manifest_update` (atomic, revision-checked against `updated`), so live deactivation and activation run through the same durable file mutation. Closing a workspace deactivates every plugin instead of leaving commands registered against a workspace that is gone. Navigation and route guards derive from the reconciled state: modules that are off genuinely simplify the workspace.

## Capabilities

`PluginContext` exposes only what the plugin manifest declares: `workspace.files`, `workspace.objects`, `workspace.search`, `workspace.commands`, `workspace.events`, `workspace.storage`, `ai.tools`, and `ai.context`. Every call guards against undeclared capabilities before it reaches a service. Plugins never see transport internals, SQLite, or Rust types.

## Storage contract

`plugin_state` rows live in the disposable per-device index. The runtime contract is:

- Plugin state is **cache state only** (view selections, scroll positions, projection hints). Deleting `index.sqlite` or rebuilding the index may erase it.
- Durable plugin data must remain in workspace files through `workspace.objects` and `workspace.files`, so plugins can disappear while the information survives. This mirrors the master rule: plugins add functionality without taking ownership of data.
- The host namespaces storage keys by manifest id; plugins pass plain keys.

## Boundaries

- `packages/workspace` may import first-party plugins; plugins never import `packages/workspace`.
- The MCP server does not construct a plugin runtime. Plugin-contributed MCP tools are future work behind the same capability contracts.
- Rich UI contributions from plugins (a tree, boards, custom panes) do not exist yet. Shell-level surfaces are organized behind the **per-module sidebar registry** (`apps/app/src/lib/sidebar-modules.ts`): each module decides what lives in the sidebar for its routes — or opts out entirely, in which case those routes render full-width. Contributions are gated by the module's plugin being enabled, and sections read module-owned stores (the file browser reads the folders-gated tree, the tasks views read the shared tasks projection). A `workspace.views.register`-style capability is the intended upgrade path so plugins can own their own UI; the registry shape is designed so those entries can replace the hardcoded ones without changing the renderer.
