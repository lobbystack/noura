# Encrypted file synchronization, version 1

This document defines the implemented file-change payload and its local durable journal. It does not change canonical Markdown serialization. Rust in `crates/local-core` owns serialization; the TypeScript workspace-schema package only validates the shared `fixtures/sync-v1.json` conformance cases.

## File changes

An encrypted operation contains one UTF-8 JSON object with exactly these fields, serialized in this order:

```json
{
	"version": 1,
	"path": "notes/example.md",
	"previousPath": null,
	"baseRevision": null,
	"content": "SGVsbG8K"
}
```

`content` holds canonical standard base64 of the complete file bytes, or `null` for a deletion. `baseRevision` is the lowercase, 64-character BLAKE3 hash of the previous bytes, or `null` when the path must be absent. `previousPath` identifies the source of a move; it cannot equal `path` or accompany a deletion.

Paths are portable relative paths of at most 4096 UTF-8 bytes. The client rejects absolute paths, traversal, empty components, control characters, Windows reserved names and characters, trailing dots/spaces, and reserved root entries. The reserved roots are `.noura`, `.git`, `node_modules`, and `target`. Their ASCII case variants are also reserved on every platform. Device-name validation includes the superscript-digit COM/LPT aliases described in [Microsoft's filename rules](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file) and the [console device names](https://learn.microsoft.com/en-us/windows/console/console-handles). A device-name stem with surrounding ASCII spaces is still not acceptable. The native applicator also rejects symlinks along the path. Paths are encrypted payload data; they are not server routing identifiers.

Managed Markdown uses its existing frontmatter object ID. Ordinary files use a sidecar object ID. Moves of managed files retain that ID. An ordinary file move can currently be inferred only when exactly one missing catalog path has the same content hash; simultaneous edits and renames need explicit identity support.

## Encryption and signatures

`packages/shared/src/sync.ts` and the native `EncryptedOperation` define the envelope. A fresh 12-byte nonce accompanies AES-256-GCM ciphertext and its tag. Additional authenticated data is the UTF-8 encoding of this JSON tuple:

```text
["noura.sync.payload",version,workspaceId,objectId,deviceId,operationId,epoch,policyRevision]
```

The Ed25519 signature covers the UTF-8 JSON tuple:

```text
["noura.sync.operation",version,workspaceId,objectId,deviceId,operationId,epoch,policyRevision,nonce,ciphertext]
```

Binary envelope fields use canonical standard base64. Epochs are positive integers within JavaScript's safe integer range. `policyRevision`, server sequences, and policy revisions use canonical nonnegative decimal strings within signed 64-bit range. The server rejects a future policy revision and accepts an earlier revision only while the sender still has current write access. Clients authorize downloaded operations against the exact durable policy-revision checkpoint carried by the signature. An operation ID must never be reused with different bytes.

The client verifies a previously trusted signing key and the authenticated ciphertext before parsing file data. A key supplied alongside an operation by the server is not sufficient evidence of trust.

## Access-policy history

Every signed access policy contains `previousPolicyDigest`. Revision 1 uses `null`; every later revision contains the lowercase SHA-256 digest of the prior policy's signing tuple followed by its 64-byte signature. The previous digest is inside the new policy signature. The server enforces this link in the same transaction as membership, object grants, key epochs, and access revision.

Clients fetch every transition after their durable checkpoint. A transition must increment the revision by one, match the previous digest, verify under a locally pinned device key, and carry a signature from an account that was an owner or admin in the preceding policy. A locally approved device must sign the root policy, and its bound account must be an owner in that policy. A relay cannot restore authority to a removed administrator by returning a new self-authorizing policy.

The journal stores the accepted policy head and compact writer checkpoints for each policy revision. Revision 0 is anchored to the root owner's locally approved devices when revision 1 is accepted. Historical operations remain recoverable after revocation, but an operation is applied only when its signed device is a workspace or object writer in its named checkpoint. Access changes reset the pull cursor while retaining authenticated receipts.

## Content-key envelopes

Device signing identities and age X25519 private identities live in the native OS credential store. An object key is wrapped for an approved device using age. The age plaintext is this context-bound JSON tuple:

```text
["noura.sync.object-key",1,workspaceId,objectId,epoch,keyBase64]
```

Its outer Ed25519 signature authenticates:

```text
["noura.sync.key",1,workspaceId,objectId,epoch,signingDevice,recipientDevice,wrappedKey]
```

Validated envelopes may live at `.noura/sync/keys/<object>/<epoch>/<recipient-device>.json`. The native layer rejects replacing an existing envelope with different bytes. Loading requires the recipient's native identity and a previously trusted signing key. The client writes no plaintext content key to this directory.

The desktop recovery export writes a signed workspace kit to a newly created JSON file outside every workspace, with mode 0600 on Unix. The kit includes the workspace/origin/account binding, public trust pins, recipient-encrypted object keys, and the exporting device's age recovery identity. It does not include a device signing seed, session token, or workspace content. This backup is secret and user-held; the hosted service cannot recover its identity.

The signature covers the UTF-8 JSON tuple `["noura.sync.recovery",1,config,identity,envelopes]`. Maps use Rust's sorted `BTreeMap` order, and envelopes are sorted by object ID and epoch. The root fields are `version`, `config`, `identity`, `envelopes`, and `signature`. The selected backup file is a user-authorized trust source; a self-contained signature is not independent proof of who supplied an unknown file. Imported pins cannot replace existing local pins. Kits are limited to 8 MiB and 1,000 envelopes.

You must sign in and join or open the matching workspace before an import. It verifies context, pins, signatures, and wrapped keys natively; recovers later server envelopes addressed to the exported identity when still authorized; and rewraps recovered keys to the newly signed-in device before downloading content. It never restores the old signing identity or session. Newly encountered signers still require device approval. The workspace remains paused after import so device approval and any required epoch rotation can be completed. Import retries retain the same durable receipts and preserve subsequent external edits. A revoked workspace membership still requires a teammate's re-invitation; a kit cannot bypass server authorization. The low-level raw age identity helper remains available for legacy backups but is no longer the desktop export format.

Device approval fingerprints are the lowercase BLAKE3 digest of the UTF-8 JSON tuple `["noura.device.card",1,deviceId,accountId,signingPublicKey,ageRecipient]`. The account ID comes from the native device-enrollment proof. Users compare the fingerprint with the independently displayed fingerprint on the actual device; the server directory is not a trust source. Local configuration binds the approved account, signing key, and encryption recipient together.

## Durable application

The `.noura/sync/state.json` journal stores the object catalog, ordered encrypted outbox, applied receipts, accepted access-policy head, per-revision writer checkpoints, access revision, and pull cursor. It is durable file state, independent of every SQLite index. It must not be uploaded as a workspace file or treated as a disposable cache.

Capture flushes current file bytes before adding their encrypted operation and catalog revision to one atomic journal update. Upload acknowledgment first persists the server receipt under `sent/`, then removes the outbox entry. The coordinator records local consent and key pins in `.noura/sync/config.json`. It scans existing canonical files before missing catalog paths so exact-content moves precede deletion records. It backs up self-recipient keys before uploading new content. This local configuration never enters synchronized payloads.

Download application persists the original envelope under `inbox/`. It then checks identity, path, and expected content revision. Canonical writes complete before the applied receipt is committed. Conflicting changes are preserved under `conflicts/` without overwriting external edits; deleted bytes are retained under `deleted/`. These local conflict/deletion files contain plaintext file data and must remain excluded from server upload.

A page cursor advances only after all its operations have matching durable receipts. Replaying a receipt does not overwrite subsequent external edits. Verified permission-revision changes reset the pull cursor to retrieve newly visible history while retaining receipts. Index reconciliation follows durable writes; an index failure does not change the outcome of an already durable mutation.

## Reviewed file resolutions

Encrypted file payload version 2 retains the version 1 fields and adds `acceptedRevisions`: one or two distinct content revisions (including `null` for an absent file). The list must include `baseRevision`; moves are prohibited in this version. A recipient may apply the choice only when its current canonical revision is explicitly accepted, or its bytes already equal the chosen content. Unreviewed later bytes remain a conflict. The outer encrypted envelope stays at version 1. Readers that do not support payload version 2 must stop without advancing their cursor.

Local resolution retains its signed ciphertext intent under `resolutions/` before writing canonical bytes. The canonical choice reaches disk before the journal publishes the resolution in its outbox and marks reviewed conflicts resolved. Restart repeats the same ciphertext instead of creating a second operation. Original incoming envelopes and conflict bytes remain available after resolution. Receipts bind retained conflict plaintext by its digest, allowing an authenticated resolution to clear only the reviewed branches on another replica.

The shared conformance fixtures cover all three payload versions. Rust remains the serializer; TypeScript exposes validation only.

Directory fsync is implemented on Unix. Equivalent crash-durability behavior on Windows still needs implementation and platform testing. Move/identity conflict resolution and collaborative text updates remain unfinished.

## Encrypted attachments

Payload version 3 retains `path`, `previousPath`, and `baseRevision`, sets `content` to `null`, and appends `blob` after those fields. `acceptedRevisions` is absent. Here `null` content does not mean deletion: `blob` is required. The blob descriptor has exactly `id`, `size`, `plaintextSize`, and `revision`, in that order. `id` is the lowercase SHA-256 digest of the entire ciphertext; `revision` is the lowercase BLAKE3 digest of canonical plaintext bytes. Sizes are byte counts. Ciphertext must be at most 1 GiB and larger than plaintext. Only the ciphertext ID, ciphertext size, object ID, and key epoch reach storage APIs. Plaintext size, content revision, and paths stay inside the signed, encrypted operation.

The native coordinator captures files above 700 KiB using age's authenticated streaming format. The recipient is an age X25519 identity derived from the object key with BLAKE3 `derive_key`, context `noura.sync.blob.x25519.v1`. The resulting 32 bytes use age's standard Bech32 secret identity encoding; the identity never leaves native memory. Every encryption uses age's fresh random file key. Retrying an upload reuses the durable ciphertext, not a new encryption. The age format implementation remains the upstream dependency.

The coordinator writes ciphertext to `.noura/sync/blobs/<id>` and fsyncs it before its descriptor enters the outbox. The HTTP transport uploads in at most 1 MiB TUS patches, resuming at the server's offset. The server fsyncs each acknowledged patch, checks the complete digest, and publishes storage metadata before accepting completion. Whole-file hashing and S3 writes run outside the workspace lock; authorization and upload identity are rechecked before completion is committed. The client uploads an operation only after its blob is complete.

Downloads use bounded byte ranges and fsync each ciphertext range. The receiver checks complete ciphertext against its signed digest before decryption. Decryption writes to an atomic temporary destination and checks the plaintext size and revision before replacement. Identity and expected content revision checks still apply. The client flushes the canonical destination and its parent directory before the applied receipt and cursor can advance. A corrupt resumed download is truncated durably so retry can fetch clean bytes.

Ordinary binary attachments stream with bounded buffers. Files starting with a Markdown frontmatter delimiter still use the existing canonical parser, which currently loads the whole file for identity validation. Attachment conflicts are retained and shown for review.

Explicit binary resolution follows the same safety conditions as reviewed text: the local file must exist, the catalogued object revision must equal the current on-disk revision, and the change must keep its path (no move). The reviewer then chooses one branch:

- `remote`: materialize the reviewed blob into the canonical path through the existing blob store, verifying its ciphertext digest and decrypting with the object key before replacement, then publish a signed version-3 change that repeats the same blob manifest. The exact ciphertext must already be present locally; otherwise the call fails with `sync_blob_unavailable` and changes nothing.
- `local`: keep the current canonical bytes and publish a signed version-2 change whose content is those bytes and whose accepted revisions include the local and remote branches.

Both choices commit canonical bytes before the journal publishes the resolution and marks the receipt resolved, and an interrupted resolution resumes the same retained ciphertext. Because version 3 carries no `acceptedRevisions`, a replica converges to a reviewed remote branch only if it already holds the blob or applies the original attachment operation first; otherwise the resolution remains a conflict. Completed blobs are retained until safe snapshot coverage and garbage collection are implemented.
