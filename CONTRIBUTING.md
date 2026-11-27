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

Run the checks CI runs before you open a pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
scripts/check-editor-deps.sh
```

If you change the website, also run `bun run check:website` and `bun run build:website`.

Describe what changed and why in the pull request. Link the issue when one exists.

## Run tests while you edit

Each crate's integration tests build into one test binary named `it`. The file `tests/it/main.rs` declares one module per former test file, so a module path such as `sync_restore::` selects the tests that file holds:

```sh
cargo test -p app-core --test it sync_restore::
cargo test -p app-core --test it sync_restore::many_devices -- --nocapture
```

A filter matches any part of a test's name, so `sync::` also selects `workspace_sync::`. Add `-- --skip workspace_sync::` or use `--exact` when a module's name ends another's.

Three app-core tests keep binaries of their own: `sync_scale`, `sync_soak` and `sync_attachment_memory`. Each one installs a counting global allocator to measure the process's peak memory, and a process holds one global allocator.

Tests in one binary share a process, so a test must not change process-wide state such as environment variables, the current directory or the global tracing subscriber. To read the logs, open a capture with `support::logs::Capture` in app-core's tests. To set environment variables, run the test's work in a copy of the test binary, as `local-core`'s `remote_image_proxy` does.

Keep the loop short:

- Run the tests near your change, by module or by name, and leave the full suite to CI
- Keep incremental compilation on, which Cargo does by default; don't set `CARGO_INCREMENTAL=0`
- Give each worktree its own `target` folder. Two worktrees that share one overwrite each other's builds of the workspace crates, and each rebuilds every time
- When `target` grows past what your disk holds, run `cargo clean -p <crate>` for the crates you build most, or `cargo clean` now and then

[cargo-nextest](https://nexte.st) runs each test in a process of its own and reads its settings from `.config/nextest.toml`:

```sh
cargo nextest run -p app-core -E 'test(sync_restore::)'
cargo nextest run --profile ci --workspace
```

The `ci` profile retries the tests listed there once, keeps going after a failure, and starts the slowest tests first.

## Know where CI runs your tests

CI tests each change once, in its pull request, and spreads the tests over several runners. The `.github/workflows/ci.yml` jobs that run your tests:

- **`build`**: builds the workspace and its tests once per platform, with dependencies optimized, then runs the doctests and the TypeScript author kit's tests
- **`test`**: runs that build's tests in shards, four on Linux, six on Windows and two on macOS, each with `cargo nextest run --partition slice:<m>/<n>`
- **`lint`**: runs `cargo fmt` on Linux and Clippy on each platform
- **`extras`**: runs the plugin host and the collaboration sandbox on Pulley, the editor's large note in release mode, and the macOS frame-time reports
- **`CI passed`**: fails when any job failed and counts a skipped job as passing, so branch protection requires only this check

Linux runs every crate's tests. macOS and Windows skip the crates in `LINUX_ONLY_TESTS` at the top of `ci.yml`: `workspace-format`, `sync-protocol`, `sync-key-envelope`, `sync-store`, `account-web`, `plugin-sdk`, the plugins in `crates/plugins` and plugin-host's test plugins. A crate joins that list only when both hold:

- Its code has no platform-specific branch. `rg 'cfg!?\((not\()?(target_os|windows|unix|target_family)' crates/<crate>` finds none
- Its tests read only checked-in fixtures, and touch no temporary file, path, process, socket or credential

A new crate runs on every platform until you add it to the list.

A pull request that changes only `apps/website/`, Markdown at the repository root, or `docs/` outside `docs/workspace-format/` and `docs/plugins/` skips the Rust jobs, since the Rust code reads those two folders. A pull request that leaves the website alone skips the website job. Both run when the pull request's base hasn't passed CI. `.github/scripts/ci-changes.sh` holds these rules.

After a squash merge, CI on `main` skips the Rust and website jobs when a pull request's run already passed on the same tree, as it does when `main` hasn't moved since that run. It still publishes the server image, and it refills the build caches when a dependency changed. If `main` has moved, CI on `main` runs every job. To run every job on any branch, start the workflow from the **Actions** tab.

## Share a build cache across worktrees

Dev and test builds keep only line tables for debug info, so each `target` folder stays smaller. If you build in several worktrees, install [sccache](https://github.com/mozilla/sccache) and add these lines to your shell profile so your worktrees share one cache capped at 30 GB:

```sh
export RUSTC_WRAPPER=sccache
export SCCACHE_CACHE_SIZE=30G
```

sccache reads the size when its server starts, so run `sccache --stop-server` after you change it. `sccache --show-stats` reports the cap as the max cache size. Without sccache, leave `RUSTC_WRAPPER` unset and Cargo builds as usual.

## Follow the architecture boundaries

Keep each responsibility where reviewers expect it:

- Put business rules in `crates/app-core`, `crates/local-core`, or a plugin. Keep them out of `crates/app-ui`, which draws screens and calls only `app-core`.
- Define plugin interfaces in the WIT under `crates/plugin-sdk/wit`. Plugins reach the workspace only through those interfaces, with the capabilities the user approved.
- Follow the format in `docs/workspace-format/`. `crates/workspace-format` owns parsing, validation, and deterministic serialization.

## Respect licensing and provenance

Noura ships under the MIT license. Check the license of any source you copy or adapt, and prefer mature permissively licensed code over a new implementation. Record the repository URL, exact commit, source path, license, destination, modifications, and reviewer under `docs/provenance/` before the code enters your branch. Update `THIRD_PARTY_NOTICES.md` for shipped notices. A dependency entry does not authorize copying its source.

## Write commit messages

Use a short, imperative subject that names the change. Keep the history neutral and professional.
