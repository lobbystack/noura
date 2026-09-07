# Experimental native text collaboration

Production discovery advertises no live-text capability. This document describes
the implemented protocol and durable records, not a completed collaboration release.

Access policy version 2 adds `document: { generation, mode }` to each object.
Modes are `text` and `attachment`. Its object signing tuple appends
`[generation, mode]` to the version-1 object fields. Version 1 signatures are
unchanged. The relay requires a signed checkpoint when the descriptor changes,
and rejects policy downgrade from version 2.

Encrypted operation version 2 adds `generation` and `kind` (`text`, `metadata`,
or `file`). Both are appended to the encryption associated-data and signature
tuples. The relay checks the current generation and rejects whole-file
replacement operations for text generations. Only text operation materialization
is implemented natively; metadata and attachment transitions remain release work.

Text plaintext contains `{ version: 1, objectId, generation, updates }`.
Updates are base64 Yjs update-v1 bytes. The root shared text is named `content`.
Yrs uses UTF-16 offsets, matching Yjs and CodeMirror. Tests consume independently
generated `fixtures/yjs-text-v1.json` and `fixtures/yrs-text-v1.json` for Unicode,
deletion, state vectors and local undo. Versions are pinned in Cargo and Bun.
No upstream implementation source was copied.

The native coordinator validates a candidate document before writing. Text is
limited to 8 MiB and encoded document history to 32 MiB. The existing encrypted
inline operation bound is still 1 MiB; large-update blob delivery is not complete.
Byte limits do not yet establish a decoded-allocation or CPU-work bound for
adversarial Yrs input. Production enablement requires that additional isolation.

For managed Markdown, the shared text is the body; native canonical Markdown
serialization remains authoritative. General text retains BOM and newline
conventions. Legacy raw/body, move and deletion mutations cannot bypass an active
text generation. Coordinated metadata, move, deletion and external-edit
translation still need implementation; competing bytes currently fail closed.

## Durable records

All records are ordinary files, resolved through native workspace path checks:

| Path beneath `.noura/sync/`                   | Contents                                                                                                 |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `policies/<revision>.json`                    | Verified policy history, linked to the accepted head.                                                    |
| `collaboration/<object>/checkpoint.json`      | Signed checkpoint and retained rollback floor.                                                           |
| `collaboration/<object>/state.json`           | Generation, covered sequence, path, materialized revision and bytes, and Yrs state.                      |
| `collaboration/<object>/intent.json`          | Recovery intent binding prior revision, candidate state, encrypted operation and optional batch receipt. |
| `collaboration/<object>/batches/<batch>.json` | Idempotency digest and durable revision acknowledgment.                                                  |

A local submission writes and flushes its intent, atomically writes and flushes
canonical bytes, writes state and encrypted outgoing operation, records the batch
receipt, then clears the intent. Committed events follow these writes. Native
server-acknowledgment persistence emits `Synced` only once that object's outbox
is empty. Index maintenance is derived and cannot make a failed file save succeed.

Recovery authenticates the encrypted operation, reconstructs the candidate,
checks materialized bytes and revisions, and either finishes an incomplete write
or finishes recording an already completed write. Competing external bytes are
preserved. Missing state requires verified checkpoint recovery rather than
silently reseeding an existing generation.

The typed workspace client exposes `open`, `submitUpdates`, `flush` and `close`.
The editor adapter batches at 100 ms, shares a document across local views,
suppresses resubmission of native updates, retains failed batches, and limits
undo to local transactions. Native leases close independently across windows.
Presence has editor/provider types and display primitives only; no native
`setPresence` command or encrypted WebSocket transport is advertised.

Durable `/operations` HTTP traffic uses a shared per-device token bucket at
20 requests/second with a burst of 40, separate from the account limit. A separate
10/second presence bucket is defined for the future transport; it stores only
rate counters, never presence payloads.
