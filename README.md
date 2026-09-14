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

The repository also contains:

- `apps/server`: an experimental encrypted sync service serving the unified `apps/app` browser build, not a production release. Account, device approval, invitation, and encrypted share pages work independently of native workspace initialization. Browser workspaces remain unavailable. See [`apps/server/README.md`](apps/server/README.md).
- `apps/website`: the marketing site.

See [`docs/architecture/`](docs/architecture/) for design notes and [`docs/workspace-format/`](docs/workspace-format/) for the workspace format.

## Development

Install Bun and Rust 1.91 or newer. The browser workspace build also requires
the Rust Wasm target and the wasm-bindgen CLI version that matches the Rust
dependency:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked
```

Then run:

```sh
bun install
bun run check
bun run test
```

Run the desktop development host with `bun run tauri dev` after installing the Tauri platform prerequisites.

Run the marketing website from the repository root:

```sh
bun run dev:website
```

The marketing site runs at `http://127.0.0.1:5174`. Its embedded product demo
uses the application development server at `http://127.0.0.1:5173` when that
server is running.

## Storage contract

Workspace files are canonical. Paths identify current locations, while frontmatter IDs identify managed objects. Noura can rebuild a deleted local index without losing durable workspace data. See the [workspace format](docs/workspace-format/v1.md) for details.

## Security

Report vulnerabilities privately. See [SECURITY.md](SECURITY.md).
