# Local core architecture

`crates/local-core` owns the durable workspace boundary. The Tauri host and standalone Model Context Protocol (MCP) server call the same services.

```text
WorkspaceEngine
├── safe path resolver and cross-process lock
├── Markdown parser and canonical serializer
├── atomic writer and workspace trash
├── SQLite full-text search derived index
├── scanner and rebuild path
├── watcher, self-write journal, and reconciliation
└── provider configuration and OS credential store
```

## Preserve durable writes

The engine writes files before it updates SQLite. An index failure after a file commit returns `repair-pending`. The next reconciliation rebuilds the derived projection. Startup reconciliation and the rebuild command work without prior SQLite state.

Chat operations update two canonical Markdown files. The engine first stages both target byte sequences in `.noura/chat-mutations/`. Each staged record includes the expected base revisions. The engine then replaces and indexes both files before emitting the mutation event.

Startup and reconciliation replay pending intents when the files match recorded base or target revisions. A divergent external edit remains unchanged. The pending intent remains available for conflict resolution.

## Reconcile external changes

The watcher coalesces native paths and treats each event as a hint. It suppresses an application write when the observed bytes match the write journal. Other paths trigger a filesystem scan, index transaction, and typed event. Duplicate events and editor replacement files do not assign identity to a path.

`workspace.yaml` remains outside the object index. Its watcher adopts the on-disk manifest behind the `manifest_update` write lock. The update journals its atomic write like other write sites. The watcher emits `workspace:manifest-updated`, so runtimes reread the file while the app is open.

An invalid partial manifest edit keeps the last valid snapshot. A later filesystem event retries the read.

## Refresh desktop projections

Desktop projections share one refresh lifecycle. Object mutations, external changes, index updates, workspace readiness, and manifest changes schedule a coalesced typed-client read.

The lifecycle serializes reads and filters events by workspace ID. It rereads when the window regains focus or visibility. A failed background read leaves the last successful projection on screen. Editors own revision and conflict state, so projection refreshes cannot replace active drafts.

## Query Calendar intervals

Calendar projects the `due`, `date`, and `start` properties without changing their source files. Queries use half-open ranges. A valid `end` after `start` creates an interval. The query returns that entry when its interval overlaps the requested range.

Missing, malformed, mixed-format, and non-increasing ends create point entries. Timed values compare as instants. Date-only values compare as civil dates. A date-only point occupies one day.

Calendar results sort by start, title, source ID, and property.

## Keep plugin state disposable

Plugins and user interface code cannot access `IndexStore`. They use generic object, search, Calendar, event, command, and local-storage capabilities. First-party domains validate values before the engine serializes frontmatter.

The [plugin runtime](plugin-runtime.md) activates first-party domains and enforces capabilities. It also reconciles active plugins with `workspace.yaml`. `plugin_state` IDs in the disposable index hold cache state only. Durable plugin data stays in workspace files.
