# Initial MVP threat model

## Protected assets

Noura protects workspace bytes, provider credentials, stable identity, and the integrity of writes that follow an external edit. The Initial MVP assumes a trusted local user and trusted first-party bundled plugins. It does not claim isolation from malware running under the same operating system account.

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

## Known limitations

- **Relay completeness.** Managed sync is end-to-end encrypted: a relay only ever sees ciphertext, and every operation is signature-verified against a trusted device key. A malicious or compromised relay can still omit operations or advance a client's pull cursor, because the client accepts the server's monotonic cursor without a cryptographic completeness commitment. This is an availability and integrity limit, not a confidentiality one; closing it requires a protocol-level commitment and a larger review.
- **Local symlink races.** Mutation paths reject symlink components and validate the destination before writing, but a concurrent process could replace a parent directory between validation and the write. The threat model excludes malware running under the same operating-system account. PDF reads close this window by opening each path component relative to an already-open directory with `NOFOLLOW`.
- **Plugin isolation.** Community plugins do not run. The host validates and freezes every manifest, snapshots it at activation, and enforces declared capabilities at each context call. Admitted plugin code would still execute in the application WebView, so a real sandbox remains a prerequisite for third-party loading.

## Deferred and release-gated risks

The Initial MVP does not load community plugins or serve remote MCP transports. Those features require separate authorization and sandboxing reviews before implementation. Sync, account, and collaboration paths remain experimental and require their documented network, encryption, cross-platform, and independent-review release gates before user-facing release.
