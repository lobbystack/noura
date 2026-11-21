# Orbs: designs from OpenClaw, IronClaw and ZeroClaw

Orbs borrow file conventions and scheduler designs from three open-source personal agents. Noura copied no source from any of them: `crates/workspace-format/src/orb.rs`, `crates/app-core/src/ai/orbs.rs` and `crates/app-core/src/ai/orbs/triggers.rs` are new code. This record names what each project contributed as design, so a later change that does copy code starts from a verified source.

## OpenClaw

- Repository: https://github.com/openclaw/openclaw
- Commit: `a27f0c3b4e8d767debdc758669522a4059cf9ae7` (2026-11-21)
- License: MIT, Copyright (c) 2026 OpenClaw Foundation (`LICENSE`). The files read carry no headers of their own and sit under no directory with its own license.
- Read: `src/agents/workspace-bootstrap-policy.ts`, `src/agents/identity-file.ts`, `src/agents/embedded-agent-helpers/bootstrap.ts`, `src/auto-reply/heartbeat.ts`, `src/auto-reply/tokens.ts`, `docs/concepts/agent-workspace.md`, `docs/reference/templates/`
- Borrowed: an agent's folder of Markdown files. Noura's Orb file holds the instructions (`AGENTS.md`, `SOUL.md`) in its body and the identity (`IDENTITY.md`: emoji or avatar, theme) in its frontmatter; `Memory.md` stands for `MEMORY.md`; `Heartbeat.md` keeps the checklist of the older `HEARTBEAT.md`, which OpenClaw has since moved into SQLite; `HEARTBEAT_OK` and `NO_REPLY` mark a quiet answer; a request carries at most 20,000 characters of the memory file, OpenClaw's limit for a workspace file.

## IronClaw

- Repository: https://github.com/nearai/ironclaw
- Commit: `b0b999d96781516ee05e6ba961d6f3ead900da96` (2026-09-10)
- License: MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`, root `Cargo.toml`), Copyright (c) 2026 NEAR AI. No NOTICE file.
- Read: `crates/domains/ironclaw_triggers/src/lib.rs`, `src/libsql.rs`, `src/worker.rs`, `src/worker/config.rs`, `src/worker/due_fire.rs`, `src/worker/active_cleanup.rs`, `crates/contracts/ironclaw_host_api/src/capability.rs`
- Borrowed: a deterministic identity for each firing, kept with the run so a retry or a restart doesn't fire twice (`src/lib.rs:876-945`); one active run per trigger; a 30-second poll; a minimum time between a schedule's times (Noura uses 5 minutes, Grok Bot's, where IronClaw uses 60 seconds); skipping the times a paused trigger missed (`src/lib.rs:714-738`). Noura doesn't replay every missed time, as IronClaw does, and reads crontab weekdays through croner, which avoids IronClaw's Sunday-as-1 mistake.

## ZeroClaw

- Repository: https://github.com/zeroclaw-labs/zeroclaw
- Commit: `fa48765b3e3c80681d01d637b3629af37cb0398f` (2026-11-21)
- License: MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`, workspace `Cargo.toml`), Copyright (c) 2025 ZeroClaw Labs. Its `NOTICE` reads "Copyright 2025 ZeroClaw Labs" and "This product includes software developed at ZeroClaw Labs". Noura copied no code, so no NOTICE duty applies; code taken later under the MIT option would carry the MIT notice only.
- Read: `crates/zeroclaw-runtime/src/sop/engine.rs`, `src/sop/store/sqlite.rs`, `src/sop/store/model.rs`, `src/cron/scheduler.rs`, `src/cron/store.rs`, `src/cron/schedule.rs`
- Borrowed: a durable record of each run, written before its work starts and again when it ends, which the next launch reads to take runs up (`engine.rs:685-889`); missed schedule times collapsing into one run at startup (`cron/scheduler.rs:653-714`). ZeroClaw leaves a run that was mid-step without a rule; Noura answers again once when no tool call waits for its result, and otherwise ends the run as interrupted.

## Dependency

Schedule triggers parse 5-field cron patterns and find their times through `croner` 4.0.3 (MIT, Copyright (c) 2023 Hexagon), with its `jiff` backend and no default features. It is a dependency, not copied source, and the generated license file carries its license.

Reviewed by the implementing agent on 2026-11-21 for the owner's approval.
