# Plugin runtime architecture

Noura’s first-party notes, tasks, calendar, projects, folders, and AI plugins use shared capability contracts. `packages/plugin-sdk` defines those contracts, `packages/workspace` adapts them to the typed client, and `apps/app` constructs the runtime in the WebView:

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

`syncWithManifest` treats `.noura/workspace.yaml` as authoritative and reconciles the active host set in this order:

1. Read `enabled_plugins` from the durable manifest. Preserve unknown IDs in the manifest without activating them.
2. Deactivate active plugins whose IDs are absent from `enabled_plugins`.
3. Activate enabled first-party plugins that are not active.

The desktop runtime reconciles at startup and after `workspace:ready`, `workspace:manifest-updated`, or `file:changed` events. The engine adopts external manifest edits and emits `workspace:manifest-updated` with source `external`. Its atomic-write journal suppresses notifications for its own writes.

Settings updates `enabled_plugins` through the engine’s atomic, revision-checked `manifest_update` operation. Navigation and route guards follow the reconciled plugin state.

Plugin definitions may implement `deactivate(context)`. The host supplies the activation context, and capability disposers unregister commands, remove AI tools and context, and unsubscribe events. Closing a workspace deactivates its plugins and removes their registrations.

## Capabilities

`PluginContext` exposes the capabilities declared by the plugin manifest:

- `workspace.files` and `workspace.objects`
- `workspace.search` and `workspace.commands`
- `workspace.events` and `workspace.storage`
- `ai.tools` and `ai.context`

Every call checks the declared capability before it reaches a service. Plugins never see transport internals, SQLite, or Rust types.

## Storage contract

`plugin_state` rows live in the disposable per-device index. The runtime contract is:

- Plugin state is **cache state only** (view selections, scroll positions, projection hints). Deleting `index.sqlite` or rebuilding the index may erase it.
- Durable plugin data must remain in workspace files through `workspace.objects` and `workspace.files`, so plugins can disappear while the information survives. This mirrors the master rule: plugins add functionality without taking ownership of data.
- The host namespaces storage keys by manifest id; plugins pass plain keys.

## Boundaries

Keep dependency and transport responsibilities separate:

- `packages/workspace` may import first-party plugins; plugins never import `packages/workspace`.
- The MCP server does not construct a plugin runtime. Plugin-contributed MCP tools are future work behind the same capability contracts.

## UI contributions

Plugins do not yet contribute rich UI such as trees, boards, or custom panes. The shell uses the per-module sidebar registry in `apps/app/src/lib/sidebar-modules.ts`; each module defines sidebar content for its routes or opts out so its routes render full width.

The registry includes a module only when its plugin is enabled. Sidebar sections read module-owned stores: the file browser reads the folders-gated tree, while task views read the shared tasks projection. A future `workspace.views.register` capability can replace these hardcoded entries without changing the renderer.
