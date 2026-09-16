# Encrypted synchronization service

## Dependency decision (2026-09-05)

We inspected Syncular at commit `e4ae85f33617909ad41a2af1a9c7f5296fa77b00` in https://github.com/syncular/syncular (Apache-2.0). We did not copy any donor source.

The native client owns a rusqlite connection. Its private `apply_section_body` and `apply_commit_changes` methods apply remote changes inside the client's SQLite observation transaction. There is no public file-commit acknowledgment hook. Its server `pruneCommitLog` considers cursor age and a retained-count floor; it has no client-created encrypted snapshot coverage predicate.

These are static compatibility findings, not results of a working Syncular prototype. Adopting the complete native client would require modifying its persistence boundary. Using only the server would still require a separate Noura client and snapshot retention protocol. We therefore use the approved fallback: Bun/Hono, PostgreSQL, and a Noura-owned opaque-operation protocol. This supersedes the provisional Syncular recommendation, not the file-first or encryption invariants. SuperSync and Secsync remain research references; we have not copied or adapted their source.

## Trust boundary

The service receives signed ciphertext envelopes, never workspace plaintext. Authentication and authorization are separate from possession of content keys. An operation's signature binds all routing fields and ciphertext. Database transactions serialize workspace changes, including revocation, before the service returns an acknowledgment. Operation IDs are durable retry identities.

Client keys do not belong on this server. Native credentials belong in the OS credential store. The server can see opaque workspace, object and device IDs, authorization relationships, ciphertext length, and traffic timing.

## Release status

This service is under implementation. Do not describe it as production ready or as integrated desktop synchronization until native durable application, identity/recovery, collaborative editing, granular sharing, public viewing, and the complete acceptance gates are verified. Read the service README for the currently executable surface and verification commands.

The current implementation includes browser-approved native account sign-in, signed access policies, encrypted public snapshots and viewing, native file transport, age key envelopes, and a durable file journal. See [`sync-v1.md`](../workspace-format/sync-v1.md) for the file and key protocol. Desktop Settings now exposes background workspace sync, pause/resume, joining an existing member workspace in an empty local folder, explicit device-fingerprint approval, and same-path conflict review. The native coordinator retains consent, sender pins, encrypted keys, and its outbox across restarts. It never acquires trust from an unverified server key. Fingerprints bind the enrolled account ID, device ID, Ed25519 key, and age recipient together. Device account labels cannot change an existing approval's scope.

A writer can back up its own signed recipient envelope before uploading its first operation. This does not grant another account access or rotate an epoch; authorization and epoch changes still require a complete signed access policy. Shared coordinator passes use that policy to distribute keys only to explicitly approved recipients. The key-delivery endpoint now permits an existing object writer to supply immutable envelopes to already-authorized devices. This closes the editor-created-object gap without granting editors permission-management rights. Viewers use a receive-only transport after verifying their signed role. Revoked recipients stop further shared uploads until keys are rotated.

File payload version 2 adds explicit conflict resolution over reviewed content revisions. Version 1 remains unchanged. This is necessary because a single-base file update cannot converge the two reviewed branches after an offline conflict. It does not permit overwriting an unrelated later revision and is not a substitute for the planned collaborative text protocol.

Payload version 3 adds encrypted attachment descriptors and age streaming files. The server reuses the MIT-licensed Tus server and file store for resumable transfer; PostgreSQL authorization and durable-file acknowledgments wrap that dependency. See the workspace-format specification for exact descriptor and encryption rules.

Workspace owners can create seven-day invitation links in desktop Settings. Recipients accept through an authenticated browser session, then connect a desktop device. Acceptance alone creates no membership or key access. The owner compares the recipient's device fingerprints, approves them locally, and grants access by signing the next complete access policy with recipient key envelopes. The owner can revoke pending or accepted invitations. Each workspace permits at most 100 active invitations; the review list prioritizes active invitations over history.
