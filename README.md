# Noura

Noura is the open workspace for humans and AI. The Local Alpha stores durable workspace state in ordinary Markdown and binary files. A local SQLite database supplies its disposable index, metadata cache, and full-text search.

## Status

The repository contains the desktop Local Alpha with:

- File-backed notes, tasks, and projects
- Month Calendar projection
- External-edit reconciliation and conflict review
- Model Context Protocol (MCP) mutations
- Full-text search
- Index recovery from canonical workspace files

## Development

Install Bun and Rust 1.91 or newer, then run:

```sh
bun install
bun run check
bun run test
```

Run the desktop development host with `bun run tauri dev` after installing the Tauri platform prerequisites.

## Storage contract

Workspace files are canonical. Paths identify current locations, while frontmatter IDs identify managed objects. Noura can rebuild a deleted local index without losing durable workspace data. See the [workspace format](docs/workspace-format/v1.md) for details.
