# Security policy

Report suspected vulnerabilities privately. Do not open a public issue.

## Report a vulnerability

Use GitHub's private vulnerability reporting: [Report a vulnerability](https://github.com/lobbystack/noura/security/advisories/new).

If you cannot use GitHub, email `hello@lobbystack.com`. Include a description, reproduction steps, and the affected version or commit.

We acknowledge reports within a few business days. Allow time for a fix before public disclosure.

## In scope

Noura is local-first and stores durable workspace state in ordinary files. Review these areas:

- Parsing, validation, normalization, and deterministic serialization of workspace files
- Path handling across the Rust/TypeScript boundary: traversal, absolute paths, symlink escapes, and non-UTF-8 paths
- Credential storage and the encrypted synchronization service
- Plugin input, workspace frontmatter, MCP arguments, and AI provider output

## What to include

Send these details:

- A minimal reproduction or proof of concept
- The affected component, version, or commit
- Any preconditions or required privileges

## Out of scope

These findings are out of scope:

- A malicious local user with access to an unlocked device and its credential store
- Denial of service through resource exhaustion on a single-user local instance

Test only infrastructure you own or have permission to test.
