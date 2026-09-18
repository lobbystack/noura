# Contributing to Noura

This guide explains how to report a bug, propose a change, and pass review. Noura is local-first, so reviewers check data safety before features.

## Before you start

Workspace files are the durable product. Read [AGENTS.md](AGENTS.md) for the invariants reviewers enforce. In short: files are canonical, the local index is disposable, a path is not an identity, and managed sync stays end-to-end encrypted.

Set up the toolchain with the steps in the [README](README.md).

## Report a bug

Open a GitHub issue that includes a minimal reproduction, the expected result, the actual result, and your platform and version. Use synthetic content. Never attach real workspace files, tokens, or personal data.

Report security vulnerabilities through [SECURITY.md](SECURITY.md). Don’t open a public issue for them.

## Propose a change

Branch from `main` and give the branch a neutral name, such as `feature/workspace-reconciliation` or `fix/sync-startup-retry`. Keep each pull request focused on one problem.

Add tests with your change. Storage, format, and sync changes need coverage for malformed input, round trips, external edits, stale indexes, interrupted writes, moves, duplicate IDs, path traversal, and rebuilds.

Run these checks before you open a pull request:

```sh
bun run check
bun run test
bun run format:check
```

Describe what changed and why in the pull request. Link the issue when one exists.

## Follow the architecture boundaries

Keep each responsibility where reviewers expect it:

- Put business rules and canonical behavior in `crates/local-core` and `packages/*`, not in Svelte components or stores.
- Call typed contracts from `apps/app`. Don’t call raw Tauri commands from components.
- Define capability contracts in `packages/plugin-sdk`. Plugins must not import SQLite or Rust internals.
- Follow the format in `docs/workspace-format/`. Rust owns parsing, validation, and deterministic serialization; TypeScript only validates.

## Respect licensing and provenance

Noura ships under the MIT license. Check the license of any source you copy or adapt, and prefer mature permissively licensed code over a new implementation. Record the repository URL, exact commit, source path, license, destination, modifications, and reviewer under `docs/provenance/` before the code enters your branch. Update `THIRD_PARTY_NOTICES.md` for shipped notices. A dependency entry does not authorize copying its source.

## Write commit messages

Use a short, imperative subject that names the change. Keep the history neutral and professional.
