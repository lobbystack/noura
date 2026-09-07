# Experimental encrypted checkpoint protocol

This protocol is implemented for native and relay testing. The HTTP routes require
`createApp(store, { checkpointTransitions: true, ... })`; production startup does not
set this option. Capability discovery returns empty checkpoint and transition
version lists by default. Native text checkpoint retrieval and durable editing
are experimental; activation, rotation rebase and the remaining release gates
must be completed before enabling the routes for users.

A checkpoint carries version `1`, a fresh document generation, a decimal
`coveredSequence`, an encrypted operation payload, and a separate Ed25519
signature. The payload uses the existing operation encryption and signature
format. Its plaintext contains `CheckpointContent`: version, stable object ID,
generation, content revision, and a current `FileChange` snapshot. Paths and
content revisions remain encrypted. Base revisions, previous paths and accepted
conflict revisions are forbidden in a checkpoint. The content revision must
match the current content or encrypted blob descriptor. This format contains no
CRDT history. Native text bootstrap deterministically creates a fresh Yrs
document from current visible text and the signed generation; it does not copy
the previous CRDT state.

Signatures use UTF-8 JSON tuples without whitespace:

- Checkpoint: `["noura.sync.checkpoint",1,generation,coveredSequence,operationDigest]`.
- Transition: `["noura.sync.transition",1,transitionId,coveredSequence,policyDigest,checkpointDigests]`.

Digests are lowercase SHA-256 over signing bytes followed by the decoded
64-byte signature. Checkpoints are strictly sorted by object ID, with no duplicate
objects. Every checkpoint must match its policy's workspace, device, revision and
object epoch. The transition signature binds all checkpoint digests and the
existing signed access policy digest. Clients must verify the policy against
previously trusted authority before accepting checkpoint content; a valid
signature alone does not prove that a checkpoint is current or committed.

The shared `fixtures/checkpoint-v1.json` fixture verifies the tuple format in
Rust and TypeScript. Its content key is the public test value `[7; 32]`. Generate
a replacement with `cargo run -p local-core --example checkpoint_fixture` and
review changes before replacing it.

## Relay transaction

Authenticated experimental endpoints:

| Method | Path beneath `/v1/workspaces/:workspace` | Behavior                                            |
| ------ | ---------------------------------------- | --------------------------------------------------- |
| POST   | `/transitions`                           | Stage an exact signed transition and reserve quota. |
| GET    | `/transitions/:transition`               | Read its digest and committed status.               |
| POST   | `/transitions/:transition/commit`        | Commit the staged transition.                       |
| GET    | `/objects/:object/checkpoint`            | Read the current checkpoint if authorized.          |

Staging changes no permissions, keys or visible epochs. A reused transaction ID
must have the identical digest and does not reserve quota twice. Staged data is
retained; this version provides no automatic pruning or abandonment refund.

Commit uses the operation-write workspace lock. It checks the operation boundary,
policy revision and hash chain, role authority, recipient coverage, epochs and
generations before committing checkpoints, envelopes, policy, grants, public-link
revocations and transaction status together. Failed validation rolls back every
change. Retrying an already committed transaction remains idempotent even after a
later policy revision. A stale operation boundary requires a newly prepared
transition. Checkpoint generations cannot be reused for the same object.

For objects enrolled in this protocol, reader additions and removals require a
new epoch, and every rotation requires a checkpoint. The standalone key-sharing
endpoint cannot add recipients to such objects. Unchanged legacy workspaces
retain their existing protocol behavior. This compatibility path does **not**
provide fresh-recipient history isolation.

The native journal stores the exact pending transition in
`.noura/sync/state.json` before network submission. Uploads for its checkpoint
objects pause while the outbox remains intact. Submission retries reuse the
stored transaction identity, querying its status before staging or retrying a
commit. The pending transition deliberately remains until a
future recovery/rebase implementation can safely release it; successful relay
submission is not a complete desktop permission change.

## Release limitations

This foundation does not complete the key-rotation feature. Native committed
text checkpoints are retrieved against the accepted policy hash chain and
persist a rollback floor. New-epoch blob staging, pending-edit rebase, reader-addition
migration for legacy workspaces, and end-to-end permission-change orchestration
remain required. Inline ciphertext retains the existing 1 MiB operation bound.
Blob descriptors can be represented, but next-epoch blob transport is not wired.

Folder inheritance, scoped invitations, partial joins, sharing UI, expanded
public publishing and realtime presence are not implemented by this protocol.
The experimental text coordinator and editor adapter are described in
[collaboration.md](collaboration.md). Cross-platform acceptance, load/soak tests, restore coverage for these
new tables and an independent security implementation review remain release
gates. The implementation and tests were author-performed, not an independent
audit.
