# Noura

Noura is the open workspace for humans and AI. The Initial MVP stores durable workspace state in ordinary Markdown and binary files. A local SQLite database supplies a disposable index, metadata cache, and full-text search.

## Status

The repository contains the Initial MVP core implementation. The visual application remains a separate workstream.

## Development

Install Bun and Rust 1.91 or newer, then run:

```sh
bun install
bun run check
bun run test
```

Run the desktop development host with `bun run tauri dev` after installing the Tauri platform prerequisites.

## Storage contract

Workspace files are canonical. Paths identify current locations, while frontmatter IDs identify managed objects. Noura can rebuild the local index after deletion without losing durable workspace data. See `docs/workspace-format/v1.md` for the file format.
