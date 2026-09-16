# Browser key custody and encrypted synchronization

This document proposes how a hosted browser client custodies workspace keys and
participates in Noura encrypted synchronization. It is an architecture decision
record, not an implemented feature. No code in this repository implements the
browser flows described here unless a section explicitly says so.

Decision markers used throughout:

- **Locked** — an existing architecture invariant or a decision already recorded
  in `AGENTS.md`, `docs/architecture/sync-server.md`, or
  `docs/workspace-format/sync-v1.md`. Do not change without documenting the
  reason.
- **Proposal** — a recommendation in this document. Not approved, not
  implemented, and requiring the listed follow-up work before it becomes locked.

## Status

Browser workspaces run in the hosted `apps/app` build and synchronize with the
encrypted service. The installed desktop app remains the trust root; the hosted
browser client is a convenience client. This document records the implemented
design, the verified behavior, and the remaining gaps.

**Verified live.** A native desktop device (the `desktop_probe` example)
enrolled with the service, created a workspace, wrapped an object key to a
browser device as a `noura.sync.key.web` envelope, published a signed
version-1 access policy covering both devices, and pushed an encrypted
operation. A separate browser device enrolled earlier received the delivered
key, pulled the operation, and decrypted it to the original native file. The
browser-to-browser path (bootstrap, key delivery, operation push/pull,
cross-device decryption, post-bootstrap object provisioning) was verified the
same way. The reverse path was also verified: the browser edited the native
file, sealed and pushed a version-1 operation, and the desktop probe pulled and
decrypted it to the edited bytes.

**Implemented foundation.** The platform-independent
`noura.sync.key.web` version 1 envelope format described under "Where the browser
needs new envelope handling" is implemented in `crates/sync-key-envelope`
(native/portable Rust) and `packages/sync-key-envelope` (browser WebCrypto),
conforming to the shared fixture
`docs/workspace-format/fixtures/browser-key-v1.json`. The browser device
recipient encoding, device fingerprint, and enrollment proof are also implemented
in both languages and conform to
`docs/workspace-format/fixtures/browser-device-v1.json`.

**Partially implemented server.** `apps/server` now accepts `x25519:` browser
recipients during device enrollment and verifies the `noura.device.enroll.web`
proof, stores browser `noura.sync.key.web` envelopes alongside native `age`
envelopes, verifies browser envelope signatures on key upload, access policies,
and collaboration-v2 object activations, enforces the browser recipient against
the enrolled device on every one of those paths, and returns the construction
and browser fields from key delivery and access-state. The server's
`POST`/`GET /v1/workspaces/:workspace/operations`
push/pull routes and the `noura.sync.payload`/`noura.sync.operation` tuples were
already the shared native protocol; the browser client speaks them and the
hosted browser app wires enrollment, custody, key delivery, reconciliation,
conflict resolution, and a recovery kit (see "Browser client foundation" and
"App host wiring"). Revocation transitions the client to a locked state; epoch
rotation is applied by the native coordinator and tolerated by the browser
client. Encrypted attachment download and decryption are implemented; the
browser send path and attachment UI remain outstanding.

**Implemented native client support.** `crates/local-core` now depends on
`crates/sync-key-envelope` and treats a browser device as a first-class remote
recipient: it recognizes the `x25519:` `encryptionRecipient`, renders the
`noura.device.card.web` fingerprint, wraps object keys to a browser recipient
with `sync-key-envelope::wrap_key` (supplying the ephemeral secret, salt, and
nonce from the OS random source), verifies and unwraps browser-authored
`noura.sync.key.web` envelopes against locally pinned signer keys before
decryption, and reads and produces the extended `construction` plus browser
fields from key delivery, access-state, signed access policies, and
collaboration-v2 object activations. Native `age`
envelopes, the seven-field wire shape, and the existing public types keep their
prior behavior. The local `KeyEnvelope` and `PolicyEnvelope` gained an explicit
`construction` discriminator and the four optional browser byte fields, and an
installed app can also run as a `x25519:` browser recipient device itself.

**Implemented browser client foundation.** `packages/browser-sync`
(`@noura/browser-sync`) implements the browser-side custody, enrollment, and
key-delivery foundation described under "Browser client foundation" below. It
generates an Ed25519 signing key and an X25519 recipient key with WebCrypto,
derives a passphrase key-encryption key with PBKDF2-SHA256 (random 32-byte salt,
at least 310,000 iterations, 32-byte output), and persists the signing seed, the
X25519 secret, and the device bearer token only as an AES-256-GCM-wrapped
`WrappedKeyBundle` behind an injectable `KeyStore` (in-memory for tests,
IndexedDB/OPFS for the host). It requests a device challenge, signs and submits
the `noura.device.enroll.web` proof, and pulls and verifies `noura.sync.key.web`
envelopes against caller-pinned signer keys before unwrapping with the recipient
secret. It also seals and opens `EncryptedOperation` envelopes and speaks the
push/pull operation transport at the library level, conforming to the shared
`operation-v1.json` fixture. It implements canonical version 1 through version 3
file-change encoding and decoding against the shared `sync-v1.json` fixture and a
`FileChangeCodec` bridge that seals a file change into an `EncryptedOperation`
and verifies and opens one back to its change, workspace, object, and epoch. The
hosted browser still has no revocation lock state and no UI wiring, so it still
cannot open or synchronize a workspace end to end; local replica reconciliation,
outbox/conflict handling, and a browser-specific recovery kit exist at the
library level.

**Implemented browser replica engine.** `packages/browser-sync-engine`
(`@noura/browser-sync-engine`) implements the local replica reconciliation layer
at the library level over injectable `storage`, `remote`, `codec`, and durable
`state` boundaries. It seals local file changes into a durable outbox before any
network call, flushes the outbox in order in batches of at most 100 operations,
pulls pages from the stored cursor, and applies version-1 file-change rules
against expected revisions, recording conflicts without overwriting local bytes.
A recorded conflict retains the encrypted remote operation and its object
identity, and `resolveConflict` either force-applies the remote operation or
enqueues the current local bytes as a replacement operation so the choice can be
retried and pushed. Durable state carries a schema version, and reading an older
state upgrades it and drops unresolvable legacy conflicts with a reported
migration rather than failing. A revoked codec or remote error transitions the
engine to a locked state through an injected `onRevoked` hook. It holds no keys
and performs no cryptography, serialization, or network I/O.
`createFileSystemSyncStateStore` supplies the durable `SyncStateStore` over an
injected filesystem boundary at an adapter-owned path outside the canonical
workspace, and `createWorkspaceStorageAdapter` is the concrete
`BrowserSyncStorage` bridge that derives its path list from
`BrowserWorkspaceStorage.rebuild()`. The `apps/app` controller now composes that
bridge over the workspace worker, bootstraps one remote object and wrapped key
per managed local object, refreshes delivered keys before reconcile, records and
resolves conflicts, and persists only wrapped keys and locally approved signer
keys. Key-rotation application, native recovery-kit interoperability, and
device-management UI are still outstanding, so the hosted browser still cannot
synchronize a workspace end to end without a trusted device to approve and
deliver keys; the browser-specific recovery kit now exists in
`packages/browser-sync`.

## Locked invariants this proposal must satisfy

These are restated verbatim in intent from `AGENTS.md` and the sync design
documents. Any browser design that violates one of them is rejected.

- **Locked.** Managed synchronization is end-to-end encrypted. Hosted
  infrastructure must not require plaintext workspace content to store,
  synchronize, route, or index data.
- **Locked.** The service receives signed ciphertext envelopes, never workspace
  plaintext. Authentication and authorization are separate from possession of
  content keys.
- **Locked.** Signing in establishes account identity. It does not grant
  workspace content keys. Key access requires device approval and a signed
  access policy.
- **Locked.** Noura does not write secrets to workspace files, SQLite, frontend
  storage, logs, snapshots, or IndexedDB in plaintext. This includes
  `localStorage`, `sessionStorage`, Cache Storage, OPFS, and any other
  script-writable origin storage.
- **Locked.** Workspace files are canonical durable state. A browser store is a
  replica, never the only copy and never a durable authority.
- **Locked.** Object IDs are stable across moves and renames; a path is not
  permanent identity.
- **Locked.** A key supplied by the server is not proof of trust. Clients verify
  envelopes against locally pinned device keys before use.
- **Locked.** Recovery kits are user-held and secret. They cannot bypass server
  authorization or owner approval and never restore an old signing identity or
  session.
- **Locked.** Operation IDs are never reused with different bytes. Access
  policies chain through `previousPolicyDigest`, and a removed administrator
  cannot be restored by a relay returning a self-authorizing policy.

## Terminology alignment with sync-v1

The browser must speak the same protocol as the native client
(`docs/workspace-format/sync-v1.md`):

- **Ed25519 signing.** Device signing identities sign operations, key envelopes,
  access policies, enrollment proofs, recovery kits, and device fingerprints.
- **AES-256-GCM operations.** An `EncryptedOperation` is a 12-byte nonce plus
  AES-256-GCM ciphertext and tag. Additional authenticated data is the UTF-8
  JSON tuple `["noura.sync.payload",version,workspaceId,objectId,deviceId,operationId,epoch,policyRevision]`.
- **age X25519 key envelopes.** Object keys are wrapped with `age` X25519. The
  plaintext is `["noura.sync.object-key",1,workspaceId,objectId,epoch,keyBase64]`;
  the outer Ed25519 signature covers
  `["noura.sync.key",1,workspaceId,objectId,epoch,signingDevice,recipientDevice,wrappedKey]`.
- **Device fingerprint.** The lowercase BLAKE3 digest of
  `["noura.device.card",1,deviceId,accountId,signingPublicKey,ageRecipient]`.
  The browser variant uses the domain `noura.device.card.web` and a raw X25519
  recipient instead of an `age` recipient.
- **Epoch and policy revision.** `epoch` is a positive safe integer;
  `policyRevision`, server sequences, and policy revisions are canonical
  nonnegative decimal strings within signed 64-bit range.

### Where the browser needs new envelope handling

The operation envelope maps cleanly onto WebCrypto: AES-256-GCM and Ed25519 are
available in current browsers, and `apps/app/src/lib/account/share.ts` already
verifies and decrypts a version-1 `EncryptedOperation` with `crypto.subtle`. The
browser can reproduce the exact AAD and signing tuples byte-for-byte. No new
operation envelope is required for version 1, and version 2 (`generation`,
`kind`) adds no new cryptography, only the same AAD/signing extension.

The **object-key envelope is different**. The current envelope uses `age`, whose
wire format combines X25519, HKDF, and ChaCha20-Poly1305. WebCrypto does not
expose that as one primitive, and a non-extractable WebCrypto X25519 key cannot
be handed to the Rust `age` implementation. There are two honest paths:

1. **WASM `age`.** Reuse the mature `age` crate compiled to WebAssembly so
   envelopes stay byte-identical. This preserves a single wire format, but the
   raw X25519 identity must exist in WASM memory, so the long-term device
   recipient key cannot be non-extractable.
2. **A browser-capable envelope version.** Define a new, versioned envelope
   domain using WebCrypto X25519 ECDH + HKDF-SHA256 + AES-256-GCM, signed with
   the existing Ed25519 `noura.sync.key` shape. Native Rust implements the same
   KEM so it can wrap to browser devices and unwrap browser-authored envelopes.

**Proposal.** Adopt path 2 for long-term device recipient keys. Envelopes must
self-describe their construction; the current tuple hard-codes version `1`, so a
browser envelope needs a new domain string and signing-tuple version, for
example `noura.sync.key.web` version `1`, and a corresponding update to
`device_fingerprint` that distinguishes an `age` Bech32 recipient from a raw
32-byte X25519 recipient. Unknown envelope versions are rejected, never guessed.

**Proposal.** Allow path 1 for attachment blobs only. Blob identities are
derived from an already-unlocked object key via BLAKE3 `derive_key` context
`noura.sync.blob.x25519.v1`, so WASM can decrypt without exposing a long-term
device key.

**Implemented in the browser (receive).** `packages/browser-sync` implements the
same `age` version-1 blob construction in TypeScript (BLAKE3 `derive_key`,
X25519, HKDF-SHA256, ChaCha20-Poly1305, the `age-encryption.org/v1` header, and
the 64 KiB STREAM payload) with a fixture generated by native
`EncryptedBlob::encrypt`, bounded 1 MiB resumable upload/download against the
server's `tus`/range routes, and an engine attachment fetcher that materializes a
version-3 file change or fails without writing an empty file. Browser memory
bounds cap a received attachment at 64 MiB (native limit 1 GiB).

**Implemented in the browser (send).** The browser encrypts an attachment with
the containing note's object key, uploads it through the bounded resumable path,
writes the plaintext to the local replica under `attachments/<objectId>/`, and
seals a version-3 file change that reuses the note's object, so no new remote
object is provisioned. The notes editor exposes attach, progress, list, and
download controls. Both directions are capped at 64 MiB of browser memory
against the protocol's 1 GiB limit.

**Implemented.** Native binary attachment conflict resolution keeps the local
bytes and publishes a version-2 change; choosing remote materializes the blob and
publishes a version-3 change. The remaining caveat is that a peer needs the blob
or the original operation to converge. The engine also skips operations whose
envelope `deviceId` matches its own device, so it no longer re-downloads and
re-applies its own pushed operations.

**Proposal.** Any new envelope or fingerprint version requires shared Rust and
TypeScript conformance fixtures that accept and reject the same cases before it
is considered implemented, consistent with `AGENTS.md`.

**Implemented.** The version 1 envelope (`noura.sync.key.web`) exists in
`crates/sync-key-envelope` and `packages/sync-key-envelope`, and both languages
run `docs/workspace-format/fixtures/browser-key-v1.json`.

**Implemented.** The browser device recipient, device fingerprint, and
enrollment proof are implemented in both languages, and both run
`docs/workspace-format/fixtures/browser-device-v1.json`. `encode_recipient` /
`encodeRecipient` produce, and `decode_recipient` / `decodeRecipient` accept
only, the exact form below; every other form returns the public error code
`sync_invalid_recipient`.

- **Recipient encoding.** A browser device recipient is the ASCII prefix
  `x25519:` followed by standard (padded) base64 of the raw 32-byte X25519 public
  key, for example `x25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=`.
  Decoding requires the prefix and exactly 32 decoded bytes. This is the browser
  counterpart to an `age` Bech32 recipient and is a distinct form, not a variant
  of `age1...`.
- **Device fingerprint.** The browser fingerprint is lowercase hexadecimal
  BLAKE3 over the canonical JSON tuple
  `["noura.device.card.web",1,deviceId,accountId,base64(signingPublicKey),recipient]`.
  It differs from the native `["noura.device.card",1,...]` tuple only in the
  domain string and the raw X25519 recipient representation.
- **Enrollment proof.** The browser signs the canonical JSON tuple
  `["noura.device.enroll.web",1,origin,accountId,deviceId,base64(publicKey),recipient,challenge]`
  with its Ed25519 device key. `enrollment_proof` / `enrollmentProof` return the
  raw signature bytes, and the server verifies them against the device's
  submitted public key. This mirrors `noura.device.enroll.v2` while adding the
  versioned browser domain and the raw X25519 recipient.

`packages/sync-key-envelope` also exports `deviceFingerprintAge` for a native
`age1` recipient (`noura.device.card`) and `deviceFingerprintForCard`, which
dispatches on the recipient form and rejects a recipient that is neither an
`x25519:` nor an `age1` form with `sync_invalid_recipient`. The `age` domain
function matches `crates/local-core` `device_fingerprint` byte-for-byte, and a
conformance test pins it and the browser fixture values.

The native `device_fingerprint` now renders the browser variant, and
`crates/local-core` both consumes and produces the extended key-envelope fields;
see "Native client support" below. The `apps/server` enrollment, key-storage, and
key-delivery paths accept browser recipients and browser envelopes; see
"Server-side support" below.

## Server-side support

The encrypted sync service now understands the browser recipient and envelope
formats. It remains zero-knowledge: it verifies signatures, validates
authorization, and stores ciphertext fields, but never unwraps a key and never
sees workspace plaintext.

### Device enrollment

`POST /v1/devices` accepts `encryptionRecipient` in one of three forms:

- Absent: the existing `noura.device.enroll` version 1 proof, unchanged.
- An `age1...` Bech32 recipient: the existing `noura.device.enroll.v2` proof,
  unchanged.
- A browser `x25519:` recipient (prefix plus standard padded base64 of exactly
  32 raw bytes): the `noura.device.enroll.web` version 1 proof
  `["noura.device.enroll.web",1,origin,accountId,deviceId,base64(publicKey),recipient,challenge]`.

The recipient is stored byte-for-byte unchanged in
`noura_devices.encryption_recipient`. An unusable recipient returns
`sync.invalid_recipient`; a proof that does not verify against the submitted
Ed25519 key returns `sync.invalid_signature`. Single-use challenge consumption,
the seven-day session rotation, and the revoked-device re-enrollment refusal are
unchanged.

### Key envelopes and storage

`PUT /v1/keys/self` and `PUT /v1/keys/share` accept an optional `construction`
discriminator:

- Absent, or `"age"`: the existing native seven-field record and the
  `noura.sync.key` signing tuple, unchanged.
- `"web"`: the native routing fields plus `recipientPublicKey`,
  `ephemeralPublicKey`, `salt`, and `nonce`, signed over the
  `noura.sync.key.web` version 1 tuple
  `["noura.sync.key.web",1,workspaceId,objectId,epoch,signingDevice,deviceId,recipientPublicKey,ephemeralPublicKey,salt,nonce,wrappedKey]`.

Browser envelopes must wrap to the recipient device's enrolled recipient: the
device's stored `x25519:` recipient and the envelope's `recipient_public_key`
must decode to the same 32 bytes, or the upload returns
`sync.invalid_recipient`. Signer authorization, current-epoch enforcement,
idempotent retries, change detection, and the checkpoint key-rotation refusal
are unchanged for both constructions.

`noura_key_envelopes` gains `construction` (`text NOT NULL DEFAULT 'age'`) and
nullable `recipient_public_key`, `ephemeral_public_key`, `salt`, and `nonce`
columns through idempotent `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` statements.
`SyncStore.ready()` selects the new columns, so a migrated database reports
ready and an unmigrated one fails the readiness probe. `GET
/v1/workspaces/:workspace/keys` and `GET /v1/workspaces/:workspace/access-state`
return `construction` and the browser fields for every envelope, with the native
`signingPublicKey` join, pagination, and access filtering unchanged.

### Access policies

`objects[].envelopes[]` in a signed access policy accepts a browser entry with
`construction: "web"` and the same four browser fields; a missing discriminator
means `age`. The policy parser validates every base64 length, rejects unknown
constructions and mixed or missing fields, verifies the browser envelope against
the signing device over the `noura.sync.key.web` tuple, and persists the
construction fields. For browser entries the policy signing tuple additionally
binds `"web"` and the four browser fields, so the policy signature and its
`previousPolicyDigest` chain cover them. Revision continuity, object coverage,
writer authorization, epoch rules, and the native `age` tuples are unchanged.
Browser recipients are accepted as active approved devices anywhere an `age`
recipient is.

### Native client support

`crates/local-core` (`sync::keys`, `sync::approvals`, `sync::access`,
`sync::transport`) implements the native side of the browser device contract:

- **Recognition and fingerprint.** `device_fingerprint` accepts an `x25519:`
  recipient, validates it with `sync_key_envelope::decode_recipient`, and
  returns the `noura.device.card.web` digest. An `age` recipient keeps the
  `noura.device.card` digest. Approval, invitation, recovery-configuration, and
  access-state paths all compare the same fingerprint, so a changed browser card
  is rejected with the existing `sync_device_changed` error.
- **Wrapping.** `DeviceKeys::wrap_key` branches on the recipient. A native `age`
  recipient keeps the existing `age` path; an `x25519:` recipient decodes with
  `sync_key_envelope::decode_recipient` and wraps with
  `sync_key_envelope::wrap_key`, drawing the ephemeral secret, HKDF salt, and
  AES-GCM nonce from the OS random source. The result is the discriminated
  `KeyEnvelope` with `construction: "web"` and the four browser fields.
- **Verification and unwrap.** `KeyEnvelope::verify` verifies browser envelopes
  with `sync_key_envelope::verify_envelope` over the `noura.sync.key.web` v1
  tuple; `DeviceKeys::unwrap_key` verifies against the locally pinned signer and
  only then calls `sync_key_envelope::unwrap_key`. `receive_keys` parses the
  server's extended rows into this type before verifying and unwrapping, exactly
  as it does for `age`.
- **Access policies.** The local `PolicyEnvelope` carries the same
  `construction` discriminator and browser fields. `AccessPolicy` signing and
  verification bind the eight-element browser envelope tuple
  `[deviceId, wrappedKey, signature, "web", recipientPublicKey,
ephemeralPublicKey, salt, nonce]` for web entries and the unchanged
  three-element tuple for `age`, preserving revision, chain, coverage, and epoch
  rules.
- **Object activation.** `ObjectActivation` binds the same eight-element browser
  envelope tuple when an entry is `web`, verifies each entry through the shared
  `PolicyEnvelope` path, and `receive_activations` stores and unwraps a browser
  recipient key strictly as it does for `age`. `activate_pending_objects` wraps a
  new object's key to an approved browser recipient.
- **Persistence.** A `KeyEnvelope` round-trips both constructions through serde;
  `age` still serializes as the seven-field object. Browser secret material stays
  in the zeroizing native credential record, and `DeviceKeys::create_browser`
  adds a native process that can hold a `x25519:` recipient identity.

### Remaining limitations

- The browser client foundation exists in `packages/browser-sync` (device custody,
  passphrase unlock, enrollment, pinned key delivery, operation seal/open,
  canonical file-change encoding/decoding, the file-change codec, and the
  push/pull transport) and the local replica engine exists in
  `packages/browser-sync-engine` (durable outbox, capped ordered push, cursor-based
  pull and application, conflict recording and resolution, state migration,
  snapshot diffing, and revocation lock state). The `apps/app` controller now
  wires the worker's `objects_list`, per-object keys, pinned key delivery, device
  approval, binding auto-restore, conflict resolution, post-bootstrap object
  provisioning, new-device envelope wrapping, and durable re-wrapping of
  delivered keys over those packages, but epoch rotation for a revoked reader,
  native recovery-kit interoperability, and the device-management UI are still
  absent, so the hosted browser still cannot synchronize a workspace end to end
  without a trusted device approving it and delivering keys. A browser-specific
  recovery kit now exists in `packages/browser-sync` and the controller, so a
  device's wrapped bundle and binding can be re-imported on another browser
  without a trusted device.
- Browser key envelopes are delivered to clients, but clients must still verify
  them locally against pinned signer keys. The server is not a trust source.
- Revocation and epoch rotation reuse the signed access-policy path, which now
  accepts web envelopes; the native coordinator now wraps rotated keys for
  approved browser recipients, but no browser UI enforces a revoked locked state.
- **Implemented.** Collaboration-v2 object activation
  (`noura.sync.object-activation`) accepts browser recipients. The native
  `ObjectActivation::sign`/`verify`, `receive_activations`, and
  `activate_pending_objects` bind the eight-element browser envelope tuple
  `[deviceId, wrappedKey, signature, "web", recipientPublicKey,
ephemeralPublicKey, salt, nonce]` and verify it through the same
  `PolicyEnvelope` path as access policies. The server parses and verifies the
  same tuple and enforces the enrolled browser recipient. A pending activation
  that cannot complete — because it targets a browser recipient or because a
  device still awaits local approval — no longer aborts the pass: the durable
  activation is retried while already-shared objects still receive keys, and an
  unapproved device never forces a key rotation or blocks delivery to approved
  devices. The shared fixture
  `docs/workspace-format/fixtures/activation-v1.json` pins the mixed age/web
  activation signing bytes for both Rust and TypeScript.
- The policy-level `noura.sync.access` tuple extension for browser entries is now
  signed and verified by the native Rust client against the server contract, but
  no shared Rust/TypeScript conformance fixture covers it yet.
- **Implemented (browser-specific).** The browser recovery kit
  (`noura.browser-recovery-kit` version 1) is implemented in
  `packages/browser-sync` and wired into the `apps/app` controller. It encrypts a
  device's wrapped key bundle and durable binding under a passphrase-derived
  AES-256-GCM key and restores the **same** device identity and binding on
  another browser without a trusted device.
- **Implemented (native interop).** A browser can import a native
  `noura.sync.recovery` kit: `importNativeRecoveryKit` verifies the signed
  recovery object against a pinned or consistently self-described recovery
  signer, unwraps the native `age` object-key envelopes with the user-supplied
  recovery identity, and `recoverNativeKeysToBrowserBinding` re-wraps those keys
  as self-addressed `noura.sync.key.web` envelopes in a browser binding. The
  recovery identity and plaintext keys are never persisted. The shared fixture
  `docs/workspace-format/fixtures/native-recovery-v1.json` is read by both the
  Rust and TypeScript tests. This is one-way (native kit to browser); native
  import of a browser kit is not implemented.

## Browser client foundation

`packages/browser-sync` (`@noura/browser-sync`) is the first browser-side client
code. It is a transport-neutral library that uses `globalThis.crypto.subtle` and
an injected `fetch`; it performs no UI work and holds no global state. It is a
foundation, not a working browser workspace: operation sealing/opening, the
push/pull transport, and the browser-specific recovery kit are implemented here,
while local replica reconciliation, outbox and conflict handling, and revocation
lock state live in `packages/browser-sync-engine`. UI wiring is not implemented.

### Identity and at-rest custody

- `createDeviceIdentity` generates an Ed25519 signing key and an X25519 recipient
  key with WebCrypto and returns an opaque `WrappedKeyBundle`.
- `sealBundle` / `openBundle` derive an AES-256-GCM key-encryption key from a user
  passphrase with PBKDF2-SHA256 (random 32-byte salt, a default and enforced
  minimum of 310,000 iterations, 32-byte output), encrypt the exported signing
  seed, the X25519 secret, and the device bearer token under a random 12-byte
  nonce, and bind the bundle metadata as AES-GCM additional authenticated data.
- `unlockDeviceIdentity` / `loadDeviceIdentity` return in-memory key material; the
  unwrapped secret is never written back to storage. The `KeyStore` interface
  (`read`/`write`/`delete`) lets the host supply IndexedDB or OPFS;
  `createMemoryKeyStore` is provided for tests.
- **PBKDF2 fallback choice.** WebAuthn PRF is the preferred unlock factor, but PRF
  availability remains uneven across browsers, platforms, and authenticators. This
  foundation implements and documents the passphrase fallback and deliberately
  does not implement PRF. A wrong passphrase or a tampered bundle fails AES-GCM
  authentication and is reported as `browser_sync_passphrase_rejected`.
- No unwrapped secret is persisted. This package does not claim reliable memory
  zeroization; it clears only the references it owns.

### Recovery kit

**Implemented (browser-specific).** A browser recovery kit restores the same
browser device identity and its durable binding on another browser without a
trusted device to approve or deliver keys. It is defined and implemented in
`packages/browser-sync` and wired into the `apps/app` controller.

- `exportRecoveryKit({ bundle, binding, passphrase, kdf? })` serializes
  `{ bundle, binding }` as canonical JSON and encrypts it with AES-256-GCM under
  a PBKDF2-SHA256 key-encryption key. It draws a random 32-byte salt and 12-byte
  nonce, binds the format and version as AES-GCM additional authenticated data,
  and refuses an empty passphrase, an unsupported key-derivation function, an
  invalid bundle or binding, or an iteration count below the 310,000 minimum. The
  returned `RecoveryKitFile` carries only non-secret metadata and ciphertext.
- `importRecoveryKit(file, passphrase)` validates the format, version, key
  derivation, base64 fields, decoded lengths, and the recovered bundle and
  binding shapes before returning them. An unknown format or version is rejected,
  never guessed; oversized input is rejected before decryption; and a wrong
  passphrase or tampered ciphertext fails AES-GCM authentication and returns a
  structured `browser_sync_passphrase_rejected` error.
- The kit is **full credential material**: it carries the passphrase-wrapped
  Ed25519 seed, X25519 secret, and bearer token plus the binding's self-wrapped
  object keys. It is encrypted and user-held and is never written to origin
  storage in plaintext. It restores the **same** device identity, not a new
  device, and it does not bypass server authorization or owner approval.
- In `apps/app`, `BrowserSyncController.exportRecoveryKit(passphrase)` requires
  unlocked custody and a loaded binding and never returns plaintext.
  `BrowserSyncController.importRecoveryKit(file, passphrase, localWorkspaceId)`
  decrypts the kit, stores the recovered wrapped bundle as-is under its bundle id
  through the existing `KeyStore` (the device passphrase is not known, so it
  cannot be re-sealed), writes the binding under `localWorkspaceId`, caches it,
  and leaves the controller locked until the user unlocks the recovered device
  with its original passphrase.

This is a **browser-specific format**, not the signed native
`noura.sync.recovery` object. Interoperating the two remains a proposal; see
"Remaining limitations".

**Divergence from native recovery semantics.** The locked invariant that a
recovery kit "never restores an old signing identity or session" describes the
native signed `noura.sync.recovery` object. This browser-specific kit
deliberately restores the same device's at-rest custody: it is a portable copy of
a credential the user already holds, not a newly issued recovery identity. It
grants no content access the device did not already have, does not bypass owner
approval because it re-establishes an already-approved device under its existing
bundle, and cannot override server authorization, which is checked on every
request.

### Enrollment

- `requestDeviceChallenge` calls `POST /v1/device-challenges` with
  `credentials: 'include'` and returns `{challenge, accountId, expiresIn}`.
- `enrollBrowserDevice` signs the exact `noura.device.enroll.web` version 1 tuple
  with the device signing key via `enrollmentProof`, posts
  `{challenge, deviceId, publicKey, proof, encryptionRecipient}` to
  `POST /v1/devices`, and returns the device bearer token. `publicKey` is the raw
  32-byte Ed25519 public key as standard base64, matching the server. The token is
  secret and is persisted only by re-sealing the identity.

### Key delivery

- `receiveKeys` paginates `GET /v1/workspaces/:workspace/keys` with
  `afterObject`/`afterEpoch`, sends the bearer token, and for each web envelope
  checks the recipient against the device's X25519 key, resolves the signer in a
  caller-pinned map, verifies with `verifyEnvelope`, and only then unwraps with
  `unwrapKey`. An unpinned signer or a recipient mismatch rejects the call with a
  structured error; `age`-construction envelopes are returned as
  `unsupported_envelope` markers rather than mis-decrypted.
- Errors are a stable `BrowserSyncError` code set. The library never logs secrets
  and does not claim reliable memory zeroization.

### Operation sync

**Implemented at the library level.** `packages/browser-sync` reproduces the
version 1 and version 2 `EncryptedOperation` cryptography byte-for-byte:

- `sealOperation` encrypts plaintext with AES-256-GCM using the workspace object
  key directly, authenticating the versioned tuple
  `["noura.sync.payload",version,workspaceId,objectId,deviceId,operationId,epoch,policyRevision]`
  and, for version 2, `generation` then `kind`. It then signs the envelope with
  the device Ed25519 key over
  `["noura.sync.operation",version,workspaceId,objectId,deviceId,operationId,epoch,policyRevision,nonce,ciphertext]`
  plus `generation` and `kind` for version 2. It emits canonical standard base64
  and camelCase wire fields.
- `openOperation` validates the envelope, verifies the signature against a
  caller-pinned signer **before** decrypting, then opens it with the object key
  and the same associated data. An unpinned, tampered, wrong-key, or
  wrong-version envelope returns a structured `BrowserSyncError`.
- `BrowserSyncTransport`, `pushOperations`, `pullOperations`,
  `listWorkspaces`, and `accessState` speak the native wire contract:
  `POST /v1/workspaces/:workspace/operations` with `{operations}` returns
  `{sequences}`; `GET /v1/workspaces/:workspace/operations?after=<cursor>`
  (optional `accessRevision` and `wait=25`) returns
  `{accessRevision, operations, cursor, hasMore}`. Bearer-token auth is sent on
  every request, and cursors and sequences remain canonical decimal strings so a
  server bigint is never rounded through a JS number.
- **Conformance.** `crates/local-core/src/sync/crypto.rs` contains an ignored
  `regenerate_operation_fixture` test that writes
  `docs/workspace-format/fixtures/operation-v1.json`, plus a normal test that
  reproduces the fixture's version 1 and version 2 operations, verifies and opens
  them, and asserts tamper rejection. `packages/browser-sync` runs the same
  fixture and asserts byte-identical signature and ciphertext, so Rust and
  TypeScript accept and reject the same vectors.

### File-change codec

**Implemented at the library level.** `packages/browser-sync` owns the canonical
file-change payload format and the codec bridge that the local replica engine
consumes:

- `encodeFileChange` and `decodeFileChange` (in `src/file-change.ts`) implement
  version 1, version 2, and version 3 file changes exactly as the Rust
  serializer writes them. Encoding emits no whitespace in the fixture's field
  order (`version`, `path`, `previousPath`, `baseRevision`, `content`, then
  `acceptedRevisions` for version 2 and `blob` for version 3) with
  `serde_json`-compatible string escaping and canonical standard base64 content.
  Decoding parses strict UTF-8 JSON and validates with `syncFileChangeSchema`
  from `@noura/workspace-schema`. Both directions conform to the shared
  `docs/workspace-format/fixtures/sync-v1.json` fixture, so Rust and TypeScript
  accept and reject the same vectors.
- `createFileChangeCodec({ identity, objectKeys, pinnedSigners })` (in
  `src/codec.ts`) is the sync codec bridge. Sealing encodes a version 1 change,
  looks up the object key for the operation's `objectId`, and calls
  `sealOperation` with the device identity and the caller-supplied `epoch` and
  `policyRevision`. Opening calls `validateOperation`, resolves a
  caller-pinned signer for `operation.deviceId`, and only then decrypts with the
  object key for `operation.objectId` and decodes the payload.
- The codec reports the new stable codes `browser_sync_untrusted_signer` for an
  absent trust pin, `browser_sync_missing_key` for an absent object key, and
  `browser_sync_invalid_file_change` for a payload that is not valid UTF-8 JSON
  or fails the schema. The local replica engine consumes this codec through its
  `codec` boundary: the host binds the envelope identity (`workspaceId`,
  `objectId`, `epoch`, `policyRevision`) for each seal and maps the engine's
  byte-oriented file-change descriptor to the codec's canonical change; opening
  returns that descriptor plus the recovered `workspaceId`, `objectId`, and
  `epoch`. A host that treats an untrusted signer as revocation maps
  `browser_sync_untrusted_signer` to its revoked code.

### Local replica reconciliation

**Implemented at the library level.** `packages/browser-sync-engine`
(`@noura/browser-sync-engine`) is the local replica and orchestration layer, not
cryptography. It composes four injectable boundaries and performs no network or
cryptographic work itself:

- **`storage`** reads, writes, moves, and deletes canonical files with expected
  revisions. `BrowserWorkspaceStorage` provides compatible operations;
  `createWorkspaceStorageAdapter` is the concrete bridge and derives its path
  list from `rebuild()` so it reflects canonical ordinary and managed files
  rather than a stale index. `createBrowserStorageAdapter` is the lower-level
  bridge that accepts a caller-supplied `list`. `MemorySyncStorage` is an
  in-memory replica used by tests.
- **`remote`** pushes operations and pulls pages using the shared `SyncPage` and
  `SequencedOperation` shapes, with cursors kept as canonical decimal strings.
- **`codec`** seals a file-change descriptor and opens an encrypted operation
  back to its `workspaceId`, `objectId`, `epoch`, `path`, `previousPath`,
  `baseRevision`, and `content`. The codec owns keys, signing, and verification
  and must throw an error with code `"revoked"` for an untrusted signer or an
  undecryptable operation that indicates revocation. `createFileChangeCodec` in
  `@noura/browser-sync` implements this boundary over the canonical file-change
  codec; a host maps its `browser_sync_untrusted_signer` error to the revoked
  code when it treats an unpinned signer as revocation.
- **`state`** is the durable `SyncStateStore`
  `{version, cursor, pushedRevisions, knownPaths, outbox, conflicts}`. Each
  `SyncConflict` keeps its `operationId`, `objectId`, `path`, `reason`,
  `expectedRevision`, `currentRevision`, `previousPath`, `detectedAt`, and the
  original encrypted `operation`, so a conflicting remote operation can be
  retried without re-pulling. A host supplies IndexedDB, OPFS, or a native
  bridge; `createMemorySyncStateStore` is for tests only.
  `createFileSystemSyncStateStore(fileSystem, path)` persists state through an
  injected `{read, write, remove}` filesystem boundary at an adapter-owned path
  (default `.noura-adapter/sync-state.json`). It returns an empty state only
  when the file is absent, rejects malformed or oversized bytes with a typed
  `InvalidState` error, refuses to overwrite a corrupt file, and resolves a
  write only after the injected filesystem write resolves. Durable state carries
  a schema `version`; reading an older state upgrades it to the current version,
  drops conflict records that no longer hold the encrypted operation, and
  reports the migration through an optional `onMigration` callback (also
  surfaced by the engine as `onStateMigration`) instead of crashing. A state
  written by a newer, unknown version is rejected. Remote replicas are left for
  the caller to purge on revocation.

Adapter state path constraints:

- The state file is **adapter state, never canonical workspace state**. It holds
  only the schema version, cursor, pushed revisions, known paths, the encrypted
  outbox, and recorded conflicts; conflict records retain the encrypted
  operation envelope only. It never holds workspace plaintext or unwrapped keys.
- The path must stay **outside the canonical workspace files and outside every
  workspace backup or snapshot**. The host must place it where
  `BrowserWorkspaceStorage.exportSnapshotEntries`, `BrowserWorkspaceStorage.rebuild()`,
  and native workspace scans do not enumerate it, and it must not appear in a
  recovery kit, an export, or a synchronization payload. The default
  `.noura-adapter/sync-state.json` is a non-canonical name, not an exemption: a
  filesystem boundary whose root overlaps the canonical workspace must exclude
  that location, or the host must supply a separate root for adapter state.
- Because the state file is disposable derived sync metadata, losing or deleting
  it may force a re-diff from canonical files, but it never loses canonical
  content. Deleting an `index.sqlite` remains safe for the same reason.

Behavior:

- `enqueueFileChange` seals through the codec and appends to the durable outbox
  before any network call, returning only after the outbox is persisted.
- `reconcile` flushes the outbox in order in batches of at most 100 operations,
  removing operations only after a successful push. On failure the outbox is
  left intact and a typed error is reported. It then pulls pages from the stored
  cursor and applies the version-1 rules using expected revisions: a non-null
  `baseRevision` requires the current revision to match, a null `baseRevision`
  requires the path to be absent, `previousPath` moves delete the source and
  write the destination, and null `content` deletes. A revision or absence
  mismatch, or an occupied destination, records a `SyncConflict` for that path
  and leaves existing bytes untouched. Each conflict stores the encrypted remote
  operation, and a repeated pull of an operation that already has a conflict is
  skipped so it is never duplicated. The durable cursor advances only after
  every operation in a page is applied or recorded.
- `resolveConflict(operationId, choice)` resolves one recorded conflict. With
  `'remote'` it re-opens the stored encrypted operation through the codec and
  force-applies it — write, delete, or move — to local storage without the
  original `baseRevision` guard, because the user chose the remote bytes, then
  removes the conflict and updates `pushedRevisions`/`knownPaths`. With
  `'local'` it seals the current local bytes for the conflict path as a fresh
  operation based on the conflicting operation's expected revision, enqueues it
  in the outbox so local wins, and removes the conflict; a path that no longer
  exists locally becomes an enqueued deletion. Resolution returns
  `{resolved, remaining}`; an unknown operation id is rejected with
  `ConflictNotFound`, and a failed resolution is reported with `ResolveFailed`
  and leaves the conflict in place. It never fabricates success.
- `snapshotLocalChanges` diffs `list()` plus revisions against
  `pushedRevisions`/`knownPaths` and returns descriptors for added and changed
  paths plus deletion descriptors for missing known paths; unchanged files are
  omitted. The caller seals and enqueues them.
- A revoked codec or remote error, or an explicit `lock()`, transitions the
  engine to a `locked` state, calls the injected `onRevoked` hook so the host can
  clear key material, and refuses further reconciliation. Persisted remote
  replicas are intentionally left for the caller to purge.

`packages/browser-sync-engine` runs an in-memory test suite covering outbox
durability and order, capped batching, push-failure recovery, add/update/delete/
move application, revision and occupied-destination conflicts, conflict
recording with the encrypted operation, remote force-apply and local-wins
resolution, repeated-pull deduplication, unknown-operation rejection, legacy
state migration, cursor advancement, snapshot diffing, revocation locking,
malformed cursor and operation rejection, and a round trip of a shared
`sync-v1.json` fixture file change through the codec boundary.

### App host wiring (`apps/app`)

`apps/app/src/lib/browser-sync.ts` is the controller that binds the packages
above to the hosted app. It reconciles **per object**, not through one shared sync
object:

- The workspace worker exposes `objects_list`, a minimal managed-object
  projection `{id, path, type}` derived from `BrowserWorkspaceStorage.rebuild()`;
  `BrowserWorkspaceFiles.listObjects()` wraps it.
- `enableSync` creates one remote object and one random object key per managed
  local object, wraps every key to this browser device, signs one version-1
  access policy covering all of the objects, and persists only the wrapped keys.
- The reconcile codec seals each file change under the object that owns its path
  and opens operations by `operation.objectId`. A local path with no owning
  object or no object key is skipped and counted in `skippedUnmanaged`; it is
  never sealed under a different object.
- Before reconcile the controller calls `receiveKeys` with the pinned signer set
  (this device plus locally approved devices) and merges delivered keys over the
  local self-wrapped ones. A delivery failure keeps the local keys. A delivered
  key that differs from the stored self-wrapped key is re-wrapped to this device
  with `wrapKey` and persisted in the durable binding, so a later sync can seal
  under it even if the server is unreachable. Only the wrapped envelope is ever
  persisted; the plaintext key stays in tab memory.
- Before reconcile the controller also provisions managed objects created after
  bootstrap. It lists the workspace's managed objects, finds local object ids the
  binding does not yet cover (matched by stable local object id, falling back to
  path), and for each creates a remote object, generates a fresh 32-byte key,
  wraps it to every active browser-capable device, and adds the self envelope to
  the binding. It also wraps an existing bound object's key to any newly active
  browser device that is missing an envelope for that object, skipping an object
  whose key is not available locally. It rebuilds the full signed access policy
  from the current one returned by `access-state` (preserving members, grants,
  documents, and existing envelopes), advances the revision by one canonical
  decimal step, chains `previousPolicyDigest` through `accessDigest` (the SHA-256
  of the canonical signing bytes concatenated with the decoded signature), signs,
  and uploads. A no-op never uploads a policy; a `sync.policy_revision_changed`
  conflict re-reads access-state once and retries; the updated binding and
  revision are persisted only after a successful upload. When a native `age`
  device is active, a browser cannot wrap a new object's key to it, so a new
  object is left unprovisioned and its files stay unmanaged rather than
  uploading an incomplete policy; existing objects keep syncing.
- The controller exposes `bindWorkspace`, and `syncNow`/`workspaceSummary`
  accept an optional `workspaceId`; when no binding is in memory they load the
  durable record for that workspace instead of reporting `not_configured` for a
  workspace that has not mounted yet.
- `workspaceSummary` includes each recorded conflict's `path` and `reason`, and
  `resolveConflict(operationId, choice)` delegates to the engine and refreshes
  the summary.

Device approval is local trust, never server trust: `listWorkspaceDevices`
computes each device fingerprint locally (through `deviceFingerprintForCard`,
which handles both `x25519:` and `age1` recipients), `approveDevice` stores a
device's signing public key only after the supplied fingerprint matches the
locally computed one, and `revokeDeviceApproval` removes it. Only this device
and device keys approved this way are pinned as signers; the access state is
never added to the pins. Approvals are persisted in the durable binding store.

Because the engine's bundled `createWorkspaceStorageAdapter` maps an unguarded
force-apply or force-delete (`expectedRevision` absent) to `null`, which
`BrowserWorkspaceStorage` rejects as "path must be absent", the app supplies its
own `createWorkspaceSyncStorage` adapter. It resolves the current revision for
unguarded calls so a user-chosen remote conflict resolution can overwrite or
delete local bytes, while guarded calls still pass their expected revision
through unchanged.

Provisioning a remote object for a managed object created after bootstrap and
wrapping an existing object key to a newly active browser device are now
implemented in the controller, together with persisting re-wrapped delivered
keys. Epoch rotation for a revoked reader, removal of a revoked device's
envelope, native recovery-kit interoperability, and the device-management UI
remain outstanding; the controller surface exists and now exposes recovery-kit
export and import, but the hosted UI does not yet drive approval, epoch rotation,
or recovery. Adding an object or a reader envelope to a
collaboration-v2 workspace still requires a signed transition that this
controller does not produce, so the server rejects such a policy upload and the
error is surfaced rather than hidden.

## Threat model: installed app versus hosted web client

This is the central difference this document exists to make explicit. The
installed app and the hosted browser client do not have the same trust root, and
the browser client must not be described as equivalent.

### Installed application

- The trust root is the operating-system user account plus the OS credential
  store. Signing seeds, `age` identities, and device session tokens live there.
- The Tauri WebView does not receive provider secrets, and secret material is
  not serialized across the IPC boundary. Credentials are held in zeroizing
  native types.
- The running executable is an installed artifact. Code changes arrive through
  the application's update channel, not by re-fetching JavaScript on every page
  load.
- Malware running under the same OS account is explicitly out of scope
  (`docs/security/threat-model.md`).

### Hosted browser client

- The trust root is whatever JavaScript the origin serves for the current page
  load. There is no OS credential store and no guarantee that tomorrow's bundle
  equals today's.
- **Whoever can serve or inject future JavaScript for the origin can access
  unlocked workspace content.** This includes a compromised host, reverse proxy,
  or CDN, a malicious client update, a cross-site scripting flaw, or a
  compromised third-party script. Such code can call `crypto.subtle.decrypt`
  with the user's unlocked keys and exfiltrate plaintext, and it can do so
  silently while the user sees normal behavior.
- **Non-extractable keys do not defend against this.** `extractable: false`
  prevents _exporting_ key material; it does not prevent _using_ it. Any script
  running in the origin can wield an unlocked key.
- Transport security and Content Security Policy reduce the attack surface but
  do not remove the risk. The hosted build's restrictive CSP and its verified
  account/share import boundary are necessary but not sufficient.
- Browser extensions with broad host permissions, shared profiles, and
  same-account malware can observe or manipulate page state. These are outside
  the protected boundary for the browser client.

### Consequence

The installed application remains the high-assurance client and the trust root
for a workspace. The hosted browser client is a convenience client. It must be
safe to use, but it must never be presented as, or relied upon as, the strongest
custody tier. Workspace content viewed in a browser is only as trustworthy as
the JavaScript the origin is serving at that moment.

## Design options and tradeoffs

Each option is a building block, not a complete system. The recommendation below
composes several of them.

### Option A — non-extractable WebCrypto device key with wrapped workspace keys

Generate the browser's X25519 recipient key and Ed25519 signing key locally with
WebCrypto. Workspace object keys are delivered wrapped to the device's X25519
public key. The browser unwraps them into worker memory only.

- **Pros.** The raw private key is never readable by script after generation;
  no passphrase UX; supports background sync while the page is unlocked;
  matches the native per-device wrapping model; the Ed25519 public key plugs
  into the existing enrollment proof and device fingerprint.
- **Cons.** Non-extractable keys are still _usable_ by any same-origin script
  while loaded, so this does not stop a malicious origin bundle; IndexedDB
  `CryptoKey` persistence support varies; a lost or cleared profile loses keys
  unless re-enrolled; the wrapping construction differs from `age`, so it needs
  the new envelope version described above.

### Option B — passphrase-derived key-encryption key (KEK)

Derive a KEK from a user passphrase with a memory-hard KDF (for example
Argon2id, or PBKDF2 where Argon2id is unavailable) and use it to wrap the
browser device or workspace keys. Nothing secret is stored at rest without the
passphrase.

- **Pros.** Works in essentially every browser; independent of authenticator
  hardware; portable across machines; user-controlled.
- **Cons.** Weak passphrases are brute-forceable if a wrapped blob leaks;
  vulnerable to phishing and keylogging; re-entry friction every session;
  memory-hard KDFs in JavaScript/WASM are slower and easier to weaken; once
  unlocked it provides no protection against malicious served JavaScript.

### Option C — WebAuthn PRF

Register a WebAuthn credential and evaluate the PRF extension with a
Noura-specific salt to derive a stable per-credential secret. Run HKDF over the
PRF output to obtain a KEK that wraps the browser device or workspace keys. The
authenticator requires user verification (biometric or PIN).

- **Pros.** Secret is bound to hardware or a synced passkey and scoped to the
  origin by WebAuthn; phishing-resistant; no password reuse; no secret stored on
  disk; requires an explicit user gesture.
- **Cons.** PRF support remains uneven across browsers, platforms, and
  authenticators in 2026; synced-passkey PRF behavior varies; losing the
  credential requires fallback recovery; the PRF output is available to script
  at evaluation time, so malicious same-origin JavaScript can request it again
  after a user gesture.

### Option D — one-time enrollment via a trusted device

The new browser generates its device key, then an already-trusted device (a
signed-in installed app or an already-authorized browser) explicitly approves
it. The trusted device wraps the workspace object keys to the browser's X25519
public key and uploads signed envelopes through the existing authorized
key-delivery path. No plaintext key ever reaches the server.

- **Pros.** Reuses the existing device-approval fingerprint and key-delivery
  flows; the user authorizes access explicitly and locally; strong against a
  server that stores data but cannot read it; no long-term secret on the server.
- **Cons.** Requires an already-trusted device, so it does not bootstrap the
  first device; approval UX and fingerprint comparison can be skipped or
  socially engineered; the installed app or first client is the trust root.

### Option E — recovery kit

The browser downloads a user-held recovery kit, adapting the existing signed
`noura.sync.recovery` version 1 object. It carries recipient-wrapped object keys
and a recovery identity, but the whole kit is encrypted under a
passphrase-derived (or PRF-derived) KEK before download, so no plaintext key
material is written to disk or persisted in origin storage.

- **Pros.** Offline last resort; user-held, no server recovery; mirrors the
  desktop concept; can be stored anywhere the user chooses.
- **Cons.** File custody and operational burden fall on the user; kit strength
  is only as good as the passphrase; import still requires a signed-in device,
  the matching workspace, and server authorization; the browser cannot write
  outside its sandbox except through a download.

## Recommendation

**Proposal — recommended composition.** Adopt Option A as the browser device
identity, Option D as the only normal way to obtain workspace keys, Option C
(preferred) or Option B (fallback) as the unlock factor protecting the key
material at rest, and Option E as the offline recovery path. Treat `age` in WASM
only for object-key envelopes where required for wire compatibility and for
attachment blobs; prefer the new browser-capable envelope version for long-term
device recipient keys.

Rationale:

- The server stays zero-knowledge: it receives signed ciphertext and
  recipient-wrapped keys, never plaintext.
- No unwrapped secret is persisted to `localStorage`, `sessionStorage`,
  IndexedDB, OPFS, Cache Storage, or logs. Persisted key material is ciphertext
  under a KEK.
- Sign-in is cleanly separated from key access. Account identity and workspace
  keys are obtained by different steps with different authorizations.
- The browser device key is non-extractable while unlocked, and its persisted
  form is always wrapped.
- Enrollment is explicit, reuses existing fingerprint approval and key
  delivery, and does not require a new server trust assumption.
- The design degrades gracefully when PRF is unavailable (passphrase) and when
  no trusted device is online (recovery kit).

**Proposal — at-rest key construction.** Persist a wrapped key bundle, never a
raw key. Generate the device X25519 recipient key and Ed25519 signing key with
WebCrypto. If the platform cannot derive the device identity from the KEK
directly, export the generated key material exactly once, wrap it under the KEK
with AES-256-GCM, zeroize the raw buffer, and delete the unwrapped copy. On
unlock, import the wrapped material into non-extractable WebCrypto keys and keep
them only in the worker. A derivation-only variant — deterministically deriving
the device key pair from the KEK so nothing is stored — is a valid alternative
that removes wrapped-key storage entirely, at the cost of forcing
re-enrollment whenever the unlock factor changes.

**Proposal — lock lifecycle.** The browser client has an explicit locked and
unlocked state. Unlock requires the PRF gesture or passphrase. Locking zeroizes
in-memory object keys in the worker, drops the non-extractable `CryptoKey`
handles, and keeps only wrapped ciphertext at rest. Signing out locks the client
and clears account session material. Closing the tab or crashing returns the
client to a locked state on next load.

## Enrollment flow

1. **Sign in.** The browser completes account authentication using the existing
   account flow (passkey or magic link). This yields an account session and no
   workspace keys.
2. **Create or unlock the device key.** The browser generates the Ed25519
   signing key and X25519 recipient key, or recovers them from the wrapped
   bundle. It establishes the KEK through WebAuthn PRF (preferred) or a
   passphrase (fallback). The wrapped bundle is stored in origin storage; the
   operation envelope and access policy bindings use the public keys only.
3. **Register the device.** The browser requests a device challenge from the
   server and submits the signed possession proof implemented by
   `enrollment_proof` / `enrollmentProof`, binding origin, account,
   `deviceId`, signing public key, X25519 recipient, and challenge in the
   `noura.device.enroll.web` version 1 tuple. This extends
   `noura.device.enroll.v2` with a browser-specific domain and a raw
   X25519 recipient representation. Registration alone grants no content key.
4. **Approve the device.** An owner or admin on a trusted device compares the
   device fingerprint (`noura.device.card`, with the browser recipient form) and
   approves it locally. The server directory is not a trust source.
5. **Deliver keys.** The trusted device wraps the workspace object keys to the
   browser's X25519 recipient, signs every envelope, and uploads through the
   authorized key-delivery endpoint. The server verifies the signer and the
   recipient's authorization but cannot read the key.
6. **Verify and store.** The browser verifies each envelope against locally
   pinned signers, unwraps object keys into worker memory, and writes a
   KEK-wrapped copy. It never trusts a key merely because the server returned it.
7. **Synchronize.** The browser signs operations with its Ed25519 device key and
   uses the same push/pull `SyncTransport` contract as the native client.

**Bootstrap.** The first device for a workspace is the installed app, which is
the trust root. A second browser is enrolled by an already-trusted device
through this flow. A browser-only first device must create the workspace and
hold its keys locally; a second device then needs enrollment or a recovery kit.

## Revocation flow

1. **Server-side.** An owner or admin revokes the browser device and signs the
   next complete access policy with a new epoch and updated recipient
   envelopes for the remaining approved devices. The revoked device's sessions
   and tokens are invalidated, and further uploads or key deliveries are
   refused. This matches the existing native revocation rules: revoked
   recipients stop uploading until keys are rotated.
2. **Browser-side.** On observing revocation, the client transitions to a
   locked, revoked state: it clears wrapped key bundles and in-memory keys,
   deletes origin-stored workspace replicas, and refuses to decrypt. It must not
   keep serving cached plaintext from OPFS, Cache Storage, or memory.
3. **Honest limits.** Revocation cannot retroactively erase content a revoked
   client already decrypted, viewed, or copied. Epoch rotation makes new content
   unrecoverable to the removed device; it is not retroactive per-file forward
   secrecy. A lost or offline browser profile cannot be wiped remotely; only its
   server authorization is removed.

## Recovery flows

1. **Locked or forgotten KEK, same browser.** Re-derive the KEK through the PRF
   credential, or through the passphrase fallback. If both are lost, the
   persisted bundle is unreadable; the browser is treated as a new device and
   must re-enroll.
2. **New browser with a trusted device available.** Use the enrollment flow. The
   trusted device approves the new device and delivers keys.
3. **New browser without a trusted device (browser-specific kit).** Import the
   user-held `noura.browser-recovery-kit`. The kit restores the **same** device
   identity — the wrapped Ed25519 signing seed, X25519 recipient secret, and
   device bearer token — together with its durable binding, so the object keys
   the device already held become usable on the new browser without a new
   approval or a key rewrapped to a new device. The kit cannot bypass server
   authorization: a revoked or expired device token is refused by the server, and
   a revoked membership still requires re-invitation. Server-side authorization
   remains authoritative over any imported credential.
4. **Installed app kit as a trust source.** Native-exported kits remain the
   signed `noura.sync.recovery` object, which is `age`-only; a browser recipient
   still returns `sync_browser_recovery_unsupported`. The browser
   `noura.browser-recovery-kit` format is implemented, but interoperating the two
   is a **Proposal** that requires its own shared conformance fixtures.
5. **Never restored by native recovery.** Native recovery never restores an old
   signing identity, device session, or server trust. The browser-specific kit
   instead restores the same device's wrapped custody rather than issuing a new
   identity; it is full credential material and must be encrypted and user-held,
   and it is not a substitute for device approval or server authorization. Newly
   encountered signers still require explicit device approval.

## Browser storage durability and eviction

**Proposal.** Treat every browser store as a disposable replica and document
that plainly in the client UI.

- **OPFS.** Durable across reloads in normal use, but subject to origin quota and
  eviction under storage pressure unless `navigator.storage.persist()` is
  granted, and always deletable by the user's "clear browsing data". Some
  browser configurations evict script-writable storage aggressively after
  periods of inactivity. Private or incognito sessions are ephemeral.
- **IndexedDB.** Same eviction and clearing rules. It may hold only wrapped key
  bundles and non-secret metadata; never unwrapped keys or plaintext content.
- **Cache Storage and service workers.** Must never contain plaintext content or
  secrets. A service worker must not be used to serve stale workspace bytes as
  if they were current.
- **In-memory worker state.** Object keys live only in the worker while
  unlocked. JavaScript engines do not guarantee prompt zeroization, and crash
  dumps or snapshots are outside our control. We do not claim reliable memory
  erasure; we claim only that we clear references and zeroize buffers we own.
- **Multiple tabs.** Use the Web Locks API (already used by browser storage) to
  serialize replica mutations, and require each tab to unlock independently.
- **Adapter sync state.** The durable `SyncStateStore` file written by
  `createFileSystemSyncStateStore` is disposable adapter metadata, not a
  workspace file and not a backup. It shares the eviction and clearing rules
  above, and it must be stored outside the canonical workspace and its backups so
  it is never enumerated, exported, or synchronized. If it is evicted, the host
  rebuilds the baseline by diffing canonical files; nothing canonical is lost.
- **Consequence.** A browser workspace can be lost entirely to eviction or
  clearing. The canonical copy must exist on a native device, or be recoverable
  through an enrolled trusted device or a recovery kit. The hosted web client is
  never the sole custodian.

## Out of scope and explicitly not claimed

- Browser workspace synchronization is not implemented end to end. The operation
  cryptography and push/pull transport exist at the library level, the local
  replica engine and its durable file-system state store exist in
  `packages/browser-sync-engine`, and the `apps/app` controller now wires the
  worker's managed-object list, per-object keys, pinned key delivery, device
  approval, binding auto-restore, conflict resolution, and the browser-specific
  recovery kit. It cannot open a synchronized workspace without a trusted device
  approving this browser and delivering keys, and key-rotation application,
  native recovery-kit interoperability, and the device-management UI are not
  implemented. This document does not authorize implementing those as part of
  unrelated work.
- The hosted web client is not claimed to be as secure as the installed app.
  Whoever serves future JavaScript can access unlocked content.
- Non-extractable keys are not claimed to prevent exfiltration; they prevent
  export, not use.
- Browser memory is not claimed to be reliably zeroized.
- Revocation is not claimed to erase already-fetched plaintext.
- Epoch rotation is not claimed to provide retroactive per-file forward secrecy.
- Recovery kits are not claimed to bypass server authorization or owner
  approval.
- WebAuthn PRF is not claimed to be available in every browser, platform, or
  authenticator; the passphrase fallback exists precisely because it is not.
- Encrypted attachments, streaming `age` blobs, collaborative text, presence,
  and plugin execution in the browser are out of scope for the first slice.
- Protection from malicious browser extensions, shared browser profiles, or
  same-account malware is not claimed.
- OPFS and IndexedDB are not claimed to be durable backups.
- The browser-specific recovery kit (`noura.browser-recovery-kit` version 1) is
  implemented, but native `noura.sync.recovery` interoperability remains a
  proposal until shared Rust and TypeScript conformance fixtures exist. The
  browser key-envelope, recipient, device fingerprint, enrollment-proof, and
  operation formats are implemented against those fixtures.
- This document does not relax the locked invariants. Where an optional design
  would violate one, the design is wrong, not the invariant.

## Follow-up work required before this becomes locked

1. Approve or reject the recommended composition and record the decision.
2. ~~Specify the browser key-envelope domain, version, and signing tuple; add
   Rust and TypeScript conformance fixtures.~~ **Implemented** as
   `noura.sync.key.web` version 1 in `crates/sync-key-envelope` and
   `packages/sync-key-envelope`, with the shared fixture
   `docs/workspace-format/fixtures/browser-key-v1.json`.
3. ~~Specify the browser device fingerprint form and update
   `device_fingerprint` handling for a raw X25519 recipient.~~ **Implemented.**
   The `noura.device.card.web` fingerprint, the `x25519:` recipient encoding, and
   the `noura.device.enroll.web` proof are implemented in
   `crates/sync-key-envelope` and `packages/sync-key-envelope`, with the shared
   fixture `docs/workspace-format/fixtures/browser-device-v1.json`. The
   `apps/server` enrollment path accepts the browser recipient and verifies the
   browser proof, and the native `device_fingerprint` in
   `crates/local-core/src/sync/approvals.rs` now renders the browser fingerprint,
   with a conformance test against the shared fixture vector.
4. Specify PRF salts and HKDF `info` strings, and define the passphrase fallback
   parameters and minimums. **Passphrase fallback implemented.**
   `packages/browser-sync` derives the key-encryption key with PBKDF2-SHA256
   (32-byte random salt, a minimum of 310,000 iterations, 32-byte output) and
   wraps the device material with AES-256-GCM. PRF salts and the PRF unlock path
   remain unspecified.
5. Extend the server enrollment and key-delivery paths for browser devices,
   including revocation and epoch rotation. **Partially implemented.**
   `apps/server` now accepts browser enrollment, verifies and stores
   `noura.sync.key.web` envelopes, returns them from key delivery and
   access-state, and accepts web envelopes in signed access policies. The native
   `crates/local-core` client now recognizes browser recipients, computes their
   fingerprint, wraps rotated keys to them, verifies and unwraps their envelopes,
   and signs and verifies the browser access-policy tuple. The browser client
   foundation in `packages/browser-sync` now performs enrollment, pinned key
   delivery, operation seal/open, canonical file-change encoding/decoding, the
   file-change codec, and the push/pull transport, conforming to the
   shared `operation-v1.json` fixture, and `packages/browser-sync-engine` now
   implements local replica reconciliation at the library level: durable outbox,
   capped ordered push, cursor-based pull and version-1 application, conflict
   recording and resolution, legacy state migration, snapshot diffing,
   revocation-driven lock state, a durable file-system `SyncStateStore` at an
   adapter-owned path, and a concrete `createWorkspaceStorageAdapter` bridge over
   `BrowserWorkspaceStorage` that derives its path list from `rebuild()`. The
   `apps/app` controller now wires the worker's managed-object list, per-object
   objects and keys, pinned key delivery, locally verified device approval,
   binding auto-restore, conflict resolution, post-bootstrap object
   provisioning, new-device envelope wrapping, and durable re-wrapping of
   delivered keys, and supplies a storage adapter that preserves the engine's
   unguarded force-apply and force-delete. Still open:
   epoch rotation for a revoked reader, browser object activation, native
   recovery-kit interoperability, and device-management UI wiring.
6. Define the browser recovery-kit format and its conformance fixtures.
   **Implemented (browser-specific).** The `noura.browser-recovery-kit` version 1
   format and its `exportRecoveryKit`/`importRecoveryKit` implementation live in
   `packages/browser-sync`, with in-memory round-trip, wrong-passphrase, tamper,
   unknown format/version, oversize, and no-plaintext tests and controller wiring
   tests in `apps/app`. Native `noura.sync.recovery` interoperability and shared
   Rust/TypeScript conformance fixtures remain to be defined.
7. Add a browser-specific section to `docs/security/threat-model.md` once the
   design is approved.
