# Encrypted synchronization service

Noura’s sync service uses Bun, Hono, and PostgreSQL to route signed, encrypted operations between authorized clients. Clients own workspace content keys and canonical file writes. Read the [server setup guide](../../apps/server/README.md) to run the service.

## Trust boundary

Clients send signed ciphertext envelopes rather than plaintext workspace content. Authentication and authorization are separate from possession of content keys. An operation’s signature binds its routing fields and ciphertext.

Database transactions serialize workspace changes, including revocation, before the server returns an acknowledgment. Operation IDs are durable retry identities. Native credentials remain in the operating system’s credential store.

The server can observe this metadata:

- Opaque workspace, object, and device IDs
- Authorization relationships
- Ciphertext length and traffic timing

The server does not hold client content keys. Clients must authenticate and decrypt incoming content; the server cannot prove that a malicious client encrypted submitted bytes.

## Service scope

The service supports the encrypted transport described in [`sync-v1.md`](../workspace-format/sync-v1.md): signed opaque operations, access policies, encrypted key envelopes, attachment ciphertext, and public encrypted snapshots. The native client owns canonical file application, local consent, trust pins, credentials, and durable journal state.

The service verifies authentication and authorization, serializes workspace changes, and stores opaque protocol records. It does not own workspace content keys or canonical workspace files. Experimental collaboration capability routes remain disabled by standard service startup; durable HTTP pull and acknowledgment remain authoritative when those routes are enabled for testing.

The service remains experimental. Follow the [operations guide](../../apps/server/OPERATIONS.md) for deployment, backup, and validation requirements.
