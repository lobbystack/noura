# Local core architecture

`crates/local-core` implements the durable workspace boundary. The Tauri host and standalone MCP server call the same services.

```text
WorkspaceEngine
├── safe path resolver and cross-process lock
├── Markdown parser and canonical serializer
├── atomic writer and workspace trash
├── SQLite/FTS derived index
├── scanner and rebuild path
├── watcher, self-write journal, and reconciliation
└── provider configuration and OS credential store
```

The engine writes files before it updates SQLite. An index failure after a file commit returns `repair-pending`; the next reconciliation rebuilds the derived projection. Startup reconciliation and the rebuild command work without prior SQLite state.

The watcher coalesces native paths and treats each event as a hint. It suppresses an application write only when the observed bytes match the write journal. Remaining paths trigger a filesystem scan, index transaction, and typed event. This design handles duplicate events and editor temporary-file replacement without assigning identity to a path.

Plugins and UI code cannot access `IndexStore`. They use generic object, search, calendar, event, command, and local-storage capabilities. First-party domains apply validation before the engine serializes frontmatter.
