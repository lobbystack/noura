# Project Overview

Noura is "The open workspace for humans and AI." It is a local-first workspace whose normal files remain useful without the application.

## Architecture Invariants

- NEVER store durable workspace information only in SQLite.
- NEVER synchronize local SQLite databases.
- NEVER use a path as permanent object identity.
- NEVER make local functionality depend on our cloud.
- NEVER copy OSS code without license/provenance verification.
- ALWAYS treat workspace files as canonical durable state.
- ALWAYS support external filesystem changes.
- ALWAYS prefer mature permissively licensed existing implementations over custom infrastructure when reasonable.
- DO NOT change locked architecture decisions without documenting the reason.
- Managed sync MUST be end-to-end encrypted. Hosted infrastructure must not require plaintext workspace content to store, synchronize, route, or index workspace data.
- AI systems may access plaintext workspace content only on an authorized client or device, or through an explicitly authorized agent action.

Files win whenever a file and a derived index disagree. Users must be able to delete every `index.sqlite` file and rebuild the same workspace state from ordinary files. Stable object IDs survive moves and renames. Plugins store durable data in workspace files, not private database tables.

A durable mutation succeeds only after the canonical file operation completes durably. SQLite updates, cache updates, and emitted events cannot turn a failed file write into a successful mutation.

## Toolchain

- Write the product in Rust, on the toolchain pinned in `rust-toolchain.toml`.
- Build the desktop UI with egui through eframe.
- Format Rust with `cargo fmt`, lint it with Clippy, and check licenses and advisories with `cargo deny`.
- `sdk/typescript` is the only TypeScript in the product. It uses npm and Node because the `jco` toolchain requires them.
- `apps/website` stays on SvelteKit and Bun until it moves to Zola. Use the [@svelte](plugin://svelte@svelte) plugin when you edit it.

## Rust rewrite

The all-Rust desktop app replaced the legacy TypeScript and Tauri app. The owner signed off on parity, and the 2026-11-14 cleanup deleted the legacy code. To check how the legacy app behaved, read it in the Git history before the commit that removed it.

## Code Ownership Boundaries

- `crates/workspace-format` owns platform-independent parsing, validation, and canonical serialization of workspace files, with no filesystem, database, or credential dependencies.
- `crates/local-core` owns filesystem safety, atomic writes, indexing, watching, reconciliation, credentials, and domain operations.
- `crates/app-core` holds product behavior and view state for every client. It has no UI dependency, so a mobile client can reuse it later.
- `crates/app-ui` holds the egui screens and the widget kit that draws plugin UI. It calls only `app-core`.
- `crates/editor-core` holds the Markdown editor's buffer, parsing and reveal rules, with no UI dependency. It sends a text-mode note's local edits into the collaboration sandbox, which owns that note's only yrs document, its client IDs and its undo, and it applies the text changes the sandbox returns (`docs/workspace-format/sync-v2.md` §22). `crates/editor-egui` draws and drives it.
- `crates/speech` holds speech in the kernel (ADR 0006): microphone and system audio capture, the speech-to-text engine behind a trait, the model catalog and its downloads, pasting into other apps, and this device's speech choices in the app data folder. It touches no workspace file and has no UI dependency; `app-core` serves plugins from it.
- `crates/plugin-host` runs plugins in Wasmtime and enforces their capabilities. `crates/plugin-sdk` holds the WIT interfaces and the Rust author library.
- `crates/collab-sandbox` runs the collaboration sandbox (`docs/workspace-format/sync-v2.md` §22) in Wasmtime: the verdict on each remote yrs record, each text-mode note's document on its own thread, writer allowances and load numbers. It hands `editor-core` the note's document and the sync client its side of the note.
- `crates/collab-sandbox-guest` is the sandbox's guest module, with its patched copy of yrs. The workspace excludes it; `build.sh` builds the `.wasm` that `crates/collab-sandbox` embeds and `sync-v2-collaboration.json` pins by hash, so any change to it is a protocol change.
- `crates/plugins/*` holds first-party plugins. They build to WebAssembly against `plugin-sdk`, exactly like third-party plugins.
- `crates/mcp-server` adapts MCP tools to shared services and duplicates no business logic.
- `crates/sync-protocol` holds the wire types, signing, and verification shared by clients and the server. `crates/sync-key-envelope` owns the key envelope and device formats.
- `crates/sync-server` is the sync service: HTTP, commands and logs. `crates/sync-store` holds the store's transaction traits, the protocol checks and commits every store shares, and the conformance suite. `crates/sync-store-sqlite` and `crates/sync-store-postgres` implement those traits and pass that suite. Local features never depend on the server.
- `crates/account-web` is the WebAssembly module for passkey ceremonies and the encrypted share viewer.
- `apps/desktop` is the desktop binary: windows, menus, drag and drop, and updates.
- `sdk/typescript` holds the `@noura` packages for TypeScript plugin authors: the types generated from the WIT, the project template, the CLI and the mock host. Its tests drive `crates/kit-host`, a test-only binary that serves `app-core`'s plugin service over stdio as the real host. Author docs live in `docs/plugins/`, and `scripts/plugin-reference.sh` regenerates the interface reference from the WIT.
- `apps/website` contains the static marketing site and legal pages for noura.app. It uses no other workspace package.

### Workspace schema ownership

`docs/workspace-format/` defines the public workspace format, and `crates/workspace-format` is authoritative for parsing, validation, normalization, and deterministic serialization. Keep exactly one definition of the format: plugins, the MCP server, and the sync server reach workspace files only through Rust APIs.

The format may change during the rewrite without migrations, because there are no users to migrate. Update the spec and the conformance fixtures in the same commit as the change.

## Git Workflow

Use GitHub for code and pull requests. Use neutral branch names such as `feature/workspace-reconciliation`; do not use public-facing agent prefixes. Never discard uncommitted work or run destructive Git commands without explicit approval.

## Licensing and Provenance

The project uses the MIT license. Verify the license for each source path before copying or adapting OSS code. Record the repository URL, exact commit, source path, applicable license, destination, modifications, notice obligations, reviewer, and date under `docs/provenance/` before donor code enters the branch. Update `THIRD_PARTY_NOTICES.md` for shipped notices. A dependency entry does not authorize copying its source.

## Testing Requirements

Run focused tests while editing and the full checks before handoff. Storage changes require tests for malformed input, round trips, external edits, stale indexes, interrupted index updates, moves, duplicate IDs, path traversal, and rebuilds. Mutations must prove that durable bytes reach disk before the call resolves. MCP, plugin, and UI adapters must call shared services. Documentation examples must compile in CI.

Each crate's integration tests form one binary, `tests/it`, with a module per test file: add a test file as a module there, not as a new file in `tests/`. Run focused tests with `cargo test -p <crate> --test it <module>::<name>` or `cargo nextest run -p <crate> -E 'test(<name>)'`, and leave the full suite to CI. Keep incremental compilation on, give each worktree its own `target` folder, and run `cargo clean -p <crate>` when `target` grows too large. Tests share a process, so never set environment variables, the current directory or a global subscriber in one. `CONTRIBUTING.md` explains the alternatives.

Any change to workspace-format parsing, validation, normalization, or serialization must update the shared conformance fixtures. The editor must leave an unchanged note byte-for-byte identical through open, display, and close. Test UI through AccessKit with `egui_kittest`. Run the sync store conformance suite against both SQLite and PostgreSQL.

## Scope Discipline

Follow the scope of the current task. Do not implement unrelated future features for convenience.

Keep architectural responsibilities separated:

- Core and domain work belongs outside visual components.
- UI work consumes public typed contracts and must not bypass them.
- Keep business logic in `app-core`, `local-core`, or plugins, never in UI crates.
- Do not modify core architecture during UI work unless you identify and document a blocker.
- Do not implement cloud, collaboration, meeting, or calling features unless the task includes them.

Internal plans and product notes belong in the private Noura-Internal workspace, not this public repository. Public workspace-format, contributor, provenance, and security documents belong here.

## Interface Copy

Write interface text the way Apple Reminders does: sparse. Every string must earn its place by telling the user something they need and can't already see.

- Label a control with one or two words: "Save key", "New task", "Done".
- A heading names the screen, and the controls under it explain themselves. Add a description line only when a choice has a consequence the label can't carry, such as data leaving the device or something that can't be undone.
- An empty state is a short title, plus at most one line that names the next action.
- An error says what failed and what to do next, in one sentence.
- One string says each thing once. Cut a line that repeats a heading, a label, a tooltip or what the screen already shows.
- Before you add a string, check whether the screen works without it. If it does, leave it out.

Accessibility labels follow the same rule: name the control, and leave the rest to its role and state.

## Security Principles

Treat workspace content, plugin input, paths, frontmatter, provider output, and MCP arguments as untrusted. Resolve and validate relative paths inside the selected workspace; reject traversal, absolute paths, and symlink escapes. Paths crossing the plugin boundary need a lossless, validated representation. Do not corrupt or reinterpret non-UTF-8 paths; return a structured unsupported-path error when the current platform or API cannot represent one safely. Keep credentials in the operating system credential store and never hand secrets to plugins or write them to workspace files, SQLite, logs, or snapshots. Check every plugin call against the capabilities the user approved for that plugin. Escape FTS queries instead of accepting raw SQLite syntax. Use expected content revisions to prevent silent overwrite of external edits. Return structured public errors and keep raw OS, database, and provider details in local diagnostic logs.

## Agent skills

### Issue tracker

Specs and tickets are Markdown files in the private Noura-Internal workspace, never GitHub Issues or this repository. See `docs/agents/issue-tracker.md`.

### Domain docs

Single context: `GLOSSARY.md` at the repository root, with ADRs in Noura-Internal. See `docs/agents/domain.md`.

### Research notes

Save research notes in the Noura-Internal `Research/` folder.
