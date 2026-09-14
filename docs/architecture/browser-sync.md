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
`docs/workspace-format/fixtures/browser-key-v1.json`. This is a format and
fixture foundation only. No enrollment, key delivery, revocation, key storage,
or synchronization is implemented, and the hosted browser client still cannot
open a synced workspace.

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
run `docs/workspace-format/fixtures/browser-key-v1.json`. The fingerprint
extension for a raw X25519 recipient (follow-up 3 below) is still a proposal.

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
   server and submits a signed possession proof binding origin, account,
   `deviceId`, signing public key, X25519 recipient, and challenge. This
   extends `noura.device.enroll.v2` with a browser-specific domain and a raw
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

- Browser workspace synchronization is not implemented by this change, and this
  document does not authorize implementing it as part of unrelated work.
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
- The browser recovery-kit and browser key-envelope formats are proposals, not
  finalized formats, until shared Rust and TypeScript conformance fixtures exist.
- This document does not relax the locked invariants. Where an optional design
  would violate one, the design is wrong, not the invariant.

## Follow-up work required before this becomes locked

1. Approve or reject the recommended composition and record the decision.
2. ~~Specify the browser key-envelope domain, version, and signing tuple; add
   Rust and TypeScript conformance fixtures.~~ **Implemented** as
   `noura.sync.key.web` version 1 in `crates/sync-key-envelope` and
   `packages/sync-key-envelope`, with the shared fixture
   `docs/workspace-format/fixtures/browser-key-v1.json`.
3. Specify the browser device fingerprint form and update
   `device_fingerprint` handling for a raw X25519 recipient.
4. Specify PRF salts and HKDF `info` strings, and define the passphrase fallback
   parameters and minimums.
5. Extend the server enrollment and key-delivery paths for browser devices,
   including revocation and epoch rotation.
6. Define the browser recovery-kit format and its conformance fixtures.
7. Add a browser-specific section to `docs/security/threat-model.md` once the
   design is approved.
