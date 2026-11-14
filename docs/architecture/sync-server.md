# Encrypted synchronization service

`crates/sync-server` is Noura's sync service. It routes signed, encrypted records between your devices under [sync protocol version 2](../workspace-format/sync-v2.md). Your devices hold the content keys and write the canonical workspace files. To run the service, follow the [self-hosting guide](../self-hosting.md).

## Trust boundary

Clients send signed ciphertext, never plaintext workspace content. Signing in proves who you are. It gives the server no content key. Each signature binds a record's routing fields to its ciphertext.

The server stores ciphertext and the metadata that [§29 of the protocol](../workspace-format/sync-v2.md#29-what-the-server-stores) lists, such as opaque IDs, sizes after padding, and request times. It can't prove that a client encrypted the bytes it sent, so each client verifies and decrypts what it receives. A client also detects a server that drops, reorders, replays or forks its records ([§1](../workspace-format/sync-v2.md#1-goals)).

## Service layout

The service is one binary with `serve`, `migrate` and `healthcheck` commands. It keeps everything in SQLite in a data directory, or in PostgreSQL when you give it a database URL, so several instances can share one database:

- **`crates/sync-server`**: HTTP, accounts, rate limits, logs and blob uploads
- **`crates/sync-store`**: the store's transaction traits, the protocol checks and commits every store shares, and the conformance suite
- **`crates/sync-store-sqlite`** and **`crates/sync-store-postgres`**: the two stores, which both pass that suite
- **`crates/account-web`**: the WebAssembly module the account pages load for passkey ceremonies

Local features never depend on the server. A workspace without sync works offline with no account.
