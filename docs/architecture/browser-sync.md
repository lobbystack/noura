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

The encrypted sync service currently serves account, device-approval,
invitation, and encrypted public-viewer pages for the hosted `apps/app` build.
Browser workspaces are explicitly unavailable in the hosted build today. This
document describes what browser workspace synchronization would require; it does
not claim that browser workspaces work, and it does not authorize starting that
work as part of an unrelated task.

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
envelopes, verifies browser envelope signatures on key upload and access
policies, and returns the construction and browser fields from key delivery and
access-state. The server's `POST`/`GET /v1/workspaces/:workspace/operations`
push/pull routes and the `noura.sync.payload`/`noura.sync.operation` tuples were
already the shared native protocol; the browser client now speaks them at the
library level. The hosted browser client does not exist yet, so a browser still
cannot open a synced workspace, and browser revocation/epoch-rotation wiring,
browser recovery kits, local replica reconciliation, outbox/conflict handling,
and UI wiring are not implemented.

**Implemented native client support.** `crates/local-core` now depends on
`crates/sync-key-envelope` and treats a browser device as a first-class remote
recipient: it recognizes the `x25519:` `encryptionRecipient`, renders the
`noura.device.card.web` fingerprint, wraps object keys to a browser recipient
with `sync-key-envelope::wrap_key` (supplying the ephemeral secret, salt, and
nonce from the OS random source), verifies and unwraps browser-authored
`noura.sync.key.web` envelopes against locally pinned signer keys before
decryption, and reads and produces the extended `construction` plus browser
fields from key delivery, access-state, and signed access policies. Native `age`
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
`operation-v1.json` fixture. The hosted browser still has no local replica
reconciliation, no outbox or conflict handling, no revocation lock state, no
recovery kit, and no UI wiring, so it still cannot open or synchronize a
workspace end to end.

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
device key. Encrypted attachments (payload version 3) and streaming `age` blobs
are out of scope for the first browser sync slice because of browser memory and
file-size limits.

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
- **Persistence.** A `KeyEnvelope` round-trips both constructions through serde;
  `age` still serializes as the seven-field object. Browser secret material stays
  in the zeroizing native credential record, and `DeviceKeys::create_browser`
  adds a native process that can hold a `x25519:` recipient identity.

### Remaining limitations

- The browser client foundation exists in `packages/browser-sync` (device custody,
  passphrase unlock, enrollment, pinned key delivery, operation seal/open, and the
  push/pull transport), but there is no local replica reconciliation, no outbox or
  conflict handling, no revocation-driven lock state, no recovery kit, and no UI
  wiring, so the hosted browser still cannot open or synchronize a workspace end
  to end.
- Browser key envelopes are delivered to clients, but clients must still verify
  them locally against pinned signer keys. The server is not a trust source.
- Revocation and epoch rotation reuse the signed access-policy path, which now
  accepts web envelopes; the native coordinator now wraps rotated keys for
  approved browser recipients, but no browser UI enforces a revoked locked state.
- Collaboration-v2 object activation (`noura.sync.object-activation`) still
  parses only the three-field native envelope. The native activation paths
  (`ObjectActivation::sign`/`verify`, `receive_activations`, and
  `activate_pending_objects`) reject a browser recipient with the structured
  `sync_browser_activation_unsupported` error instead of mis-verifying it;
  activating a new object for a browser recipient is not implemented.
- The policy-level `noura.sync.access` tuple extension for browser entries is now
  signed and verified by the native Rust client against the server contract, but
  no shared Rust/TypeScript conformance fixture covers it yet.
- The browser recovery-kit format remains a proposal. Native recovery kits and
  recovery identities are `age`-only; a browser recipient returns
  `sync_browser_recovery_unsupported` rather than emitting an unusable kit.

## Browser client foundation

`packages/browser-sync` (`@noura/browser-sync`) is the first browser-side client
code. It is a transport-neutral library that uses `globalThis.crypto.subtle` and
an injected `fetch`; it performs no UI work and holds no global state. It is a
foundation, not a working browser workspace: operation sealing/opening and the
push/pull transport are implemented, but local replica reconciliation, outbox and
conflict handling, revocation lock state, recovery kits, and UI wiring are not.

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

Remaining browser operation work is the local replica and orchestration layer,
not cryptography: pulling and applying operations to a replica, an outbox for
unacknowledged pushes, conflict detection and resolution, and revocation-driven
lock state.

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
3. **New browser without a trusted device.** Import the user-held recovery kit.
   The kit is unlocked with its passphrase, object keys are unwrapped and rewrapped
   to the new device key, and the workspace remains paused until device approval
   and any required epoch rotation complete. The kit cannot bypass server
   authorization; a revoked membership still requires re-invitation.
4. **Installed app kit as a trust source.** A native-exported kit can seed a
   browser import, but the browser variant must re-encrypt under a browser KEK
   and must never persist the exporting device's raw `age` recovery identity.
   Adding a browser recovery-kit format is a **Proposal** and requires its own
   conformance fixtures.
5. **Never restored.** Recovery never restores an old signing identity, device
   session, or server trust. Newly encountered signers still require explicit
   device approval.

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
- **Consequence.** A browser workspace can be lost entirely to eviction or
  clearing. The canonical copy must exist on a native device, or be recoverable
  through an enrolled trusted device or a recovery kit. The hosted web client is
  never the sole custodian.

## Out of scope and explicitly not claimed

- Browser workspace synchronization is not implemented end to end. The operation
  cryptography and push/pull transport exist at the library level, but no local
  replica, outbox, conflict handling, revocation lock state, recovery kit, or UI
  is implemented, and this document does not authorize implementing those as part
  of unrelated work.
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
- The browser recovery-kit format remains a proposal, not a finalized format,
  until shared Rust and TypeScript conformance fixtures exist. The browser
  key-envelope, recipient, device fingerprint, enrollment-proof, and operation
  formats are implemented against those fixtures.
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
   delivery, operation seal/open, and the push/pull transport, conforming to the
   shared `operation-v1.json` fixture. Still open: local replica reconciliation,
   outbox and conflict handling, revocation-driven client lock state, browser
   object activation, browser recovery kits, and UI wiring.
6. Define the browser recovery-kit format and its conformance fixtures.
7. Add a browser-specific section to `docs/security/threat-model.md` once the
   design is approved.
