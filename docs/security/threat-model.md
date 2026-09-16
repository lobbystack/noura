# Initial MVP threat model

## Protected assets

Noura protects workspace bytes, provider credentials, stable identity, and the integrity of writes that follow an external edit. The Initial MVP assumes a trusted local user and trusted first-party bundled plugins. It does not claim isolation from malware running under the same operating system account.

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

## Deferred risks

The Initial MVP does not load community plugins, serve remote MCP transports, synchronize data, or expose accounts. Those features require separate authorization, sandboxing, network, and encryption reviews before implementation.
