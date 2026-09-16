# AI runtime architecture

Noura runs the Pi agent loop in the authorized client WebView. Provider transport, credentials, workspace mutation, and durable state remain native and local-first.

## Boundaries

```text
Svelte projection → workspace client → Pi controller → typed native stream transport
                                                   ↕
                                          Tauri Channel<AiStreamFrame>
                                                   ↕
                         local-core provider stream, operation registry, keyring
```

- The WebView owns the Pi loop, transient transcript projection, and TypeBox conversion for already-validated tool schemas.
- Native code owns provider requests, endpoint and credential resolution, cancellation registry, backpressure, public error redaction, and all durable workspace mutations.
- Desktop startup does not load provider or consent files. The native AI foundation initializes on the first AI command and retries after a load failure; failures return the safe, retryable `ai_unavailable` error without configuration or OS details.
- `packages/ai` is transport-neutral. It must not depend on `packages/workspace`.
- `packages/workspace` supplies typed adapters to the Noura client and native commands. Svelte components and stores never call raw Tauri commands.
- Providers receive plaintext only on an authorized client after device-local consent. Credential values and references never cross IPC, enter workspace files, or appear in errors or logs.

## Authoritative inputs

Noura owns the base instructions, workspace `AGENTS.md`, plugin instructions, context composition, tool capability checks, and chat persistence. The agent cannot discover credentials, execute a shell, access SQLite, read arbitrary paths, or invent a workspace mutation path.

Plugin contributions use public capabilities. A tool, context provider, or instruction provider has an owning plugin ID, revision, and read/write/network risk classification. Plugin tools keep JSON-Schema-compatible contracts; the Pi adapter converts them internally. MCP and plugin adapters call the same local-core services as the application and do not duplicate domain rules.

At run start, the controller composes instructions deterministically: Noura base instructions, workspace `AGENTS.md`, enabled plugin `docs/agent.md` ordered by plugin ID, then attributed context-provider results. Disabling a contributing plugin cancels the active provider stream, durably interrupts the run, removes its contributions, and rebuilds the agent before a later run.

## Streaming and events

Each stream operation has a stable UUID. Every `AiStreamFrame` includes that operation ID and a strictly increasing sequence number. Native code delivers frames over a dedicated `tauri::ipc::Channel<AiStreamFrame>` because token and tool deltas are frequent, ordered, and must not be dropped.

`noura://core-event` remains for durable, lower-frequency facts such as `chat:created`, `chat:renamed`, `chat:message-appended`, `chat:assistant-finished`, and `chat:expired`. It is not a streaming transport: its broadcast semantics intentionally allow lag recovery. Files remain canonical when an event or derived index disagrees.

The native provider bridge applies bounded backpressure. A closed Channel receiver is cancellation. Public errors contain a stable code and safe message only; the native bridge never emits raw provider bodies, request headers, credential references, or secrets.

## Cancellation and lifecycle

Cancellation is bidirectional: Pi's `AbortSignal` asks native code to cancel the operation; native cancellation or receiver loss terminates the provider stream and produces an aborted terminal frame. Only one active run is allowed per chat, and a duplicate start cannot replace the active run's cancellation handle.

On application suspension, native code cancels or terminates operations and persists the available partial assistant text as cancelled. On startup or resume, it recovers in-progress assistant and tool-call records as interrupted unless a matching, durable tool result proves completion. It never retries ambiguous mutations automatically.

## Durable chat records

Canonical chat and message Markdown files under `chats/` are the source of truth. Before provider transmission, Noura persists the user message and an in-progress assistant record. Before a tool executes, it persists the in-progress tool-call intent; after execution it persists the result and completion state. Final assistant content and status reach disk before a run resolves. Stream deltas are transient UI state only.

The client sends non-executable tool schemas and canonical tool-call/result history to native provider transport. Native tool-call frames re-enter the Pi loop, which records the call before requesting explicit one-time client approval. Closing, cancelling, changing workspaces, or denying the approval resolves as denial; no tool execution is authorized by default.

All mutation requests use expected revisions. External edits win and surface a conflict instead of being silently overwritten. The SQLite index is derived, rebuildable from Markdown, and never the authoritative transcript or retention record.

## Web access

Web search and URL fetch are later, separate native capabilities. They accept only validated HTTPS destinations, reject private/local addresses and unsafe redirects, enforce size/type/timeout limits, return text-oriented extracted content with source attribution, and have their own one-time disclosure. They never use browser cookies, browser history, local URL schemes, or credential discovery.

Noura does not enable Exa automatically. A September 2026 review found that its API requires an account and API key, grants use only to authorized users under its Terms, is subject to documented usage limits, and has usage-based pricing. Noura therefore cannot establish zero-configuration redistribution. Noura's native web service fails closed unless a compliant, explicitly configured provider and the separate web disclosure are present.

## Superseded sidecar direction

The Noura AI runtime supersedes the former sidecar approach. Noura bundles Pi as a browser-safe first-party dependency in the WebView; the WebView reaches native provider access only through typed IPC. The Phase 0 compatibility record remains in [Pi runtime spike](./ai-runtime-spike.md).
