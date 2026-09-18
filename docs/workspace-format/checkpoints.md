# Experimental encrypted checkpoint protocol

This protocol is implemented for native and relay testing. The HTTP routes require
`createApp(store, { checkpointTransitions: true, ... })`; production startup does not
set this option. Capability discovery returns empty checkpoint and transition
version lists by default. Native text checkpoint retrieval, invitation activation,
rotation rebase, durable editing, native lifecycle mutations, and realtime delivery
are experimental. The cross-platform, sustained-load, and independent-review gates
must be completed before enabling the routes for users.

Rollout admission is separate from protocol continuity. With
`checkpointTransitions: true, collaborationRollout: false`, the relay continues
serving committed capabilities, transitions, checkpoints, existing live-text
generations, and object activations for already-enrolled workspaces, but rejects
new workspace capabilities with `sync.collaboration_rollout_disabled`. Since a
valid object activation is bound to a stored signed workspace capability, this
stops new workspace enrollment without downgrading existing generations or
discarding local drafts.

The same experimental switch exposes an immutable owner-signed workspace
collaboration capability. Its version-1 tuple is
`["noura.sync.workspace-capability",1,workspaceId,1,1,1,deviceId]`, where the
three numeric values declare collaboration protocol 1, minimum native client
protocol 1, and minimum relay protocol 1. Staging verifies the stored signature;
feature discovery without this signed workspace authority is insufficient.

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
- Transition v1: `["noura.sync.transition",1,transitionId,coveredSequence,policyDigest,checkpointDigests]`.
- Transition v2 appends a sorted checkpoint-blob manifest:
  `["noura.sync.transition",2,transitionId,coveredSequence,policyDigest,checkpointDigests,[[objectId,epoch,ciphertextDigest,ciphertextSize],...]]`.
- Object activation v1 binds
  `["noura.sync.object-activation",1,activationId,workspaceId,policyRevision,coveredSequence,capabilityDigest,deviceId,[generation,mode],recipientEnvelopes,checkpointDigest]`.
- Object activation v2 appends its sorted checkpoint-blob manifest to that tuple.

Version-1 signing bytes are unchanged. Version 2 is used only when a checkpoint
payload refers to separately uploaded ciphertext. Each manifest entry is bound
to an object and next epoch in the signed policy and to a checkpoint in the same
transition.

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

| Method   | Path beneath `/v1/workspaces/:workspace` | Behavior                                            |
| -------- | ---------------------------------------- | --------------------------------------------------- |
| POST     | `/transitions`                           | Stage an exact signed transition and reserve quota. |
| GET      | `/transitions/:transition`               | Read its digest and committed status.               |
| POST     | `/transitions/:transition/commit`        | Commit the staged transition.                       |
| GET      | `/objects/:object/checkpoint`            | Read the current checkpoint if authorized.          |
| GET/PUT  | `/collaboration-capability`              | Read or immutably establish owner-signed authority. |
| POST/GET | `/activations[/:activation]`             | Stage or resolve an exact object activation.        |
| POST     | `/activations/:activation/commit`        | Atomically publish its object and checkpoint.       |

Staging changes no permissions, keys or visible epochs. A reused transaction ID
must have the identical digest and does not reserve quota twice. Staged data is
retained; this version provides no automatic pruning or abandonment refund.
Version-2 ciphertext reservations name the transition ID and must exactly match
its signed object, epoch, digest, and size. They remain unreadable while staged.

Commit uses the operation-write workspace lock. It checks the operation boundary,
policy revision and hash chain, role authority, recipient coverage, epochs and
generations, and the durable completion of every manifest blob before committing
checkpoints, envelopes, policy, grants, public-link
revocations and transaction status together. Failed validation rolls back every
change. Retrying an already committed transaction remains idempotent even after a
later policy revision. A stale operation boundary requires a newly prepared
transition. Checkpoint generations cannot be reused for the same object.

For objects enrolled in this protocol, reader additions and removals require a
new epoch, and every rotation requires a checkpoint. The standalone key-sharing
endpoint cannot add recipients to such objects. Unchanged legacy workspaces
retain their existing protocol behavior. This compatibility path does **not**
provide fresh-recipient history isolation.

New workspace members and object grantees created by a committed transition are
assigned its signed `coveredSequence` as their history floor. Pull authorization
filters operations at or below that floor and still advances an empty page to the
workspace cursor. Existing readers retain their earlier floor. This keeps retired
ciphertext out of a fresh recipient's response rather than relying on missing old
keys as the privacy boundary.

The native journal stores the exact pending transition in
`.noura/sync/state.json` before network submission. Uploads for its checkpoint
objects pause while the outbox remains intact. Submission retries reuse the
stored transaction identity, querying its status before staging. After staging,
the client verifies the already-flushed local ciphertext against the signed
manifest, resumes each upload under the transition identity, and only then
requests commit. A missing or changed local blob leaves the transition staged
and retryable. The pending transition advances durably through `prepare`,
`stage`, `resolve_commit`, `install`, `rebase`, and `complete`. It is cleared
only after the accepted policy and every text generation are installed. A crash
after `complete` clears the same transaction idempotently on recovery.

At a text-generation boundary, native state compares the acknowledged baseline
with the current draft and rebases that text onto the fresh checkpoint. A clean
three-way merge becomes a newly signed operation in the new generation; old
binary CRDT updates are never replayed. The old state and exact encrypted
operations are archived before the active outbox changes. Overlapping edits,
destination changes, competing external bytes, and drafts held by a removed
writer are retained in an ordinary-file review record and leave installation
pending.

## Release limitations

Native committed checkpoints are retrieved against the accepted policy hash chain
and persist a rollback floor. Final fingerprint approval prepares and commits the
same rotation transaction automatically, and normal sync resumes it after
interruption. Attachment migration and writer-created object activation use
signed, resumable checkpoint transactions; capability-enrolled device-recipient
changes use the full-object rotation coordinator. User-facing member removal and
object-grant commands still need to be connected to that coordinator. Inline
ciphertext retains the existing 1 MiB operation bound. Version-2 transitions and
activations support transaction-bound blob staging and commit-time completeness
checks.

Folder inheritance, scoped invitations, partial joins, sharing UI, and expanded
public publishing are not implemented by this protocol. The experimental native
realtime client supplies authenticated notifications and encrypted presence, while
durable HTTP pulls and acknowledgements remain authoritative.
The experimental text coordinator and editor adapter are described in
[collaboration.md](collaboration.md). Production startup keeps these routes
disabled. The implementation and tests were author-performed, not an
independent security audit.
