# Initial MVP threat model

## Protected assets

Noura protects workspace bytes, provider credentials, stable identity, and the integrity of writes performed after an external edit. The Initial MVP assumes a trusted local user and trusted first-party bundled plugins. It does not claim isolation from malware running under the same operating system account.

## Local and network-enabled scope

The local core runs workspace operations without a Noura account or network service. It reads and writes canonical workspace files locally, and the desktop host and standalone Model Context Protocol (MCP) server call the same services.

Network-enabled features add separate boundaries. You authorize the client before it sends workspace content to an AI provider. The native web-access boundary requires explicit consent and validates public HTTPS destinations.

Experimental account and sync services process authentication metadata and encrypted workspace content. Clients retain workspace content keys; managed sync must never require plaintext workspace bytes on the server. Local workspace operations remain independent of these services.

## Trust boundaries

- Workspace paths and file contents are untrusted input.
- Frontmatter, search text, MCP arguments, plugin inputs, and provider responses are untrusted input.
- The WebView does not receive provider secrets.
- Community plugin code does not execute during this phase.
- SQLite contains derived data and receives no trust as a durable authority.

## Controls

The native layer rejects absolute paths, traversal, special components, symlink mutation paths, unsupported managed-file extensions, and non-UTF-8 mutation targets. BLAKE3 revisions prevent silent overwrite after external changes. Atomic sibling writes flush the file and parent directory before the promise resolves. A cross-process lock serializes app and MCP mutations.

Search escapes tokens before building an FTS expression. The UI receives text snippets rather than HTML. Public errors use stable categories and codes; diagnostic strings from operating-system, SQLite, keyring, and provider libraries stay out of IPC responses.

Provider settings contain opaque credential references. The OS credential store holds secret values. Noura does not write secrets to workspace files, SQLite, frontend storage, logs, or snapshots.

## Deferred and release-gated risks

The Initial MVP does not load community plugins or serve remote MCP transports. Those features require separate authorization and sandboxing reviews before implementation. Sync, account, and collaboration paths remain experimental and require their documented network, encryption, cross-platform, and independent-review release gates before user-facing release.
