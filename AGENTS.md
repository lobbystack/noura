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
- AI systems may access plaintext workspace content only on an authorized client or device, or through an explicitly authorized agent action. Sending content to an AI provider requires explicit authorization.

Files win whenever a file and a derived index disagree. A user must be able to delete every `index.sqlite` file and rebuild the same workspace state from ordinary files. Stable object IDs survive moves and renames. Plugins store durable data in workspace files, not private database tables.

A durable mutation succeeds only after the canonical file operation completes durably. SQLite updates, cache updates, and emitted events cannot turn a failed file write into a successful mutation.

## Toolchain

- Use Bun as the JavaScript and TypeScript runtime, package manager, and workspace tool.
- Build the frontend with SvelteKit, Svelte 5, and TypeScript.
- When working with Svelte, use the [@svelte](plugin://svelte@svelte) plugin.
- Use Tauri 2 for the desktop and mobile shell.
- Keep native and local-core work in Rust.
- Use shadcn-svelte with preset `b2ZtALO3cm` for UI primitives. Do not replace the preset or introduce a competing component system without explicit approval.
- When working with shadcn-svelte, use the [$shadcn-svelte](.agents/skills/shadcn-svelte/SKILL.md) skill.
- Do not add npm, pnpm, Yarn, Nx, or Turborepo to the repository without a demonstrated need and explicit approval. Do not introduce another JavaScript lockfile.
- Format Rust with `cargo fmt` and lint it with Clippy.

You are able to use the Svelte MCP server, where you have access to comprehensive Svelte 5 and SvelteKit documentation. Here's how to use the available tools effectively:

## Available Svelte MCP Tools:

### 1. list-sections

Use this FIRST to discover all available documentation sections. Returns a structured list with titles, use_cases, and paths.
When asked about Svelte or SvelteKit topics, ALWAYS use this tool at the start of the chat to find relevant sections.

### 2. get-documentation

Retrieves full documentation content for specific sections. Accepts single or multiple sections.
After calling the list-sections tool, you MUST analyze the returned documentation sections (especially the use_cases field) and then use the get-documentation tool to fetch ALL documentation sections that are relevant for the user's task.

### 3. svelte-autofixer

Analyzes Svelte code and returns issues and suggestions.
You MUST use this tool whenever writing Svelte code before sending it to the user. Keep calling it until no issues or suggestions are returned.

### 4. playground-link

Generates a Svelte Playground link with the provided code.
After completing the code, ask the user if they want a playground link. Only call this tool after user confirmation and NEVER if code was written to files in their project.

## Code Ownership Boundaries

- `apps/app` contains the SvelteKit application, visual UI, and typed client wiring. Components must not call raw Tauri commands.
- `packages/shared` contains generated/native DTOs, errors, events, and transport-neutral types.
- `packages/workspace-schema` contains public TypeScript validation for the workspace format.
- `packages/workspace` contains the typed client, service facades, transports, and thin reactive adapters. Keep business rules out of stores.
- `packages/plugin-sdk` contains capability contracts and the trusted first-party plugin host. Plugins must not import SQLite or Rust internals.
- `packages/editor` contains headless CodeMirror 6/Yjs configuration and Markdown safety checks. Visual editor components belong in `apps/app`.
- `packages/ai` contains provider-neutral TypeScript context and tool registries. Secrets stay behind the native boundary.
- `plugins/*` contains first-party domain adapters built against the same public capabilities intended for future plugins.
- `crates/local-core` owns canonical parsing, deterministic serialization, filesystem safety, atomic writes, indexing, watching, reconciliation, credentials, and domain operations.
- `crates/mcp-server` adapts MCP tools to local-core services and must not duplicate business logic.
- `src-tauri` owns the desktop process and typed IPC/event bridge.
- `apps/server` contains the experimental encrypted sync service built with Bun, TypeScript, and Hono. It must not become a dependency of local features. `apps/server-web` contains its account pages and encrypted share viewer.

### Workspace schema ownership

`docs/workspace-format/` defines the normative public workspace format. `crates/local-core` is authoritative for parsing, validation, normalization, and deterministic serialization of durable workspace files.

`packages/workspace-schema` exposes compatible TypeScript validators for frontend and plugin consumers. It must not parse or serialize durable files independently, redefine canonical serialization, or create new format semantics. Generate shared DTO shapes from Rust where practical.

Do not maintain two independently evolving definitions of the workspace format. Any mirrored TypeScript validation must pass the same conformance fixtures as Rust.

## Git Workflow

Use GitHub for code and pull requests. Use neutral branch names such as `feature/workspace-reconciliation`; do not use public-facing agent prefixes. Never discard uncommitted work or run destructive Git commands without explicit approval.

## Licensing and Provenance

The project uses the MIT license. Verify the license for each source path before copying or adapting OSS code. Record the repository URL, exact commit, source path, applicable license, destination, modifications, notice obligations, reviewer, and date under `docs/provenance/` before donor code enters the branch. Update `THIRD_PARTY_NOTICES.md` for shipped notices. A dependency entry does not authorize copying its source.

## Testing Requirements

Run focused tests while editing and the full checks before handoff. Storage changes require tests for malformed input, round trips, external edits, stale indexes, interrupted index updates, moves, duplicate IDs, path traversal, and rebuilds. Mutations must prove that durable bytes reach disk before the call resolves. MCP and Tauri adapters must call shared services. Generated TypeScript bindings and documentation examples must compile in CI.

Any change to workspace-format parsing, validation, normalization, or serialization must update the shared conformance fixtures. Rust and TypeScript must accept and reject the same fixtures. Test deterministic serialization against canonical Rust output; TypeScript must not implement a competing serializer.

## Scope Discipline

Follow the scope of the current task. Do not implement unrelated future features because they are easy or adjacent.

Keep architectural responsibilities separated:

- Core and domain work belongs outside visual components.
- UI work consumes public typed contracts and must not bypass them.
- Do not move business logic into Svelte components or stores for convenience.
- Do not modify core architecture during UI work unless you identify and document a blocker.
- Do not implement cloud, collaboration, meeting, or calling features unless the task includes them.

Internal plans and product notes belong in the private Noura-Internal workspace, not this public repository. Public workspace-format, contributor, provenance, and security documents belong here.

## Security Principles

Treat workspace content, plugin input, paths, frontmatter, provider output, and MCP arguments as untrusted. Resolve and validate relative paths inside the selected workspace; reject traversal, absolute paths, and symlink escapes. Paths crossing the Rust and TypeScript IPC boundary need a lossless, validated representation. Do not corrupt or reinterpret non-UTF-8 paths; return a structured unsupported-path error when the current platform or API cannot represent one safely. Keep credentials in the operating system credential store and never return secrets over IPC or write them to workspace files, SQLite, logs, snapshots, or frontend storage. Escape FTS queries instead of accepting raw SQLite syntax. Use expected content revisions to prevent silent overwrite of external edits. Return structured public errors and keep raw OS, database, and provider details in local diagnostic logs.
