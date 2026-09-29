# Plugin runtime architecture

Noura’s first-party notes, tasks, calendar, projects, folders, AI, and sync plugins use shared capability contracts. `packages/plugin-sdk` defines those contracts, `packages/workspace` adapts them to the typed client, and `apps/app` constructs the runtime in the WebView:

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
- `workspace.collaboration` (trusted)

Every call checks the declared capability before it reaches a service. Plugins never see transport internals, SQLite, or Rust types.

A trusted capability reaches native services that handle credentials or encrypted workspace keys. The host grants it only to plugin definitions passed in `PluginHostOptions.trustedPlugins`, matched by object identity, so a copy of a trusted definition is untrusted. `PluginRuntime` trusts the bundled first-party plugins. Activating an untrusted plugin that declares a trusted capability fails with `plugin_capability_untrusted`, and the context guard rejects calls with the same code.

## Storage contract

`plugin_state` rows live in the disposable per-device index. The runtime contract is:

- Plugin state is **cache state only** (view selections, scroll positions, projection hints). Deleting `index.sqlite` or rebuilding the index may erase it.
- Durable plugin data must remain in workspace files through `workspace.objects` and `workspace.files`, so plugins can disappear while the information survives. This mirrors the master rule: plugins add functionality without taking ownership of data.
- The host namespaces storage keys by manifest id; plugins pass plain keys.

## Host-backed plugins: sync

The `sync` plugin (`plugins/sync`) turns encrypted sync and live collaboration on for one workspace replica. Unlike the domain plugins, most of its behavior lives in the native host, so the TypeScript runtime and the Rust host both read the same switch: `sync` in `enabled_plugins`.

### Decision

Sync is opt-in per replica. New local workspaces and browser workspaces start with it off. A replica created by joining a synchronized workspace starts with it on. `.noura` is never synchronized, so turning sync off on one device leaves other devices unchanged.

Reasons:

- A local-only workspace should not run a sync loop, read the credential store, or ask the native side about collaboration. With the plugin off, none of that happens.
- The manifest is canonical workspace state. Using it for the switch keeps SQLite as a cache (`plugin_state` rows are never the source of truth) and lets an external edit of `.noura/workspace.yaml` turn sync on or off.
- The MCP server has no plugin runtime. Reading the manifest natively keeps its object routing in line with the desktop app.

### Capability

The plugin declares the trusted `workspace.collaboration` capability. On activation it registers `context.collaboration.service`, the host’s typed collaboration service, as the workspace’s collaboration provider. `PluginRuntime.collaboration` holds that provider in a `CollaborationProviderSlot` with at most one entry and a generation counter.

Editors reach collaboration only through the slot (`apps/app/src/lib/editor/collaboration-access.ts`). With an empty slot they open documents as plain local files and send no collaboration request. The host removes the provider before an asynchronous `deactivate` runs, and the notes and projects views reopen editors when the generation changes.

The manifest supports desktop, mobile, and the web. Browsers activate it with no capabilities (`activationCapabilities.web` is empty), so the browser slot stays empty; browser sync has its own settings and runs only while the plugin is on.

### Native gating

`crates/local-core` exposes `SYNC_PLUGIN_ID`, `WorkspaceEngine::sync_plugin_enabled()` (from the in-memory manifest), and `require_sync_plugin(operation)`. With the plugin off, every workspace entry point of `WorkspaceSyncCoordinator` fails with this error:

| Field       | Value                                                    |
| ----------- | -------------------------------------------------------- |
| `code`      | `sync_plugin_disabled`                                   |
| `category`  | `validation`                                             |
| `retryable` | `false`                                                  |
| `message`   | `Turn on the Sync plugin for this workspace to use sync` |
| `details`   | `{ "pluginId": "sync" }`                                 |

The desktop host applies the same check to `sync_workspace_*` commands and to opening, submitting, flushing, and presence for collaboration sessions. Closing a session, account commands, the service configuration, listing remote workspaces, and joining stay available.

The desktop sync loop parks while no workspace is open or its plugin is off. A parked loop clears the live sync status and realtime handle, closes collaboration sessions, and waits for a wake-up without a timer, configuration read, credential access, or network request. The `workspace:manifest-updated` event wakes it and cancels a running pass.

Object create, update, move, and delete share one routing rule in `local-core` (`sync::route_*_object`), used by the desktop host and the MCP server. A mutation takes the collaboration path only while the plugin is on and the workspace or object collaborates. The device connection is read only on that path.

### Migration

Workspaces that set up sync before the plugin existed keep syncing. When a workspace with `.noura/sync/config.json` opens, including a paused one, the engine adds `sync` to `enabled_plugins` once and writes the marker `.noura/sync/plugin.json`. The marker keeps a later “off” in place.

If `sync` is already enabled but the marker is missing, as after a crash between the two writes, the engine writes only the marker. A failed migration goes to the diagnostic log and the workspace still opens. Saving a sync configuration also writes the marker.

Browser workspaces follow the same rule. A workspace with a binding under `.noura-adapter/browser-sync/bindings` turns the plugin on once, and a marker under `.noura-adapter/browser-sync/plugin` records it. Writing a binding also writes the marker.

### Settings

`apps/app/src/lib/settings-sections.ts` lists settings sections and their gating. Sync and People appear only while the plugin is enabled. Account also appears while the device is signed in, so you can always sign out.

The app reads sign-in state at startup only when the plugin is on; otherwise Settings reads it when it opens. Turning Sync off asks for confirmation and first saves pending drafts. If a draft can’t be saved, the plugin stays on.

## Boundaries

Keep dependency and transport responsibilities separate:

- `packages/workspace` may import first-party plugins; plugins never import `packages/workspace`.
- The MCP server does not construct a plugin runtime. Plugin-contributed MCP tools are future work behind the same capability contracts.

## UI contributions

Plugins do not yet contribute rich UI such as trees, boards, or custom panes. The shell uses the per-module sidebar registry in `apps/app/src/lib/sidebar-modules.ts`; each module defines sidebar content for its routes or opts out so its routes render full width.

The registry includes a module only when its plugin is enabled. Sidebar sections read module-owned stores: the file browser reads the folders-gated tree, while task views read the shared tasks projection. A future `workspace.views.register` capability can replace these hardcoded entries without changing the renderer.
