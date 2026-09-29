# Local core architecture

`crates/local-core` owns the durable workspace boundary. The Tauri host and standalone Model Context Protocol (MCP) server call the same services.

```text
WorkspaceEngine
├── safe path resolver and cross-process lock
├── Markdown parser and canonical serializer
├── atomic writer and trash
├── SQLite full-text search derived index
├── scanner and rebuild path
├── watcher, self-write journal, and reconciliation
└── provider configuration and OS credential store
```

## Source layout

`engine.rs` defines `WorkspaceEngine` and opens or creates a workspace. Each domain adds its methods in its own `impl WorkspaceEngine` block under `src/engine/`:

| Module | Contents |
| --- | --- |
| `objects.rs` | Create, adopt, read, update, move, and delete managed objects |
| `drafts.rs` | Editor draft merges, conflict resolution, and recovery snapshots |
| `raw_markdown.rs` | Markdown files without a managed ID, saved as raw text |
| `chats.rs`, `chat_journal.rs` | Chats, messages, and the two-file mutation journal |
| `files.rs`, `copy.rs`, `trash.rs` | Folders, ordinary files, PDFs, local assets, copies, and deletes |
| `search.rs` | Object queries, full-text search, and the calendar |
| `manifest.rs` | `.noura/workspace.yaml` and plugin state |
| `scan.rs`, `reconcile.rs` | Workspace walks, index rebuilds, and external-change reconciliation |
| `durable.rs` | Path normalization, atomic writes, and fsync helpers |
| `sync.rs`, `sync/` | The encrypted sync replica, with collaborative documents in `sync/collaboration/` |

`src/ai.rs` holds the AI provider and consent API. The `genai` adapter, stream operations, and input checks live in `src/ai/`.

The desktop host in `src-tauri/src/lib.rs` holds the shared app state and `run()`. Its Tauri commands live in `src-tauri/src/commands/`, one module per domain, and call these engine methods.

## Delete to the system trash

Deleting a note, task, or project moves its file to the operating system trash: the macOS Trash, the Windows Recycle Bin, or the freedesktop.org trash on Linux. People restore it there like any other file, so the app has no restore screen of its own. Expired chats follow the same path.

Some systems have no trash, and some volumes refuse the move. In that case the engine moves the file to `.noura/trash/<timestamp>/` in the workspace, so a delete never destroys data. On macOS the engine calls `NSFileManager`, because the Finder method asks for permission to control Finder.

The desktop app and the MCP server turn the system trash on with `WorkspaceEngine::set_system_trash`. Other embedders and the tests use `.noura/trash` unless they opt in. Collaboration deletes keep their own recoverable copies under `.noura/trash/collaboration/`, because sync recovery reads them back.

## Duplicate a file

`WorkspaceEngine::copy_file` copies a file to a path that doesn't exist yet and refuses to overwrite. A copy of a note, task, or project gets a new ID and takes its title from the new file name, so both files keep their own identity. Other files, including Markdown without an ID, copy byte for byte. The copy reads the file on disk, so an edit the index hasn't seen yet still reaches the copy.

## Preserve durable writes

The engine writes files before it updates SQLite. An index failure after a file commit returns `repair-pending`. The next reconciliation rebuilds the derived projection. Startup reconciliation and the rebuild command work without prior SQLite state.

Chat operations update two canonical Markdown files. The engine first stages both target byte sequences in `.noura/chat-mutations/`. Each staged record includes the expected base revisions. The engine then replaces and indexes both files before emitting the mutation event.

Startup and reconciliation replay pending intents when the files match recorded base or target revisions. A divergent external edit remains unchanged. The pending intent remains available for conflict resolution.

## Reconcile external changes

The watcher coalesces native paths and treats each event as a hint. It suppresses an application write when the observed bytes match the write journal. Other paths trigger a filesystem scan, index transaction, and typed event. Duplicate events and editor replacement files do not assign identity to a path.

`.noura/workspace.yaml` remains outside the object index. Keeping the manifest under Noura's hidden metadata directory leaves the workspace root entirely for user content. The watcher adopts the on-disk manifest behind the `manifest_update` write lock. The update journals its atomic write like other write sites. The watcher emits `workspace:manifest-updated`, so runtimes reread the file while the app is open.

An invalid partial manifest edit keeps the last valid snapshot. A later filesystem event retries the read.

A walk that can't open a folder or read a file keeps going. The engine keeps the index rows under that path, adds a plain `unreadable_folder` or `unreadable_file` diagnostic to the workspace state, and writes the operating system's reason to the log. An iCloud placeholder or a dataless file keeps its row and lists as not downloaded, because reading it would block on a download. When the watcher can't start, for example when Linux runs out of inotify watches, the workspace runs on the periodic rescan alone.

## Reconcile only what changed

The watcher thread marks the index dirty when a visible path changes, skipping paths this engine just wrote. Reads such as the file list and chat list walk the workspace only while that flag is set or the watcher is down. Autosaves and chat steps check just their own file: they re-read it and re-index it when its revision differs from the index. They walk everything only when the file moved or lost its ID.

One walk updates the index and builds the file list. Reconciliation compares only the stable IDs whose files changed, so a small edit costs the same in a large workspace. Up to 32 object changes go out as `object:created`, `object:updated`, `object:moved`, and `object:deleted` events. A larger batch, such as a git checkout or a sync pull, goes out as one `objects:changed` event. Its `changes` list carries the same payloads plus an `event` field. When the watcher queue overflows, the next poll walks the whole workspace and reports the changed paths in `file:changed` with `rescanned: true`.

The index runs SQLite in WAL mode with `synchronous=NORMAL`: the files stay canonical, and a rebuild restores anything the last transactions lost. A busy timeout lets the MCP server share the index file.

## Refresh desktop projections

Desktop projections share one refresh lifecycle. Object mutations, external changes, index updates, workspace readiness, and manifest changes schedule a coalesced typed-client read.

The lifecycle serializes reads and filters events by workspace ID. It rereads when the window regains focus or visibility. A failed background read leaves the last successful projection on screen. Editors own revision and conflict state, so projection refreshes cannot replace active drafts.

## Query calendar intervals

Calendar projects the `due`, `date`, and `start` properties without changing their source files. Queries use half-open ranges. A valid `end` after `start` creates an interval. The query returns that entry when its interval overlaps the requested range.

Missing, malformed, mixed-format, and non-increasing ends create point entries. Timed values compare as instants. Date-only values compare as civil dates. A date-only point occupies one day.

Calendar results sort by start, title, source ID, and property.

## Keep plugin state disposable

Plugins and user interface code cannot access `IndexStore`. They use generic object, search, Calendar, event, command, and local-storage capabilities. First-party domains validate values before the engine serializes frontmatter.

The [plugin runtime](plugin-runtime.md) activates first-party domains and enforces capabilities. It also reconciles active plugins with `.noura/workspace.yaml`. `plugin_state` IDs in the disposable index hold cache state only. Durable plugin data stays in workspace files.
