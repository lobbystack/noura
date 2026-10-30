# Encrypted sync protocol, version 2

Status: signed off on 2026-10-21. The owner delegated sign-off to Claude, the orchestrating agent, on 2026-10-20. A ticket that corrects this spec does so in the same commit as its code, and its review checks the change.

This document defines Noura's encrypted sync protocol for the Rust product: the records a desktop client signs and encrypts, the keys that let devices and members read them, the server's checks, and the HTTP API that carries them. It replaces [sync-v1.md](sync-v1.md), [checkpoints.md](checkpoints.md) and [collaboration.md](collaboration.md), which describe the legacy app.

The server stores ciphertext and the routing fields listed in [§29](#29-what-the-server-stores). It never needs plaintext workspace content to store, order, route or compact your data.

## 1. Goals

- Sync a workspace of 100,000 objects across 200 devices, with no request or response body above 1 MiB.
- Keep key rotation proportional to devices and key domains. Removing a device never re-encrypts objects.
- Let a client detect a server that drops, reorders, replays or forks a device's operations, or that forks or rolls back an account's device chain, the policy, the keys or the manifest.
- Keep object types, creation times and exact sizes out of the data the server stores. The server still sees when requests arrive, and its blob store's file times date each attachment upload ([§29](#29-what-the-server-stores)).
- Let a new device download current state without replaying full history.
- Keep both versions when two devices change the same object.

Out of scope: hiding when you sync, how much you sync, your IP address, your account email, or who belongs to a workspace.

## 2. Version 1 is unsupported

Version 2 shares no record, route or fixture with version 1. A v2 client refuses a server that doesn't advertise protocol 2 ([§26](#26-http-api)), and a v2 server serves no `/v1` route. Nothing migrates: v1 workspaces, accounts, devices, keys, operations and recovery kits stay with the legacy app, and the hosted v1 database is wiped at cutover. To move a workspace from the legacy app, turn on v2 sync from its folder as if it were new.

## 3. Primitives

| Purpose | Primitive | Rust crate, version and license |
|---|---|---|
| Signatures | Ed25519 (RFC 8032), verified with `verify_strict` by the client, the server and `account-web` alike | `ed25519-dalek` 3.0.0, BSD-3-Clause |
| Key wrapping | Hybrid Public Key Encryption (HPKE, RFC 9180) base mode: the X-Wing key encapsulation mechanism (ML-KEM-768 combined with X25519, draft-connolly-cfrg-xwing-kem-10) `0x647a` as draft-ietf-hpke-pq-04 defines it for HPKE, HKDF-SHA256 `0x0001`, ChaCha20Poly1305 `0x0003` | `hpke` 0.14.1 with the features `mlkem` (which brings `x-wing` and `ml-kem`), `x25519`, `hkdfsha2` and `chacha`; hpke exports its X-Wing KEM only when both `mlkem` and `x25519` are on, and `mlkem` doesn't enable `hkdfsha2`, so the build names both; MIT or Apache-2.0 |
| Record encryption | XChaCha20-Poly1305 with a random 24-byte nonce | `chacha20poly1305` 0.11.0, MIT or Apache-2.0 |
| Attachment encryption | STREAM (Hoang, Reyhanitabar, Rogaway and Vizár) over XChaCha20-Poly1305, 32-bit big-endian counter and last-chunk flag | `aead-stream` 0.6.0, type `StreamBE32<XChaCha20Poly1305>`; MIT or Apache-2.0 |
| Digests, message authentication codes, key derivation | BLAKE3: `hash`, `keyed_hash`, `derive_key` | `blake3` 1.8.7, CC0-1.0 or Apache-2.0 |
| Randomness | the operating system's cryptographically secure random number generator (CSPRNG) | `getrandom` 0.4.3, MIT or Apache-2.0 |
| Text-mode note bodies ([§22](#22-live-collaboration)) | yrs update format v1 with client IDs below 2^53 | `yrs` 0.26.0 with its default features off, so neither `small-client`, which holds a client ID in 32 bits, nor `sync` is on, and with one patch for surrogate pairs, below; MIT |

Ticket 43 pins these versions in `Cargo.lock`, which also holds older ones (`ed25519-dalek` 2.2.0, `chacha20poly1305` 0.10.1) for the v1 sync code in `crates/local-core`. That code stays on `age` 0.11: `age` 0.12 depends on `hpke` 0.12 and `ml-kem` 0.2, whose `kem` 0.3 pre-release can't share a lockfile with the `kem` 0.3 that `hpke` 0.14.1's X-Wing needs. The workspace's `[workspace.dependencies]` turns on `small-client` and `sync` for `crates/local-core`, whose v1 sync code needs Yjs 13's 32-bit client IDs ([yrs provenance](../provenance/yrs.md)). The collaboration sandbox's guest module builds from its own crate, which the workspace's `exclude` list holds like `crates/workspace-format-wasm`, and declares `yrs = { version = "=0.26.0", default-features = false }` directly, so no feature unification turns `small-client` on. A module built with it fails the pinned module hash ([§22](#collaboration-sandbox)) and the fixture whose client ID needs more than 32 bits. The guest also patches yrs's `ItemContent::splice` to split strings as Yjs's `ContentString.splice` does: when integration splits a string item between the two UTF-16 units of a surrogate pair, the left part ends and the right part starts with U+FFFD, one unit each. yrs 0.26.0 leaves that code commented out and rounds the split to the next code point while the item keeps the offset as its length, so a device that integrates records as they arrive and one that bootstraps from a snapshot would reach different bodies. The module hash covers the patch, and ticket 43 records it in the [yrs provenance](../provenance/yrs.md) file before it lands. From then on, a version change updates this table. Continuous integration runs the published test vectors for each primitive: for the HPKE suite above, the base-mode vectors of the draft-ietf-hpke-pq reference implementation (`hpkewg/hpke-pq` at commit `6433c8f`, shipped with `hpke` 0.14.1 as `test-vectors/pq-6433c8f.json`), which cover X-Wing with HKDF-SHA256 and ChaCha20Poly1305; Wycheproof vectors for XChaCha20-Poly1305, Ed25519 and X25519 (low-order points included); and the BLAKE3 reference vectors. RFC 9180's own vectors cover no X-Wing suite.

X-Wing keeps an envelope safe while either ML-KEM-768 or X25519 holds. The server keeps envelopes, and lineage turns one current key into every earlier key of its domain ([§9](#9-key-domains)), so an envelope wrapped with X25519 alone and copied today would open a domain's whole history once a quantum computer breaks X25519.

XChaCha20-Poly1305 removes the nonce-collision limit that AES-GCM's 96-bit random nonces impose on long-lived keys such as presence keys. Every random value in this document comes from the operating system's CSPRNG.

Each epoch key ([§9](#9-key-domains)) serves several purposes, so clients derive one subkey per purpose with BLAKE3 `derive_key(context, epoch_key)`:

| Context string | Subkey for |
|---|---|
| `noura.sync.v2 operation key` | operation payloads |
| `noura.sync.v2 lineage key` | the lineage field of an epoch record |
| `noura.sync.v2 key check` | the `key_check` field of an epoch record |
| `noura.sync.v2 presence key` | presence payloads and presence channels |
| `noura.sync.v2 profile key` | the workspace profile |

Nine contexts derive from other inputs. `noura.sync.v2 share key` derives the snapshot key from a share link's key. `noura.sync.v2 invitation key` derives the context key, and `noura.sync.v2 invitation proof` the proof key, from an invitation secret. `noura.sync.v2 recovery kit` derives the kit key from the kit's secret and salt ([§12](#12-recovery-kit)). `noura.sync.v2 blob nonce` derives a blob's nonce prefix from its key and ID ([§21](#blob-format)). `noura.sync.v2 account id` derives the account ID key, `noura.sync.v2 secret check` the secret's key check, `noura.sync.v2 passkey user` the key of each account's passkey user handle, and `noura.sync.v2 device confirmation` the key of the device page's confirmations ([§7](#desktop-sign-in)), from the server's secret ([§14](#after-a-server-restore)).

## 4. Encoding

Every signed record, hashed tuple, request and response uses one binary encoding. The bytes a device signs are the bytes it sends, so a verifier never rebuilds a tuple from parsed fields. `sync-protocol` owns this encoding, and `sync-key-envelope` builds the device card, the enrollment proof, the account chain link and the envelope on top of it, so both crates use one encoder.

### Field types

| Type | Bytes |
|---|---|
| `u8`, `u16`, `u32`, `u64` | unsigned integer, big-endian, fixed width |
| `id` | 16 bytes |
| `digest` | 32 bytes, a BLAKE3-256 output |
| `key` | 32 bytes |
| `nonce` | 24 bytes |
| `sig` | 64 bytes, an Ed25519 signature |
| `bytes` | `u32` length, then that many bytes |
| `str` | `u16` length, then that many bytes of UTF-8 |
| `list<T>` | `u32` count, then that many `T` |
| `opt<T>` | `u8` 0 for absent, or `u8` 1 followed by `T` |

### Tags

Every record and hashed tuple starts with a tag: one byte holding the length of its domain string, the ASCII domain string, then the protocol version as `u16`, which is `0x0002`. This document writes `tag(noura.sync.op)` for the 16 bytes `0d 6e6f7572612e73796e632e6f70 0002`. [§32](#32-fixtures) lists every tag's exact bytes.

A v2 tag starts with a byte below `0x40`, so no v2 signing input can collide with a v1 JSON tuple, which starts with `[` (`0x5b`).

Unsigned request and response bodies ([§26](#26-http-api)) carry no tag: the route fixes their type.

### Records, signatures and digests

A signed record is its body followed by `sig`, an Ed25519 signature over the body bytes. The signer is the device named in the body, or the device the body's context names. The record's digest is `BLAKE3(body || sig)`.

When a record carries ciphertext, its associated data for authenticated encryption (AEAD) is the body prefix that ends before its `nonce` field. Each layout below marks that point. A record encrypted under a subkey of an epoch key (an operation, presence record or profile) appends to that prefix the digest of the epoch record that announced the epoch. The record doesn't carry that digest: writer and reader each take it from their accepted epoch chain. A writer and a reader that hold different epoch records for one epoch therefore can't read each other's records.

### Decoding rules

Decoders reject, with a structured error and without panicking:

- trailing bytes after the last field;
- a length above the limit for that field ([§27](#27-limits)) before allocating, and a `bytes` field whose fixed length this document states at any other length;
- an unknown tag, a version other than 2, or an unknown enum value;
- a record whose tag isn't the tag of the type its position names, such as a `record_ref` type, a route or a field;
- an `opt` discriminator other than 0 or 1;
- a list that this document calls sorted but that isn't strictly increasing by its sort key;
- an all-zero `id` where this document forbids it;
- invalid UTF-8 in a `str`;
- a card whose device ID doesn't derive from its signing key ([§5](#5-identifiers)).

A field that [§27](#27-limits) gives no limit of its own takes two general ones: a `str` holds at most 4,096 bytes, and a `bytes` length or `list` count can't exceed the bytes left in the body, each list element counted at its smallest encoding. `sync-v2-encoding.json` pins the limit of every variable field.

A sorted list's sort key is the whole element, or the fields the list names, compared in order: IDs, digests and strings as unsigned bytes, integers by value. Each element's key lies strictly above the previous one's, so a sorted list holds no duplicate key: an `access` list sorted by account names each account once, and leaf refs sorted by device and counter name each position once.

## 5. Identifiers

Remote IDs are 16 bytes, random from the operating system's CSPRNG except where the table derives them. They carry no timestamp, version bits, type or local object ID, and the all-zero value is reserved.

In URL paths, query parameters and headers, binary values are base64url without padding (RFC 4648 §5): a 16-byte ID or token takes 22 characters, and a 32-byte value (a digest, an offer or checkpoint digest, a token hash, a session token) takes 43. Integers (`after`, `at`, `level`, `index`, `epoch`, `counter`, a revision or key sequence) are unsigned decimal with no sign or leading zeros. A structured value passed in a query, such as a `live_cursor` ([§26](#message-bodies)), is base64url of its encoding. The header `Authorization: Bearer <token>` carries the session token's 43 characters.

| ID | Created by | Notes |
|---|---|---|
| Account ID | server | derived from the email the account signs up with, under a key from the server's secret, so signing up again after a restore gets the same ID and Reset devices can give it a new chain ([§7](#resetting-your-devices)) |
| Device ID | derived | the first 16 bytes of `BLAKE3(tag(noura.sync.device.id) \|\| signing_public)`, so no other key pair can claim it |
| Card index | derived | a card's number in one workspace, from that workspace's policy log ([§8](#card-index)); it places the card's yrs client IDs ([§22](#the-document-on-a-device)) |
| Workspace ID | the creating device | random, so it pins no log: a device trusts a workspace only from an anchor ([§8](#verifying-the-chain)); unrelated to the local `workspace_...` ID in `.noura/workspace.yaml` |
| Domain ID | the device that creates the domain | unique within its workspace; the server keys domain and blob rows by workspace and ID, and `id_conflict` applies within a workspace |
| Remote object ID (`rid`) | the device that first syncs an object | stable across moves inside one domain; a move to another domain gets a new `rid` ([§9](#moving-objects-between-domains)) |
| Text lineage ID | derived, not random: from the `rid`, the base operation and the first document's bytes | names the yrs document of a text-mode note, so two devices that start one from the same state agree on it ([§22](#22-live-collaboration)) |
| Blob ID | the uploading device | unique within its workspace |
| Invitation ID, invitation token | the inviting device | the server stores the token's hash |
| Share token | the sharing device | the server stores the token's hash |
| Presence session ID | the device, per connection | never reused |
| Replica ID | the app, when it connects a folder to a remote workspace | local only, never sent ([§15](#client-journal)) |

An operation has no ID of its own. Its identity is its position in a device chain, `(device, domain, counter)`, and its digest.

The local object ID (a managed file's frontmatter `id`, or nothing for an ordinary file) travels only inside ciphertext. The client's sync journal maps each `rid` to its local object and current path.

## 6. Padding

Every encrypted payload, blob, presence message, profile, share snapshot and invitation context is padded with Padmé (Nikitin and others, "Reducing Metadata Leakage from Encrypted Files and Communication with PURBs", PETS 2019) before encryption.

For a framed length `L`:

```text
padme(L):
  if L < 2: return L
  E = floor(log2(L))
  S = floor(log2(E)) + 1
  mask = 2^(E - S) - 1
  return (L + mask) & ~mask

padded_length(L) = max(256, padme(L))
```

A padded plaintext is `u32 n || content || zero bytes`, where `n` is the content length and the total length is `padded_length(n + 4)`. A length `x` is a valid padded length when `x >= 256` and `padme(x) = x`. Blobs use a `u64` length prefix instead ([§21](#21-attachments)). Unpadding happens after AEAD verification and before any parsing. It rejects a total length that isn't a valid padded length, an `n` larger than the space after the prefix, and any nonzero padding byte.

**Overhead bound.** Lengths below 256 pad to 256. From 256 bytes, Padmé adds less than 2^-S of the framed length: at most 6.25% below 64 KiB, at most 3.125% below 4 GiB, and at most 1.5625% above. From 256 bytes up, the worst case is 6.247%, at a framed length of 32,769 bytes. Padmé leaks `O(log log L)` bits of the length.

The server checks padding without reading content: it rejects an operation, presence message, profile, share snapshot or invitation context whose ciphertext length minus the 16-byte tag isn't a valid padded length, and a blob whose length doesn't match a valid padded plaintext ([§21](#21-attachments)). A client that forgets to pad can't upload.

## 7. Devices

### Device card

A device holds an Ed25519 signing key and an X-Wing HPKE key pair. Its card binds them to its account.

`card`, unsigned:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.device.card)` | |
| account | `id` | |
| device | `id` | derived from `signing_public` ([§5](#5-identifiers)) |
| signing_public | `key` | Ed25519 public key |
| hpke_public | `bytes` | X-Wing public key, exactly 1,216 bytes |

The device fingerprint is `BLAKE3(card)`. Apps show it as 64 lowercase hexadecimal characters in 16 groups of 4, and the device list shows every device's full fingerprint.

The keys live in the operating system's credential store, in items that never sync or migrate to another machine. On macOS they are data protection keychain items with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` and synchronization off. On Windows they are keys bound to the machine through DPAPI or CNG. On Linux the Secret Service can't stop a copied home folder from carrying them, and the app documents that limit. The X-Wing secret key is its 32-byte seed. A sync journal ([§15](#client-journal)) found without its device keys belongs to a new device, which enrolls and waits for approval.

### Enrollment proof

A signed-in device enrolls by proving it holds its signing key. The server issues a fresh 32-byte challenge, valid for 10 minutes and usable once.

`enroll`, signed with the card's signing key:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.device.enroll)` | |
| origin | `str` | the server's public origin in ASCII: lowercase scheme and host, port only when not the default, no trailing slash, such as `https://sync.noura.app` |
| card | `bytes` | the encoded card |
| challenge | `key` | 32 bytes from the server |

The server verifies the signature with the card's `signing_public`, then checks that `origin` is its own configured origin, the card's account is the session's account, the challenge is current and unused, and the device ID derives from the card's signing key and is new. `sync-key-envelope` exports this verifier.

A device whose session ended without its card being removed, such as after a server restore lost the session's row ([§14](#after-a-server-restore)), signs in again and enrolls with its existing card and a fresh proof. When the server holds that card byte for byte and no chain of the account removed it, the server binds the new session to the existing card instead of answering `id_conflict`. A card whose device ID the server holds with other bytes, or that a link of its account removed or a reset retired, fails with `id_conflict`. Signing in again never replaces the keys the credential store holds for an active card: the device takes only the new session token. A device whose card gets `id_conflict` keeps that card's secrets aside in the credential store and enrolls a new card, which needs approval. When a restore lost the account itself, signing up again with the same email gets the same account ID ([§14](#after-a-server-restore)).

An account's first device starts the account's chain when it enrolls ([Account chain](#account-chain)) and can then create workspaces. Every later device needs approval from an active device of the account ([§10](#10-approval)), except a device you recover with a kit ([§12](#12-recovery-kit)) and the first device of a reset ([Resetting your devices](#resetting-your-devices)).

### Account chain

Each account has one account chain: a linear hash chain of signed links that holds the account's device cards and its recovery identity. It follows Keybase's per-user sigchain. The server stores it and checks every link, and every device that shares a workspace with the account verifies it. The account's fingerprint is the digest of the chain's genesis link, shown like a device fingerprint ([Device card](#device-card)).

`account_link`, signed:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.account)` | |
| account | `id` | |
| seqno | `u64` | 1 for the genesis, then +1 |
| previous | `digest` | digest of link `seqno` − 1; 32 zero bytes at seqno 1 |
| signer | `id` | device ID of the signing card |
| kind | `u8` | table below |
| body | kind-specific | table below |
| sig | `sig` | by `signer` |

| Kind | Name | Body | Who signs it |
|---|---|---|---|
| 1 | genesis | `bytes card` | the card itself, which becomes the account's first device |
| 2 | add_device | `bytes card`, `digest offer` | an active device; `offer` is the digest of the approval offer whose code you compared ([§10](#10-approval)) |
| 3 | add_recovery | `bytes card` | an active device, while no recovery identity is active |
| 4 | recover_device | `bytes card` | the active recovery identity, once per identity ([§12](#12-recovery-kit)) |
| 5 | remove | `id device`, `list<id> keep`, sorted | an active device |

Every card a link adds names the link's `account`, and its device ID appears in no earlier link of any chain of the account. The server accepts a genesis, `add_device` or `recover_device` only for a card that a session of the account enrolled ([Enrollment proof](#enrollment-proof)). A recovery identity signs only `recover_device`. The active cards at a head are the cards that links up to that head added and didn't remove: at most 256, of which at most one is a recovery identity, the card that an `add_recovery` added.

A card's parent is the signer of the link that added it; the genesis card has none. A `remove` link removes the card it names and, in turn, every active card whose parent it removes, except the link's signer and the devices that `keep` names. `keep` names only active devices, never a recovery identity, that the link would otherwise remove. A `remove` link that names its own signer removes that card alone, which is how a device signs out.

The server takes links through `POST /v2/accounts/chain`, in order and all or none, only from a session of the signer, or for `recover_device` from a session that proved that recovery identity ([§12](#12-recovery-kit)). It answers `account_conflict` when a link's `previous` isn't the chain's head, and `forbidden` when a link breaks a rule above. It accepts a genesis only for an account without a chain, or with a reset code ([Resetting your devices](#resetting-your-devices)). When a link that removes cards commits, the server marks their sessions removed in the same transaction, without deleting them, and each then keeps only the right described in [Desktop sign-in](#desktop-sign-in). A recovery identity holds no session of its own: the session that proved it belongs to the recovered card, so a `remove` link that takes the identity down ends none of that session's rights, and the session keeps reading the identity's envelopes until the recovered card is active in each workspace the identity read ([§12](#12-recovery-kit)).

Every device keeps, in the app's application data under a BLAKE3 keyed hash as it does for its journal ([§15](#client-journal)), the latest head it accepted of each chain of its own account and of every account it shares a workspace with: the genesis digest, the seqno and the head digest, with the digest of each link up to that head that it verified. It accepts a link only when its `seqno` is one above the kept head, its `previous` is that head's digest, its signature verifies under a card active there, and it follows the rules above. A chain under a kept genesis that doesn't extend the kept head reports `sync_account_forked`: one that stops short of the kept head or holds other links up to it, and one with a link whose `previous` isn't the digest of the link before it, at any seqno. One case is exempt, because the read bounds below let an honest server stop another account's chain short of the kept head: through a workspace whose verified policy doesn't name that chain for a current member, or doesn't count the device among its devices, such as after `remove_member`. Opening such a workspace, the device takes a shorter chain when it reaches the latest head that workspace's log records on it, the device kept the digest of its last link, and every link digest the device kept up to there matches. It keeps its higher head and judges no link past the served ones. A chain that the policy names for a current member, served short to a device the policy counts, still reports `sync_account_forked`, unless the policy gained a change after the device fetched it, in which case the device opens the workspace again. A link that fails any other of these checks reports `sync_account_unauthorized`. Either report stops every workspace the device shares with that account, or every workspace when the account is its own. A chain under a genesis the device doesn't keep enters only when a workspace's verified log names it in its genesis, an `add_member` or an `accept_reset` ([§8](#account-heads)), or, for its own account, through the approval offer or recovery kit that brought the device in ([§10](#10-approval), [§12](#12-recovery-kit)) or a reset genesis the device signed itself. A device that keeps two current chains of one account, because two workspaces name different ones, alerts its user.

A device learns of new links from the `account_changed` realtime frame and from the account heads that `policy` returns ([§26](#26-http-api)), and fetches them with `POST /v2/accounts/chains`. The server lets a session read every chain of its own account, and the current chain of an account whose approval request names an invitation to a workspace the session's account belongs to. For each workspace whose policy the session may read ([§8](#policy-change-record)), up to its own removal for a device that `remove_member` removed, it also serves every chain, current or retired, of each account that the policy records in `heads`, which name every signer's account: up to the latest head recorded on it, and, to the workspace's devices, the chain the policy names for each current member in full, including one a reset retired before the workspace accepted the reset. A device that verifies a policy from genesis therefore reads the chains of former members and of reset accounts, as Keybase serves any user's sigchain to any verifier.

### Device list

The app builds your device list from your account chain: each active card, the device that added it and how, and each removed card with the link that removed it. It reads `GET /v2/devices` only to show which cards hold a session. It alerts you to an enrolled card there that your chain doesn't list byte for byte, whatever state the server reports for it and also when it holds no session, and that has no pending approval request, whose sessions any active device of your account can end with `DELETE /v2/devices/{device}`; to a workspace that `GET /v2/workspaces` lists for your account and whose log it can't verify; to each link of your chain that the device didn't sign and that isn't the `add_device` of an approval it took part in, naming the signing device, the card's fingerprint, and whether the card is a device or a recovery identity; and to each policy change other than `account_heads` that another device of your account signed after this device first trusted that workspace, naming the signing device, the workspace and the change. When the device first trusts a workspace through a change that another device of your account signed ([§8](#verifying-the-chain)), rather than through an approval anchor, a kit or a genesis it signed, it alerts you to every change in that log other than `account_heads` that another device of your account signed, the earlier ones included.

### Removing a device

Removing a device of your own account is account-wide. On a device you hold, you pick the card to remove. The app shows that card and every active card the `remove` link would take down with it, the cards it added directly or in turn, and lets you keep any device among them that you recognize; it never offers to keep a recovery identity. The device then appends the `remove` link, which marks the removed cards' sessions removed on the server at once ([Desktop sign-in](#desktop-sign-in)), and pushes, in every workspace of your account, one `account_heads` change that records the new head ([§8](#account-heads)). Each of those changes takes the removed cards out of that workspace and makes every domain they could read due for rotation. The device rotates each domain it reads, and the other devices that read the rest rotate them; until then, no device that knows of the removal writes in those domains ([§9](#rotation)). When the removing device stops before it reaches every workspace, any device of your account, or any member device, records the head there ([§8](#account-heads)). The app then asks you to export a new recovery kit ([§12](#12-recovery-kit)). A thief who added a device or a recovery identity from your stolen laptop therefore loses it with the laptop, because the laptop's card is its parent, and the rotations that follow wrap no key for it. Workspace changes that the removed cards signed stay in force, so the app lists them in each workspace and offers the undo your role allows, such as `remove_member`, `set_role` or `set_access` ([§30](#30-threats)). Another account can't remove one of your devices; an owner or admin removes your account from a workspace with `remove_member` instead.

### Resetting your devices

When you hold no active device and no working kit, or after a server restore lost a link of your account chain ([§14](#after-a-server-restore)), Reset devices starts a new chain for your account. It follows Keybase's account reset.

1. On a device you hold, you ask for a reset code (`POST /v2/account/reset-code`). The server emails 8 characters from the device grant's alphabet to the account's address, valid for 10 minutes and usable once.
2. The device generates a new card with fresh keys, enrolls it, and posts the code with a genesis that the new card signs (`POST /v2/account/reset`). The server retires the account's chain: it ends the sessions of every card on it, keeps the chain readable, and never lets one of its cards enroll again. The new genesis starts the account's chain.
3. The app shows your account's new fingerprint. You approve your other devices into the new chain ([§10](#10-approval)), each with new keys, and export a new kit.
4. In each workspace, your old cards keep their place in the policy, without sessions, until an owner or admin of another account accepts the reset. Your new device posts an approval request naming the workspace, and an owner or admin device answers it as an approval ([§10](#10-approval)): the offer anchors the workspace, you compare the approval code with that person over a channel you both trust, and the commit is an `accept_reset` ([§8](#account-heads)). It takes the old cards out, so what they read rotates, brings your new cards in, and drops your account from every restricted domain's access list. A manager grants restricted access again after checking with you. Until a workspace accepts the reset, your devices can't verify it and report `sync_workspace_unverified` there ([§8](#verifying-the-chain)). Once it accepts, your requesting device records your account's head there, so your other devices trust it too ([§8](#account-heads)), and each device's folder bootstraps again and matches its files as after a lost journal ([§15](#client-journal)), which holds files under a marker the device can't apply ([§9](#domain-roots)) and files that differ from the live state.

Every member device that kept your old chain shows its user that your devices changed, with the new fingerprint, until it verifies an `accept_reset` that names the new chain, and wraps no key for the new cards before then. This follows Keybase, where a reset user regains a team only when an admin adds them again: one owner or admin accepts for the workspace, and every other member sees the alert. A workspace in which no other account is an owner or admin can't accept the reset; when you own it, you set it up again from a folder you hold ([§14](#after-a-server-restore)).

### Desktop sign-in

The desktop signs in with the OAuth 2.0 device authorization grant (RFC 8628). The device code is 32 random bytes in base64url, the user code is 8 characters from `BCDFGHJKLMNPQRSTVWXZ` shown as two groups of 4, codes expire after 10 minutes, and the app polls every 5 seconds, adding 5 seconds after each `slow_down`. The app opens the system browser on the server's device page, `{origin}/device`, and on no other page the server names, where you sign in, enter the user code, and approve or refuse it; the page takes the code in any letter case, with or without its hyphen, and at most 10 codes per signed-in account in 10 minutes (RFC 8628 §5.1). The approve and refuse form that follows a code entry carries the second it expires, 10 minutes later, and a confirmation: `BLAKE3.keyed_hash(derive_key("noura.sync.v2 device confirmation", secret), account ID || BLAKE3(user code) || u64(expiry))`, over the 8 letters in upper case. The server takes a decision only with a confirmation that matches the signed-in account, the code and an expiry still ahead, else it shows the page's unknown-code error, so every decision follows a code entry that the limit counted. The server keeps the device code and the user code only as their BLAKE3 hashes. A successful grant returns a session token of 32 random bytes, which the server stores only as its BLAKE3 hash and the device stores only in its credential store, and the ID of the account you signed in to, which the device's card names. The session holds no card until its device enrolls ([Enrollment proof](#enrollment-proof)), and until then it takes only `POST /v2/devices/challenge` and `POST /v2/devices`; every other route answers it with `forbidden`, so a device code that nobody enrolled reads nothing. A session has no expiry. A link of the account chain that removes its card, or a reset that retires the chain, marks it removed, as below; for an enrolled card that no chain lists, an active device of the account ends it ([Device list](#device-list)), and the server deletes it. The server never ends a session on its own. The server deletes a device grant, an enrollment challenge, a magic link or a reset code when it's used, or 10 minutes after it issued it.

A session whose card a link removed, or whose chain a reset retired, stays stored, marked removed, and keeps one right: reading its account's chains through `POST /v2/accounts/chains`. A cursor for another account's chain, and every other request, answers `device_removed`, whose detail names the removing link or the new genesis, so the device can verify its removal and show who removed it. A session alone removes nothing: a magic link, a phished device code or an enrolled card that nobody approved can't remove your devices, because only an active device signs a `remove` link.

Before it signs out, a device finishes the pending rotation of each domain it reads ([§9](#rotation)), so the readers it leaves hold the current key. It skips the domains of a workspace whose sync is stopped, which uploads nothing ([§14](#after-a-server-restore)), and each domain whose rotation it can't commit, such as one where its envelope failed and it holds no key, and leaves those domains to the readers that remain. Signing out then appends a `remove` link that names the device itself, which marks its sessions removed, and deletes its keys. Its card leaves each workspace when a device records that head there ([§8](#account-heads)), and the devices that still read each domain it read then rotate that domain ([§9](#rotation)). A device ID that a link removed never enrolls or binds a session again. Before you sign out the last active device of an account, the app warns that you'll need a recovery kit or a reset to read synced data again.

## 8. Workspace policy

A workspace's membership, roles and key domains live in its policy: a hash chain of signed changes that every member device verifies. The policy names member accounts and their roles, never device cards: a member's devices in the workspace are the active cards of its account chain ([§7](#account-chain)) at the head the policy last recorded for it. This follows Keybase teams, which name members by user and check each signer against the user's sigchain.

### Policy change record

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.policy)` | |
| workspace | `id` | |
| revision | `u64` | 1 for genesis, then +1 per change |
| previous | `digest` | digest of revision − 1; 32 zero bytes at revision 1 |
| signer | `id` | device ID |
| heads | `list<account_head>` | sorted by account; the account chain heads the change relies on ([Account heads](#account-heads)) |
| kind | `u8` | table below |
| body | kind-specific | table below |
| sig | `sig` | by `signer` |

| Kind | Name | Body |
|---|---|---|
| 1 | genesis | `id workspace_domain`; `heads` holds only the owner account's head, and that account becomes the owner |
| 2 | account_heads | empty; records newer heads ([Account heads](#account-heads)) |
| 3 | add_member | `id account`, `u8 role`, `id invitation`; `heads` also holds the new member's head |
| 4 | set_role | `id account`, `u8 role` |
| 5 | remove_member | `id account` |
| 6 | create_domain | `id domain`, `id marker_rid`, `list<access>`: creates a restricted domain |
| 7 | set_access | `id domain`, `list<access>` (replaces the domain's list) |
| 8 | delete_domain | `id domain` |
| 9 | transfer_owner | `id account` |
| 10 | accept_reset | `id account`; `heads` also holds that account's head on its new chain ([§7](#resetting-your-devices)) |

`access` is `id account | u8 level`, sorted by account, with levels 1 read, 2 write and 3 manage. Each level includes the ones below it: a manager writes and reads, and a writer reads. Roles are 1 owner, 2 admin, 3 editor and 4 viewer.

Every route under `/v2/workspaces/{w}/` serves only a session whose device is active in w's policy, or a recovery identity's proven session ([§12](#12-recovery-kit)). When a change commits, the server stops serving this workspace to the devices it removes, whatever their account: a pull returns status 4 for every domain, and every other request fails with `forbidden`, except that a device whose account a `remove_member` removed may still fetch the policy chain up to and including that change, so it can verify its removal ([§15](#pull)). A change that takes read access from a device requires rotations, which follow it ([§9](#rotation)).

A `remove_member`, whoever signs it, removes every device of the account from this workspace and drops the account from every restricted domain's access list, as a grantee and as a manager. An account invited again reads a restricted domain only after a manager checks with that person and grants it again with `set_access`. A restricted domain whose last manager leaves keeps its other grants, and an owner or admin can still delete it.

`delete_domain` deletes the domain's log, epochs, envelopes and checkpoints on the server, and its blobs, and the server rejects any later record for that domain with `not_found`. Devices that held the domain keep their local files. Those files lose their marker's protection, so the rule in [§9](#domain-roots) holds them for review instead of uploading them to a wider domain. An owner or admin can delete a restricted domain they can't read.

### Account heads

`account_head` is `id account | digest genesis | u64 seqno | digest head`: the digest of an account chain's genesis link, a seqno, and the digest of the link at that seqno. Every change records in `heads` the account chain heads it relies on, and the workspace's devices at revision r are, for each member account, the active cards at the latest head that a change at or below r recorded for it. A change is valid only when:

- `heads` holds the signer's account, at a head where the signer is an active device;
- every other account in `heads` is the account an `add_member` adds, the account an `accept_reset` accepts, or, in an `account_heads` change, a member account;
- each head of a member account lies on that account's chain at or after the head the workspace records for it, under the same genesis; the head that an `add_member` records for its new member, and the head that an `accept_reset` records, may lie on another chain;
- an `add_member` or `accept_reset` records the signer's account at the head the workspace already records, so neither change takes a card out; a signer that holds a later head records it first with an `account_heads` change;
- an `account_heads` change records at least one head later than the one the workspace records, except one that records only its signer's account, at the recorded head, while no change that a device of that account signed records that head.

A change whose heads take a card out makes every domain the card could read due for rotation ([§9](#rotation)). A change whose heads bring a card into the workspace carries, after it in the same push, a current-epoch envelope for that card in every domain its account reads that the signer could read before the change, also when the change makes that domain due ([§9](#granting-and-wrapping)). In a domain the signer couldn't read, a change that also takes a card out commits without them, and the rotation it makes due wraps for the new card; any other such change is left to a device that reads the domain, such as one of that account's own. The server checks these rules against the account chains it holds, answering `account_conflict` for a head off its chain or behind the recorded one and `forbidden` for the rest; clients report `sync_account_forked` or `sync_policy_unauthorized`.

Each member device that accepts a head of a member account, its own included, later than the one a workspace records pushes an `account_heads` change there that records it; when the head takes a card out, the device writes nothing in a domain that card read until its accepted policy no longer counts the card among the domain's readers, through a change recording that head or a later one, a `remove_member` or `accept_reset` of the card's account, or a `set_access`, and an epoch of the domain at or above that change's revision commits ([§9](#rotation)). The device drops its pending `account_heads` push once the account is no longer a member or the workspace records another chain for it, which also ends the alert that the push's refusals raise ([§9](#rotation)). Such a head adds only cards that the account's own active devices signed into its chain, so recording it plants nothing, as any MLS member may commit another member's pending proposals. A removal therefore reaches every workspace even when no device of that account is online, as after the last one signs out, or when the removed card was the account's only device there. A device of that account waits a random 0 to 60 seconds first and a device of another account 60 to 120 seconds, so an online device of the account records the head first, and each skips the push when another change recorded it meanwhile.

A change may record any later head of an account, one link or many past the recorded one, and a device picks heads whose envelopes fit one push ([§20](#20-atomic-changes-and-quota)). When the envelopes that a later head's new cards need in every domain wouldn't fit 1 MiB, the device records that account's heads one card-adding link at a time, one change per push, up to the head it accepted. A lagging head that ends in a removal therefore still reaches the workspace, after the heads that bring the earlier cards in.

The head a workspace records for an account may come from a change that no device of that account signed: the `add_member` or `accept_reset` that answered the account's request, or another account's `account_heads` change. A device of that account that trusts the workspace then pushes an `account_heads` change there too, after the same random 0 to 60 seconds, while no change signed by a device of its account records the recorded head. It records its account's latest head that it accepted, in steps as above when its envelopes wouldn't fit one push, even when that is the recorded one, and the account's other devices trust the workspace through it ([Verifying the chain](#verifying-the-chain)), as in Matrix cross-signing, where one device's verification of another user, signed with the user-signing key, carries to every device of that user. The rules above leave no repeats in the chain, also for an account invited again with an unchanged chain: no push is due while a change signed by a device of the account records the recorded head, and the account's other devices already trust the workspace through it. When no device of the account that trusts the workspace is left to push it, an owner or admin removes the account and invites it again.

`accept_reset` names a member account and records a head on a chain whose genesis differs from the one the workspace records for it. It is the commit of an approval ([§10](#10-approval)): the account's reset device posts a request naming the workspace, an owner or admin device answers with an offer that anchors the workspace, and the two people compare the approval code. The server accepts it only for the account's current chain and only when that account's approval request named this workspace. It takes the old chain's cards out, brings the new chain's active cards in with envelopes as above, and drops the account from every restricted domain's access list, as a grantee and as a manager, as `remove_member` does. The account keeps its role and its history floor.

A card that a link removed never becomes active again: an account chain never adds a device ID that a chain of the account listed before, and a reset retires every card of the old chain. A member that `remove_member` removed and an `add_member` brings back returns with its account's active cards, which keep their card indices and continue their device chains ([§17](#checkpoint)). Clients refuse, with `sync_policy_unauthorized`, heads that would make one device ID active under two accounts, and the server rejects them with `id_conflict`.

No domain may have more than 512 active readers, recovery identities included ([§27](#27-limits)). The server rejects a change that breaks this rule with `recipient_limit`, and clients report it as `sync_policy_unauthorized`. A workspace keeps an account's older head while every later head would break the rule, so the account's newer cards, and a removal that follows them in its chain, stay out until room frees; a later head that takes out of each full domain at least as many cards as it brings in still commits ([§30](#30-threats)).

### Card index

Each card that ever becomes a device of the workspace gets a card index. At the change whose heads first bring cards in, each new card takes the next unused number from 1, in order of account ID and then of the seqno of the link that added it. A card keeps its index for the workspace's life, also when it leaves and its account comes back, and no other card takes it. The index places the card's yrs client IDs ([§22](#the-document-on-a-device)). A workspace indexes at most 16,777,215 (2^24 − 1) cards, and the server rejects a change that would pass that with `recipient_limit`.

### Roles and authority

| Action | Who may sign it |
|---|---|
| genesis | the creating device, an active device of the account that becomes the owner |
| account_heads | any member device ([Account heads](#account-heads)) |
| add_member | owner (admin, editor or viewer); admin (editor or viewer); requires an account that isn't a member and an invitation it accepted; the inviting device signs it ([§11](#11-invitations)) |
| set_role | owner for any non-owner; admin between editor and viewer; never to owner, which only transfer_owner grants |
| remove_member | owner for any non-owner; admin for editors and viewers; the member themself, except the owner, who transfers ownership first |
| create_domain | owner or admin; the access list must name the signer's account as a manager |
| set_access | a manager of that domain for any change; an owner or admin only to remove entries |
| delete_domain | owner or admin, or a manager of that domain |
| transfer_owner | owner; the old owner becomes an admin |
| accept_reset | an owner or admin device, for another member account whose chain a reset replaced |

A recovery identity signs no policy change. The joining device checks that the `add_member` names its invitation and records, for its account, the head its approval request committed to ([§10](#approval-request)), at which its own card is active, and reports `sync_approval_mismatch` otherwise; a reset device checks the same of the `accept_reset` that answers its request. Because `add_member` takes only an account that isn't a member, and no change records another account's head except `add_member`, `accept_reset` and `account_heads`, an owner or admin can't plant a device in a member's account: the account chain holds every card, and only that account's active devices sign links that add one. The server accepts an `add_member` only for an open invitation of the workspace that the signer's account sent and the new member's account accepted, with the invitation's role, for an account whose unexpired approval request named the invitation, and deletes the invitation's row in the transaction that commits the change.

Roles grant these rights:

| Right | Owner | Admin | Editor | Viewer |
|---|---|---|---|---|
| Read the workspace domain | yes | yes | yes | yes |
| Write the workspace domain | yes | yes | yes | no |
| Invite and remove editors and viewers | yes | yes | no | no |
| Approve devices of new members | yes | yes | no | no |
| Create restricted domains | yes | yes | no | no |
| Create and revoke share links | yes | yes | no | no |
| Change the workspace profile | yes | yes | no | no |
| Add or remove admins, transfer ownership | yes | no | no | no |

A restricted domain's access list alone decides who reads and writes it, whatever their workspace role. Grantees must be members, and every active device of an account on the list reads the domain. A manager hands a domain to another account in two changes: a `set_access` that grants, with the new readers' envelopes, then a later `set_access` that takes itself off, since the server takes envelopes only from a device that reads the domain under the policy after the change ([§9](#granting-and-wrapping)).

### Policy entries

When a change commits, the server appends an entry for it, `u64 revision | digest head`, to every domain's log in the same database transaction, under the locks that [§15](#store-transactions) orders. A domain that the change creates gets that entry first. The entry fixes where the change takes effect in each log. A client checks each entry's `head` against its own change at that revision, fetching the chain first when the revision lies above its accepted head, and reports `sync_policy_forked` on a mismatch.

Server and clients check one rule for every record a device signs into a domain's log, an operation or a checkpoint: a record of device X in domain D is valid only when X could write D at the latest policy entry before the record, and also at the record's own `policy_revision`, which is at most that entry's revision. The server checks a new record against its current policy, because the record goes after the latest entry, and rejects it with `forbidden` or `policy_stale` ([§13](#server-checks-on-an-operation)). The server's commit order settles any race between a writer and the change that removes its write access: every record that lands after the change's entry fails, so a removal needs no coordination with the devices it removes. A client checks the same rule as it reads the log and reports `sync_writer_not_authorized` for a record that breaks it.

A client that accepted change `r` requires `r`'s entry in each log it reads, at or below the `head` of the first page it requests afterwards, and reports `sync_chain_gap` when its cursor passes that head without the entry. Epoch entries follow the same rule ([§9](#rotation)). A client that bootstrapped a domain from a checkpoint needs no entry there for a change at or below the checkpoint's `policy_revision`, or for an epoch at or below its `epoch`: those entries sit at or below `covered_seq`, and the checkpoint stands in for them. The device that committed `r` also holds a receipt with each entry's sequence ([§15](#push)) and requires the entry at exactly that sequence, so in the remover's view the server can't move a removal past the removed device's later records.

The entries of a restricted domain reach only its readers, and the server already knows the policy, so they add no metadata.

### Verifying the chain

A client accepts change `r` only when its revision is one above the accepted head, its `previous` equals the head's digest, its `heads` follow [Account heads](#account-heads) against account chains the client verified ([§7](#account-chain)), its signature verifies under the signer's card from its account chain, and that account holds the authority above. The server checks every signed record a device pushes the same way, under the signer's card in its account chain, never under an enrolled card that no chain lists. A device trusts revision 1 through an approval anchor ([§10](#10-approval)), its recovery kit ([§12](#12-recovery-kit)), or because it created the workspace. It trusts a workspace that none of these anchors, such as one its account joined after the device's approval, once the log records a head of its own account, at or after the device's reference head, in a change whose signer is active, under the same genesis, at the head of its own account that the device keeps ([§7](#account-chain), [Account heads](#account-heads)). The reference head is the head of its own account chain from which the device verified that chain: the genesis it signed, the `approver_head` of the offer that approved it into its own account, or its kit's `account_head`, never the `approver_head` of an invitation or reset offer, which lies on another account's chain. A card removed by any link the device accepted therefore confers no trust, as in Matrix cross-signing, where a signature from a device you no longer trust confers none. When the account's only devices that trust a workspace were removed, its other devices can't trust it, and an owner or admin removes the account and invites it again ([Account heads](#account-heads)). In the workspace its own approval request named, a reset device meets no such change until its account is accepted, so it trusts that workspace through the approval that commits the `accept_reset` ([§7](#resetting-your-devices)), as an invited member does through `add_member`. A sibling that a device of the reset account approved into the new chain isn't a reset device, and trusts by the general rule above. Every device of the reset account, the reset device included, trusts any other workspace through an `account_heads` change there that a device of the account signed, once that workspace accepted the reset, as the rule above says. The workspace ID alone pins nothing, so a device refuses a log that nothing anchors, even one served under a workspace ID it knew, with `sync_workspace_unverified`. The policy head in the device's credential-store mark ([§15](#client-journal)) is also an anchor: after a lost or rolled-back journal, or when you connect a copied folder, the device trusts the log whose change at the mark's revision has the mark's head digest. The hash chain fixes the prefix up to that change, so this anchor confers no trust the device didn't already hold, and a workspace it trusted only through a sibling that a link has since removed stays trusted.

Records name the policy by revision and digest. A record that carries `policy_revision` (an operation, epoch record, checkpoint or profile) also carries `policy_head`, the digest of the change at that revision. The client compares it with its own accepted change at that revision.

The client stores the accepted head durably, in its journal and in its high-water mark ([§15](#client-journal)), and reports:

- `sync_policy_forked` when the server returns a different change for a revision the client already accepted, or a record's `policy_head` differs from the client's change at that revision;
- `sync_policy_rollback` when the server reports a head below the accepted one;
- `sync_policy_unauthorized` when a change fails the authority rules.

A record that names a policy revision above the client's accepted head waits until the client has fetched and accepted that revision. If the server can't supply it, the client reports `sync_policy_rollback`.

Policy chains stay short (one change per membership or device event), so a new device replays the whole chain and v2 defines no policy snapshot.

## 9. Key domains

A key domain is a set of objects that share one symmetric key at a time. Each workspace has exactly one workspace domain and any number of restricted domains, at most 256 domains in all.

| Domain | Who reads it | Objects |
|---|---|---|
| Workspace | every member | every object outside a restricted domain, and the workspace profile |
| Restricted | only the members its access list names | the objects under one folder |

Each domain has its own epochs and keys. A restricted domain's audience comes only from its grants: the owner and admins don't read it unless a manager grants them access.

### Domain roots

A restricted domain covers the folder that holds its marker file, `.noura-domain`, and everything beneath it. The marker is the synced object whose `rid` the domain's `create_domain` names, and it lives in that domain's log, so a member outside the domain never receives it. Its content is one line, `domain: <base64url domain ID>`, followed by a line feed; clients write exactly that and rewrite the file when an outside edit changes it. Because the marker moves with its folder, moving the folder keeps the domain and no path serves as identity. A client refuses to create a marker inside another domain's root. The creating device writes the marker in the same push as `create_domain`.

A marker is valid when the live set of `marker_rid` in the named domain's own log holds exactly one operation, a state operation whose decrypted path ends in `/.noura-domain`, and the policy still lists the domain. A deleted marker, or one in conflict with two live states, isn't valid. A `.noura-domain` file written by hand is an ordinary file. A client refuses, and lists for review, any object whose last path component is `.noura-domain` and that isn't a domain's `marker_rid`: it writes no such file to disk, so an editor can't plant a marker that a later sync without a journal would take for a held root. On a device, a marker is also invalid when it sits at or beneath the recorded root of a domain whose `create_domain` has a lower policy revision, or at the same path as that domain's marker. The earlier root wins, so someone outside restricted domain R who signs a marker inside R's folder can't redirect R's files on the devices of R's grantees.

The journal records each domain root as (domain, folder, marker `rid`) when the client creates or applies a valid marker, and that record decides where a new file goes: the domain of the innermost recorded root that holds it, or the workspace domain. Roots nest only when a later domain's root holds an earlier one, and the earlier root keeps its own files. A client applies a domain's marker before any other object of that domain, during bootstrap too ([§18](#18-bootstrap)), so the root exists before the first file beneath it. During a bootstrap or a journal rebuild, the client holds every new local file until it has applied the marker of every restricted domain it reads. When a marker disappears or stops being valid, or a marker's move would change the domain of existing objects, the client keeps the recorded root, holds the affected files and any new files under it for review, and uploads none of them to another domain until you confirm. An iCloud placeholder of the marker, `.noura-domain.icloud` or `..noura-domain.icloud`, counts as present. Restoring the marker from synced state ends the hold.

A client records a held root, with the domain whose files it holds, in two cases. When a bootstrap or a journal rebuild meets a marker that is moved, deleted or in conflict, so the client holds live objects of a restricted domain that no recorded root covers, it records one for that domain at each folder that directly contains such an object, before it writes the object's file. When a folder syncs without this device's journal, after a lost or rolled-back journal, Reset devices, Set up sync again or connecting a copied folder ([§15](#client-journal)), it records one at the folder of each local `.noura-domain` file other than the valid markers it applied, for the domain that file names, such as one the device doesn't read or that the policy no longer lists, because the files beneath it had an audience the device can't see. The client holds every file at or beneath a held root, lists those already on disk for review with the number of people who would see each in the domain it would otherwise go to, and uploads none of them to any domain until you confirm a domain for them, or until the innermost valid root that covers the folder belongs to the held domain, which the device records only when it reads that domain. A valid marker of any other domain, at that folder or above it, never ends the hold, so the files never reach an audience the hold kept them from. When a hold ends, the client matches the files beneath it with that domain's live objects, as after a lost journal ([§15](#client-journal)), before it writes any of the domain's files there.

While a domain's marker is valid, an object of that domain whose path leaves the root, whether from its payload or from a text-mode note's merged `meta` ([§22](#22-live-collaboration)), records no held root. The client keeps the file at its last path inside the root, or doesn't write it when it has none there, lists the change for review and reports it. A device never writes such a path itself: a rename that would take a file across a root boundary is a move between domains ([Moving objects between domains](#moving-objects-between-domains)).

A client never uploads a delete of a marker. When you delete a marker file locally, the client writes it again from its journal; only `delete_domain` retires a marker. A client that receives a delete of `marker_rid`, or a move of it to a path that doesn't end in `/.noura-domain`, keeps its recorded root, holds files under it as above, and reports the record.

A device writes an object of another domain under `<root> (shared)`, the root folder's name followed by ` (shared)`, when the object's path lies at or beneath the root of a restricted domain the device reads, and reports it. It never classifies such an object into that root, and new files you create under the root still go to the root's domain. The relocation is local: an edit to a file under `<root> (shared)` uploads to that object's own domain at its synced path.

Restricting a folder that already holds files makes one exception. The creating device writes `create_domain` and the marker, then moves each workspace-domain object under the new root into the domain, one object pair at a time ([Moving objects between domains](#moving-objects-between-domains)). A workspace-domain object whose latest state operation sits before the `create_domain` entry in the workspace domain's log stays at its path on every device until its move arrives, instead of going under ` (shared)`. A grantee device holds your edits to it for review and uploads none of them until the move lands. If the creating device stops before it finishes, any device of a manager of the domain moves the rest, after it has known of the unfinished moves for 10 minutes. A device that bootstrapped past the `create_domain` entry can't see that position, so it compares revisions: an object whose latest state operation carries a `policy_revision` below the `create_domain` change's counts as waiting for its move, and any other object goes under ` (shared)`. That revision is the writer's own claim, so an object waiting for its move never takes a path from an object of the root's domain, whichever has the higher digest ([§16](#16-conflicts)): it goes under ` (shared)` while the domain's object holds the path, and moves there when the domain's object arrives after it.

Sync covers every file that the workspace scanner reports ([v1.md](v1.md#local-metadata)) and passes the path rules in [§13](#payload), plus each valid `.noura-domain` marker, the one dotfile sync reads. It skips the manifest's `ignore` patterns, symlinks, empty folders, and files whose names aren't valid UTF-8 or break the path rules; the client reports each file it skips. v1.md and its conformance fixtures gain the marker exception in the change that implements domain roots.

### Moving objects between domains

A move across a domain boundary writes a new state operation with a new `rid` in the target domain, whose payload names the source domain and `rid` in its `origin` field, and a `delete` operation (reason 2, moved) in the source domain, whose payload names the target. Both commit atomically in one push ([§15](#push)), so the server can link the two `rid`s ([§29](#29-what-the-server-stores)). A blob serves only operations of the domain it was created for, so a moved attachment gets a new blob: the client encrypts it again under a new key and blob ID and uploads it. Members of both domains see one object, because the local object ID travels inside both payloads. The new state operation is always a `file`, so a text-mode note leaves text mode when it changes domain and enters it again on its next edit ([§22](#entering-text-mode)). A move inside one domain is a state operation on the same `rid` with the new path, and it reuses the `blob_ref`.

A client applies the create before the delete. When it can read the target domain, it defers a reason-2 delete until it has pulled the target domain past the move; a device that can't read the target applies the delete at once. The new object takes over the source object's local file only when the file holds the create's revision or the two share a frontmatter ID, so a writer of the target domain can't take another object's file by naming it in `origin`.

A client never moves an object from a restricted domain to a wider audience on its own. When a file leaves a restricted root, or its marker disappears, the client holds those files for review and shows how many would become visible to more people. Moving files into a restricted domain yourself needs no confirmation, but the app warns you that their earlier versions stay in the source domain's log under its keys: its current members already hold them, and a later member could read them if the server served that old ciphertext ([§30](#30-threats)). A marker change that would move existing objects goes to review as above. A move of many objects commits one object pair at a time, with many pairs per push ([§20](#20-atomic-changes-and-quota)), so each file sits in exactly one domain throughout.

### Epochs

The workspace domain and each restricted domain change keys in epochs, numbered from 1. An epoch key is 32 random bytes. An epoch record announces each epoch.

`epoch`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.epoch)` | |
| workspace | `id` | |
| domain | `id` | |
| epoch | `u64` | 1 for a new domain, then +1 |
| previous | `digest` | digest of the previous epoch record; 32 zero bytes at epoch 1 |
| policy_revision | `u64` | the policy head the rotating device saw; recipients are computed at this revision; at least the previous record's |
| policy_head | `digest` | digest of the change at `policy_revision` |
| rotating_device | `id` | |
| recipients_digest | `digest` | `BLAKE3(tag(noura.sync.epoch.recipients) \|\| list<id>)` over the sorted device IDs that receive this epoch's key |
| key_check | `digest` | `BLAKE3.keyed_hash(derive_key("noura.sync.v2 key check", epoch_key), tag(noura.sync.key-check) \|\| workspace \|\| domain \|\| u64 epoch)` |
| *AEAD associated data ends here* | | |
| lineage_nonce | `nonce` | 24 zero bytes at epoch 1 |
| lineage | `bytes` | previous epoch key, XChaCha20-Poly1305 under the lineage subkey of this epoch's key; empty at epoch 1 |
| sig | `sig` | by `rotating_device` |

A device pushes an epoch as an epoch bundle, `bytes epoch | list<id> recipients`, and the server keeps the bundle whole. It serves only the epoch record, because readers compute the recipients from the policy. When an epoch commits, the server also appends its epoch record to that domain's log as an entry, in the same database transaction ([§15](#15-domain-log-cursors-and-pulls)). A reader verifies that entry as an epoch record, and the entry fixes where the epoch starts in the log ([§9](#rotation)).

The server accepts an epoch record only when its rotating device could read D at its `policy_revision` and still can under the current policy (else `forbidden`), and only for epoch 1 of a domain, committed with genesis or `create_domain`; for a rotation that a committed policy change requires and that hasn't committed yet, with a `policy_revision` at or above the latest change that requires it; or for a replacement, as below. It rejects any other epoch record with `rotation_not_due`. Outside a required rotation, a member rotates a domain only with a replacement, which must skip at least one epoch, each used by no device but its rotator and devices that lost read access to D through a change at or below the replacement's `policy_revision` ([§30](#30-threats)). An epoch record whose `previous` names the latest epoch counts as a rotation, which the server refuses with `rotation_not_due` while none is due.

The lineage field links each epoch to the one before, so a device that holds the current key derives every earlier key by walking backward. A newly approved device therefore needs one envelope per domain, and objects written under old epochs stay readable without re-encryption. A key that a newer key's lineage yields needs no signature check of its own record: the lineage AEAD authenticates it, and the device checks it against that record's `key_check`.

`key_check` lets every recipient confirm it received the same key as the others. A rotating device can't hand different recipients different keys without failing that check.

A client accepts an epoch record when:

- its signature verifies under the rotating device's card from its account chain;
- the rotating device was active and could read the domain at `policy_revision`; epoch 1 of a restricted domain comes from the device that signed its `create_domain`;
- `policy_revision` is at least the previous record's, and `policy_head` matches the client's change at that revision;
- `recipients_digest` matches the active devices that could read the domain at that revision, a list the client computes from its verified policy;
- the epoch is one above the previous accepted epoch and `previous` equals that record's digest, or the record is a replacement;
- the lineage decrypts to a key matching the `key_check` of the record that `previous` names; at epoch 1 the lineage is empty;
- the client holds the key, from an envelope it accepts ([Envelopes](#envelopes)) or through a later key's lineage.

Every key derived through lineage is checked against its own record's `key_check`. A failure reports `sync_key_mismatch`.

**Replacing an epoch.** A device that can't accept D's latest epoch record, because of a verified failure, signs a replacement. A verified failure is one of these: the lineage or `key_check` fails on a key the device unwrapped, the envelope's signer fails its check ([Envelopes](#envelopes)), or the epoch record fails a check above. A missing envelope, or one that waits for the epoch's entry, isn't a failure: the device holds its cursor until the key arrives ([§13](#client-checks-on-an-operation)). The replacement is an epoch record one above the latest, whose `previous` names the last epoch record it accepted and whose lineage holds that record's key, with a `policy_revision` at or above the latest change that requires D to rotate. For a replacement, the previous record in the checks above is the one `previous` names. The server accepts a replacement only when `previous` names an epoch record below the latest, so it skips at least one epoch, and every operation, checkpoint or profile that names a skipped epoch was signed by that epoch's rotator or by a device that lost read access to D through a change at or below the replacement's `policy_revision`. Otherwise it answers `epoch_conflict`, and the device asks for a re-wrap instead ([Envelopes](#envelopes)). A device asks for a re-wrap whenever it can't sign a replacement: a device that accepted no earlier epoch record of D, such as a card approved after D's current epoch, has none to name as `previous`, so it asks at once. A client accepts a replacement whose `previous` names any epoch record it accepted, and the epochs it skips end, including numbers it never saw. A client that never accepted a skipped epoch counts a record under it as unreadable ([§13](#client-checks-on-an-operation)), attributed to its signer, when that signer is the epoch's rotator or lost read access to D through a change at or below the replacement's `policy_revision`. Any other record under a skipped epoch reports `sync_key_mismatch` and holds the domain's cursor, because an honest server refuses that replacement. A device that accepted a skipped epoch writes again, under the current epoch, each object where a record under that epoch, its own or another device's that it can read, was live at the replacement's entry, as a new state operation covering that record. It writes its own records at once, and other devices' records after a random wait of 0 to 60 seconds, skipping any that another device's rewrite has covered meanwhile, so a record whose rotator is gone still reaches the devices that never held its key. The new operation is the file or delete, or for a text-mode note a `text_snapshot` of its document under the same lineage. Other devices may have covered the record meanwhile, since they count it as unreadable; the new operation then covers that record alone, so the object's other live states stay as branches of a conflict ([§16](#16-conflicts)), and two snapshots of one lineage merge. The rewrite keeps that cover: when the server pruned the record or answers `supersedes_unknown` for it, the device writes nothing and lists its version for review, and when the device's own highest live operation on the `rid` lies above that record, the rewrite covers that one too, so the server neither counts the rewrite as a patch nor answers `supersedes_incomplete`. An honest rotator's edits therefore survive a replacement that skips its epoch. When a replacement skips an epoch that the current profile was signed under, the profile's signer signs it again under the current epoch, and, after the same random wait, so does each owner or admin device that accepted that epoch and finds the profile still under it ([§25](#25-workspace-profile)).

**Epoch authority.** An epoch's entry never tells a client that a rotation happened. Server and clients judge every rotation by revision: a rotation of D is pending while a policy change above the latest epoch record's `policy_revision` requires D to rotate, wherever that epoch's entry sits in the log. Suppose a malicious server commits, after r's entry, an epoch record that a device which lost access to D at revision r, or any former reader, signed at a revision below r; an honest server refuses it, because the signer can't read D under the current policy. The record's recipients are the readers before r, the lost device among them, yet the rotation that r requires stays pending: no writer encrypts under that epoch, and every operation under it after r's entry is epoch-invalid ([§9](#rotation)). Its envelopes reach no recipient that accepted r either: such a recipient takes an envelope only from a device that could read D at its accepted head or at the latest policy entry before the epoch's entry ([§9](#envelopes)), and the signer could do neither. The device that committed r therefore rotates D with a replacement. A checkpoint applies the same revision rule at `covered_seq`, so a client that follows the log and one that bootstraps from a checkpoint agree about which rotations are pending.

Epoch records form a hash chain per domain, and each checkpoint names the current epoch record ([§17](#17-manifest-and-checkpoints)). A rotating device that colludes with the server to show two recipients two different records for one epoch is caught when either recipient sees the other record or a checkpoint naming it, and their records stop decrypting for each other, because each record's associated data includes the epoch record's digest ([§4](#records-signatures-and-digests)).

### Envelopes

An envelope wraps one epoch key for one device.

`envelope`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.envelope)` | |
| workspace | `id` | |
| domain | `id` | |
| epoch | `u64` | |
| recipient | `id` | device ID |
| recipient_hpke_digest | `digest` | `BLAKE3` of the recipient card's raw 1,216-byte X-Wing public key, without its length prefix |
| wrapping_device | `id` | |
| *HPKE `info` ends here* | | |
| enc | `bytes` | HPKE encapsulated key, exactly 1,120 bytes |
| ciphertext | `bytes` | 48 bytes: the epoch key and the 16-byte tag |
| sig | `sig` | by `wrapping_device` |

HPKE runs in base mode with the suite in [§3](#3-primitives). Its `info` is the envelope prefix from the tag through `wrapping_device`, its `aad` is empty, and its plaintext is the 32-byte epoch key. A recipient accepts an envelope only when the unwrapped key matches the epoch record's `key_check` and the signature verifies under the card of a device that could read the domain at one of two points: at the latest policy entry before the epoch's entry in the domain's log, or at the recipient's accepted head. The first test applies once the epoch's entry has arrived in a pulled page, even while the recipient's cursor is held at that entry for the key ([§13](#client-checks-on-an-operation)); until then only the second test applies, and an envelope that fails it waits for the entry. A key supplied by the server, or an envelope signed by a card outside the workspace's recorded heads or by one that lost access before the epoch's entry, fails with `sync_key_mismatch`.

The server deletes an epoch's envelopes once the next epoch of that domain commits. A device that lacks a key gets an envelope for the current epoch and reaches older keys through lineage.

**Re-wrapping.** A device whose envelope for D's current epoch fails these checks asks for a new one with `POST .../domains/{d}/rewrap`: after the server refused its replacement with `epoch_conflict`, because a device other than the rotator used that epoch without losing read access to D through a change at or below the replacement's `policy_revision` ([Epochs](#epochs)), or at once when it accepted no earlier epoch record of D, such as a card that a change brought in after D's current epoch. The server takes the request from any device that the current policy authorizes to read D and for which it accepted an envelope at that epoch, else `forbidden`. It keeps one open request per device and epoch, answers a repeated one with the original result, and lists the request in the key stream of D's readers as key entry type 5 ([§26](#26-http-api)). Each device that holds the verified key waits a random 0 to 60 seconds, then wraps it for the requester when its own verified policy authorizes that device, and pushes the envelope alone. The server accepts a lone envelope only for an open request, one per wrapping device and at most 3 per request, so the first holders whose envelopes arrive answer it; it refuses the rest with `forbidden`. The requester keeps the first envelope that passes. The server closes a request when D's next epoch commits, and when the requester's key fetch passes an envelope that answers it: a `keys` read whose `after` reaches that envelope's key sequence, or a `keys/current` read, which returns it. A requester whose answers all failed may ask again.

### Granting and wrapping

Granting decides who may read a domain; wrapping hands an authorized device the key.

- **Granting** happens only through policy changes, signed by the devices the authority table in [§8](#roles-and-authority) allows: membership changes for the workspace domain, and a manager's access-list changes for a restricted domain.
- **Wrapping** is mechanical. Any device that holds an epoch key may wrap it for a device that its own verified policy authorizes to read that domain. The wrapping device takes the recipient's card byte for byte from the recipient's account chain, at the head its verified policy records for that account, and wraps only for a card that is active in the workspace at its accepted head and authorized for the domain. It refuses otherwise and never wraps for a card the server supplies.

The server accepts a push that makes a device a reader of a domain (a change whose heads bring a card in, `add_member`, `accept_reset`, a `set_access` that grants, `create_domain` or a rotation) only when the push carries, after the change, the domain's current-epoch envelope for every new reader, also when the change makes the domain due or its rotation was already pending. Lineage gives every holder of the next key the current one, so these envelopes give the new readers nothing that the next rotation wouldn't. Only a change whose signer couldn't read the domain before it, such as one in which a member device records another account's head, may leave them out, and only when it also takes a reader out of the domain: the rotation it makes due wraps for the new readers with the others ([§8](#account-heads)). Apart from these envelopes and a rotation's, the server takes no envelope in a domain while its rotation is pending. The server accepts an envelope only when its wrapping device can read the domain under the current policy (else `forbidden`), and rejects an epoch or envelope whose recipients differ from the devices its current policy authorizes to read the domain, with `recipients_invalid`. Each device therefore receives every key its account reads when it's added or from the rotation its addition makes due, and at every later rotation or grant, and with an honest server, a device lacks an envelope only until a key holder rotates, or for good if every key holder is lost first ([§30](#30-threats), Pending rotations). The signer of a granting change holds every key it must wrap: a manager holds its restricted domain's key, and every member's device holds the workspace domain's.

### Rotation

A change at revision r makes domain D due exactly when a device that could read D at revision r − 1 can't at r. Every row below is an example of that rule, and each counts only when the change takes such a device out. A member removed with no active card left, or an `accept_reset` of an account with none, makes nothing due:

| Domain | Examples |
|---|---|
| Workspace | a recorded head takes a card out, a member leaves or is removed, or an `accept_reset` retires an account's old chain |
| Restricted | a grantee loses its grant, a recorded head of a grantee's account takes a card out, a grantee leaves the workspace, or a reset of a grantee's account is accepted |

Rotating domain D at policy revision r means:

1. Generate a new epoch key.
2. Write an epoch record with `epoch` = current + 1, `policy_revision` ≥ r and at least the previous record's, a recipient list of every active device authorized to read D at that revision, and the lineage of the previous key.
3. Wrap the new key for every recipient, with cards from the verified policy.
4. Commit the epoch bundle and the envelopes in one push ([§20](#20-atomic-changes-and-quota)).

The device that commits a change requiring rotation then rotates every domain it can read, one domain per push, after the push that carries the change. Every other device that holds the key of a domain the change makes due and still reads it, such as a restricted domain the remover doesn't read or each domain a signed-out device read, waits a random 0 to 60 seconds after it accepts the change, then rotates that domain if the rotation is still pending there ([Epoch authority](#epochs)). This follows MLS, where any member commits a pending Remove proposal instead of waiting until it next sends a message. Until the rotation commits, the server accepts no record, presence or lone envelope in that domain and answers `rotation_pending`, and a device that gets that answer fetches policy and keys and rotates first. Concurrent rotations race: the server accepts the first epoch record for each epoch and rejects the rest with `epoch_conflict`. While a rotation is pending, nothing is written or forwarded in that domain, so rotating late exposes nothing.

A restricted domain's epoch 1 commits with its `create_domain`, from the device that signed it, whose account the access list names as a manager. Clients reject an epoch 1 from any other device.

From revision r until the rotation commits, the server also stops serving D's operations and keys to the removed devices. A device keeps at most one push in flight per workspace. Every push response carries the server's policy head revision ([§15](#push)), and a device fetches the policy chain and keys before its next push whenever a push response, a pull or the realtime channel named a revision above its accepted head. When it accepts a change that requires D to rotate, it re-seals every record in its outbox that it hasn't sent and that uses an epoch the change invalidated. No device that has seen revision r encrypts under an epoch whose recipients include a device that no longer has access.

A device can learn of a removal before a workspace records it. A device that accepted a head of a member account, later than the one a workspace records, that takes a card out of that workspace, or that signed the change recording such a head, treats each domain of that workspace the card could read as rotation-pending. It encrypts, wraps and checkpoints nothing there, apart from the envelopes of the change that records the head and of the rotation, until its accepted policy no longer counts the card among the domain's readers, through a change recording that head or a later one, a `remove_member` or `accept_reset` of the card's account, or a `set_access`, and an epoch of the domain at or above that change's revision commits; it then re-seals its unsent records there. The app alerts you while the server refuses the recording change. The one exception is a `recipient_limit` refusal that the device confirms from its verified policy ([§8](#account-heads)), after which it writes as before. This follows Signal's groups, where a client that learns of a removal stops sending under a key the removed member holds without waiting for the server.

**Epoch validity.** Server and clients check one rule: an operation of domain D is epoch-valid only when its `epoch` is that of the latest epoch entry before it in D's log, and no policy entry before it requires D to rotate with a revision above that epoch record's `policy_revision`. A writer encrypts under the latest epoch it accepted only while no change in its accepted policy above that epoch's `policy_revision` requires D to rotate and no removal it knows of holds D as above; otherwise it rotates D first, or waits for a device that can. The server checks a new operation against its current state and rejects anything else with `epoch_stale` or `rotation_pending`, and the writer re-seals under the new epoch ([§14](#your-own-chain)). A keystroke batch in flight across a rotation therefore costs one re-seal round trip. A client that meets an operation breaking the rule, such as an old-epoch record after a newer epoch entry, treats it like any other record an honest server would have rejected: it reports `sync_epoch_stale` and holds the domain's cursor ([§13](#client-checks-on-an-operation)).

After a bootstrap, the checkpoint stands in for the entries at or below its `covered_seq`: `epoch_record` names the latest epoch entry, `policy_revision` names the latest policy entry, and a rotation is pending at `covered_seq` when a change above the epoch record's `policy_revision`, up to the checkpoint's, requires D to rotate ([§17](#checkpoint)). That is the rule above, applied at `covered_seq`.

A reader that learns epoch e from the key route before its log entry requires that entry in D's log at or below the `head` of the first page it requests afterwards. It reports `sync_chain_gap` when its cursor passes that head without the entry, or when a record under e arrives before it.

With an honest server, nothing after a new epoch's entry uses a key that a removed device holds. A server that hides a removal from a writer lets the removed device read that writer's new records ([§30](#30-threats)).

**Cost.** A rotation writes one epoch record and one envelope per remaining authorized device, for the workspace domain and each restricted domain the removed device could read. Its size depends on devices and domains and never on objects. [§28](#28-worked-example-100000-objects) works through the numbers.

**What rotation protects.** A removed device keeps the keys and plaintext it already had. It can't decrypt anything encrypted under a later epoch, and a card that a link removed never returns ([§8](#account-heads)). Lineage gives every holder of a current key every earlier key, so a member that loses access and later gets it back can read what was written while it was away, if it can get that ciphertext. The app warns whoever invites a member again, or grants a restricted domain to an account again, that the account will be able to read what was written while it was away.

### History floor

A domain key opens every earlier ciphertext in that domain through lineage, so the server decides how much history a new member sees. The server keeps a history floor per account and domain. It never serves an entry with a sequence at or below the floor through pulls or chain fetches, except that a chain fetch serves the requesting device its own records, which it signed. `live` and `manifest` answer only at a checkpoint whose `covered_seq` is at or above the floor, and there they serve every operation live at that checkpoint, whatever its sequence, because a member sees current content ([§18](#18-bootstrap)). The server serves a blob only when an operation that lists it sits above the floor or is live at the checkpoint the device reads.

| Event | Floor |
|---|---|
| A member joins | `covered_seq` of the checkpoint committed with the approval |
| A grantee joins a restricted domain | `covered_seq` of the checkpoint committed with the grant |
| A member's new device | the member's existing floor |
| A device restored from the member's recovery kit | the member's existing floor |
| A member's account accepted after a reset | the account's existing floor |

An approval or grant that sets a floor carries a fresh checkpoint for each affected domain, signed by a device that can write it, and the floor equals that checkpoint's `covered_seq`. When an `add_member`'s push carries no checkpoint of the workspace domain, or a grant's none of its domain, the floor is the sequence of the change's own entry there, so the account reads nothing earlier and bootstraps once a later checkpoint commits. That `covered_seq` may equal the sequence of the domain's latest checkpoint entry, so a quiet domain still takes one ([§17](#checkpoint)). Records that land between signing and commit sit above the floor, so the new device pulls them normally. The server answers a request below the floor with `before_history_floor`, and the client bootstraps.

History isolation for new members depends on the server. [§30](#30-threats) records that limit.

## 10. Approval

Approval binds a new device to a device you already hold, and the reverse, with one short code shown on both screens. It brings in your own new device, an invited member's device, and the device of an account whose reset a workspace accepts ([§7](#resetting-your-devices)). The code mixes in a secret that the candidate commits to before it sees the offer, so a party in between can't steer the code.

### Approval request

The candidate enrolls and, for each attempt, draws a fresh 32-byte nonce `nc` and posts a request (`POST /v2/approval-requests`) carrying its commitment:

```text
commit = BLAKE3(tag(noura.sync.approval.commit) || candidate_card_digest || opt<account_head> candidate_head || nc)
```

where `candidate_card_digest` is `BLAKE3(candidate card)`, and `candidate_head` is absent for a device of your own account, which joins your chain, and for an invitation or a reset is the head of the candidate's account chain at which its card is active ([§8](#account-heads)). The request carries `candidate_head` too. For an invitation, the request also names the invitation and carries the invitation proof ([§11](#11-invitations)). For a reset, it names the workspace whose acceptance it asks for, and the server takes it only from a member account whose current chain isn't the one that workspace records. A request names at most one of an invitation and a workspace (else `bad_request`), and comes from a session whose card enrolled; a request for your own account comes from a card that no chain of the account lists. A request expires 10 minutes after the candidate posts it. An account holds at most 32 pending requests, and the server answers another with `rate_limited`, whose `retry_after_seconds` says when the oldest one expires. A request that names an invitation needs that invitation open and accepted by the session's account, which isn't a member of its workspace, and a `candidate_head` of that account (else `invitation_invalid` or `forbidden`). A workspace holds at most 100 pending requests that name it through an invitation or a reset, and the server answers another the same way. Approvers see the card and the request through `GET /v2/approval-requests` for devices of their own account, or, from an owner or admin device, `GET /v2/workspaces/{w}/pending-devices` for invited and reset accounts. The server takes an offer on an invited account's request only from a device of the inviting account that is an owner or admin of the workspace while the invitation is open.

### Approval offer

The approving device catches up on every domain the approval covers, then sends a signed offer.

`offer`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.approval.offer)` | |
| approver_card | `bytes` | encoded card of the approving device |
| candidate_card_digest | `digest` | `BLAKE3(candidate card)` |
| candidate_commit | `digest` | the request's `commit` |
| candidate_head | `opt<account_head>` | copied from the request |
| approver_head | `account_head` | the approver's account chain head; for your own new device, the `add_device` follows it directly |
| nonce | `key` | 32 random bytes chosen by the approver after it received the request |
| anchors | `list<anchor>` | sorted by workspace |
| sig | `sig` | by the approving device |

`anchor` is `id workspace | u64 revision | digest policy_head | u64 generation | checkpoint_anchor`, and `checkpoint_anchor` is `id domain | u64 covered_seq | digest checkpoint`: the approver's accepted policy head in each workspace the approval covers, the restore generation in which it accepted the checkpoint that follows ([§14](#after-a-server-restore)), and its latest accepted checkpoint of the workspace domain. Until that domain holds a checkpoint, `checkpoint_anchor` names none, with `covered_seq` 0 and `checkpoint` 32 zero bytes, and the candidate pulls the domain from sequence 0. Approving your own new device lists every workspace your account belongs to, about 120 bytes each, so an offer for 1,000 workspaces stays near 120 KB. Approving an invited member's device, or accepting a reset, lists one workspace. The candidate bootstraps restricted domains without an anchor ([§17](#checking-a-checkpoint)). A restore hands out again sequences that clients saw, so a device compares a checkpoint anchor with a checkpoint only when both come from the same generation. When a restore lost the checkpoint anchor itself, the device accepts the server's checkpoint under the other bootstrap checks; a restore that lost the anchor's policy head stops the device in that workspace, as [§14](#after-a-server-restore) describes.

### Approval code

```text
offer_digest  = BLAKE3(offer body || sig)
code_input    = tag(noura.sync.approval.code) || offer_digest || nc
approval_code = (u64 from the first 8 bytes of BLAKE3(code_input), big-endian) mod 100,000,000
```

Both devices show the approval code as 8 decimal digits, with leading zeros, in two groups of 4.

1. The candidate enrolls, posts its request and shows its own fingerprint.
2. You select the request on the approving device, which shows the candidate's fingerprint, catches up, and sends the offer.
3. The candidate takes the first offer that names its commitment, checks it, then reveals `nc` for that offer alone (`POST /v2/approvals/{offer_digest}/reveal`). The approving device fetches `nc` and checks it against `candidate_commit`. Both devices show the code.
4. You compare the codes and confirm on both devices. A mismatch, or a cancel on either device, ends the attempt: both discard the offer and `nc`, and a new attempt starts from a new request.
5. The approving device commits the approval. For your own new device, it appends `add_device`, with `offer` set to `offer_digest`, to your account chain right after `approver_head`, then pushes, in each anchored workspace, an `account_heads` change recording the new head, with the envelopes that [§8](#account-heads) requires. For a new member, it pushes one `add_member` recording `candidate_head`, a current-epoch envelope of every domain the member reads for each card active at that head, and a fresh checkpoint of the workspace domain, so the member's floor has a checkpoint to bootstrap from. For a reset, it pushes one `accept_reset` recording `candidate_head`, with the envelopes that [§8](#account-heads) requires.
6. A device of your own account verifies your chain from genesis to `approver_head` and the `add_device` right after it, signed by the approver's card, and keeps that head ([§7](#account-chain)). Every candidate verifies each anchored workspace's log from genesis to its anchor, then onward. Only the approver's card signs the `add_device` link, or the `add_member` or `accept_reset` of an invitation or a reset; for your own new device, the candidate accepts any valid change that records your account at or after the head that `add_device` makes, whichever device of your account signed it first ([§8](#account-heads)). A candidate for an invitation or a reset then records its account's head in that workspace itself, while no change signed by a device of its account records that head, so its account's other devices trust the workspace too ([§8](#account-heads)).

Before revealing `nc`, the candidate checks the offer's signature under `approver_card`, and that `candidate_card_digest` equals `BLAKE3` of its own card and `candidate_commit` equals its own commitment and `candidate_head` is its own; otherwise it reports `sync_approval_mismatch` and reveals nothing. Each `nc` serves one offer and one comparison: the candidate never shows a code for any offer but the first. When a second offer names the same commitment before you confirm, whether it arrives before or after the reveal, the candidate ends the attempt with `sync_approval_mismatch`, discards `nc`, and shows no code; a new attempt needs a new request with a fresh `nc`. A second offer that arrives after you confirm changes nothing, and the candidate reports it. If two of your devices both answer a request, the app asks you to approve from one of them. An approving device sends an offer only for a request you selected on it, keeps at most one offer outstanding, and shows a code only for that offer; it ignores other requests until that attempt ends. After you confirm, the candidate verifies each workspace's chain up to its anchor and checks that the approver's card is a device of the workspace at that anchor with the authority to sign the change that follows. It bootstraps the workspace domain only from its checkpoint anchor or a checkpoint that descends from it ([§17](#checking-a-checkpoint)), else `sync_manifest_rollback`. For an invitation, the approver must be the inviting device: `BLAKE3(approver_card)` must equal the context's `inviter_fingerprint` ([§11](#11-invitations)). An offer expires 10 minutes after the approver sends it.

A server that swaps either card must show each device an offer or a commitment of its own choosing. The candidate's code depends on `nc`, which the server learns only after it has fixed the one offer the candidate will ever use with that `nc`. A server that learns `nc` and then grinds further offers toward the approver's code gains nothing, because the candidate ends the attempt at the second offer. The approver's code depends on an offer that already commits to the request, so the server can't change it after the approver signs. The approver offers only on the request you selected and keeps one offer outstanding, so the server learns one approver code per attempt you start and can't collect codes from requests it posted itself. The two codes therefore match with probability 10^-8 per attempt, and every attempt asks you to confirm. Passing the approver's offer for a substitute card through to the real candidate fails the digest check. A server that swaps the candidate's account head fails the same way, because the commitment covers it and the approving device checks `nc` against it.

## 11. Invitations

An owner or admin invites someone by link. The link carries an invitation secret in its fragment, so the server never sees the workspace name or the inviter's details.

```text
{origin}/invite/{base64url(token)}#{base64url(secret)}
```

The inviting device generates the invitation ID (16 bytes), the token (16 bytes) and the secret (32 bytes), and keeps the secret until the invitation ends. While the invitation is open, the server stores `BLAKE3(tag(noura.sync.invite.token) || token)`, the workspace, the role, the inviting account, the accepting account once known, the status, an expiry seven days out, and the context below. It deletes the row once the invitation is approved, revoked or expired, or once a change leaves the inviting account unable to grant the invitation's role under [§8](#roles-and-authority), such as an admin demoted or removed. After an approval or a revocation it keeps only the invitation ID until the expiry the row had, and answers a retried creation under that ID with `invitation_invalid`, so a late retry can't open the link again. A workspace holds at most 100 open invitations.

`invite_context`, encrypted with the subkey `derive_key("noura.sync.v2 invitation key", secret)`:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.invite.context)` |
| workspace | `id` |
| invitation | `id` |
| role | `u8` |
| *AEAD associated data ends here* | |
| nonce | `nonce` |
| ciphertext | `bytes` |

Its padded plaintext:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.invite.payload)` |
| workspace_name | `str` |
| inviter_name | `str` |
| inviter_fingerprint | `digest` |

You open the link in the desktop app, or paste it there. The page the server serves at `{origin}/invite/{token}` only tells a browser visitor to open the link in Noura, and never reads the fragment. The app keeps the secret in memory, fetches the context by token, decrypts it, and shows the workspace name, the inviter and the role; a wrong secret or tampered context shows a generic error. You sign in or sign up through the device authorization grant ([§7](#desktop-sign-in)), where the server's email allowlist still applies, and the app accepts the invitation with the token. Acceptance binds the invitation to your account and grants nothing.

Your device's approval request names the invitation and carries `invite_proof = keyed_hash(derive_key("noura.sync.v2 invitation proof", secret), card)`. The inviting device checks the proof before it sends an offer, so a server that never saw the secret can't put its own card in your place. The inviting device then approves your device with an approval code ([§10](#10-approval)), which you compare with the inviter over a channel you both trust, such as a call. Only that commit adds you to the policy and wraps keys for you. Your app checks that the approval offer comes from the card matching `inviter_fingerprint`, and that the `add_member` change names this invitation and records the account head your request committed to. Only the inviting device can approve; if that device is lost, an owner or admin revokes the invitation and sends a new one. Revoking an invitation makes its link fail.

## 12. Recovery kit

A recovery kit is an offline device. Exporting a kit creates a recovery identity, a card with its own signing and HPKE keys, and appends it to your account chain with `add_recovery`, after a `remove` of the previous identity when one is active, so one kit works at a time ([§7](#account-chain)). The exporting device then records that head in every workspace of your account, with the envelopes that [§8](#account-heads) requires, and rotations wrap keys for the recovery identity like any other card, so the kit never goes stale. The app asks you to export a new kit after you join a workspace, so the kit lists it. Removing the recovery identity revokes the kit. When you replace a kit, the `remove` of the old identity also takes down every device that the old kit recovered unless you keep it, and the app lists those devices for you. A recovery identity signs one link, `recover_device`, once, and no policy change ([§7](#account-chain)).

A kit is also a standing key. Because every rotation wraps the new key for the recovery identity, the kit file and its secret together decrypt every current and future record in every domain your account reads, for anyone who also gets the ciphertext: the server's operator, a copy of a server backup, or someone holding your account session. That reader needs no device, and nothing alerts you. When you export a kit, the app says so and asks you to keep the file and the secret in different places. It offers to revoke the kit and export a new one after you remove a device, and whenever you suspect the kit was exposed.

The kit is a file you pick outside every workspace, written with mode 0600 on Unix, plus a secret the app generates: 20 random bytes, shown once as 32 characters of RFC 4648 base32 in 8 groups of 4, for you to write down. Readers ignore spaces, hyphens and letter case when you type it back. With 160 random bits, an offline guess at the secret is out of reach, so the kit key needs no stretching.

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.recovery-kit)` | |
| salt | `id` | 16 random bytes |
| *AEAD associated data ends here* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | XChaCha20-Poly1305 under the kit key `derive_key("noura.sync.v2 recovery kit", secret \|\| salt)`, where `secret` is the 20 secret bytes |

Its plaintext:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.recovery-kit.payload)` |
| origin | `str` |
| account | `id` |
| account_head | `account_head` |
| card | `bytes` |
| signing_seed | `key` |
| hpke_secret | `key` |
| workspaces | `list<kit_workspace>` |

`kit_workspace` is `id workspace | digest genesis | u64 revision | digest head | u64 generation | list<checkpoint_anchor>`, sorted by workspace: the digest of the workspace's revision-1 policy change, the accepted policy head, the restore generation of the checkpoints that follow, and the latest accepted checkpoint per domain when the kit was written; the workspace domain's anchor names none, as in [§10](#approval-offer), until that domain holds a checkpoint. The device compares a checkpoint anchor only with a checkpoint of the same generation ([§10](#approval-offer)). `account_head` is your account chain's head when the kit was written, at which this recovery identity is active. `hpke_secret` is the 32-byte X-Wing seed. A wrong secret or tampered file reports `sync_recovery_kit_invalid`.

To recover after losing every device, you sign in on a new computer, enroll it, import the kit and type the secret. The new device:

1. Verifies the kit's card against its seeds.
2. Proves to the server that it holds the recovery identity's signing key. It signs a `recovery_proof` and posts it to `POST /v2/recovery`; for the rest of that session the server lets it read the envelopes addressed to the recovery identity and append the `recover_device` link that the identity signs. The proven session lasts until the recovered device is active in each workspace, also after a `remove` link takes the identity down ([§7](#account-chain)).
3. Checks that your account chain extends the kit's `account_head` and that the recovery identity is still active at the server's head, the confirmed head, and that each listed workspace's log starts at the stored genesis digest and extends the stored head (else `sync_account_forked` or `sync_policy_rollback`), and refuses a checkpoint below the kit's checkpoint anchors. For each workspace that `GET /v2/workspaces` lists and the kit doesn't, such as one you joined after writing the kit, it verifies the log through a change that records a head of your account from the kit's `account_head` through the confirmed head, signed by a card active at the confirmed head, else `sync_workspace_unverified`. It shows you every account link and policy change after the kit's heads, and every change and member of each workspace it verified this way, with their signers, and you confirm them and choose the devices you still hold and keep before it goes on, so steps 4 and 5 run without a pause. Until step 5's `remove` links commit, the device trusts no workspace other than those the kit lists and step 3 verified, and reports `sync_workspace_unverified` for any other.
4. Appends `recover_device` for its own card, signed by the recovery identity, which makes it an active device.
5. In the same flow, removes every other card of your account, except the devices you kept in step 3, in this order. First, in each workspace that the kit lists or step 3 verified, it pushes one `account_heads` change recording the head right after `recover_device`, which adds its own card and removes none, with that card's current-epoch envelopes, wrapped with the keys it unwrapped from the recovery identity's envelopes ([§8](#account-heads)). Then it appends a `remove` link for each other active card whose parent isn't active, so the links take down every card but itself and the devices you keep, the recovery identity included. Last, in each of those workspaces, it pushes one `account_heads` change recording the head after the `remove` links, which only takes the other cards out, then rotates every domain it reads. The `remove` links may commit before the device's first change reaches its last workspace, because the proven session keeps its rights until then (step 2). It lists the members added after the kit's heads for you to review, then exports a new kit with a new secret.

The kit's heads are the recovering device's only floor against rollback. A server can show it your account chain up to the kit's head, extended with links that a stolen device signed, such as adding devices. Removing every other card of your account by default keeps the rotations in step 5 from wrapping keys for those. To keep the floor recent, the app asks you to export a new kit after each removal. When a device removed the recovery identity, the kit can't recover; you reset your devices instead ([§7](#resetting-your-devices)).

`recovery_proof`, signed with the recovery identity's signing key:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.recovery.proof)` | |
| origin | `str` | as in the enrollment proof |
| recovery_device | `id` | the recovery identity's device ID |
| new_card_digest | `digest` | `BLAKE3` of the new device's enrolled card |
| challenge | `key` | a fresh enrollment challenge |

The server checks the signature under the recovery identity's card, that the card is the active recovery identity of the session's account chain, that the new card is the session's enrolled card, and that the challenge is current and unused.

The recovered device becomes an active device of your account: it can approve your later devices. The kit restores keys, never the old device's signing identity or session.

After the flow, the new device trusts a workspace that neither the kit nor step 3 covered only as [§8](#verifying-the-chain) describes: through a change that records a head at or after the kit's `account_head`, signed by a card active at the head the device keeps. A card that a link removed, such as a stolen device that step 5 removed, confers no trust. Until then the device reports `sync_workspace_unverified` there and tells you it can't verify it.

## 13. Operations

An operation records one change to one object.

### Operation record

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.op)` | |
| workspace | `id` | |
| domain | `id` | |
| device | `id` | the signer |
| counter | `u64` | position in this device's chain for this domain: at least 1, and above the previous record's; it skips the counters a device reserved and never used ([§14](#your-own-chain)) |
| previous | `digest` | digest of the device's previous record in this domain; 32 zero bytes for its first |
| epoch | `u64` | the epoch whose key encrypts the payload |
| policy_revision | `u64` | the policy head the device saw when it signed; at least the previous record's in this chain |
| policy_head | `digest` | digest of the change at `policy_revision` |
| previous_attempt | `digest` | digest of the attempt the server last rejected at this chain position, when this record re-seals the same change or replaces a record the server rejected for its object ([§14](#your-own-chain)); otherwise 32 zero bytes |
| generation | `u64` | the server's restore generation when the device signed ([§14](#after-a-server-restore)); at least the previous record's in this chain |
| checkpoint_seq | `u64` | `covered_seq` of the latest checkpoint the device reached in this domain, from its log, its own receipt or bootstrap; 0 before the first |
| checkpoint | `digest` | digest of that checkpoint; 32 zero bytes before the first ([§17](#checkpoint-chain)) |
| rid | `id` | |
| flags | `u8` | 1 for a delete, 0 for any other operation ([§19](#19-compaction)) |
| supersedes | `list<cover>` | sorted by device, one entry per device, at most 256 |
| blobs | `list<id>` | blob IDs the payload references, sorted, at most 4 |
| *AEAD associated data ends here; the epoch record's digest follows ([§4](#records-signatures-and-digests))* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | padded payload under the operation subkey, with its 16-byte tag |
| sig | `sig` | by `device` |

`cover` is `id device | u64 counter`. It means every operation on this `rid` from that device with a counter at or below this one is superseded.

### Payload

| Field | Type |
|---|---|
| tag | `tag(noura.sync.payload)` |
| rid | `id` |
| kind | `u8` |
| body | kind-specific |

| Kind | Name | Class | Body |
|---|---|---|---|
| 1 | file | state | `str object_id`, `bytes path`, `digest revision`, `content`, `opt<origin>` |
| 2 | delete | state | `str object_id`, `digest revision`, `u8 reason` (1 deleted, 2 moved to another domain), `opt<origin> target` |
| 3 | text_snapshot | state | `str object_id`, `digest revision`, `id text_lineage`, `list<meta>`, `content` holding an encoded yrs document, `opt<origin>` |
| 4 | text_update | patch | `str object_id`, `id text_lineage`, `u32 load`, `list<meta>`, `content` holding a yrs update |

- `object_id` is the managed file's frontmatter ID, or empty for an ordinary file.
- `path` is a workspace-relative UTF-8 path of at most 4,096 bytes, with `/` between components. Clients reject absolute paths, `.` and `..` components, empty components, control characters, Windows reserved names and characters, trailing dots or spaces, and the reserved roots `.noura` and `.git` in any ASCII case. Paths compare byte for byte; [§16](#16-conflicts) defines when two paths collide.
- `revision` is the BLAKE3 hash of the file's bytes as materialized; for a delete, of the deleted bytes, or zeros when unknown.
- `content` is `u8 form` then either form 1, `bytes inline`, or form 2, `blob_ref`.
- `blob_ref` is `id blob | u64 ciphertext_length | digest ciphertext_digest | key blob_key | u64 plaintext_length`, where `plaintext_length` is the file's length `L` ([§21](#21-attachments)).
- `origin` is `id domain | id rid`: in a state operation, where a cross-domain move came from; in a reason-2 delete, where it went.
- `text_lineage` names the yrs document that holds a text-mode note's body ([§22](#22-live-collaboration)).
- `load` is the load number, from 1 to 2^29 − 1, of the document load whose yrs client ID a text update inserts under: the signer's card index in the workspace ([§8](#card-index)) times 2^29, plus `load` ([§22](#the-document-on-a-device)).
- `meta` is `str key | u64 seq | opt<bytes> value`, sorted by key: a text-mode note's path under the key `path`, and each property under `p:` followed by its name, whose value is the property's RFC 8785 canonical JSON. A text-mode note's path and properties come from these entries, which merge per key as [§22](#updates) describes, with `seq` placing a snapshot's entries.

A payload's encoded length, `n` in [§6](#6-padding), is at most 262,144 bytes, so the server rejects an operation whose ciphertext exceeds 270,352 bytes, `padded_length(262,148)` plus the tag, with `too_large`. Larger content goes in a blob.

### Supersession and live operations

The server and the client compute the same live set for each `rid` from cleartext fields: an operation is live unless another operation on the same `rid` covers it. The server needs no plaintext to compute it, compact the log, or check a manifest.

- A state operation covers the writer's view of that `rid`, naming each covered device's highest live counter, its own device's included. While the `rid` holds one live state operation, that view is every live operation. While it holds two or more, a conflict ([§16](#16-conflicts)), an ordinary edit or delete covers only the branch the writer's file matched and every live state operation identical to it (the same decrypted path and revision), with their patches and its own device's earlier operations, so the other branches stay live; only a choice between branches, or the merge of disjoint changes, covers every branch. A delete sets `flags` to 1, and once it covers every live operation the tombstone stays live alone until compaction drops it ([§19](#19-compaction)). A delete that covers nothing on an existing `rid` counts as a patch, and clients treat it as unreadable ([Client checks](#client-checks-on-an-operation)). Covers name only live operations, except a rotator's rewrite after a replacement ([§9](#epochs)), and the server prunes nothing live except such a lone delete behind the retention floor.
- A patch covers nothing. It applies to the text lineage it names ([§22](#22-live-collaboration)).

The server counts an operation whose `supersedes` is empty on an existing `rid` as a patch, and any other operation as a state operation. When a state operation would need more than 256 covers, it covers the 256 devices with the most live operations on that `rid`, its own device among them, and the rest stay live.

### Server checks on an operation

The server accepts an operation only when:

- the session's device is the signer and is active in this workspace's policy, and the signature verifies under its card from its account chain;
- the device can write the domain at the domain's latest policy entry, which is the current policy (else `forbidden`), and could write it at `policy_revision`, which is at most the current revision (else `policy_stale`) and at least the previous record's in this chain (else `policy_regressed`); `policy_head` equals the server's change at that revision (else `policy_conflict`) ([§8](#policy-entries));
- `epoch` is the epoch of the domain's latest epoch entry, and no rotation is pending (else `epoch_stale` or `rotation_pending`; [§9](#rotation));
- `generation` is at most the server's current restore generation and at least the previous record's in this chain (else `policy_regressed`), and when it equals the current generation and `checkpoint_seq` is at or above the retention floor, `checkpoint` names a checkpoint of this domain with that `covered_seq` (else `checkpoint_unknown`);
- `counter` is above the server's head for this device and domain, and `previous` equals that head's digest, or 32 zero bytes when the device has no record there (else `chain_conflict` with the head);
- `rid` is new, or already belongs to this domain;
- every cover names an existing operation on this `rid` with exactly that device and counter (else `supersedes_unknown`);
- a state operation leaves no earlier state operation of its own device live on this `rid` (else `supersedes_incomplete`), so one device holds at most one live state operation per `rid`;
- after this operation the `rid` holds at most 2,048 live patches with at most 8 MiB of ciphertext (else `snapshot_required`) and at most 64 live state operations (else `conflict_limit`);
- every blob is a completed upload created for this domain (else `blob_incomplete` or `blob_domain_mismatch`);
- the ciphertext length is a valid padded length (else `padding_invalid`).

A retried upload of an operation the server already holds, with the same digest, returns the original receipt. The same position with a different digest returns `chain_conflict`.

### Client checks on an operation

Before decrypting, the client verifies the signature under the signer's card from its account chain, at the head the workspace's verified policy records, checks `policy_head` against its own change at `policy_revision`, checks that the signer could write the domain at the latest policy entry before the record and at `policy_revision`, which is at most that entry's revision ([§8](#policy-entries)), checks that neither `policy_revision` nor `generation` decreased along the chain, checks that the operation is epoch-valid ([§9](#rotation)), runs the chain verifier ([§14](#14-device-chains)), and checks the checkpoint it names ([§17](#checkpoint-chain)). A failure reports `sync_writer_not_authorized`, `sync_policy_forked`, `sync_policy_regressed`, `sync_epoch_stale` or a chain error, changes no file, and holds the domain's cursor. An honest server rejects such a record, so the failure shows a lying server or a misused key.

The client also needs the verified key of the record's epoch. At an epoch entry, or a record under an epoch, whose key it doesn't hold yet, it fetches keys ([§26](#26-http-api)) and holds the domain's cursor until it has unwrapped the key, or derived it through lineage, and checked it ([§9](#epochs)), or until a replacement skips that epoch; when its envelope fails, it replaces the epoch or asks for a re-wrap. Until then it neither applies the record nor counts it as unreadable, so a key that arrives late, or a server that delays the key route, costs time and never drops a record.

A record that passes those checks, under a key the client holds, but fails to decrypt, unpad, parse, match its header's `rid` and delete flag, match the class the server gave it, or pass the collaboration sandbox ([§22](#22-live-collaboration)) came from its signer's key, and the server couldn't see the problem. A payload matches its class when a `file`, `delete` or `text_snapshot` sits in an operation the server counts as a state operation and a `text_update` in one it counts as a patch ([Supersession and live operations](#supersession-and-live-operations)). The client counts it as applied: its chain head, cursor, live set and manifest include it, and a later state operation can supersede it. The client keeps the object's last good state on disk, lists the record in the review store as an unreadable version attributed to its signer, and reports `sync_record_unreadable` with the device and domain. A record encrypted under a different epoch record for the same epoch fails to decrypt the same way ([§4](#records-signatures-and-digests)), and checkpoints then expose the split as `sync_key_mismatch` ([§17](#17-manifest-and-checkpoints)). A record under an epoch that a replacement skipped counts as unreadable too, on a device that never accepted that epoch, when [§9](#epochs) says so.

## 14. Device chains

Each device keeps one chain of operations per domain it writes to. Every record carries the device's counter and the digest of its previous record under the signature, so a server can't remove, reorder or duplicate a record without breaking a link that the device signed.

Chains are per domain because a member outside a restricted domain never sees that domain's records. A single chain per device would show that member gaps it could never explain.

### Verifier

The client keeps, per domain and device, the accepted head `(counter, digest, sequence)` and the counter, digest and sequence of each accepted record above its latest accepted checkpoint in that domain, with the digests of a settled fork's other branch above it ([§14](#verifier)); below that checkpoint it keeps only the device's head there. Its verifier state therefore stays within one checkpoint interval per domain ([§17](#checkpoint)). Heads taken from a checkpoint bundle carry their sequences ([§17](#17-manifest-and-checkpoints)), so these checks work right after bootstrap. The client processes each pull page in sequence order. For a record with counter `c`, previous digest `p` and digest `h` at sequence `s`:

1. **Counter at or below the head.**
   - The client holds the record at `c` with digest `h` at sequence `s`: the page repeats a position the client already applied (for example after a restart), so the client ignores it.
   - The client holds the record at `c` with digest `h` at another sequence: `sync_chain_replayed`.
   - The client holds a different record at `c`, and one of the two names the other in `previous_attempt`, directly or through other attempts at `c` that the client has seen or the page carries: the two are attempts at one position ([§14](#your-own-chain)), and the one whose links reach the other is the later. If the client holds the later attempt, it ignores the earlier. If it holds the earlier attempt as its head, with no successor applied, it switches to the later attempt and recomputes the object's live set. If the earlier attempt has a successor that the client applied, it reports `sync_chain_forked`.
   - The client holds a different record at `c` and neither names the other, or two records at `c` name the same `previous_attempt`: `sync_chain_forked`. Attempts at one position form a single line, each naming the one before, and their number has no cap when newer epochs or policy changes force the re-seals ([§14](#your-own-chain)).
   - The client holds no record at `c` (it bootstrapped past it, the record sits below its latest accepted checkpoint, or the device skipped that counter): every legitimate record at or below the head has a sequence at or below the head's, so `s` above the head's sequence reports `sync_chain_replayed`.
2. **Counter above the head, and `p` equals the head's digest** (32 zero bytes when the client knows no record of that device in the domain): accept and advance the head. A counter more than one above the head skips counters the device reserved and never sent, or signed before a server restore lost them ([§14](#your-own-chain)); the device's signature over `previous` shows that it skipped them.
3. **Counter above the head, and `p` differs from the head's digest.** The page left records out, the record follows a later attempt of the head, or the chain forked. The client first fetches the head's own position: when the server returns a later attempt of the head that rule 1 accepts, the client replaces the head with it and judges the record again. Otherwise it fetches the device's records after its head in chain order (`GET .../chain/{device}/{counter}` returns the record at that counter or, when the device skipped it, the device's next record above it, [§26](#26-http-api)) and judges each by these rules until it reaches the record. A fetched record whose `previous` names neither the head nor the record fetched before it reports `sync_chain_forked`. A fetched record at a sequence above `s` reports `sync_chain_reordered`. One at a sequence below `s`, or an answer that leaves the chain short of the record, reports `sync_chain_gap`. A `pruned` answer means the client's head fell below the retention floor, and the client bootstraps ([§18](#18-bootstrap)).

On any of these errors the client applies nothing from the page, changes no file, keeps its cursor and reports the error with the device and domain. It retries on the next pull, so a transient server fault clears itself.

A device that forks its own chain halts that domain for its readers until someone removes its write access. A reader holding `sync_chain_forked` for device X in domain D keeps its cursor until it accepts a policy change r that ends X's write access to D. It then fetches checkpoints from the latest back through `previous` to the first whose `policy_revision` is at or above r, which is the first to cover r's entry, keeps the branch of X whose head that checkpoint lists, ignores the other branch's records, and resumes. The reader looks for r after the latest policy entry before X's forked record, so r is the first change after the fork that ends X's write access. The kept branch reaches back, through `previous` links or as a later attempt, either to the head of X that the reader applied or to a record of X that it applied below that head: the fork point. Above a fork point the reader applied the other branch, so it reverts those records: each leaves its object's live set and the journal, the object's file follows its live set again, X's head moves back to the fork point, and the reader ignores them with the rest of the other branch. Below its latest accepted checkpoint the reader keeps only heads, so it can't find a fork point there and keeps reporting `sync_chain_forked`; only a checkpoint whose heads list X on the other branch leads there, and an honest signer never signs one, because it applies the same records. The checkpoint still gets its full check when the reader reaches it in its log ([§17](#checking-a-checkpoint)).

A retried upload never triggers an error: the server stores one copy, and a page that repeats a position at the same sequence is ignored.

### Suffix withholding

A chain proves what the server served, never that it served everything. A client learns a device's latest counter from three sources: later records from that device, the chain heads in each checkpoint, which reach every puller as log entries ([§17](#17-manifest-and-checkpoints)), and, for its own chain, the receipts the server returned. A head above what the client holds, at a sequence the server claims to have delivered, reports `sync_chain_gap`. Checkpoints form a chain, and every record names the latest checkpoint its writer reached ([§17](#checkpoint-chain)). A server that hides a checkpoint from one client must therefore also hide every later checkpoint and every record whose writer reached it, and the split shows once one such record or checkpoint crosses to that client. A server that withholds a device's records from everyone, and every checkpoint mentioning them, looks like a device that went offline; the app shows when it last heard from each device.

### Your own chain

A device persists each signed record before sending it, so a retry sends the same bytes. The credential store holds a reserved counter per domain ([§15](#client-journal)), and the device signs no counter above it: it first raises the reservation by a block of 1,024, so the credential store takes one write per 1,024 records. Each time the app starts, the device raises every reservation by one more block and signs its next new record in each domain at the start of that block, with `previous` naming its last record there ([Verifier](#verifier) rule 2). The counters it skips were either never signed, or signed into a journal that a crash or a restored backup rolled back, so the device never signs one counter twice, whatever order a crash leaves the journal and the credential store in. On startup, and on `chain_conflict`, the device compares its journal's head with the server's head for its own chain (`GET .../heads/{device}`):

- A server head above the journal's means the journal lost records that the device sent. The device verifies that the server's records above its journal carry its own signature, takes them into its journal, and continues after them.
- A server head below a record the server acknowledged, or below the device's head in its latest accepted checkpoint, in the same restore generation, means the server went back: the device reports `sync_server_rollback` ([§15](#pull)). After a generation change, [After a server restore](#after-a-server-restore) applies.
- A journal without a reserved counter in the credential store means the keys are new: the machine enrolls as a new device ([§7](#device-card)).

When the server rejects a record for a reason the device must fix (`epoch_stale`, `rotation_pending`, `supersedes_unknown`, `blob_incomplete`, `policy_regressed`), the device first checks the reason against data it can verify:

- for `epoch_stale` and `rotation_pending`, it fetches policy and keys, and re-seals only after accepting a newer epoch or a change that requires one;
- for `supersedes_unknown`, it fetches each covered operation by chain position and drops the covers the server can't return;
- for `blob_incomplete`, it reads the upload's offset;
- for `policy_regressed`, it rereads its accepted policy and its own chain.

When the check contradicts the server, the device doesn't re-seal: it retries the same bytes later and reports the server's error. Otherwise it re-seals the change at the same counter with `previous_attempt` set to the digest of the attempt the server rejected last at that position. A re-seal that follows a newer epoch or a rotation-requiring policy change the device accepted has no cap. Any other re-seal counts toward a cap of 3 per position. At the cap the device reports `sync_reseal_limit` for that domain, signs no other attempt at that position, and stops writing the domain: it retries the last attempt's bytes later, and its local files keep their changes. A rejected push names the failing record ([§26](#message-bodies)), and the device re-seals that record and every later record of the same chain in the batch, whose `previous` links change; each of those names the attempt it replaces at its own position. A server that kept a rejected attempt and serves it shows readers two consecutive attempts, which rule 1 above resolves without decryption. A device whose write access is gone drops the record; its local file keeps the change.

Three rejections concern the record's object, not the record: `snapshot_required`, `conflict_limit` and `quota_exceeded`. For these the device puts a different record at the same position instead of the same change, so one busy object never stalls the rest of the domain. For `snapshot_required` it writes a text snapshot that includes the rejected patch ([§22](#22-live-collaboration)). Otherwise it sends the next record in its outbox, such as a delete you made to free space, or retries later when the outbox holds nothing else, and keeps the rejected change locally: it retries a `quota_exceeded` change once space frees up, and shows a `conflict_limit` change as a conflict for you to resolve, which the server accepts because a resolution covers every live branch. The replacement names the rejected attempt in `previous_attempt` and doesn't count toward the cap.

### After a server restore

A restore from a backup brings back older server state. The server forgets the account chain links, policy changes, epochs, envelopes, records, chain heads, checkpoints, accounts, sessions, share links and blob rows created after the backup. v2 repairs no signed state after a restore: each device stops what relied on signed state the server lost, until you recover it by hand, and writes lost content again from its files.

**Server secret.** The server keeps a 32-byte secret outside its database. A SQLite deployment keeps it as a file in its data directory, and a PostgreSQL deployment gives every instance the same secret through the environment variable `NOURA_SERVER_SECRET` or a mounted secret file that `NOURA_SERVER_SECRET_FILE` names, as 64 hexadecimal digits. In SQLite mode the first `migrate` on an empty database creates the secret file; in PostgreSQL mode the operator creates the secret, for example with `openssl rand -hex 32`, and `migrate` refuses to run without one. The first `migrate` on an empty database stores the secret's key check, `derive_key("noura.sync.v2 secret check", secret)`, in the database. `serve`, `migrate`, `restore` and `mark-restored` refuse to run when the secret is missing or doesn't match that check, and nothing creates a secret for a database that holds one. `backup` writes the secret beside the database copy, an operator who backs up PostgreSQL with its own tools keeps the secret with each backup, and a restore needs both: with another secret, every account ID would change. The server derives the `noura.sync.v2 account id` key from the secret with BLAKE3 `derive_key`, and gives each account its ID: the first 16 bytes of `BLAKE3.keyed_hash(key, email)`, over the email address the account signed up with, with its ASCII letters lowercased and every other byte kept, so no Unicode update changes an ID. No Unicode case mapping, case folding or normalization applies: `ÅSA@Example.se` and `Åsa@example.se` share an ID, and `åsa@example.se` gets another (`sync-v2-ids.json`). v2 has no route that changes that address, so signing up again after a restore recreates the account under the ID its workspaces name, and nobody else can claim that ID. Every passkey of an account carries the WebAuthn user handle made of the first 16 bytes of `BLAKE3.keyed_hash(derive_key("noura.sync.v2 passkey user", secret), account ID)`, so a sign-up ceremony, which anyone can start with an email, doesn't show the ID that email gets.

**Restore generation.** The server keeps one restore generation for all its workspaces. `backup` copies a running SQLite database with `VACUUM INTO`, which reads one snapshot beside `serve`, then stamps the copy, and `serve` refuses to start on a stamped database. While the copy runs, the write-ahead log grows by everything the server writes meanwhile, because no WAL checkpoint can pass the copy's snapshot. To restore SQLite, the operator runs `restore` with the backup file and its secret, in the old data directory or a fresh one; it replaces the database, removes its write-ahead log files, and writes the secret when the directory has none. After restoring PostgreSQL with its own tools, or failing over to a standby that may lack commits, the operator runs `mark-restored` before `serve`. Both raise the generation to the larger of the stored generation plus 1 and the current Unix time in seconds, so a restored copy never repeats a value clients saw after an earlier restore. Both clear the stamp and check the blob store against the database ([§19](#19-compaction)). The server reports the generation in the `Noura-Restore-Generation` header of every response, in every pull page, and in a `restored` realtime frame ([§23](#realtime-channel)), so a device learns of a restore from its next request of any kind. Clients can't check the generation: it is the server's claim, and every change of it shows you an alert that the server reports a restore from a backup. A generation below one the client saw reports `sync_server_rollback`. A database put back any other way, such as a file copy, a virtual machine or filesystem snapshot, point-in-time recovery or a failover to an asynchronous standby, hands out again, in the same generation, sequences that clients already passed. Clients catch it: a pull page `head` or `checkpoint_seq`, a `heads/{device}` answer or a checkpoint `covered_seq` below one the client saw in the same generation reports `sync_server_rollback`, a policy head below the client's accepted one reports `sync_policy_rollback`, an account chain that doesn't reach a kept head reports `sync_account_forked`, and a record the client's cursor skipped past reports `sync_chain_gap` once a later record of its chain arrives. The client then stops advancing and writing that workspace and alerts you until the generation changes. The operator runs `mark-restored` once they know, and the new generation makes every device compare as below. A PostgreSQL failover target therefore replicates synchronously, and any restore or failover that can lose commits needs `mark-restored`.

**Comparing after a restore.** A client that sees a generation above the last one it saw stops writing, keeps its outbox, and checks that the server still holds the head it keeps of each account chain ([§7](#account-chain)), its accepted policy head in each workspace, its latest accepted epoch record of each domain it reads, and, for each domain whose current epoch on the server is that one, the envelope it accepted. A failed check means the server lost signed state that the client relied on. The client reports `sync_restore_lost`, naming what the server lost, and stops:

- every workspace, when the server lost a link of the client's own account chain; the app offers Reset devices ([§7](#resetting-your-devices));
- every workspace it shares with another account, when the server lost a link of that account's chain; the app says that the account's person must reset their devices and an owner or admin must accept the reset;
- the workspace, when the server lost a policy change, an epoch record or an envelope, or when, during the reconciliation below, the server serves none of the checkpoints the client reached in a domain, or a device's records or a checkpoint's heads disagree with a final head the client kept ([Final heads](#after-a-server-restore)); the app says that an owner can set the workspace up again.

A stopped account or workspace keeps its files, and the client applies, writes and uploads nothing for it. It never sends the lost links, changes, epochs or envelopes again. A workspace stopped only for another account's chain resumes, with the reconciliation below, once the client verifies there an `accept_reset` that names that account's new chain; an owner or admin device may sign that change, and the rotations it requires, while the workspace is stopped. Devices that never accepted the lost state find nothing missing and keep syncing, until an owner closes the workspace or the account resets ([§30](#30-threats)). When a workspace passes every check, the server lost at most operations, checkpoints and blobs there, and the client reconciles the workspace's content, below, before it writes there again.

**Set up sync again.** An owner device can set up a workspace again whether or not it found the loss itself, because a member's device may be the only one that did. It closes the old workspace with `DELETE /v2/workspaces/{w}`, which the server then answers with `not_found` on every route and reclaims in batches. It then turns sync on for its own folder as a new workspace, with a new ID, genesis and keys, and you invite the members again. Restricted domains, their grants, share links and history start over. A member connects the folder it synced before, and its app matches the files as after a lost journal ([§15](#client-journal)): it holds every file under a marker of the old workspace's restricted domains ([§9](#domain-roots)) and every file that differs from the new workspace's live state. Devices still syncing the old workspace get `not_found` and show that its owner closed it.

**Reconciling content.** Each device's files hold the content, and devices write what the restore lost as new operations. Once the comparison passes, a client takes each domain through these steps:

1. It rewinds the domain to the newest checkpoint that it reached or bootstrapped from before the restore and that the server still serves, rebuilds from it as a returning device does ([§18](#18-bootstrap)), and pulls from there, checking every later entry as a log entry. Recency ([§17](#checking-a-checkpoint)) doesn't apply to that checkpoint, because the client checked it before the restore and step 2 writes again whatever the server lost after it. When the server serves none of them, the client reports `sync_restore_lost` and stops the workspace. A client that reached no checkpoint in the domain pulls from sequence 0 with empty chain heads. Either way its chain heads, verifier state and live set become the server's, except the final heads below.
2. It finds each `rid` whose server state lies behind its journal: one the server lacks, or one whose live operations on the server the journal's live operations all cover. It waits a random 0 to 60 seconds, then, for each such `rid` that still lags, writes the journal's last applied state again as a new operation covering the server's live operations: the file or delete, reusing its `blob_ref` and uploading the blob again byte for byte when the server lacks it ([§21](#blob-format)), or, for a text-mode note, a `text_snapshot` of its document under the same lineage, merged first with the records the server took since the restore. That snapshot's `meta` entries name the effective sequence of each write the server still holds and 0 for a write the restore lost, which therefore counts as a new write ([§22](#snapshots)). When another device wrote the same state first, the two branches are identical and merge ([§16](#16-conflicts)), as do two snapshots of one lineage. For a text-mode note whose lineage the server still holds live, the client writes such a snapshot when its document holds more than the server's state, because a text update may insert only under its signer's client ID ([§22](#the-document-on-a-device)).
3. It drops each signed record in its outbox whose `previous` the server doesn't hold; the change stays in the file.
4. It signs its next record in the domain above every counter it signed there, with `previous` naming the server's head for its chain ([Verifier](#verifier) rule 2).

A file that then differs from the rebuilt state counts as a local edit when the server's live state of its `rid` is the one the journal last applied or the one step 2 wrote again; otherwise it goes to review as a held local version, as after a lost journal ([§15](#client-journal)). When the steps finish, the app lists the objects the device wrote again. A lost change comes back from any device that applied it, even when its author device no longer exists. A change that no surviving device applied is gone, and its object returns to its state in the backup. A rewound writer can sign any checkpoint that falls due, including one that a surviving removal made due when the restore lost the checkpoint after it.

**Final heads.** A rewind takes the server's positions again, so on its own it would let the server place a writer's new records, signed at an older revision, before the entry of the change that ended its write access. For each change it accepted that ended a device's write access to a domain, the client therefore keeps the device's final head there, as a counter and digest: the device's chain head when the client applied the change's entry, or, when the client bootstrapped past that entry, the head its bootstrap checkpoint lists. A device with no record there, or that the bootstrap checkpoint doesn't list, has the final head (0, 32 zero bytes), as in [Verifier](#verifier) rule 2. The client keeps one final head per such change, also after a later change gives the device write access again, and a generation change never alters it. After a generation change, the client checks the final heads before it rebuilds from, or applies, anything the server serves:

- Every checkpoint from the first that covers the change's entry, up to the entry of a change that gives the device write access again, must list exactly the final head, or no head of the device when the final head is zero or the heads rule of [§17](#checkpoint) lets it leave. The client checks the checkpoint that step 1 starts from before it rebuilds from it, and each later one as it reaches it.
- When step 1 starts below the change's entry, from a checkpoint or from sequence 0 with each device at (0, 32 zero bytes), the device's head at that start must lie at or below the final head's counter. Before it rebuilds or applies anything, the client fetches the device's records above that head through the final head's counter by chain position (`GET .../chain/{device}/{counter}`), counters 1 through the final head's counter when it starts from sequence 0, and requires that the head and those records form one chain through `previous` that ends at the final head. The digests fix these records, so the server can't put others in their place.
- During the pull, the client refuses a record of the device at or below the final head's counter other than those, a record above it before the change's entry, and a chain head at the entry other than the final head.

Any of these failures reports `sync_restore_lost` and stops the workspace, and the client applies nothing from the page that carried it, as for a chain error ([Verifier](#verifier)), so no file changes. A consistent backup that holds the change holds every record committed before its entry, so an honest restore passes. A client that hadn't reached the change's entry before the restore keeps no final head for it ([§30](#30-threats)).

**Recovery, sessions, share links and invitations.** A recovering device compares the kit's account head and workspace heads ([§12](#12-recovery-kit)) like any state it accepted: it offers Reset devices when the server lacks the account head, and stops a workspace whose head the server lacks. A device whose session the restore lost signs in again and binds its card ([§7](#enrollment-proof)). After a generation change, each device revokes again each share link it revoked before ([§24](#24-share-links)). An invitation that the restore brings back stays harmless: only the inviting device approves, and it discarded the secret when it revoked the invitation.

## 15. Domain log, cursors and pulls

Each domain has its own log. The server assigns each accepted operation, checkpoint, epoch entry ([§9](#epochs)) and policy entry ([§8](#policy-entries)) a sequence number, increasing within the domain and not necessarily dense. Per-domain sequences keep a member outside a restricted domain from counting its activity through gaps.

Records become visible in sequence order: the server assigns a domain's sequences inside the committing database transaction, under the locks that [Store transactions](#store-transactions) orders.

### Push

`POST /v2/workspaces/{w}/ops` carries `list<record_ref>`, at most 1 MiB, where `record_ref` is `u8 record_type | bytes record` with types 1 operation, 2 policy change, 3 epoch bundle, 4 envelope, 5 checkpoint bundle and 6 profile. A push may span domains. The server decodes it and checks its signatures, padding and ciphertext lengths before it opens its database transaction, then validates the records in order against the current state plus the records before them, and commits all of them in that one transaction or none. A push that lists the same record twice gets `bad_request` naming the second copy's index. It returns its policy head revision, `list<receipt>`, one receipt per log entry the push created, in request order, with a policy change's entries in domain-ID order. A receipt is `digest record_digest | id domain | u64 seq`, where `record_digest` is the record's digest, and for an epoch or checkpoint bundle the digest of the epoch or checkpoint record inside it. Like every response, a push response carries the restore generation, and a device that sees a new one stops writing and follows [§14](#after-a-server-restore) before it signs anything else. A device keeps at most one push in flight per workspace and fetches the policy chain before its next push when the returned revision is above its accepted head ([§9](#rotation)). A refused policy change doesn't stop the device's later pushes, except in the domains that an unrecorded removal holds ([§9](#rotation)). Every atomic change fits one push ([§20](#20-atomic-changes-and-quota)). Besides workspace creation, push is the only route that writes records to a workspace, so the one in-flight rule covers every write.

### Store transactions

Both stores commit a push the same way, so no record lands after a change that forbids it and no two commits deadlock:

- **Lock order.** Each push takes, in this order: the workspace's policy lock, shared for a request without a policy change and exclusive for one with a policy change; the workspace's key-sequence row, when the request carries an epoch bundle or envelope; the head rows of the domains it touches, in domain-ID order; then the rows of the blobs it lists, in blob-ID order. In PostgreSQL the policy lock is a transaction-level advisory lock in the one-key form, keyed by the first 8 bytes of the workspace ID, `pg_advisory_xact_lock_shared` or `pg_advisory_xact_lock`, so concurrent pushes don't lock one hot row; in SQLite the single writer stands in for it. The collector, tus creation and the share routes, which change a domain's usage, lock that domain's head row before any blob row. A share creation also locks the workspace domain's head row, in domain-ID order, so concurrent creations count a workspace's active links one at a time. Commits into different domains of one workspace proceed in parallel unless one of them changes the policy. A push whose changes record account heads reads those accounts' chains after it holds the policy lock. An account chain append locks only that account's chain row and takes no workspace lock, so the two never wait on each other in a cycle. Workspace creation takes the new workspace's locks in this order, then its creator's chain row. It counts the workspaces that account owns ([§27](#27-limits)) only after that, so one account's creations count one at a time.
- **Reads after locks.** The server reads policy, epoch, rotation, checkpoint and quota state only after it holds those locks, under READ COMMITTED in PostgreSQL and inside `BEGIN IMMEDIATE` in SQLite, so it never validates a record against a policy older than the one it commits after.
- **Sequences.** The server assigns a domain's sequences while it holds that domain's head row, so no record commits below a sequence a reader has already passed. Key sequences follow the same rule on the key-sequence row.
- **Quota.** Each domain's head row carries the domain's usage and history weight ([§20](#20-atomic-changes-and-quota)), and the quota check sums the workspace's domains, so pushes into different domains don't queue on one quota row. The sum may miss a concurrent push into another domain.
- **Reads.** A pull, `live` or `manifest` response comes from one snapshot: a read-only REPEATABLE READ transaction in PostgreSQL, one read transaction in SQLite. `complete_through`, `retention_floor` and the entries therefore agree while pruning runs. No read transaction stays open across a pull's wait: the server waits outside the database and takes the snapshot when it has something to return.
- **Pruning.** One instance at a time prunes a domain, under a per-domain advisory lock in PostgreSQL's two-key form, which never meets a one-key lock, keyed by the first 4 bytes of the workspace ID and of the domain ID; a collision only delays pruning. It persists the new floor before it deletes anything, then deletes in batches in ascending sequence order ([§19](#19-compaction)).
- **PostgreSQL.** Pushes into one domain queue behind its head row. The server may commit several queued pushes into one domain in one transaction with a savepoint per push, as in SQLite, so one push's failure rolls back only that push and one WAL flush covers the group. It groups only pushes that take the shared policy lock and that domain's head row and no other lock, so the group's transaction takes its locks in the order above. A push that carries a policy change, an epoch bundle or an envelope, or that lists a blob, commits alone. Each push in the group validates its records against the state the pushes before it left.
- **SQLite.** SQLite allows one writer, so commits from every workspace on a server queue behind each other. The server holds the writer only while it validates and commits pushes it has already decoded and checked. It may commit several queued pushes in one transaction with a savepoint per push, so one push's failure rolls back only that push and one fsync covers the group, and it runs WAL checkpoints itself so the write-ahead log stays bounded. SQLite mode suits a self-hosted server for one team; a hosted service with many tenants runs PostgreSQL.

### Pull

`POST /v2/workspaces/{w}/pull` takes `list<pull_cursor>`, each `id domain | u64 after`, and a wait of up to 25 seconds, and returns a page for each domain with records after its cursor, at most 1 MiB in all. It waits while none has any. A pull lists at most 256 cursors, the most domains a workspace holds ([§27](#27-limits)), so its pages fit that limit; the server answers more with `too_large`. A client pulls every domain it reads through this one request, so a device makes one pull per round however many domains exist.

A page:

| Field | Type | Notes |
|---|---|---|
| domain | `id` | |
| status | `u8` | 1 ok; 2 compacted, the cursor lies below the retention floor; 3 before the account's history floor; 4 forbidden, the device can't read the domain; 5 not found, the domain was deleted |
| after | `u64` | the requested cursor |
| complete_through | `u64` | the page holds every record with a sequence in (after, complete_through] |
| head | `u64` | the domain's latest sequence |
| retention_floor | `u64` | [§19](#19-compaction) |
| checkpoint_seq | `u64` | `covered_seq` of the domain's latest checkpoint, 0 before the first |
| restore_generation | `u64` | [§14](#after-a-server-restore) |
| policy_revision | `u64` | the server's policy head revision ([§9](#rotation)) |
| checkpoint_due | `u64` | the sequence the next checkpoint's `covered_seq` must reach, or 0 while none is due ([§17](#checkpoint)) |
| entries | `list<entry>` | sorted by sequence |

`entry` is `u64 seq | u8 record_type | bytes record`, with record types 1 operation, 5 checkpoint bundle, 8 epoch record ([§9](#epochs)) and 9 policy entry, whose record is `u64 revision | digest head` ([§8](#policy-entries)). A page holds at most 1 MiB. A page whose status isn't 1 holds no entries, its `complete_through` equals `after`, and `head` is 0; status 2 still carries `retention_floor` and `checkpoint_seq`, and statuses 3 to 5 set both to 0; `checkpoint_due` is 0 for every status but 1. One domain's status never fails the request, so the other domains keep syncing. The client bootstraps a domain whose page says 2 or 3 ([§18](#18-bootstrap)). For 4 or 5 it fetches the policy and stops pulling that domain once a change it accepted took its read access or deleted the domain; until then it reports the status and retries. A device whose account a `remove_member` removed gets status 4 on every page and the policy chain up to that change ([§8](#policy-change-record)). Once it verifies the removal, it stops syncing the workspace and shows that its account was removed, by which device and account, and that its files stay as they are. A device that a link removed gets `device_removed` instead ([§7](#desktop-sign-in)).

The client advances a domain's cursor to `complete_through` only after every record in the page passes the checks in [§13](#client-checks-on-an-operation) and [§14](#14-device-chains) and every resulting file change reaches disk. A text-mode record that the collaboration sandbox is still decoding counts once the record itself reaches the journal on disk ([§22](#22-live-collaboration)). Policy changes and keys have their own cursors ([§26](#26-http-api)). The realtime channel ([§23](#23-presence-and-realtime-channel)) delivers small records and announces new heads.

The server never publishes a retention floor below one it published before, unless its restore generation changed ([§19](#19-compaction)). A `retention_floor` below one the server reported before in the same generation reports `sync_manifest_rollback`. A `head` or `checkpoint_seq` below one the server reported before in the same generation reports `sync_server_rollback` ([§14](#after-a-server-restore)). For a page with status 2, the client reports `sync_manifest_rollback` when its `retention_floor` is below one reported before in the same generation, or when its cursor isn't below that floor. Otherwise it bootstraps, and it requires the checkpoint it bootstraps from to have a `covered_seq` at or above both its cursor and its latest accepted checkpoint's, else `sync_manifest_rollback`. An honest server passes both checks, because the floor never passes the latest checkpoint.

### Client journal

The client keeps its sync state in ordinary files under `.noura/sync/<device-id>/`, never only in SQLite, and never uploads them. It ignores a journal named for another device, and locks its own while the app runs. The journal holds:

- the accepted policy chain and its head, and each account chain it records, up to the recorded head ([§8](#account-heads));
- epoch records and the envelopes addressed to this device, still wrapped;
- per domain: the cursor, the latest accepted checkpoint, the digest, `covered_seq` and sequence of the checkpoint it bootstrapped from and of each it reached since at or above the retention floor ([§17](#checkpoint-chain)), the last retention floor the server reported, the chain heads and records the verifier needs ([§14](#verifier)), and one final head for each change it accepted that ended a device's write access ([§14](#after-a-server-restore));
- per domain, for every record above the latest verified checkpoint, its sequence, `rid`, chain position, digest and covers, and the same for each operation such a record covered, so the client can compute its live set at any later `covered_seq`; and every live lone delete, indexed by sequence, so the client can drop the ones a rising checkpoint `floor` passes ([§17](#leaves-and-tree));
- this device's counters, and every signed record not yet acknowledged, persisted before sending;
- the map from each `rid` to its local object, path, live operations and text-mode state;
- the domain roots ([§9](#domain-roots));
- the replica ID and the last restore generation it saw;
- the review store for conflicts, unreadable records, held files, and text updates whose lineage isn't live.

Each journal file carries a BLAKE3 keyed hash under a 32-byte journal key that the device keeps in the credential store, and the client refuses a file whose hash fails.

The credential store also holds a high-water mark per workspace: the accepted policy revision and head digest, the latest accepted epoch record and checkpoint of each domain, this device's reserved counter per domain ([§14](#your-own-chain)) and its reserved load number ([§22](#the-document-on-a-device)). It also holds the head of the device's own account chain that it last accepted, and the accounts whose chains the device refused with their codes, so a rebuild of the kept heads keeps every stop until you act on its alert ([§7](#account-chain)). The client raises a reserved counter or load number before it uses one past it, one block of 1,024 at a time and one more block at each start, and writes the policy, epoch and checkpoint parts no more often than it flushes its journal, so typing costs no credential store write per keystroke batch. On startup the client refuses a journal that sits below the mark, or kept account heads whose own-account head lies below the stored one, and reports `sync_journal_rollback`; it rebuilds the kept heads from the account chains that its workspaces' logs record. It then bootstraps ([§18](#18-bootstrap)) and matches its files as after a lost journal, below, accepting no account head, policy head, epoch or checkpoint below the mark, and handles its own counters as [§14](#your-own-chain) describes. After a server restore, the checkpoint part may move back to the checkpoint the client rewinds to ([§14](#after-a-server-restore)); no other part ever does.

Connecting a folder discards its journal but not its review store. The client first moves the old journal's conflict branches, held versions and orphaned text to `.noura/sync/archive/<old-replica>/` and keeps listing them for review until you resolve each, so a conflict branch whose author device is gone survives Set up sync again ([§14](#after-a-server-restore)). The archive never uploads.

A device syncs one folder per remote workspace. The app keeps a registry in its application data, outside every workspace, that maps each account and remote workspace to one folder and a random replica ID, which the journal also stores. A folder whose journal holds another replica ID, such as a copy you made in Finder or a folder another Mac opened through a cloud drive, doesn't sync until you connect it, and connecting it discards its journal, bootstraps and matches its files as after a lost journal.

Device signing and HPKE secrets live only in the operating system's credential store. Epoch keys exist only in memory: the client unwraps them from the stored envelopes with the device's HPKE secret.

Within one scan, a deleted ordinary file and a created one with the same content revision, and the same operating system file ID where the platform reports one, count as a move: the client writes a state operation on the same `rid` with the new path and reuses the `blob_ref`. Two files that share one frontmatter ID don't sync as that object until you resolve the local `identity_conflict` ([v1.md](v1.md#classification)).

If the journal is lost, or the folder holds only another device's journal, as after Reset devices, the client bootstraps again ([§18](#18-bootstrap)) and matches local files to objects: managed files by frontmatter ID first, ordinary files by content revision, then by path. Without a journal it can't tell which state a differing file started from, so it never uploads one as an edit. A live state here is the state the client derives from the `rid`'s live set ([§16](#16-conflicts)), the merged body for a text-mode note, compared on path and revision; a match that differs only in path, such as a managed note matched by ID at another path, is held like a content difference. A matched file that differs from every live state of its `rid` goes to the review store as a held local version, and the client uploads nothing for that `rid` until you choose: keep the live state, which replaces the file; keep this device's version, which writes a state operation covering every live operation of the `rid`; or keep both, which saves this device's version as a new object at the first free name `name (n).ext`, a managed file's copy with a new frontmatter ID. Files under a held root wait for that hold first ([§9](#domain-roots)). With no journal, the client continues its chains from the server's heads, at counters above its reserved ones ([§14](#your-own-chain)).

## 16. Conflicts

The client derives an object's state from its live set:

1. One live state operation: that state, plus the live text updates of its text lineage, which merge in canonical order ([§22](#updates)).
2. Two or more live state operations: a conflict, and each one is a branch, except live text snapshots of one text lineage, which merge ([§22](#22-live-collaboration)).
3. Text updates whose lineage isn't live: kept for review as text.

Sync keeps both versions:

- The device whose local file matches a branch, or has local edits, keeps its file untouched.
- Every other branch goes to the local review store under `.noura/sync/<device-id>/conflicts/`, which never uploads.
- A device that holds neither materializes the branch with the lowest operation digest and stores the rest for review.
- Status counts the conflict until you choose.

Choosing writes a state operation that covers every branch. Any other edit you make to a conflicted object covers only the branch your file matched and its identical copies ([§13](#supersession-and-live-operations)), so the conflict stays, with your edit as that branch's new state, until you choose.

Clients resolve these cases without asking:

- **Identical branches.** Branches whose decrypted path and revision match aren't a conflict.
- **Disjoint changes.** When one branch changed only the path and another only the content, relative to the common base operation the covers name, the client materializes the merged state. Any writer device that holds the base writes a merge operation covering both. Two devices writing the same merge produce identical branches, which the previous rule collapses.
- **Edit against delete.** The edit stays on disk and the conflict asks whether to delete it. An edit to an object whose delete a checkpoint's `floor` has already dropped from the live set ([§17](#leaves-and-tree)) revives the object without asking.

Two objects with different `rid`s collide when their paths are equal after NFC normalization and Unicode simple case folding, or when one object's path equals a folder prefix of the other's. A valid domain marker ([§9](#domain-roots)) never yields, so the other object yields, even when its path is only the marker folder's prefix, such as a file `Projects` against `Projects/.noura-domain`. Otherwise the object whose current state operation has the higher digest yields. Its author's device moves it to the first free name `name (n).ext`, with n counting up from 2, with a normal operation, and other devices show it at that name without writing until the move arrives. A text-mode note moves by a text update that writes its `path` ([§22](#22-live-collaboration)).

## 17. Manifest and checkpoints

A domain's manifest is a Merkle tree over its live operations. Its root proves that a client holds every current object, and a new device bootstraps from it.

### Leaves and tree

Each `rid` with at least one live operation has a leaf. Lone deletes that a `floor` dropped don't count as live ([§19](#19-compaction)):

```text
leaf_hash = BLAKE3(tag(noura.sync.manifest.leaf) || rid || list<ref>)
ref       = id device || u64 counter || u64 seq || digest op_digest    (sorted by device, then counter)
```

Each ref carries the sequence the server assigned, so the root also commits to where the server placed each live operation, and the heads below give the verifier positions right after a bootstrap ([§14](#verifier)).

When a checkpoint's `floor` rises above the previous checkpoint's, each delete whose sequence lies between the two floors and that is its `rid`'s only live operation at the checkpoint's `covered_seq` leaves the live set for good. It stays out of every later checkpoint's live set, even when a later operation on its `rid` doesn't cover it; such an operation revives the object ([§16](#16-conflicts)). The server and clients remove these leaves when they compute the new root, alongside the changes that the records between the two checkpoints make.

The tree has a fixed shape: 4,096 buckets keyed by the first 12 bits of the `rid` (`rid[0] << 4 | rid[1] >> 4`), under three levels of 16-way nodes.

```text
bucket_hash(i)  = BLAKE3(tag(noura.sync.manifest.bucket) || u16 i || list<digest>)
                  leaf hashes of bucket i, sorted by rid; an empty list for an empty bucket
node(2, j)      = BLAKE3(tag(noura.sync.manifest.node) || u8 2 || u16 j || bucket_hash(16j) .. bucket_hash(16j+15))   j in 0..255
node(1, j)      = BLAKE3(tag(noura.sync.manifest.node) || u8 1 || u16 j || node(2, 16j) .. node(2, 16j+15))            j in 0..15
root = node(0, 0) = BLAKE3(tag(noura.sync.manifest.node) || u8 0 || u16 0 || node(1, 0) .. node(1, 15))
```

Children are concatenated 32-byte digests, 16 per node. Random `rid`s keep the buckets even: 100,000 objects average 24 leaves per bucket. Two devices find where their manifests differ by comparing 16 hashes per level.

### Checkpoint

A writer device signs a checkpoint for a domain after it has applied every record through a sequence.

`checkpoint`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.checkpoint)` | |
| workspace | `id` | |
| domain | `id` | |
| previous | `digest` | digest of the domain's previous checkpoint; 32 zero bytes for the first |
| number | `u64` | 1 for the domain's first checkpoint, then +1 |
| skip | `digest` | digest of the checkpoint numbered `number & (number − 1)`, its number with the lowest set bit cleared; 32 zero bytes when that is 0 |
| covered_seq | `u64` | the manifest covers every record through this sequence; at or above the sequence of the previous checkpoint's own entry, and at or above the retention floor |
| floor | `u64` | the retention floor as the signer read it, between the previous checkpoint's `floor` and the server's current floor; the live set leaves out lone deletes at or below it ([§19](#19-compaction)) |
| policy_revision | `u64` | the revision of the latest policy entry at or below `covered_seq` |
| policy_head | `digest` | digest of the change at `policy_revision` |
| live_objects | `u64` | number of leaves |
| manifest_root | `digest` | |
| heads_digest | `digest` | `BLAKE3(tag(noura.sync.manifest.heads) \|\| list<ref>)` over the heads below |
| epoch | `u64` | the epoch of the latest epoch entry at or below `covered_seq` |
| epoch_record | `digest` | digest of that epoch record |
| signer | `id` | |
| sig | `sig` | |

`covered_seq` may equal the sequence of the previous checkpoint's entry. A checkpoint entry doesn't change the live set, so the root stays valid, and a domain whose last entry is a checkpoint can still take a fresh one.

The heads are the chain head, as a `ref` with its sequence, of every device with at least one accepted record in the domain at `covered_seq`, sorted by device. A device leaves the list only once its card can't come back: when a change at or below the previous checkpoint's `policy_revision` recorded a head of its account at which a link had removed the card, or a head of its account on a chain other than the card's, as an `accept_reset` or an `add_member` after a reset does ([§8](#account-heads)). A card that leaves never returns, so a device that left the list never writes the domain again. A device that only lost access, such as an editor demoted to viewer, a grantee cut back to read, or a device whose account `remove_member` removed, stays listed with its last head, so a client that bootstraps meanwhile still knows where its chain stopped if access comes back. No device leaves the first checkpoint of a domain, and the first checkpoint that covers a removal's entry still lists the removed device's final head. Signer and readers derive the list from the log and the policy chain alone, so they agree on it.

A `checkpoint_bundle` is `bytes checkpoint | list<ref> heads`. It goes in a push of its own, or last in the push of an approval or grant that sets a history floor, after the change; anywhere else the push fails with `bad_request`. The server checks:

- that the signer can write the domain at the latest policy entry ([§8](#policy-entries)), else `forbidden`;
- that `policy_revision` and `policy_head` name the latest policy entry at or below `covered_seq`, and `epoch` and `epoch_record` the latest epoch entry at or below it, else `manifest_mismatch`;
- that `previous` names the domain's latest checkpoint, else `checkpoint_stale`, and that `number` and `skip` follow from it, else `manifest_mismatch`;
- that `covered_seq` sits at or above that checkpoint's entry sequence and at or above the retention floor, and at or below the domain's latest sequence, else `checkpoint_not_due`;
- that `floor` lies between the previous checkpoint's `floor` and the current retention floor, else `manifest_mismatch`;
- that a checkpoint is due and `covered_seq` reaches the due point it published, or that the checkpoint comes in the same push as an approval or grant that sets a history floor ([§9](#history-floor)), else `checkpoint_not_due`.

Before it opens the committing transaction, the server recomputes, at `covered_seq`, the root, the number of leaves and the chain heads, from the bucket hashes and heads it stored for the previous checkpoint, the records between the two, and the lone deletes that the rising `floor` drops ([§17](#leaves-and-tree)). It rejects a root, `live_objects` or `heads_digest` that differs with `manifest_mismatch`, so an honest server and an honest writer always agree, and a bootstrapping device never starts from heads a writer made up. The records at or below `covered_seq` are already committed, and pruning reaches them only after a later checkpoint commits, because the floor never passes the latest checkpoint, so this work needs no lock. The server checks `previous` again under its locks, and before it reports a mismatch, so a racing checkpoint gets `checkpoint_stale`. An accepted checkpoint becomes a log entry with its own sequence.

The server decides when a checkpoint falls due and publishes the due point as `checkpoint_due` in pull pages and in notify and records frames ([§23](#realtime-channel)): the sequence that the next checkpoint's `covered_seq` must reach, or 0 while none is due. A checkpoint falls due:

- at the entry that brings the domain's log to 10,000 entries or 64 MiB of weight ([§19](#19-compaction)) since the last checkpoint entry;
- at a policy entry that ends the write access of the latest checkpoint's signer or of any device with a chain head in the domain;
- at a replacement's epoch entry ([§9](#rotation)), since the latest checkpoint may name the epoch it skips, which devices that never accepted that epoch fail with `sync_key_mismatch`; the fresh checkpoint names the replacement, so new devices can bootstrap the domain again;
- once each time its workspace's usage and history together go from within its quota to above it ([§20](#20-atomic-changes-and-quota)), at the next entry of each domain with entries after its latest checkpoint.

Writers don't compute due points: blob weights hide inside payloads they may not read, and a bootstrapped device doesn't know which blobs pruned history listed. A writer device that has applied the domain past the published due point waits a random 0 to 60 seconds, then signs a checkpoint if none has arrived since. The server keeps the first one and answers the rest with `checkpoint_stale`. A writer that gets `checkpoint_not_due` drops its checkpoint and waits for the next due point. Every policy change adds an entry to each log, so even a quiet domain gains an entry after a demotion, and the fresh checkpoint has something to cover.

### Checkpoint chain

Each checkpoint names the one before it in its domain, and every operation names the latest checkpoint its writer reached (`checkpoint_seq` and `checkpoint`, [§13](#operation-record)). A server that leaves a checkpoint out of one client's log therefore can't hide it once a later checkpoint, or a record whose writer reached it, crosses to that client. The entry of the checkpoint a client bootstrapped from reaches it again in its log, and the client skips it. A client fetches a checkpoint by digest (`GET .../checkpoints/{digest}`) in two cases:

- a checkpoint entry in its log whose `previous` isn't the last checkpoint the client reached or bootstrapped from in that domain;
- a record of the current restore generation whose `checkpoint_seq` sits at or above both the retention floor and the `covered_seq` of the checkpoint the client bootstrapped from in this generation (0 if it pulled from the start), when the client hasn't reached `checkpoint`.

The client fetches the named checkpoint, then each one its `previous` names, back to the last checkpoint it reached. If the server can't serve one, a fetched checkpoint's `number` isn't one below the checkpoint whose `previous` named it, or the walk passes the client's last checkpoint without meeting it, the client reports `sync_chain_gap` for the domain, so a served loop of `previous` links ends the walk. Otherwise the client checks the fetched checkpoints as log entries, oldest first, as if its pages had carried them. An honest server never causes a fetch. A record gets its sequence after the checkpoint its writer reached, so a client that pulls in sequence order reaches that checkpoint first. A writer that lags names a checkpoint the client already reached, or one below the client's bootstrap point or the floor, which needs no check.

### Checking a checkpoint

A client checks each checkpoint it reaches in its log after it has applied every record through `covered_seq`, and accepts it when:

- `BLAKE3(tag(noura.sync.manifest.heads) || heads)` equals `heads_digest`, and every head's sequence is at or below `covered_seq`;
- `previous` names the last checkpoint the client reached in that domain, and `number` is one above that checkpoint's;
- its signer could write the domain at the latest policy entry before the checkpoint's entry ([§8](#policy-entries)) and at its `policy_revision`;
- `policy_revision` is the revision of the latest policy entry at or below `covered_seq`, at least the previous checkpoint's, and `policy_head` matches the client's change at that revision;
- `epoch_record` is the digest of the latest epoch entry at or below `covered_seq`, an epoch record the client accepted for `epoch`, else `sync_key_mismatch`;
- `floor` is at least the previous checkpoint's;
- `covered_seq` lies above that of the latest checkpoint the client accepted, each device listed in both has a head there at or above its earlier head, and each device listed only in the earlier one may leave under the heads rule above, else `sync_manifest_rollback`.

The client then computes its live set and chain heads at `covered_seq` from its journal ([§15](#client-journal)) and compares them with the checkpoint. A root mismatch reports `sync_manifest_mismatch`, and the client walks the tree to find which objects differ. A checkpoint head above the client's own reports `sync_chain_gap`, and one below it reports `sync_manifest_mismatch`.

A checkpoint in the log that fails a check is reported and never becomes the client's latest accepted checkpoint, an approval or kit anchor, or a source of state. It doesn't hold the cursor: each record around it passed its own chain checks, and the next checkpoint still names it as `previous`.

A checkpoint a device bootstraps from ([§18](#18-bootstrap)) passes the checks above that need no local history: the heads digest, `policy_head` and the epoch record. It also passes three more:

- **Descent.** It is the device's anchor, or descends from it through `previous` and `skip` links. The anchor is the device's latest accepted checkpoint in the domain, or else the checkpoint anchor of its approval or kit. The device fetches the walk between them with `GET .../checkpoint-chain`, which the server takes through `skip` wherever that doesn't pass the anchor and through `previous` elsewhere, and checks that each record is the `previous` or `skip` of the one after it, with the matching number, back to the anchor, within the anchor's generation. When a restore lost an approval's or kit's anchor, [§10](#approval-offer) applies. When it lost the device's latest accepted checkpoint, the device rewinds to the newest checkpoint it reached that the server still serves, and stops the workspace with `sync_restore_lost` when there is none ([§14](#after-a-server-restore)). The walk takes at most k(k + 1) / 2 records when the domain holds fewer than 2^k checkpoints: 210 records, about 80 KB, after a million. A walk that fails reports `sync_manifest_rollback`. The server keeps every checkpoint record while the domain exists ([§19](#19-compaction)), so the walk reaches an old kit's anchor too. A device with no anchor in the domain, such as a restricted domain after an approval, which anchors only the workspace domain, or a domain granted later, skips this check and relies on the two below.
- **Signer.** Its signer could write the domain at the latest policy entry before the checkpoint's entry and at its `policy_revision`. `GET .../checkpoint` returns the entry's sequence with the bundle, and before installing anything the device pulls the log from `covered_seq` up to that entry, applying nothing, to learn the policy entries it needs for the check.
- **Recency.** No change in the device's accepted chain above the checkpoint's `policy_revision` takes its signer or any device in its heads out of the workspace, through a recorded head of its account at which a link had removed the card or on another chain, or a `remove_member`. The server places entries, so it could slip a checkpoint that a removed device signed at an older revision in front of the removal's entry; the device has verified the whole chain, so it knows every removal and refuses such a checkpoint. A device that rewinds after a server restore skips this check for a checkpoint it reached or bootstrapped from before the restore ([§14](#after-a-server-restore)). A bootstrapping device skips it for the checkpoint its approval offer or recovery kit anchors in the current generation, which the device that wrote the offer or kit accepted ([§18](#18-bootstrap)).

A checkpoint whose signer was only demoted after its `policy_revision` still serves, so a domain whose remaining grantees only read can still take new devices; a demoted signer that colludes with the server is a colluding writer ([§30](#30-threats)). When the latest checkpoint fails, the device reports the error, shows the domain as waiting for a checkpoint, and tries again on each pull. A removal makes a checkpoint due at its entry, so a checkpoint that passes follows once a writer signs it; a domain that lost every writer waits until a manager grants someone write access.

## 18. Bootstrap

A new device, or one whose cursor fell below the retention floor, loads current state from the latest checkpoint instead of history:

1. Fetch and verify the policy chain from its anchor ([§8](#verifying-the-chain)) and the account chains its changes record ([§7](#account-chain)), and through `GET .../keys/current` the current epoch record and envelope of each domain it reads, then follow the key stream from the key sequence that route returns. Fetch older epoch records only when a live operation, the checkpoint it bootstraps from or the workspace profile ([§25](#25-workspace-profile)) names an older epoch, and walk back to them through `previous` digests and lineage; `GET .../epochs/{epoch}` returns as many earlier records as fit one response.
2. For each domain, fetch the latest checkpoint bundle with its entry's sequence. Check it as in [§17](#checking-a-checkpoint): its descent from the device's anchor, its signer by position and the recency of its policy. Check its `covered_seq` against the device's history floor, its checkpoint anchors and the latest checkpoint the device accepted (`sync_manifest_rollback`). When only Recency refuses it, as after a recovery that removed its signer and left no other writer to sign the due checkpoint ([§12](#12-recovery-kit)), the device bootstraps from its anchor's checkpoint instead, under the same checks less Recency, and checks every later checkpoint as a log entry in step 7. Without an anchor, it reports `sync_manifest_rollback`.
3. Fetch the 4,096 bucket hashes at `covered_seq` in one request and check them against `manifest_root`.
4. For a restricted domain, apply the marker first. Fetch the leaf refs of the bucket that holds `marker_rid` (level 3 of `manifest`), check them against that bucket's hash, fetch the marker's live operations (`live` with `rid`), check their digests against the refs, and apply them. Record the domain root when the marker is valid ([§9](#domain-roots)); otherwise the client records held roots as it writes the domain's files.
5. Fetch the operations live at `covered_seq` in pages ordered by `rid`, device and counter. Run the checks in [§13](#client-checks-on-an-operation) on each operation, except the chain verifier and the positional rules of [§8](#policy-entries) and [§9](#rotation), which the checkpoint's signer applied, and compute the leaves. As each bucket completes, check its hash, decrypt and materialize its files; a local file the journal doesn't map is first matched to the domain's objects as after a lost journal ([§15](#client-journal)), so the client adopts an identical file, holds a differing one as a local version, and uploads only an unmatched file as a new object. Then record the checkpoint and the next bucket in the journal, so a restart resumes at that bucket. A bucket that fails its hash, or a `live_objects` count that fails after the last bucket, reports `sync_manifest_mismatch` and installs nothing more.
6. Record the chain heads from the bundle, with their sequences, and the checkpoint as the start of the domain's checkpoint chain ([§17](#checkpoint-chain)).
7. Pull from `covered_seq` with the chain verifier starting at those heads.

A checkpoint stays servable while its `covered_seq` is at or above the retention floor. When the floor passes the device's checkpoint before the bootstrap ends, `live` and `manifest` answer `cursor_compacted`, and the device restarts from the latest checkpoint, keeping the buckets whose hashes match.

A returning device, one whose cursor fell below the floor while it kept its journal, downloads only what it lacks. In step 5 it compares each bucket hash with the one its journal yields and, for each bucket that differs, fetches the bucket's leaf refs (level 3 of `manifest`) and then, through `live` with `rid`, only the objects whose refs name an operation its journal lacks. When more than half the buckets differ, it fetches `live` pages as a new device does. Any bootstrapping device downloads a blob only when it holds no file at the payload's `revision`. A returning device also compares its journal with the manifest instead of replacing files. It never deletes a local file because the manifest lacks its object: it lists those files once for review ("deleted on another device while this one was away") and deletes them only when you confirm. A changed local copy uploads under a new `rid` unless you choose to delete it.

## 19. Compaction

The server deletes operations no client needs, and bounds each domain's retained log by volume instead of time, so it needs no table that maps sequences to dates.

**Log weight.** An entry's weight is its record's length plus, for an operation, the ciphertext length of each blob it lists that no earlier operation in the domain listed. A move that reuses a `blob_ref` adds little, while a new attachment, or a text snapshot carried as a blob, counts in full. Retention and the checkpoint interval both count weight.

**Retention floor.** A domain's retention floor is the `covered_seq` of the newest checkpoint that has at least `retained_bytes` of weight appended after its entry. `retained_bytes` is a server setting, 1 GiB by default, that `GET /v2/limits` reports. While its workspace's usage and history together exceed the quota, the server raises floors beyond that rule one checkpoint at a time, starting with the domain that holds the most history, until they fit or every floor reaches its domain's latest checkpoint, so history goes before live data and no further than it must ([§20](#20-atomic-changes-and-quota)). The floor is 0 while no checkpoint qualifies, and it never passes the latest checkpoint, so a device that falls behind it always has a checkpoint to bootstrap from. The server persists, per domain, the highest floor it has published, and never publishes a lower one unless its restore generation changes ([§14](#after-a-server-restore)). A domain without any checkpoint keeps every record, and a new device bootstraps it by pulling from sequence 0 with empty chain heads. A device whose cursor falls below the floor gets a compacted page and bootstraps, and its own changes upload as local edits or conflicts ([§18](#18-bootstrap)).

**Pruning.** The server prunes in batches, in ascending sequence order, deleting:

- each operation at or below the floor that another operation, also at or below the floor, covers;
- each delete (`flags` 1) that a checkpoint's `floor` dropped from the live set ([§17](#leaves-and-tree)), whatever operations on its `rid` follow it, once it sits at or below the floor and at or below the `floor` field of every checkpoint the server still serves.

A cover names an operation the server already held, so a covered operation always has a lower sequence than the one covering it. Deleting in ascending order therefore never removes a covering operation before the operations it covers, and a crash between batches leaves the live set at every servable checkpoint complete. A writer whose view is older than the floor can still name a pruned operation in a cover; the server answers `supersedes_unknown`, and the writer drops that cover ([§14](#your-own-chain)). An edit against a delete keeps both operations live, so neither is pruned until you choose ([§16](#16-conflicts)). The server keeps one chain-head row per device and domain, holding counter, digest and sequence, which pruning never deletes while the domain exists. A fetch of a pruned chain position returns `pruned`. The server deletes checkpoint, epoch and policy entries once their sequence falls below the floor, and keeps the bundles and bucket hashes of every checkpoint whose `covered_seq` is at or above it. It keeps every checkpoint record itself, without heads or bucket hashes, 376 bytes each, while the domain exists, so a bootstrapping device can walk back to its anchor ([§17](#checking-a-checkpoint)). A checkpoint stays servable to `live` and `manifest` while its `covered_seq` is at or above the floor.

`delete_domain` marks the domain deleted in its own transaction, and the server answers `not_found` for the domain from then on. It reclaims the domain's log, checkpoints and blobs afterwards, in batches, so no transaction holds the store for the size of a domain.

**Blobs.** A blob passes through three states:

- *Uploading*, until its offset reaches `Upload-Length`. An incomplete upload expires 24 hours after creation (`Upload-Expires`).
- *Completed*, listed by no operation yet. The server deletes it 24 hours after completion unless a push lists it first.
- *Listed*, while a retained operation lists it. When its last listing operation leaves, through pruning or `delete_domain`, the collector deletes it.

The collector deletes a blob in three steps: it marks the row deleted, deletes the store objects, then deletes the row. A crash between steps leaves a marked row that it finishes later. A push that lists a blob and the collector both take the blob's row lock, after the domain's head row ([§15](#store-transactions)), so the collector never deletes a blob that a committing push lists, and a push never lists a marked blob (`blob_incomplete`).

`restore` and `mark-restored` run while `serve` is stopped and check the blob store against the restored database. They move each store object whose blob the database doesn't know under a quarantine prefix, since the operations that listed it are gone and devices upload what they still need again ([§14](#after-a-server-restore)), and the collector deletes those objects 7 days later, so a wrong backup or a store prefix shared by mistake loses nothing at once. They give each completed blob that no operation lists a fresh 24 hours. A listed blob whose store objects are gone becomes missing: its row returns to an upload at offset 0, `GET` answers `blob_incomplete`, and `GET .../missing-blobs` lists it. A device that holds the file at a listing payload's `revision` uploads it again under the same ID, byte for byte ([§21](#blob-format)). An operator whose blob store lost objects while the database kept its rows runs `mark-restored` too.

**Bound.** While checkpoints keep committing, each domain's storage stays within the live set at its floor checkpoint, the log above the floor, 128 KiB of bucket hashes for each servable checkpoint, and one checkpoint record for each checkpoint it ever took. The last two terms don't count as weight: they grow with the number of checkpoints, not with `retained_bytes`. The log above the floor weighs at most `retained_bytes` plus two checkpoint intervals, one between the floor checkpoint's `covered_seq` and its entry and one between that entry and the next checkpoint's. An interval weighs at most 64 MiB, plus the weight of the entry that crosses the threshold (up to 1 MiB of record and 4 new blobs of 2 GiB), plus what writers append during the 0 to 60 second wait after a due point. The live set at the floor checkpoint exceeds the current live set only by what was deleted or replaced since, which the server keeps until the floor passes those changes. Nothing bounds the log while no checkpoint commits, for example while every writer is offline or disagrees with the server's root, and the quota can't reclaim history without a checkpoint either ([§20](#20-atomic-changes-and-quota)). Each device's verifier state stays within one checkpoint interval per domain ([§14](#verifier)).

## 20. Atomic changes and quota

Every atomic change is one push of at most 1 MiB ([§15](#push)), which the server commits whole or not at all. The changes a requirement needs to be atomic all fit:

| Change | Size |
|---|---|
| Your own device's approval in one workspace: an `account_heads` change and at most 256 envelopes | about 350 KB |
| An `account_heads` change: an envelope for each card it brings in, in each domain its account reads that the signer reads | about 1,371 bytes per card and domain, so a head whose new cards times domains passes about 764 goes in steps ([§8](#account-heads)) |
| A new member's approval: `add_member`, an envelope per active card of the member's account, and a checkpoint of the workspace domain | about 15 KB for an account with one card and 200 chain heads; about 365 KB for 256 cards |
| `create_domain` with its epoch 1 bundle, envelopes and marker | about 715 KB for 512 recipients and 200 accounts on its list |
| A rotation for up to 512 recipients, with its epoch bundle and envelopes | about 711 KB |
| A move between domains: the new object's state operation and the old one's delete | two operations |

A move of many objects commits one object pair at a time, with many pairs per push, so each file sits in exactly one domain throughout.

**Quota.** The operator sets each workspace's storage quota with the server setting `workspace_quota_bytes`, which applies to every workspace and is unlimited by default. `GET /v2/workspaces` reports each workspace's quota, usage and history. Usage counts live data and the records the server keeps for the workspace's whole life: the operations live at each domain's head, the blobs they list, uploads in progress or completed but not yet listed, share snapshots, policy changes and epoch bundles. A delete therefore frees its object's usage when it commits. History is everything else each domain retains: the log above its floor that no live operation accounts for, the blobs only that log lists, and checkpoint data. History counts nothing toward usage but yields to the quota. When usage and history together come to exceed the quota, a checkpoint falls due in each domain ([§17](#checkpoint)), and while they stay above it, the server raises floors as far as it must, so pruning reclaims history first ([§19](#19-compaction)). While checkpoints keep committing, a workspace's storage therefore stays within its quota plus about one checkpoint interval per domain, plus the checkpoint records and servable bucket hashes that [§19](#19-compaction) leaves outside weight, and an account owns at most 100 workspaces ([§27](#27-limits)).

While usage exceeds the quota, the server accepts envelopes, checkpoints, epoch bundles, policy changes that add nobody (any change but `add_member`, `accept_reset`, `create_domain`, a `set_access` that grants and an `account_heads` change that brings a card in without taking one out), an `account_heads` change for its signer's own account, and any operation that doesn't raise usage, such as a delete or a smaller replacement. A workspace over quota can therefore still remove a device, approve a replacement device, recover with a kit, rotate and free space, while churn in its membership can't grow its policy chain past the quota; only your own devices and kits still add to it, one approval or export at a time. The server rejects other records with `quota_exceeded`, and the writer moves its next outbox record into that position ([§14](#your-own-chain)). A tus creation request whose `Upload-Length` would take usage above the quota fails with `quota_exceeded` too ([§21](#21-attachments)).

## 21. Attachments

A file whose payload would exceed 262,144 bytes travels as a blob: a separately uploaded ciphertext that the operation references through `blob_ref`.

### Blob format

Each blob gets a fresh random 32-byte key, carried only inside the referencing operation's encrypted payload.

```text
header      = tag(noura.sync.blob) || id blob || 19-byte nonce_prefix
plaintext   = u64 L || file bytes || zero bytes        total P = padded_length(L + 8)
chunk i     = XChaCha20-Poly1305(blob_key, nonce_prefix || u32 i || u8 last, aad = header, 65,536 plaintext bytes)
ciphertext  = header || chunk 0 || chunk 1 || ...
```

`nonce_prefix` is the first 19 bytes of `derive_key("noura.sync.v2 blob nonce", blob_key || blob)`. Each blob key is fresh and random, so no key meets one nonce twice, and a device that holds the file at the payload's `revision` can rebuild the blob byte for byte, which content reconciliation after a restore relies on ([§14](#after-a-server-restore)). A device rebuilds a blob only from bytes whose BLAKE3 hash equals that `revision`, and never encrypts other bytes under an existing blob key.

Every chunk holds 65,536 plaintext bytes except the last, which holds 1 to 65,536 and sets `last` to 1. The ciphertext length is `header length + P + 16 * ceil(P / 65,536)`, which lets the server check padding from the length alone. A blob's file content `L` is at most 2^31 bytes (2 GiB).

`ciphertext_digest` is `BLAKE3(tag(noura.sync.blob.parts) || list<digest>)` over the BLAKE3 hash of each 1 MiB segment of the ciphertext, in order; the last segment may be shorter. Any server instance can then check each segment as it arrives, and the final check costs one hash per segment.

The receiver hashes each segment as it downloads, checks the segment hashes against `ciphertext_digest` before decrypting, writes the plaintext to a temporary file, checks that `plaintext_length` equals `L` and the file's `revision`, and only then replaces the destination.

### Upload over a tus subset

Uploads use tus 1.0.0 core with the `creation`, `termination` and `expiration` extensions. The server advertises `Tus-Version: 1.0.0`, `Tus-Extension: creation,termination,expiration` and `Tus-Max-Size`.

| Request | Purpose |
|---|---|
| `POST /v2/workspaces/{w}/blobs` with `Upload-Length` and `Upload-Metadata: blob <base64>,digest <base64>,domain <base64>` (standard base64, as tus requires) | create; the device must be able to write that domain, the upload must fit the workspace's quota (else `quota_exceeded`, [§20](#20-atomic-changes-and-quota)), and only operations of that domain may list the blob (else `blob_domain_mismatch`) |
| `HEAD /v2/workspaces/{w}/blobs/{id}` | read `Upload-Offset` to resume |
| `PATCH /v2/workspaces/{w}/blobs/{id}` with `Upload-Offset` and `Content-Type: application/offset+octet-stream` | append one segment: exactly 1 MiB at an offset that is a multiple of 1 MiB, or the rest of the upload; another offset, or a completed upload, gets `blob_offset_conflict` (409, as tus requires) |
| `DELETE /v2/workspaces/{w}/blobs/{id}` | abandon an incomplete upload |

The server stores each segment as its own object, records its hash, and flushes it to durable storage before it reports the new offset. When the offset reaches `Upload-Length`, it computes `ciphertext_digest` from the stored segment hashes, and on a mismatch discards the upload and answers `blob_digest_mismatch`. Incomplete uploads expire 24 hours after creation (`Upload-Expires`), and completed uploads that no operation lists 24 hours after completion ([§19](#19-compaction)). A filesystem store's file times (ctime, birth time) and an S3-compatible store's `LastModified` date each segment's upload, because no program can set ctime or birth time ([§29](#29-what-the-server-stores)).

`GET /v2/workspaces/{w}/blobs/{id}` with `Range` returns at most 1 MiB per response, as raw octets with `Content-Range`, like any HTTP range response. The server serves a blob to a device that is active in the workspace's policy and can read the domain the blob was created for, when a retained operation of that domain lists the blob and sits above the account's history floor or is live at a checkpoint the device reads, and to its uploading device before that operation exists. It deletes blobs as [§19](#19-compaction) describes. `GET /v2/workspaces/{w}/missing-blobs` lists the blobs, in domains the device can write, that a restore left missing ([§19](#19-compaction)); after a generation change each device fetches it once and uploads again each listed blob whose file it holds at a listing payload's `revision`.

## 22. Live collaboration

A Markdown note in text mode keeps its body in a yrs document, update format v1, whose one root is the text `content`, with UTF-16 offsets. Editor positions and the diffs of outside edits fall on code-point boundaries, so an honest device never splits a surrogate pair; a record that does turns that character into two U+FFFD on every device ([§3](#3-primitives)). Its path and, for a managed note, each frontmatter property travel beside the yrs bytes as `meta` entries of the payload ([§13](#payload)), under the key `path` and under `p:` followed by the property's name. A managed note's file bytes are the frontmatter that the workspace-format serializer writes from its properties, followed by the text of `content`, so a byte order mark ahead of the frontmatter goes with the first rewrite. An unmanaged note keeps its whole file, frontmatter and byte order mark included, in `content`, and its only `meta` key is `path`. Every device merges `meta` per key ([Updates](#updates)) and writes the note to the same file. A rename that would take the note across a domain root is a move between domains ([§9](#moving-objects-between-domains)), never a `path` change, and a merged `path` outside the note's domain root goes to review ([§9](#domain-roots)).

**Properties.** A property name is the frontmatter key, 1 to 256 bytes of UTF-8 with no control character. A value maps to JSON under the YAML 1.2 core schema: a null or a boolean maps to itself, an integer of magnitude below 2^53 or a finite float to a number, any other scalar, such as a date, to a string of its text, a sequence to an array, and a mapping with string keys to an object. The `meta` value is the RFC 8785 canonical JSON of the result. A managed note whose frontmatter holds anything else, such as a larger integer, a tag, an alias or a property name outside the rule, stays out of text mode, and its edits sync as `file` operations.

### Entering text mode

A device puts a note into text mode when it first sends an edit you made to that note in the editor, once a live `file` operation matches the note's file: a new note first syncs as a `file` operation, and so do a note that moves to another domain ([§9](#moving-objects-between-domains)) and a note whose file matches no live state operation. It writes a `text_snapshot` that derives from that base operation alone, so two devices that start at once from the same state write the same snapshot:

- the first document holds one transaction by yrs client ID 0 that inserts the starting text into `content`, encoded with garbage collection on; the starting text is the base operation's bytes, after the frontmatter for a managed note, as UTF-8 with any byte order mark and line endings kept;
- its `meta` entries hold the base operation's `path` and, for a managed note, the properties of its frontmatter, each naming the base operation's sequence;
- the text lineage ID is the first 16 bytes of `BLAKE3(tag(noura.sync.text.lineage) || rid || base || document)`, where `base` is the base operation's digest and `document` the first document's encoded bytes.

The first snapshot covers the base operation and holds the starting state alone; your edits follow as text updates under the device's own client ID. Identical first snapshots merge to the starting state once. Two devices that start from different states, or build different first documents from one, write different lineages, which conflict as two branches ([§16](#16-conflicts)) instead of sharing item IDs.

### Updates

- The editor batches local edits into at most one `text_update` every 500 ms while you type.
- A frontmatter edit to a managed note writes an entry for each changed property, with no value for a removed one, and a rename or move, in Noura or outside it, writes `path`. An update's entries name sequence 0.
- An outside edit to the file becomes a `text_update` computed from the difference, as the editor treats any outside edit. For a managed note, the client parses the new frontmatter, writes each property whose value changed or that disappeared, and diffs the body into `content`. An outside edit that only reformats the frontmatter changes no value and sends nothing; your file keeps its bytes until another change rewrites it in canonical form. An outside edit that leaves the frontmatter unparseable or outside the property rules, adds frontmatter to an unmanaged note or removes it from a managed one becomes a `file` operation instead, which ends text mode.
- A `file` operation that covers every live operation takes the note out of text mode. A device writes one when the file turns binary, or when its own edit takes the note's text past 4 MiB, counted as the UTF-8 bytes of `content`.

**State.** The note's body is the document that integrating the lineage's live records in canonical order yields: each live `text_snapshot` by ascending log sequence, then each live `text_update` by ascending log sequence, an update waiting while an earlier update of the same signer in the lineage is pending in yrs. Integrating records as they arrive reaches the same document while no two records hold different copies of one element and no snapshot shows an element that a record it covers deleted, which each update's own client ID ([The document on a device](#the-document-on-a-device)) and the snapshot check ([Collaboration sandbox](#collaboration-sandbox)) ensure. Each `meta` key takes the value of its latest write among those records that passed the verdict, ordered by effective sequence and then by record digest as unsigned bytes, the higher winning. An update's entry takes the update's log sequence as its effective sequence, and a snapshot's entry the sequence it names, or the snapshot's own when it names 0 or a later one. A write with no value removes the property. Concurrent edits to different keys therefore both survive, and the later committed write wins a key. Two live snapshots of one lineage merge without a conflict prompt.

### Snapshots

A device writes a new snapshot, the encoded state of the yrs document with garbage collection on and an entry for every `meta` key of the note's merged state, in three cases:

- **Idle.** When a note has had no edit for 30 seconds, or when you close it, the device that wrote its latest text update writes a snapshot if the note holds more than 8 live patches. When the note still holds more than 8 live patches after 5 minutes without an edit, because that device went to sleep or away, any writer device that holds the note's document writes one; two such snapshots of one lineage merge, and the 10-second limit below keeps the race cheap.
- **Volume.** When the note's live patches exceed 512 records, or a byte threshold of the larger of 1 MiB and a quarter of the last snapshot's encoded size, capped at 2 MiB, the device with the lowest device ID among the devices that hold a live patch of the note writes one. Another device writes it only when the live patches pass twice either threshold, at most 1,024 records or 4 MiB, which stays below the server's limits of 2,048 records and 8 MiB ([§13](#server-checks-on-an-operation)). Byte thresholds count what the server counts: the ciphertext length of the live patch operations, blob ciphertext included.
- **Required.** When the server answers `snapshot_required` ([§14](#your-own-chain)).

Each device writes at most one snapshot of a note every 10 seconds, unless the server requires one. At 20 typists sending a patch every 500 ms, the designated device snapshots about every 13 seconds. The idle rule leaves a note you edited and walked away from with one snapshot and at most 8 live patches ([§28](#28-worked-example-100000-objects)). For each device whose patches it includes, a snapshot covers the highest counter below that device's lowest live patch the document hasn't integrated: one deferred under a writer's allowance, or one pending on a dependency that hasn't arrived. Patches it didn't include therefore stay live and still apply. A patch judged unreadable, or one that depends on such a patch, merges on no other device, so a snapshot may cover it; when a replacement skipped the epoch of a patch its signer could read, the signer writes its content again ([§9](#epochs)). Each `meta` entry of a snapshot names the effective sequence of the write it holds, so a covered write keeps its place against a concurrent one the snapshot didn't see. It names 0 when no record outside the snapshot's own push has committed that write yet. A write whose record a restore lost counts as a new write when the device writes it again: its entry names 0 ([§14](#after-a-server-restore)). A reader that already holds every covered patch, and whose text matches the snapshot's `revision`, skips downloading the snapshot's blob.

**Rebase.** When the encoded document reaches 16 MiB, counted as the length of its update format v1 encoding with garbage collection on, the device that the volume rule designates writes a snapshot with a new lineage whose first document holds the current body alone, derived as on entering text mode, with `base` the note's live `text_snapshot` at the highest log sequence and the merged `meta` entries naming their effective sequences. Text updates of the old lineage that arrive afterward go to review as text. Two devices that rebase from different states write different lineages, which conflict.

### The document on a device

Each device holds one yrs document per open note, inside the collaboration sandbox. The editor sends local edits into it as transactions and receives encoded updates and text changes back; it never inserts remote text again as a local edit. Relative positions for presence encode and decode inside the sandbox, and undo tracks only transactions of local origin. Each time a device loads a document, it takes the next load number from its reservation for the workspace ([§15](#client-journal)) and inserts under the client ID `card_index × 2^29 + load`, where `card_index` is its card's index in the workspace ([§8](#card-index)); each `text_update` it sends names that `load`. Client ID 0 belongs to a lineage's first document alone ([Entering text mode](#entering-text-mode)). Every client ID stays below 2^53, the most that yrs 0.26.0 holds without `small-client` ([§3](#3-primitives)), and no two cards or loads share one, because the policy log assigns card indices and a device never reuses a reserved load number. The journal keeps each text-mode note's live snapshots and patches with the sandbox's verdict on each, and the device rebuilds the document from them in canonical order after a restart, skipping the records it judged unreadable.

### Collaboration sandbox

Remote snapshots and updates pass through the collaboration sandbox, a Wasmtime instance, before they reach the editor or the file. The sandbox works in two stages. No clock decides a verdict: the verdict stage runs under its fuel and memory limits alone, with Wasmtime's epoch interruption off.

1. **Verdict.** The sandbox parses and validates each record's yrs bytes in a fresh instance that holds no document, with a fuel budget of 1,000,000 units plus 1,000 per byte of the record's decrypted payload, without its padding, and of the blob's file when the yrs bytes travel in one, and 512 MiB of linear memory. Its inputs are the record's bytes, its signer's card index and the number of cards the workspace indexed at the record's `policy_revision`, which every device reads from the same verified log. It also checks the record's `meta` entries. The guest module parses the lib0 v1 structure itself before it hands the bytes to yrs, because yrs 0.26.0 masks a decoded client ID to its low 53 bits, wraps varints longer than their type, and reads strings without checking UTF-8. A record is unreadable when:
   - a varint in its yrs bytes isn't the shortest encoding of its value, or its value doesn't fit its field: below 2^53 for a client ID, below 2^32 for a count, clock, length or parent marker; or a client's first clock plus the lengths of its structs, or a delete range's clock plus its length, passes 2^32 − 1;
   - its struct section or its delete set names one client ID twice, or a struct or delete range has length 0;
   - a string in it isn't valid UTF-8;
   - its yrs bytes fail to parse or pass either limit, hold a GC struct, which an honest document never holds because `content` is a root nobody deletes, or hold an item whose content isn't a string or a deletion, that names a parent other than the root `content`, or that carries a map key; because every record passes this check, an item that inherits its parent from an origin in an earlier record stays in `content` too;
   - it holds an item whose origin or right origin is an element under the item's own client ID at the item's own clock or a later one, which its writer can't have seen; yrs integrates such an item in one place as it arrives and in another from a snapshot of the result, and a device that boots from that snapshot traps;
   - it is a `text_update` whose `load` lies outside 1 to 2^29 − 1, or that holds any struct, an item or a skip, under a client ID other than its signer's card index times 2^29 plus `load`;
   - it is a `text_snapshot` that holds a struct under a client ID other than 0 or `i × 2^29 + l`, with `i` from 1 to the number of cards the workspace indexed at its `policy_revision` and `l` at least 1;
   - a `meta` key is neither `path` nor `p:` followed by a valid property name, an update's entry names a sequence other than 0, or a `p:` value, when present, isn't RFC 8785 canonical JSON;
   - `path` lacks a value, isn't a path passing the rules of [§13](#payload), or is missing from a snapshot;
   - the payload's `object_id` is empty and a `p:` key appears, or it isn't and a `p:id` entry isn't the JSON string of `object_id`, or `p:id` or `p:type` is missing from a snapshot or lacks a value; only a snapshot must carry `p:id`, so an update without one stays readable;
   - the guest traps during the verdict.

   An unreadable record counts as applied, neither its body nor its `meta` entries reach the note, and it goes to review ([§13](#client-checks-on-an-operation)). The verdict depends on the record's bytes and on two numbers from the verified log, so every device reaches the same one. To keep it that way, one guest module judges every record: `sync-v2-collaboration.json` records the BLAKE3 hash of its `.wasm` file and the Wasmtime operator costs its fuel count assumes, and the app runs no other module for the verdict. The budget is a protocol constant, set far above the largest legitimate update per byte that ticket 54 measures. A change to the module, the budget or the operator costs is a protocol change, not a release choice. Continuous integration runs the near-limit fixtures under Cranelift and Pulley, on the previous and the current Noura release, and fails when their verdicts differ.
2. **Integration.** The sandbox then merges the parsed record into the note's document. The cost depends on the document, so integration gives no protocol verdict. Each writer holds an allowance per note of 2,000,000,000 fuel units, refilled each minute up to that amount. Both stages charge it the fuel each record used, so one record may overdraw it, and while the balance is negative the device defers the writer's further records and keeps merging the others: a collaborator who floods a note, or sends one huge record, slows only their own edits. When one record's integration alone passes 10,000,000,000 units, or would take the document past 32 MiB or its text past 8 MiB, the device stops merging that lineage. Ordinary growth never reaches those sizes, because a note rebases at 16 MiB and leaves text mode at 4 MiB, so only a record that jumps past them triggers the stop. A trap during integration stops the lineage too, and the device discards the instance and rebuilds it from the journal. The stop is local, and other devices may keep merging. The device keeps the note's last good state on disk, writes no snapshot or update for that lineage, lists it for review as text, and shows the note as waiting for your choice: keeping its last good state writes a `file` operation that covers every live operation of the note. That `file` operation, or a rebase from any device, ends the lineage on every device.

**Placement.** yrs lands an item whose origins misstate what its writer saw in one place as it arrives and in another from a snapshot of the result, and every copy of each element still agrees, so the snapshot check can't see it. After it merges a record, the sandbox looks at each item the record added, and refuses the record when an item doesn't sit between its origin and its right origin, or has no origins while the document holds an element under client ID 0, or has no origins at a clock above 0. A snapshot takes the same check. A refused record counts as applied, neither its body nor its `meta` entries reach the note, the device rebuilds the document without it and charges the rebuild to its writer's allowance, and it goes to review. An update that depends on it stays pending. An honest item passes: yrs lands it between its origins, every writer saw the first document, and a writer's later items saw its earlier ones. Two writers' first inserts into an empty first document both merge.

**Snapshot check.** An element is one clock position of a yrs item, with its content (one UTF-16 code unit, or deleted), its left origin and its right origin. Two copies of one element agree when their origins match and their content matches, a deleted copy matching any content. U+FFFD matches a unit of a surrogate pair only where an origin, a right origin, or a delete range's start or end, in the snapshot or in a record of the lineage the device took in, falls between the pair's two units, so that integration splits the device's own copy too ([§3](#3-primitives)). Item boundaries in the snapshot's encoding alone don't count, nor does an origin that names the element just before an item under its own client ID, which only continues a run. Any other U+FFFD against a surrogate unit is a disagreement. Before it integrates a `text_snapshot`, the sandbox compares the snapshot with every record of the lineage that the device took in: each record that was live, deferred under its writer's allowance or pending in yrs just before the device applied the snapshot. The journal keeps a covered record until the sandbox has judged every snapshot that covers it, so a restart between applying a snapshot and checking it doesn't change the check. A record pending in yrs counts for its copies alone, since a snapshot may cover a patch that merges nowhere ([Snapshots](#snapshots)). The snapshot disagrees when a copy in it disagrees with one of those records, when it lacks an element of one it covers, or when it shows as not deleted an element that one it covers deleted. The device then discards the document and rebuilds it from the lineage's live records in canonical order, charging the work to the snapshot signer's allowance; it lists the note for review with the snapshot's signer and reports `sync_text_conflict`. An update needs no such check, because its elements carry its signer's client ID and an earlier update of that signer, or a snapshot, comes first in canonical order everywhere; the device lists an update that disagrees with a copy it holds for review.

yrs holds an update whose dependencies haven't arrived as pending until they do. An update that depends on a record judged unreadable stays pending until another record supplies what it depends on, as a rotator's rewrite after a replacement does ([§9](#epochs)), and the device lists the lineage for review as text while it holds such updates.

A device counts a `text_snapshot` or `text_update` as applied for its chain head, cursor, live set and manifest once the record passes the checks in [§13](#client-checks-on-an-operation), under a key it holds, and reaches the journal, and runs the sandbox in the background. Meanwhile the note stays editable on its last merged state: your edits queue for the document and apply after the records ahead of them, and the file keeps its last merged state. On a slow machine the note catches up later, and the verdict doesn't change. A snapshot or update larger than the inline limit travels as a blob.

Edits made offline merge on reconnect: every live patch of the lineage applies whatever snapshot is current, and a snapshot never discards an update it didn't cover.

## 23. Presence and realtime channel

### Realtime channel

`GET /v2/workspaces/{w}/realtime` opens a WebSocket authenticated with the device's session token. Frames are binary, at most 64 KiB, each `u8 frame_type | bytes body`:

| Type | Direction | Body |
|---|---|---|
| 1 notify | server to client | `id domain`, `u64 head`, `u64 checkpoint_due` |
| 2 presence | both | a presence record |
| 3 presence_gone | server to client | `id device`, `id session` |
| 4 policy_changed | server to client | `u64 revision` |
| 5 keys_changed | server to client | empty |
| 6 records | server to client | `id domain`, `u64 previous_seq`, `u64 checkpoint_due`, `list<entry>`: committed log entries that fit one frame, each with its sequence |
| 7 subscribe | client to server | `list<id>`: the presence channels of the notes this device has open |
| 8 restored | server to client | `u64 restore_generation`, sent when the channel opens and whenever the generation changes ([§14](#after-a-server-restore)) |
| 9 error | server to client | `u8 rejected_type`, `id domain`, `id session`, `u64 sequence`, `str code`, `opt<bytes> detail`: the server refused a client frame; for a presence record the IDs and sequence are the record's, for a subscribe the domain, session and sequence are zero |
| 10 account_changed | server to client | `id account`, `digest genesis`, `u64 seqno`: an account that shares this workspace appended a link or started a new chain ([§7](#account-chain)) |

The channel carries no writes besides presence and subscriptions: clients push only through `POST .../ops` and pull only through `POST .../pull` ([§15](#15-domain-log-cursors-and-pulls)).

After a push commits, and never before, the server tells every connected device that can read a domain about that domain's new entries, the pushing device included. In PostgreSQL mode the instance sends `NOTIFY` with the workspace, domain and new head after the commit succeeds, from a separate statement outside the push's transaction, and the other instances read the entries from the database. A `NOTIFY` inside the transaction would take PostgreSQL's database-wide notification lock through the commit and make every push in every workspace commit one at a time. A push that rolls back wakes nobody, and a crash that loses a notification costs at most the minute until the next workspace pull. When the entries fit one frame, it forwards them in a records frame: `entries` holds every entry of the domain from the one after `previous_seq` through the frame's last, in sequence order, each as `u64 seq | u8 record_type | bytes record` with its log position, and `previous_seq` is the sequence of the domain's entry just before the first, or 0 when the first is the domain's first entry. Otherwise it sends notify with the new head, at most one notify per domain every 250 ms on a connection.

A client applies a records frame only when its cursor for that domain is at or above `previous_seq` and below the first entry's sequence, so no entry lies between its cursor and the frame. It verifies the entries exactly like pulled ones: the checks in [§13](#client-checks-on-an-operation), the chain verifier in [§14](#14-device-chains), and the positional rules in [§8](#policy-entries) and [§9](#rotation). It advances its cursor to the last entry's sequence only after every resulting file change reaches disk, as for a page ([§15](#pull)). When its cursor lies below `previous_seq`, a gap, it pulls instead; when its cursor already covers the frame's last entry, it ignores the frame; when its cursor lies at or past the first entry but before the last, it pulls too. A frame that fails a check counts like a page that fails: the client applies nothing from it, keeps its cursor and pulls.

A client judges every frame against the generation of the latest `restored` frame and the `retention_floor` of its last pull, as it would a page carrying them. Notifications and records frames only feed the same verification as pulls, so a relay that forges or drops them can delay sync but can't change what the client accepts. The app sends a WebSocket ping every 20 seconds and reconnects with jittered backoff between 1 and 30 seconds. While the channel is up, the app still makes one workspace pull ([§15](#pull)) a minute; while it's down, the app long-polls that pull.

### Presence record

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.presence)` | |
| workspace | `id` | |
| domain | `id` | |
| epoch | `u64` | |
| device | `id` | |
| session | `id` | |
| sequence | `u64` | |
| minute | `u32` | Unix time in minutes when the device sent it |
| channel | `id` | the first 16 bytes of `BLAKE3.keyed_hash(presence subkey, tag(noura.sync.presence.channel) \|\| rid)` |
| *AEAD associated data ends here; the epoch record's digest follows ([§4](#records-signatures-and-digests))* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | |
| sig | `sig` | |

The ciphertext uses the presence subkey of the domain's current epoch, so a note's channel changes with each epoch. Its padded plaintext:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.presence.payload)` |
| rid | `id` |
| state | `u8` (1 active, 2 idle) |
| selections | `list<selection>`, at most 8, each `bytes anchor`, `bytes head` (yrs relative positions, at most 192 bytes each, so the padded plaintext stays within 4 KiB) |

The server checks the signer's device, its read access to the domain (else `forbidden`), the current epoch (else `epoch_stale`), and that no rotation is pending there (else `rotation_pending`), and answers a refused record with an error frame. A device that gets `rotation_pending` rotates the domain when it holds the key, as for a refused push ([§9](#rotation)), and fetches policy and keys on `epoch_stale` or `forbidden`. An accepted record goes to the other connected devices that can read the domain and subscribed to its channel. In PostgreSQL mode the instance that accepts a record relays it to the other instances with `NOTIFY`, base64-encoded; the 4 KiB cap on presence plaintext keeps a record within the 8,000-byte payload `NOTIFY` allows. The server never stores presence. A subscribe that names more channels than the limit, or that the server can't apply, gets an error frame too. A device sends at most 2 presence records a second, on the 500 ms editing tick, and only when its selection changed. A device draws a new session ID for each connection. Receivers reject a record whose epoch isn't current, whose `minute` lies more than 2 minutes from their clock, or whose sequence is at or below the last one seen for that device and session during the epoch. They drop a session's cursors 30 seconds after its last record or when the server sends `presence_gone`.

The channel tells the server which devices have the same note open. Text updates already tell it which `rid` each writer edits and when, to the 500 ms tick ([§30](#30-threats)).

## 24. Share links

An owner or admin shares a read-only snapshot of a note:

```text
{origin}/s/{base64url(token)}#{base64url(link_key || link_verify_key)}
```

The sharing device generates a 16-byte token, a 32-byte link key, and an Ed25519 link key pair from a random 32-byte seed. The fragment holds the link key and the link's 32-byte verify key. Browsers never send the fragment, so the server never receives them.

`share`, the snapshot the viewer fetches, signed with the link's signing key:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.share)` | |
| token_hash | `digest` | `BLAKE3(tag(noura.sync.share.token) \|\| token)` |
| snapshot_revision | `u64` | 1, then +1 on each Update link |
| *AEAD associated data ends here* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | under `derive_key("noura.sync.v2 share key", link_key)` |
| sig | `sig` | by the link's signing key |

Its padded plaintext is `tag(noura.sync.share.payload) | str title | bytes markdown`, at most 983,040 bytes (960 KiB) after padding, so the record fits a 1 MiB request. The viewer in `account-web` verifies the signature with the verify key from the fragment, then decrypts under a content security policy that blocks inline scripts, renders the Markdown through a strict allowlist, and leaves images out. A bad signature, tampered ciphertext or wrong key shows an error page and renders nothing. The viewer remembers the highest `snapshot_revision` it showed for each token in the browser's local storage and warns when a later visit shows a lower one.

The device that creates a link keeps the link's token, key and signing seed in its credential store, and it is the only device that can update that link. The server stores the token hash, workspace, domain, creating device, expiry (1 to 30 days, 30 by default) and the snapshot. It accepts a new link only from an owner or admin device that is active in the workspace's policy, and an update only from the creating device's session while that device stays active and its account an owner or admin. Other owner and admin devices list links by token hash, domain, creating device and expiry, and can revoke them. The server never learns which note a link shares, so only the creating device, which records that in its own application data, lists a note's links in the note's share dialog. A member learns a link key only from its URL, so a rotation needs no link handling. A workspace holds at most 1,000 active share links.

The server can't verify a snapshot's signature, so it checks only what it can read: a snapshot whose `token_hash` isn't the link's gets `bad_request`, as does a new link whose `snapshot_revision` isn't 1 or an update that doesn't raise it; a ciphertext that isn't a padded length gets `padding_invalid`. A creation retried with identical bytes within a day returns the original expiry, since the server keeps no creation time to compare. Revoking a link that's already gone succeeds.

Update link replaces the snapshot under the same token and keys. Revoking or expiring a link deletes its snapshot, and the viewer then shows a not-found page. A link whose creating device was removed can't change any more on an honest server, and stays until it expires or someone revokes it. The other owner and admin devices flag such a link and offer to revoke it, because a removed device working with a malicious server could still replace its snapshot. A device keeps the token hash of each link it revoked until the link would have expired, and revokes it again after a server restore ([§14](#after-a-server-restore)).

## 25. Workspace profile

The workspace's display name travels encrypted, so a new device can list your workspaces by name and the server stores no plaintext name.

| Field | Type |
|---|---|
| tag | `tag(noura.sync.profile)` |
| workspace | `id` |
| profile_revision | `u64` |
| epoch | `u64` |
| policy_revision | `u64` |
| policy_head | `digest` |
| signer | `id` |
| *AEAD associated data ends here; the epoch record's digest follows ([§4](#records-signatures-and-digests))* | |
| nonce | `nonce` |
| ciphertext | `bytes` |
| sig | `sig` |

The ciphertext uses the profile subkey of the workspace domain's `epoch` key. Its padded plaintext is `tag(noura.sync.profile.payload) | str name`. Workspace creation carries the first profile, at `profile_revision` 1, and an owner or admin device replaces it by pushing a new one ([§15](#push)). The server accepts a pushed profile when:

- its signer is the session's device, an owner or admin device active in the policy, and the signature verifies under its card from its account chain (else `forbidden`);
- `policy_revision` is at most the current revision (else `policy_stale`) and `policy_head` equals the server's change there (else `policy_conflict`);
- `epoch` is the workspace domain's current epoch and no rotation is pending there (else `epoch_stale` or `rotation_pending`);
- `profile_revision` is one above the server's latest profile (else `profile_conflict`).

Clients accept a profile only when its signer is an owner or admin device in their verified policy at `policy_revision`, `policy_head` matches their change at that revision, `policy_revision` is at least the previous profile's, no change in their accepted chain above `policy_revision` takes its signer out of the workspace, as for a checkpoint ([§17](#checking-a-checkpoint)), and its epoch passes the writer rule of [§9](#rotation) at that revision: no change at or below `policy_revision` requires the workspace domain to rotate past it. When a change takes the device that signed the current profile out of the workspace, each owner or admin device that accepts the change signs the profile again at its accepted head, and the server keeps the first. A server that accepts a profile from an admin who hadn't seen a removal could still show the new name to the removed device; an honest server refuses it with `epoch_stale`. They report `sync_profile_rollback` for a `profile_revision` below the last one they accepted, or for a profile that differs from the one they accepted at the same revision.

## 26. HTTP API

Request and response bodies use the encoding in [§4](#4-encoding) with media type `application/vnd.noura.sync.v2`, except the tus requests in [§21](#21-attachments). Every route requires `Authorization: Bearer <session token>` except `GET /v2/limits`, `POST /v2/device/authorize`, `POST /v2/device/token`, `GET /invite/{token}`, `GET /v2/invitations/{token}`, `GET /s/{token}` and `GET /v2/shares/{token}`. Routes under `/v2/workspaces/{w}/` serve only active devices of w, as [§8](#policy-change-record) describes. Every response, errors included, carries the server's restore generation in the header `Noura-Restore-Generation` ([§14](#after-a-server-restore)). Errors return an HTTP status and the body `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. Messages never include database or operating system details, which stay in the server's local log.

Every route that creates something under a client-chosen ID (a workspace, blob, invitation, share link or approval offer) returns the original result when a client retries it with identical bytes, and `id_conflict` when the bytes differ. An invitation's retry returns its stored expiry for as long as the invitation is open ([§11](#11-invitations)).

| Route | Purpose |
|---|---|
| `GET /v2/limits` | protocol version (2), restore generation, and the limits in [§27](#27-limits) |
| `POST /v2/device/authorize`, `POST /v2/device/token` | device authorization grant |
| `POST /v2/devices/challenge`, `POST /v2/devices` | enrollment challenge; enroll with an `enroll` record, or bind a new session to an enrolled card ([§7](#enrollment-proof)) |
| `GET /v2/devices`, `DELETE /v2/devices/{device}` | list the account's enrolled cards that no link removed, with their session counts; end the sessions of an enrolled card that no chain lists, from an active device's session ([§7](#device-list)) |
| `POST /v2/accounts/chain`, `POST /v2/accounts/chains` | append links to the session's account chain, in order and all or none; read links of the chains the session may read, after given seqnos ([§7](#account-chain)) |
| `POST /v2/account/reset-code`, `POST /v2/account/reset` | email a reset code; start a new account chain with it ([§7](#resetting-your-devices)) |
| `POST /v2/approval-requests`, `GET /v2/approval-requests` | post a candidate's request, which names at most one of an invitation and a workspace (else `bad_request`); list, for an active device, the requests of the account's own new devices: those that name neither and carry no `candidate_head` |
| `GET /v2/workspaces/{w}/pending-devices` | requests of invited accounts, and of reset member accounts, awaiting approval in this workspace |
| `POST /v2/approvals`, `GET /v2/approvals` | send an `offer`; offers addressed to the calling device's card |
| `POST`, `GET /v2/approvals/{offer_digest}/reveal` | reveal the candidate's nonce; read it |
| `POST /v2/recovery` | prove possession of a recovery identity ([§12](#12-recovery-kit)) |
| `GET /v2/workspaces` | workspaces the account belongs to, with role, quota, usage and latest profile record |
| `POST /v2/workspaces` | create a workspace: genesis, the workspace domain's first epoch, envelopes and profile in one request |
| `DELETE /v2/workspaces/{w}` | close a workspace, from an active device of its owner account ([§14](#after-a-server-restore)) |
| `GET /v2/workspaces/{w}/policy?after={revision}` | policy changes after a revision, with the head revision and the current head of each member account's chain; for a device whose account `remove_member` removed, only up to that change |
| `GET /v2/workspaces/{w}/keys?after={key_seq}&recipient={device}` | epoch records of readable domains, envelopes addressed to `recipient`, which is the session's device unless the session proved a recovery identity, and re-wrap requests ([§9](#envelopes)), ordered by the server's key sequence |
| `GET /v2/workspaces/{w}/keys/current?recipient={device}` | the current epoch record of each domain the caller reads, `recipient`'s envelopes for those epochs and the open re-wrap requests for them, with the key sequence to continue from ([§18](#18-bootstrap)) |
| `GET /v2/workspaces/{w}/domains/{d}/epochs/{epoch}` | that epoch record and the ones its `previous` links reach, newest first, as many as fit one response, for walking lineage |
| `POST /v2/workspaces/{w}/domains/{d}/rewrap` | ask for a new envelope of the domain's current epoch, from a device that the current policy authorizes to read the domain and for which the server accepted an envelope at that epoch (else `forbidden`) ([§9](#envelopes)); one open request per device and epoch, a repeated request returns the original result, and the server closes it when the next epoch commits or the requester's key fetch passes an answer |
| `POST /v2/workspaces/{w}/ops` | push, the only route besides workspace creation that writes records to a workspace ([§15](#push)) |
| `POST /v2/workspaces/{w}/pull` | pull every listed domain in one request, the only pull route ([§15](#pull)) |
| `GET /v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}` | the device's record at that counter, or its next record above it when the device skipped the counter, as an `entry` |
| `GET /v2/workspaces/{w}/domains/{d}/heads/{device}` | the server's chain head for a device; `not_found` when the device has no record in the domain |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoint` | latest checkpoint bundle, with its entry's sequence; `not_found` before the first |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoints/{digest}` | one checkpoint by digest, as an `entry` ([§17](#checkpoint-chain)) |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoint-chain?from={digest}&to={digest}` | checkpoint records on the walk from `from` back to `to` through `skip` and `previous` links, newest first, as many as fit one response ([§17](#checking-a-checkpoint)) |
| `GET /v2/workspaces/{w}/domains/{d}/live?at={seq}&after={cursor}` or `?at={seq}&rid={rid}&after={cursor}` | bootstrap page: operations live at `at`, ordered by `rid`, device and counter, or those of one `rid` |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&level={0..3}&index={j}&after={cursor}` | 16 child hashes of a node, or a bucket's leaf refs at level 3 |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&buckets=all` | all 4,096 bucket hashes |
| tus routes in [§21](#21-attachments), `GET /v2/workspaces/{w}/blobs/{id}` | attachments |
| `GET /v2/workspaces/{w}/missing-blobs` | blobs a restore left missing, for devices to upload again ([§21](#21-attachments)) |
| `POST`, `GET /v2/workspaces/{w}/invitations`, `DELETE .../invitations/{id}` | create, list and revoke invitations |
| `GET /invite/{token}` | invitation page (HTML), which sends you to the desktop app |
| `GET /v2/invitations/{token}` | the invitation's context, for the desktop app |
| `POST /v2/invitations/{token}/accept` | bind the invitation to the signed-in account |
| `POST`, `GET /v2/workspaces/{w}/shares`, `PUT`, `DELETE .../shares/{token_hash}` | create, list, update and revoke share links |
| `GET /s/{token}`, `GET /v2/shares/{token}` | share viewer page (HTML) and its snapshot record |
| `GET /v2/workspaces/{w}/realtime` | realtime channel |

`live` and `manifest` accept `at` only when it equals the `covered_seq` of a checkpoint of the domain at or above both the retention floor and the caller's history floor ([§18](#18-bootstrap)); otherwise they return `cursor_compacted` or `before_history_floor`.

### Message bodies

Each body below uses the types in [§4](#4-encoding) and carries no tag, and the table gives every field, in order. A route not listed takes an empty request body and returns an empty response body on success. Timestamps in these bodies (`expires_at`) are Unix seconds as `u64`; they describe invitations and links, never objects.

Five routes page, each with its own cursor: `policy` by revision (`after`, the last revision received), `keys` by key sequence (`after`, the last key sequence received), `accounts/chains` by seqno per chain (`after`, the last seqno received), and `live`, in both forms, and `manifest` level 3 by `live_cursor` (`after`, the `next` of the previous page). The client asks again until `policy` reaches `head_revision`, `keys` reaches `head_key_seq` or returns no entry, each chain page reaches `head_seqno`, or `next` is absent. `keys` leaves out the records the caller doesn't receive, so its last entry can lie below `head_key_seq` while nothing more is due. `epochs` and `checkpoint-chain` return as many records as fit, and the client asks again from the last one it got. Every other list fits one response within the limits of [§27](#27-limits).

| Route | Request | Response |
|---|---|---|
| `GET /v2/limits` | | `u16 protocol`, `u64 restore_generation`, `u32 max_body`, `u32 max_inline`, `u64 max_blob`, `u32 max_covers`, `u32 max_recipients`, `u32 presence_per_second`, `u64 retained_bytes` |
| `POST /v2/device/authorize` | | `str device_code`, `str user_code`, `str verification_uri`, `u32 expires_in`, `u32 interval` |
| `POST /v2/device/token` | `str device_code` | `str session_token`, `id account`; errors use the RFC 8628 codes `authorization_pending`, `slow_down`, `expired_token` (also for a code that was used or never existed), `access_denied` |
| `POST /v2/devices/challenge` | | `key challenge`, `u32 expires_in` |
| `POST /v2/devices` | `bytes enroll` | |
| `GET /v2/devices` | | `list<device_info>`: `bytes card`, `u32 sessions`, `u8 state` (1 enrolled and listed by no chain, 2 active in the account's chain) |
| `POST /v2/accounts/chain` | `list<bytes>` of account links | `u64 seqno` of the chain's new head |
| `POST /v2/accounts/chains` | `list<chain_cursor>`: `id account`, `opt<digest> genesis` (absent for the account's current chain), `u64 after` | `list<chain_page>`, one per cursor in request order: `id account`, `digest genesis`, `u8 status` (1 current, 2 retired by a reset), `u64 head_seqno`, `list<bytes>` of links after `after` in seqno order; at most 1 MiB in all |
| `POST /v2/account/reset` | `str code`, `bytes genesis` | |
| `POST /v2/approval-requests` | `digest commit`, `opt<account_head> account`, `opt<id> invitation`, `opt<digest> invite_proof`, `opt<id> workspace` (present for a reset, [§10](#approval-request)) | |
| `GET /v2/approval-requests`, `GET .../pending-devices` | | `list<pending>`: `bytes card`, `digest commit`, `opt<account_head> account`, `opt<id> invitation`, `opt<digest> invite_proof`, `opt<id> workspace` |
| `POST /v2/approvals` | `bytes offer` | |
| `GET /v2/approvals` | | `list<bytes>` of offer records |
| `POST /v2/approvals/{offer_digest}/reveal` | `key nc` | |
| `GET /v2/approvals/{offer_digest}/reveal` | | `key nc` |
| `POST /v2/recovery` | `bytes recovery_proof` | |
| `GET /v2/workspaces` | | `list<workspace_info>`: `id workspace`, `u8 role`, `u64 used_bytes`, `u64 history_bytes`, `opt<u64> quota_bytes` (absent when unlimited), `opt<bytes> profile` |
| `POST /v2/workspaces` | `list<record_ref>`: genesis, the workspace domain's first epoch bundle, an envelope for every active card of the owner's account at the head the genesis records, and the profile | `u64 policy_revision`, `list<receipt>` |
| `GET .../policy` | | `u64 head_revision`, `list<bytes>` of the policy records after `after`, in revision order, at most 1 MiB, `list<account_head>` sorted by account, with the current head of each member account's chain |
| `GET .../keys` | | `u64 head_key_seq`, `list<key_entry>` after `after`, in key-sequence order, at most 1 MiB: `u64 key_seq`, `u8 record_type` (3 epoch record, 4 envelope, or 5 re-wrap request, whose record is `id domain \| id device \| u64 epoch`), `bytes record` |
| `GET .../keys/current` | | `u64 head_key_seq`, `list<key_entry>`, at most 1 MiB |
| `GET .../epochs/{epoch}` | | `list<bytes>` of epoch records, newest first, at most 1 MiB |
| `POST .../domains/{d}/rewrap` | `u64 epoch` | |
| `POST .../ops` | `list<record_ref>` | `u64 policy_revision`, `list<receipt>` |
| `POST .../pull` | `list<pull_cursor>`, `u32 wait_seconds` | `list<page>` of pull pages ([§15](#pull)) |
| `GET .../chain/{device}/{counter}` | | `entry` |
| `GET .../heads/{device}` | | `u64 counter`, `digest record_digest`, `u64 seq` |
| `GET .../checkpoint` | | `u64 seq` of the checkpoint's entry, `checkpoint_bundle` |
| `GET .../checkpoints/{digest}` | | `entry` |
| `GET .../checkpoint-chain` | | `list<bytes>` of checkpoint records on the walk, newest first, at most 1 MiB |
| `GET .../live` | | `u64 at`, `opt<live_cursor> next`, `list<entry>`; `live_cursor` is `id rid`, `id device`, `u64 counter`, the last entry returned, and pages exclude it |
| `GET .../manifest`, levels 0 to 2 | | `list<digest>` of the node's 16 children |
| `GET .../manifest`, level 3 | | `list<leaf_ref>`: `id rid`, `ref`, sorted by `rid`, device and counter; then `opt<live_cursor> next` |
| `GET .../manifest`, `buckets=all` | | `list<digest>` of the 4,096 bucket hashes |
| `GET .../missing-blobs` | | `list<id>` |
| `POST .../invitations` | `id invitation`, `digest token_hash`, `u8 role`, `bytes invite_context` | `u64 expires_at` |
| `GET /v2/invitations/{token}` | | `id workspace`, `id invitation`, `u8 role`, `u8 status`, `bytes invite_context` |
| `GET .../invitations` | | `list<invitation_info>`: `id invitation`, `u8 role`, `u8 status` (1 pending, 2 accepted), `opt<id> accepted_by`, `u64 expires_at`, `bytes invite_context` |
| `POST .../shares` | `digest token_hash`, `id domain`, `u8 expires_in_days`, `bytes share` | `u64 expires_at` |
| `GET .../shares` | | `list<share_info>`: `digest token_hash`, `id domain`, `id device`, `u64 expires_at`, where `device` is the creating device |
| `PUT .../shares/{token_hash}` | `bytes share` | |
| `GET /v2/shares/{token}` | | `bytes share` |

`error` is `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. The `detail` of `chain_conflict` is `id domain | u64 counter | digest record_digest`, the server's head, counting the records before the failing one in the same push; of `cursor_compacted` from `live` or `manifest`, `u64 retention_floor | u64 checkpoint_covered_seq` (a pull reports both conditions per page instead, [§15](#pull)); of a failed push, `u32 record_index | str code | opt<bytes> record_detail`, where `record_detail` holds the failing record's own detail, such as a `chain_conflict` head; of `device_removed`, `digest genesis | u64 seqno`, the link that removed the card or the genesis that retired its chain.

Server error codes:

| Code | HTTP status | Meaning |
|---|---|---|
| `bad_request` | 400 | the body doesn't decode |
| `too_large` | 413 | the body or a field exceeds a limit |
| `unauthorized`, `forbidden`, `not_found` | 401, 403, 404 | no valid session; the device lacks the right; no such resource for this device |
| `rate_limited` | 429 | over a rate limit; `retry_after_seconds` says when to retry |
| `version_unsupported` | 400 | a tag's version isn't 2 |
| `signature_invalid` | 422 | a signature fails under the signer's card |
| `padding_invalid` | 422 | a ciphertext length isn't a padded length |
| `chain_conflict` | 409 | wrong counter or previous digest; `detail` holds the server's head |
| `supersedes_unknown` | 409 | a cover names no operation on this `rid` |
| `epoch_stale`, `epoch_conflict`, `rotation_pending` | 409 | the operation's epoch isn't the latest epoch entry's; another rotation took this epoch, or a replacement skips an epoch that a device other than its rotator used without losing read access to the domain through a change at or below the replacement's `policy_revision`; the domain waits for a rotation, so it takes no record, presence or lone envelope outside that rotation ([§9](#rotation)) |
| `policy_stale`, `policy_conflict` | 409 | a record names a revision above the head; another change took this revision, or `policy_head` differs from it |
| `policy_regressed` | 422 | a record's `policy_revision` or `generation` is below the previous record's in its chain |
| `recipients_invalid` | 422 | an epoch's recipients or an envelope's recipient don't match the authorized devices |
| `manifest_mismatch` | 422 | a checkpoint's root, `live_objects` or `heads_digest` differs from the server's, or its `number`, `skip`, `floor`, policy or epoch fields don't follow from the server's log |
| `cursor_compacted`, `pruned` | 410 | a `live` or `manifest` request names a checkpoint below the retention floor; the chain position was pruned |
| `blob_incomplete`, `blob_digest_mismatch`, `blob_domain_mismatch` | 409, 422, 422 | a referenced blob isn't complete; an upload's digest doesn't match; the blob was created for another domain |
| `blob_offset_conflict` | 409 | a tus `PATCH` names an `Upload-Offset` other than the upload's, or the upload is already complete |
| `quota_exceeded` | 507 | the workspace is over its storage quota and the record would raise usage, or an upload wouldn't fit ([§20](#20-atomic-changes-and-quota)) |
| `id_conflict` | 409 | a client-chosen or derived ID already exists with other bytes, a change would make one device ID active under two accounts, or an enrolled card's device was removed |
| `rid_domain_mismatch` | 409 | the `rid` belongs to another domain |
| `checkpoint_not_due` | 409 | a standalone checkpoint arrived while none was due, or its `covered_seq` sits below the due point |
| `checkpoint_stale`, `checkpoint_unknown` | 409 | a checkpoint's `previous` isn't the domain's latest checkpoint; a record names a checkpoint this domain doesn't hold at that `covered_seq` |
| `rotation_not_due` | 409 | an epoch record arrived with no new domain, pending rotation or replacement behind it, such as one whose `previous` names the latest epoch while no rotation is due |
| `recipient_limit` | 409 | the change would give a domain more than 512 active readers, or the workspace more than 16,777,215 indexed cards |
| `account_conflict` | 409 | an account link's `previous` isn't its chain's head, or a policy change records an account head off that account's chain or behind the head the workspace records |
| `device_removed` | 401 | a link removed the session's card, or a reset retired its chain; `detail` names the link |
| `reset_code_invalid` | 400 | the reset code is unknown, used or expired |
| `profile_conflict` | 409 | a profile's `profile_revision` isn't one above the server's latest |
| `workspace_limit` | 409 | the account already owns 100 workspaces |
| `snapshot_required`, `conflict_limit` | 409 | the `rid` holds too many live patches, so the client writes a snapshot first; the `rid` holds 64 live state operations, so the client resolves first |
| `supersedes_incomplete` | 409 | a state operation leaves its own device's earlier state operation on the `rid` live |
| `domain_limit` | 409 | the workspace already holds 256 domains |
| `before_history_floor` | 403 | the request reaches below the account's history floor |
| `challenge_invalid`, `origin_mismatch` | 400 | the enrollment challenge is unknown, used or expired; the proof names another origin |
| `authorization_pending`, `slow_down`, `expired_token`, `access_denied` | 400 | RFC 8628 device grant: you haven't approved yet; add 5 seconds to the polling interval; the device code expired; you refused |
| `offer_expired`, `invitation_limit`, `invitation_invalid`, `share_limit` | 410, 409, 404, 409 | the approval offer is older than 10 minutes; the workspace already has 100 open invitations; the invitation is revoked, expired, used or unknown; the workspace already has 1,000 active share links |
| `internal` | 500 | the server failed to finish the request, such as when its database failed; it committed nothing, its local log holds the cause, and the client retries later |

Client error codes: `sync_chain_gap`, `sync_chain_reordered`, `sync_chain_replayed`, `sync_chain_forked`, `sync_policy_forked`, `sync_policy_rollback`, `sync_policy_unauthorized`, `sync_policy_regressed`, `sync_writer_not_authorized`, `sync_epoch_stale`, `sync_key_mismatch`, `sync_record_unreadable`, `sync_manifest_mismatch`, `sync_manifest_rollback`, `sync_journal_rollback`, `sync_approval_mismatch`, `sync_recovery_kit_invalid`, `sync_workspace_unverified`, `sync_profile_rollback`, `sync_reseal_limit`, `sync_server_rollback`, `sync_account_forked`, `sync_account_unauthorized`, `sync_restore_lost`, `sync_text_conflict`. Chain and record errors name the device and domain involved. A client that gets `before_history_floor` or `cursor_compacted`, or a pull page with status 2 or 3, bootstraps that domain ([§18](#18-bootstrap)).

### Rate limits

The server limits sync traffic per device and in bytes, with a separate allowance for bootstrap pages and range reads, and limits sign-in per client IP. Records that reach a device through the realtime channel don't count. A request over a limit gets `rate_limited` with `retry_after_seconds`. The defaults, which a server setting changes:

| Traffic | Default per device |
|---|---|
| Pushes | 10 a second, which leaves room for the 500 ms text tick, presence and rotations |
| Push bodies and blob uploads (tus `PATCH`) | 64 MiB a minute |
| Pulls, chain and checkpoint fetches, policy and keys | 128 MiB a minute |
| Bootstrap allowance: `live`, `manifest` and blob range reads | 512 MiB a minute |
| Other sync requests | 600 a minute |
| Sign-in requests (device grant, magic link, passkey, reset code) | 30 a minute per client IP |

At these defaults, the join in [§28](#28-worked-example-100000-objects) downloads its 10 GB of attachments in about 20 minutes.

## 27. Limits

| Limit | Value |
|---|---|
| Request or response body, push, tus PATCH, range response | 1,048,576 bytes |
| Encoded operation payload before padding; operation ciphertext | 262,144 bytes; 270,352 bytes |
| Minimum padded length | 256 bytes |
| Blob file content | 2,147,483,648 bytes |
| Blob chunk; blob segment | 65,536 bytes; 1,048,576 bytes |
| Strings without a limit of their own; `access` entries per list | 4,096 bytes; 1,024 |
| Supersedes entries per operation | 256 |
| Blobs per operation | 4 |
| Live patches per `rid` (server) | 2,048 records or 8 MiB of ciphertext |
| Live state operations per `rid` | 64, at most one per device |
| Text snapshots | after 30 idle seconds or on close above 8 live patches, by any writer after 5 idle minutes; after 512 live patches or a byte threshold of 1 to 2 MiB of ciphertext, twice that for devices other than the designated one; one per device and note every 10 seconds |
| Domains per workspace | 256 |
| Envelope ciphertext; X-Wing encapsulated key; X-Wing public key | 48 bytes; 1,120 bytes; 1,216 bytes |
| Epoch recipients, and active readers of a domain | 512 |
| Presence | 2 a second per device, 2-minute clock window, 30-second expiry, 4 KiB padded plaintext, relative positions of 192 bytes, 256 channels per subscribe |
| Realtime frame | 65,536 bytes |
| Text updates | one per note every 500 ms |
| Invitations | 7 days, 100 open per workspace |
| Share links | 1 to 30 days, 1,000 active per workspace |
| Device authorization codes, enrollment challenges, magic links, reset codes, approval requests and offers | 10 minutes |
| Sessions | no expiry; a link that removes its card or a reset that retires its chain marks a session removed, which then reads only its account's chains and answers `device_removed` elsewhere ([§7](#desktop-sign-in)); for a card no chain lists, an active device ends it |
| Active cards in an account chain | 256, at most one of them a recovery identity |
| Workspaces | 1,000 per account, 100 of them owned |
| Card indices; load numbers | 16,777,215 per workspace over its life; 1 to 536,870,911 per device and workspace |
| Pending approval requests | 32 per account, 100 per workspace |
| Re-seals per chain position that no newer epoch or policy change forces | 3 |
| Text mode | the writer's device leaves it at 4 MiB of UTF-8 text; a note rebases at 16 MiB of encoded document; a record that jumps past 8 MiB of text or 32 MiB of document stops the lineage on that device |
| Collaboration sandbox verdict, per record | 1,000,000 fuel units plus 1,000 per byte of the decrypted payload and blob file, 512 MiB of memory, one pinned guest module |
| Collaboration sandbox allowance | 2,000,000,000 fuel units per writer and note, refilled each minute and overdrawn as debt; 10,000,000,000 for one record's integration |
| Share snapshot, padded | 983,040 bytes |
| Retained log per domain | `retained_bytes` of weight, a server setting, 1 GiB by default, less while the workspace's usage and history exceed its quota |
| Checkpoint interval | 10,000 log entries or 64 MiB of weight, at due points the server publishes; a writer waits a random 0 to 60 seconds after a due point |
| Workspace quota | `workspace_quota_bytes` of live data, policy changes and epoch bundles, a server setting, unlimited by default; history yields to it |
| Completed upload that no operation lists | 24 hours |
| Sync rate limits | [§26](#rate-limits) |
| Quarantined blob-store objects after a restore | 7 days |

The targets of 100,000 objects and 200 devices are tested sizes, not caps. No structure in this document stops at 1,000 objects.

## 28. Worked example: 100,000 objects

The workspace holds 95,000 notes averaging 3 KiB and 5,000 attachments averaging 2 MiB, across 200 devices. It has 3 domains: the workspace domain and 2 restricted domains of 20 devices each.

**Record sizes** from the layouts above: an envelope is 1,366 bytes; an epoch record is 351 bytes, and its bundle with 199 recipients 3,543 bytes; an operation carrying a 3 KiB note and one cover is 3,717 bytes (389 bytes of fields, AEAD tag and signature, and a 3,328-byte padded payload); a checkpoint bundle with 200 chain heads is 13,184 bytes; a policy entry is 40 bytes.

### Removing one device

The removed device could read all 3 domains. Its owner's other device appends the `remove` link to the account chain in its own request, then pushes one `account_heads` change, and the server adds that change's entry to each of the 3 logs. The rotations follow, one push per domain: about 276 KB for the workspace domain and about 27 KB for each restricted domain.

| Record | Count | Bytes |
|---|---|---|
| `remove` link, in its own request | 1 | 178 |
| `account_heads` change | 1 | 249 |
| Epoch bundle, workspace domain | 1 | 3,543 |
| Envelopes, workspace domain | 199 | 271,834 |
| Epoch bundles, restricted domains | 2 | 1,326 |
| Envelopes, restricted domains | 2 × 19 = 38 | 51,908 |
| Record framing, 5 bytes per pushed record | 241 | 1,205 |
| Total | | 330,243 bytes |

The removal re-encrypts no object; the 100,000 objects add nothing. Each domain where the removed device wrote then takes a fresh checkpoint of about 13 KB from whichever writer signs first. The worst case is a workspace whose 256 domains all reach all 200 devices: 256 rotations of about 276 KB each, about 71 MB in all, each in its own push.

### Joining

A new member's device joins with access to the workspace domain.

1. **Approval push**, about 15 KB in one request: `add_member` with two account heads (370 bytes), one envelope for the workspace domain (1,366 bytes), and a fresh checkpoint bundle of 13,184 bytes.
2. **Policy, account chains and keys**: a policy chain of 3,000 changes takes about 760 KB in one page if each is an `account_heads` change of 249 bytes, for 1,500 device additions and 1,500 removals. The members' account chains then hold 1,500 `add_device` links of 1,503 bytes and 1,500 `remove` links of 178 bytes, about 2.5 MB in 3 requests to `accounts/chains`. The device fetches the current epoch record and its envelope of every domain it reads in one `keys/current` request; walking lineage back across 300 past epochs, when live operations name them, reads about 107 KB of epoch records in one request.
3. **Live operations**: 95,000 notes at 3,730 bytes per page entry, 281 per page, take 339 pages. The 5,000 attachment operations take 4 more.
4. **Attachments**: each 2 MiB attachment pads to a 2,163,269-byte blob, so the 5,000 blobs take 3 range requests each, 15,000 in all.
5. **Verification**: the device computes 100,000 leaf hashes and 4,369 bucket and node hashes and compares the root. That takes milliseconds.

No request or response exceeds 1 MiB.

### Text-mode notes

A note in text mode holds one snapshot and its live text updates, and a minimum update takes 634 bytes as a page entry. The idle rule of [§22](#snapshots), which any writer device carries out after 5 minutes if the last editor went away, leaves a note you edited and left with at most 8 live updates, 8,802 bytes in all against 3,730 for a file operation. If all 95,000 notes were in text mode, a new device's bootstrap would grow from 339 pages to at most about 836 MB, or 798 pages. Without the idle rule, a note left one record under the volume threshold would keep 511 updates, about 328 KB, and the same workspace's live set could reach 31 GB. The soak test in [§32](#store-and-soak-tests) checks live-set bytes per object.

### Moving a folder of attachments

Moving a folder that holds the 5,000 attachments to another folder of the workspace domain writes 5,000 state operations on the same `rid`s with new paths, about 3.3 MB in four pushes, and re-uploads nothing: each operation reuses its `blob_ref`, so other devices download no attachment again. Moving the same folder into a restricted domain writes 5,000 new state operations there and 5,000 deletes in the workspace domain, about 6.5 MB, committed one object pair at a time with many pairs per push. It re-encrypts and uploads all 10 GB, and each grantee device downloads it again.

### Download pacing

At the default bootstrap allowance of 512 MiB a minute ([§26](#rate-limits)), the join downloads its 10 GB of attachments in about 20 minutes, and its 339 pages of live operations take less than a minute of allowance. A limit counted only in requests would hold it back: at 120 requests a minute, the 15,000 attachment ranges alone would take 125 minutes.

## 29. What the server stores

| Data | Fields | What it reveals |
|---|---|---|
| Accounts | account ID, derived from the sign-up email, email, passkey credentials, email confirmation state, reset codes until used or 10 minutes old | who you are |
| Account chains | every link of each account's chains, retired ones included | which cards each account added and removed, in order, which device added each, and which card is the recovery identity; a member of a workspace you share reads in full the chain it names for you, and anyone who reads the policy of a workspace that records your account, former members included, reads your chains up to the heads it records |
| Sessions | token hash, account, device; no expiry | that a device signed in |
| Magic links, device grants, enrollment challenges | token hashes, account, expiry, and for a device grant the user code's hash, the polling interval and the earliest second the next poll may arrive; deleted when used or after 10 minutes | that you're signing in |
| Devices | enrolled cards (account, device ID, public keys) and which sessions belong to which card; whether a card is active comes from its account chain, and whether it is a device of a workspace from that workspace's policy | how many devices you enrolled and which hold a session |
| Pending approvals | approval requests with their commitments, offers and reveals until they expire | that a device is waiting for approval |
| Server | the restore generation, the secret's key check, and the stamp `backup` writes into a copy; outside the database, the server secret | when the server was last restored, since the generation is a Unix time |
| Workspaces | workspace ID, owner account, per-domain usage and history | that the workspace exists, and how much data it holds |
| Policy changes | full signed records with the account heads each records, also as entries in every domain's log | members, roles, the head of each member's account chain the workspace follows, restricted domains and their grants, in order |
| Domains | domain ID, kind (workspace or restricted), marker `rid`, current epoch, rotation state, history floors, the highest published retention floor | how many domains exist and who reads each |
| Chain heads | per device and domain: counter, digest, sequence | the last position each device wrote |
| Epoch records and envelopes | full signed bundles with their recipient lists, also as log entries in the domains they cover; envelopes of current epochs; open re-wrap requests; the key sequence | when each domain rotated, in log order, which devices held its key, and which device's envelope failed |
| Operations | domain, sequence, device, counter, previous digest, epoch, policy revision and head, restore generation, the checkpoint the writer had reached, `rid`, the delete flag, supersedes, blob IDs, nonce, padded ciphertext, signature, digest | which device changed which random ID and how often; which operations delete an object; concurrent edits and incremental updates; padded payload size; how far each writer lagged behind the latest checkpoint |
| Checkpoints | full signed records, with the previous checkpoint's digest and the floor, and chain heads, as log entries; bundles and bucket hashes of every checkpoint at or above the retention floor; every checkpoint record while the domain exists | the number of live objects per domain, and how many checkpoints the domain took |
| Blobs | blob ID, the domain it was created for, uploading device, padded ciphertext length, ciphertext digest and segment hashes, upload state, referencing operations; a deletion time only while the upload is incomplete or unlisted after completion ([§19](#19-compaction)); the blob store's file times or `LastModified`; for 7 days after a restore, store objects the database doesn't know, under a quarantine prefix | padded attachment size; that an operation refers to a large file; when each attachment was uploaded |
| Share links | token hash, workspace, domain, creating device, expiry, signed padded snapshot | that a domain has a shared item, which device shared it, and from its expiry roughly when |
| Invitations | while open: ID, token hash, workspace, role, inviter, accepting account, status, expiry, padded context | who is inviting whom, with which role, until the invitation ends |
| Workspace profile | signed encrypted record | when the name changed |
| Rate counters | key, window length, count and the end of the count's window; the server deletes a count within a minute after its window ends | request volume, and which keys sent requests in the current window |
| Presence | nothing stored | |

No stored table holds an object type, a per-record timestamp or an unpadded size. The blob store's file times date uploads, as the creation-times note below says.

- **Object types.** `rid`s are random, payload kinds are encrypted, and paths, file names and frontmatter stay inside ciphertext. Two patterns show how an object is edited, not what it is: operations that cover nothing mark a note in text mode, which any Markdown type (note, task, project, chat) can enter, and a blob ID marks content above 256 KiB, a size class the padded length would show anyway.
- **Deletes.** Each operation carries a cleartext delete flag, so the server learns that an object was deleted and where in its log, and prunes the tombstone later ([§19](#19-compaction)). A blobless operation of minimum padded length that covers everything already marks most deletes. The flag shows nothing of the object's type, creation time or size.
- **Creation times.** Operations, `rid`s and checkpoints carry no timestamp, retention counts bytes, and checkpoints come by volume, so the server keeps no table that turns a sequence into a date. The blob store dates each attachment's upload: a filesystem store through its file times (ctime, birth time), which no program can set, and an S3-compatible store through each segment's `LastModified`. The restore generation dates the last restore, not a record. Sessions carry no expiry, the server deletes sign-in grants, challenges and magic links within 10 minutes, and PostgreSQL runs with `track_commit_timestamp` off, so no table dates a record. Logs beside the tables do. PostgreSQL's write-ahead log stores each commit's time, so `pg_wal` and any WAL archive kept for point-in-time recovery date every push to the second, and `pg_waldump` reads the times out. SQLite's write-ahead log holds no times, but the object times of a replica that streams it, as Litestream does, date each batch of commits. An operator who wants nothing on the server to date records keeps WAL retention short, archives no WAL, and backs up with logical dumps or `backup`; one who runs point-in-time recovery accepts the leak. A series of backups dates each record to the backup interval. The server sees when an operation arrives while it handles the request, and it doesn't store that time. A `rid` first appears when an object first syncs: for files that existed before you turned sync on, that's the day you turned it on. Share expiries date the share, never an object, and the server deletes an invitation's row when the invitation ends.
- **Sizes.** Every ciphertext the server stores is Padmé-padded, and the server rejects unpadded lengths. Policy, epoch and envelope sizes depend on member, device and domain counts, never on content.
- **Moves between domains.** A cross-domain move commits the old `rid`'s delete and the new `rid`'s create in one push, so the server can link the two `rid`s and carry the object's edit history across domains. The moved attachment gets a new blob ID, which removes the exact match.

Logs keep route templates, request IDs and status codes, for at most 14 days. They never hold account IDs, record bodies, tokens, fragments, `rid`s or ciphertext.

## 30. Threats

### What a malicious server or relay can do, and what the client detects

| The server | Detected? | How |
|---|---|---|
| Drops a record inside a device chain | yes | `sync_chain_gap` |
| Reorders a device's records | yes | `sync_chain_reordered` |
| Replays a record at a new position | yes | `sync_chain_replayed` |
| Serves two different records at one chain position, including two re-seals of one attempt | yes | `sync_chain_forked` |
| Drops the newest records of a device | when evidence arrives | later records, checkpoint heads, the checkpoint each record names, your own receipts |
| Leaves a checkpoint out of one client's log while other records still cross | yes, once a later checkpoint or a record whose writer reached it crosses | checkpoint `previous` links and the checkpoint each record names, `sync_chain_gap` |
| Forges an operation, policy change, account link, epoch record, envelope, checkpoint, presence record or share snapshot | yes | signatures under cards from account chains, or the link's verify key |
| Inserts its own device key | yes | a card enters only through a link of its account chain, which an active device signs after a matching approval code or a recovery identity signs once |
| Plants a device in a member's account, with an owner or admin | yes | only an account's own active devices sign links that add its cards; a policy change records another account's head only in `add_member`, which the member's request committed to, in `accept_reset`, after an approval code comparison with the account's person, or in `account_heads`, whose later head holds only cards the account's own devices signed in; the member's devices alert on any link of their chain they didn't witness |
| Reuses a device ID under another card | yes | device IDs derive from the signing key, an account chain never adds one twice, and a workspace never makes one active under two accounts, `sync_account_unauthorized`, `sync_policy_unauthorized` |
| Lists an extra card in `GET /v2/devices` | harmless | devices come only from account chains; the app alerts on an enrolled card your chain doesn't list |
| Brings back a removed card, or forks or rolls back an account chain | yes, once a device that kept one branch sees the other, or a workspace log records a head on the other | a removed card never returns to its chain, every policy change records the heads it relies on, and each device refuses a chain that doesn't extend the head it keeps, `sync_account_forked` |
| Claims that an account reset its devices | shown to every member | no key reaches the new chain before an owner or admin accepts it after comparing an approval code with the account's person; the account's own devices alert |
| Serves a made-up workspace log, under a workspace ID a device knew or a new one, also one that a removed device of the account signed | yes | a device trusts revision 1 only through an approval anchor, its kit, having created the workspace, or a change there, at or after the device's reference head, signed by a device of its account that is still active at the account head the device keeps, `sync_workspace_unverified` |
| Swaps a card during approval | yes, except with probability 10^-8 per attempt | the candidate's committed nonce keeps the two codes independent, and the candidate uses each nonce for one offer |
| Returns an older or different policy chain | yes, after first contact | `sync_policy_rollback`, `sync_policy_forked` |
| Shows two devices different changes at one policy revision | when a record crosses | `policy_head` in every record, `sync_policy_forked` |
| Gives recipients of one epoch different keys (with a malicious member) | yes | `key_check` |
| Lists an unauthorized recipient in an epoch | yes | recipients checked against the verified policy |
| Serves a manifest that omits an object | yes, with an honest signer | the recomputed root differs from the signed checkpoint |
| Rolls a device back to an older checkpoint | yes | `sync_manifest_rollback`, against the latest accepted checkpoint and the anchors in approvals and kits |
| Seeds a new device from a checkpoint that a removed device signed, placed before the removal's entry, or from a checkpoint off the anchor's chain | yes, except a checkpoint off the chain in a restricted domain of an approved device, which holds no anchor there | the bootstrap checkpoint must descend from the device's anchor, and no removal in the device's verified chain may postdate its `policy_revision` for its signer or a device in its heads; in a restricted domain without an anchor, the signer and recency checks still apply, and a checkpoint off the chain that a writer signed is a colluding writer's (see "A colluding writer" below), which the device detects when the next real checkpoint arrives and doesn't name it as `previous` |
| Claims compaction or lowers the retention floor to force a bootstrap from an old checkpoint | yes | the published floor never drops within a generation, and the bootstrap checkpoint must cover the client's cursor |
| Serves a live operation at different log positions to different clients | when a checkpoint arrives | sequences in leaf refs make the roots differ |
| Shows two recipients two different epoch records for one epoch (with the rotating member) | when either sees the other | the epoch record chain, checkpoints naming the epoch record, records that stop decrypting |
| Accepts writes from a device that lost write access | yes, for a record after the change's entry; for a record the server places before that entry in one client's view, when the remover's receipts or a checkpoint covering the entry reach that client | the policy entry in each log marks where write access ends, `sync_writer_not_authorized`; the remover's receipts and the final head that the first checkpoint covering the entry lists, `sync_manifest_mismatch` (see split views below) |
| Commits after a removal an epoch record that a former reader, or the removed device, signed at an older revision | yes | rotations are judged by revision, so the rotation stays pending; recipients refuse its envelopes, `sync_key_mismatch`, and the remover rotates past it with a replacement |
| Commits an epoch record whose lineage or key check fails, or envelopes that fail for some recipients (with a reader) | yes | `sync_key_mismatch`; a reader replaces the epoch once every device that used it, besides its rotator, lost read access to the domain through a change at or below the replacement's `policy_revision`, and until then devices that hold the key re-wrap it for those that don't |
| Leaves a policy change's entry out of one log | yes | a client that accepted the change requires its entry, `sync_chain_gap` |
| Keeps writers from seeing a removal, or refuses the change that records an account's removal, so a removed device keeps reading their new records | a device that knows of the removal writes nothing under the old keys and alerts you while the change fails; for a writer that never learns of it, only after the leak, when one of its records or a later checkpoint reaches a device that accepted a policy change requiring the rotation | the hold on an unrecorded removal ([§9](#rotation)); the epoch rule, `sync_epoch_stale`; until then, each device's last-heard-from time |
| Claims a restore so clients accept lower heads and checkpoints | shown to you | every generation change raises an alert; clients accept no loss of an account link, policy change, epoch record or envelope they accepted and stop instead, `sync_restore_lost`; they rewind to a checkpoint they checked, or stop when the server serves none of those; they refuse, before any file changes, a record of a device whose write access a change ended that doesn't lead to the final head they kept for that change, and a chain or checkpoint head that differs from it; they write lost content again from their files |
| Goes back to an older state without raising the generation | yes | `sync_server_rollback` when a head, `checkpoint_seq`, chain head or checkpoint falls below one the client saw in that generation; `sync_policy_rollback` or `sync_account_forked` for a lower policy or account head |
| Takes over an account or device ID after a restore | yes | account IDs derive from the sign-up email under the server's secret and device IDs from the signing key, and a new chain for an account needs an `accept_reset` in each workspace |
| Forwards an old presence record | yes | `minute`, the current epoch, the per-session sequence |
| Forges or drops realtime notifications or records frames | harmless | both feed the same verification as pulls, and a gap sends the client to pull |

### What the client can't prevent

- **Withheld service.** The server can refuse requests, freeze a domain at an old state, or delete ciphertext. Your files stay on your devices.
- **Split views.** The server can show different clients different subsets of records. Clients detect the split once any evidence crosses it: a record, which names its writer's policy head and latest checkpoint, a checkpoint, a receipt or a policy head. A server that partitions clients completely and forever goes unnoticed. While the server keeps a writer's records from every device that accepted a removal, the removed device reads them without limit. Detection fires only when one of those records, or a later checkpoint, reaches such a device, which is after the leak; during the partition, the app's last-heard-from time for each device is the only signal. In the same way, only the remover's receipts fix where a removal's entry sits. The server can place a removed device's later records, signed at an older revision, before that entry in another client's view, and that client applies them until the checkpoint that covers the entry, signed by a device that saw the true order, reaches it and lists a different final head. Withholding that checkpoint means withholding every later checkpoint and every record whose writer reached it. After a restore, a client that hadn't reached the entry before it keeps no final head for the change ([§14](#after-a-server-restore)) and is in the same position.
- **Timing and traffic.** The server sees when you sync, how many operations you send, padded sizes, which random IDs change together, your IP address and your account email. Text updates show which `rid` each writer edits, to the 500 ms tick, and presence channels show which devices have the same note open.
- **Membership.** Every member and the server see the policy: members, roles, devices, domain kinds and restricted grants. Members outside a restricted domain learn that it exists and who holds it, never its folder name or contents. Members of a workspace you share also read your whole account chain: every device and recovery identity, and the order in which you added and removed them, including devices you use only for other workspaces. Anyone who verifies a workspace's policy, such as a later member, reads your chains up to the last head that policy records, after you leave it too.
- **History for new members.** A new member's domain keys open older ciphertext in those domains through lineage. The history floor keeps the server from serving it, and a malicious server could serve it anyway. A text-mode note also reaches a new member with its live patches, up to the snapshot thresholds of [§22](#snapshots), and those can hold text deleted since. A file you move into a restricted domain leaves its earlier versions in the source domain's log, under keys that the source domain's current and future members hold, until the floor passes them; the app warns you when you move it ([§9](#moving-objects-between-domains)).
- **Access granted again.** A member who loses access and later gets it back can read what was written while they were away ([§9](#rotation)). The app warns whoever invites or grants again. A device removed from its account chain never gets back in.
- **Pending rotations.** While a rotation is pending, the server accepts no record or presence in that domain, so nothing new uses the old key. Until a device that holds the key rotates it, which each does within about a minute of accepting the change, the server alone keeps the domain's existing ciphertext and keys from the removed device. If every device that holds the key is lost before one rotates, readers that a change brought in without an envelope never get the key, and the domain's content survives only in devices' files.
- **Old share snapshots.** The viewer warns when a later visit shows a lower `snapshot_revision`, but a first visit can't tell a current snapshot from an older one the server kept after Update link.
- **Served web code.** The server serves `account-web`. A modified viewer page could read the fragment and leak a share link's keys. A modified invitation page could read the secret of an invitation link you opened in a browser, and with it the workspace and inviter names and the means to forge an invitation proof; the approval code still stands between it and your keys. The desktop app handles invitations without the browser and doesn't depend on served code.
- **Blame for forks.** A server can't fake a fork from a re-seal, because each attempt names the attempt it replaced. Two records at one position that aren't consecutive attempts come from the device's key; clients can't tell whether the device misbehaved or a stolen key did.
- **A colluding writer.** An editor who colludes with the server can sign a checkpoint that omits objects, and a new device bootstrapping from it misses them. Current devices reach that checkpoint as a log entry and detect the mismatch against their own root; returning devices hold missing files for review instead of deleting them.
- **A new member's first device.** A new member's devices are the active cards at the account head that `add_member` records, which the member's approval request committed to. The server checks that the member's own sign-in enrolled the card; clients can't check that, so everyone relies on the inviter's code comparison with the new member, and the member's own device checks that `add_member` records its head. A member who lost every device and their kit resets their devices, and accepting the reset drops their grants and manager entries, so a manager grants restricted access again only after checking with them ([§7](#resetting-your-devices)).
- **A stolen device.** Until you remove it, a stolen unlocked device, or malware on it, holds the same authority as your other devices. It can add devices and a recovery identity to your account chain and sign workspace changes within your role, and your other devices alert you about each such link and change ([§7](#device-list)). Removing it ends its sessions and takes down every card it added, directly or in turn, in one link ([§7](#removing-a-device)), but undoes no workspace change it signed: members it invited, roles, grants and owner transfers stay until a member whose role allows the undo reverses them. After an owner transfer to the thief's account, only that account can reverse it or demote the admins it then makes, so you leave that workspace, turn on sync for your folder as a new workspace and invite the members again. The thief can also strike first and remove your other devices and your recovery identity. You then reset your devices with an email check, and each workspace takes your new chain only after an owner or admin accepts it; a workspace your account alone administers you set up again from your files. A recovery identity has no precedence over your devices, because a chain has no clock that could delay a removal, so remove a stolen device as soon as you can.
- **Recovery after first contact.** A recovering device trusts the account head and policy heads stored in its kit. A server can withhold links and changes made after the kit was written, such as the removal of a stolen device, and show links and changes that the stolen device signs instead, until another device's records reveal them. In that view the stolen device is still active, and a workspace that the kit doesn't list may be a log that the stolen device signed for the server, so the recovery flow shows you every link and change after the kit's heads and every change and member of such a workspace, removes every other card of your account before it rotates, and lists members added after the heads for review ([§12](#12-recovery-kit)). After the flow, a card that the recovery removed anchors no workspace. Exporting a new kit after each removal keeps the stored heads past it.
- **A restored machine.** A restore of a whole machine from backup can roll back both the journal and the credential store, and the device then can't tell. The server's heads and the checkpoints are its only check. The device may then sign a counter again whose first record never reached the server, and a server that kept that first record can later show readers a fork. A journal rolled back alone costs nothing, because the device starts a new counter block at each start ([§14](#your-own-chain)). A device whose credential store rolled back may also reuse a load number and its clocks; the canonical order resolves the duplicate the same way on every device, and its later edits may lose to its earlier ones, which it lists for review.
- **Restores.** A restore that lost an account link, policy change, epoch record or envelope stops every device that accepted it, and nothing brings it back automatically. One that lost every checkpoint a device reached in a domain stops that device in the workspace too. Set up sync again starts a new workspace whose members, grants and share links start over, with older history only in each device's files and journal, and Reset devices needs every device approved again and an owner or admin's acceptance in each workspace. Devices that never accepted the lost state keep syncing with the restored server, which may still let a removed member read, until an owner closes the workspace or the account resets; after a restore the operator didn't record, they stop only once evidence of a lost position reaches them. Restoring the wrong backup, or two backups out of order, causes the same stops.
- **Resets.** Someone who controls your email inbox can reset your account, so every device of yours loses its session and must be approved again, or you reset back. They read nothing until an owner or admin accepts the reset after comparing an approval code with the person asking; one who compares it with the inbox thief instead of you lets the thief's devices in under your account and role. A malicious server can claim a reset on its own, and every member's devices show the change. Until a workspace accepts a reset, the old chain's cards stay devices there without sessions.
- **A removal that hasn't reached a workspace.** Until a workspace records a `remove` link, or a `remove_member`, `accept_reset` or `set_access` takes the removed cards out of a domain's readers, and that domain rotates, a device that knows the link's head writes nothing in the domains the removed cards read, and the app alerts you while the server refuses the recording change ([§9](#rotation)), so only devices that never learn the head keep writing under keys the removed cards hold. An honest server gives those cards no session; a server that serves them and hides the head from a member's devices lets them read what those devices write. In the same way, a device of your account that hasn't yet accepted a `remove` link can still trust a workspace log that the removed card signed ([§8](#verifying-the-chain)). A malicious member can also fill a domain to 512 readers with its own cards, so that a later head of another account that brings in more cards than it takes out can't commit there, and a removal behind it waits until an owner or admin removes that member or room frees ([§8](#account-heads)); reading what is written meanwhile still needs a server that serves the removed card.
- **Logs and backups.** PostgreSQL's write-ahead log and its archives date every push to the second, and a streamed SQLite replica's object times date batches of commits ([§29](#29-what-the-server-stores)). Whoever holds them learns when each record arrived.

### Members and devices

- A member writes within their role. A malicious editor can delete objects or write records nobody can read; other devices keep their local copies and list unreadable records for review. A member rotates a domain only in a required rotation or a replacement, and a replacement skips only epochs that no device used besides their rotators and devices that lost read access to the domain through a change at or below the replacement's `policy_revision`. A reader can therefore skip an honest rotator's epoch before another device writes under it, and do so again while writers stay idle, up to its rate limit: writers re-seal, and the rotator writes its records under the skipped epoch again, which other devices list as unreadable meanwhile, as a conflict branch where another device covered them ([§9](#epochs)). A reader that rotates with a key that fails for some recipients delays them until a replacement or a re-wrap. When another device of its member, or a colluding member, writes under that epoch, no honest device holds the key, so those recipients stop in that domain until you remove every device that wrote under it; a replacement then skips the epoch, and those devices' records under it stay unreadable.
- A writer can put elements under another writer's client ID into a text snapshot, with other content or origins, or show as not deleted an element that a record it covers deleted. Every device reaches the same body, the snapshot's, because a device that held the other copy rebuilds the note in canonical order; that device lists the note for review with the snapshot's signer. A device that never held the other copy, such as one that bootstraps afterward, shows the snapshot's text without a review entry. The writer could change that text with an ordinary edit anyway. A text update can insert only under its signer's own client ID, which the workspace's policy assigns.
- A device removed from its account chain keeps everything it received, can read nothing encrypted after the rotation, and never returns. A removed member invited again reads, through lineage, what was written while they were away.
- A device that forks its own chain halts that domain for readers until someone removes its write access; readers then keep the branch whose head the first checkpoint covering the removal's entry lists ([§14](#verifier)).
- Anyone holding a recovery kit and its secret can read, with no device and no alert, every current and future record of every domain your account reads, given the ciphertext from the server, a server backup or your account session, because rotations keep wrapping keys for the recovery identity. They can also recover one device into your account chain. That device is a full device: it reads, approves your later devices and removes your other devices and the recovery identity. All of this lasts until you revoke the kit by removing the recovery identity, whose `remove` link also takes down the device it recovered unless you keep it ([§12](#12-recovery-kit)).

Adversarial agent reviews are this protocol's security review, and no product copy may claim an audit.

## 31. Comparison with SuperSync

SuperSync is the sync service of Super Productivity (MIT, `LICENSE` at the repository root, copyright 2018 Johannes Millan). This comparison reads [johannesjo/super-productivity](https://github.com/johannesjo/super-productivity) at commit `78139c531c808bab7c46ef96fd0fbb0a3b6f7d6d`.

### Operation log

SuperSync stores operations in PostgreSQL with a dense per-user `server_seq` (`packages/super-sync-server`). An operation carries a UUID v7 ID, a client ID, action, operation and entity types, entity IDs, a vector clock, a wall-clock timestamp and a schema version, all in cleartext; only the payload can be encrypted (`packages/shared-schema/src/supersync-http-contract.ts`). Uploads carry up to 100 operations, and the server deduplicates by operation ID and by a request ID cached for 5 minutes.

v2 adopts the server-ordered append-only log, batched uploads, idempotent retries and a cursor written after durable application. It differs in four ways: the log is per domain, an operation's identity is its chain position and digest instead of a time-ordered UUID, every type and entity ID stays inside the ciphertext, and each operation is signed and chained. A push is atomic, so the request-ID cache has no job for operation pushes; the other create routes are idempotent on their client-chosen IDs instead ([§26](#26-http-api)).

### Cursors

SuperSync clients pull with `GET /ops?sinceSeq&limit` and page with `hasMore`. The server reports `gapDetected` when the client is ahead, the range was purged, or a page has a hole, and the client restarts from zero (`operation-download.service.ts`, `getOpsSinceWithSeq`). A cursor behind compaction jumps to the newest full-state operation.

v2 adopts paging from a cursor and the snapshot-plus-tail recovery. It differs because a server-computed gap flag asks the client to trust the server: v2 clients detect gaps from signed chains, and a cursor behind compaction gets a compacted page and bootstraps from a signed checkpoint whose Merkle root the client verifies.

### Conflicts

SuperSync detects conflicts per entity with vector clocks, on the server at upload and on the client against an entity frontier (`conflict.ts`, `entity-frontier.ts`). The server rejects concurrent uploads. The client resolves by last write wins with a merge of disjoint fields, and the losing version is overwritten (`conflict-resolution.ts`, `planLwwConflictResolutions`). Clocks are pruned to 20 entries (`vector-clock.ts`, `limitVectorClockSize`), and the project tracks a case where pruning hides a real conflict.

v2 can't detect conflicts on the server, because entity IDs are encrypted. Supersession vectors name only the heads an operation covers, so they need no pruning, and both versions stay for review. v2 adopts the merge of disjoint changes, the rule that identical branches aren't a conflict (`isIdenticalConflict`), and a deterministic tie order, using operation digests where SuperSync uses client IDs.

### Compaction

SuperSync treats a full-state import or repair as a snapshot operation. A daily job deletes history before the newest snapshot once it is older than 45 days, and a 100 MB quota per user deletes old snapshots (`storage-quota.service.ts`, `deleteOldSyncedOpsForAllUsers`). Clients don't create periodic snapshots. A new device downloads the newest snapshot and the tail.

v2 adopts a retention window behind a snapshot boundary and the snapshot-plus-tail bootstrap. It differs in what a snapshot is and how the window is measured: writer devices sign manifest checkpoints by volume, and the server deletes covered operations and lone deletes below a retention floor that keeps `retained_bytes` of log behind a checkpoint. v2 measures the window in bytes instead of days, so the server keeps no date table. Text snapshots play the role of SuperSync's full-state operations for one note.

SuperSync refuses periodic full-state boundaries because older releases dropped concurrent edits at a repair boundary (`packages/super-sync-server/docs/architecture.md`, issue #9962). v2's text snapshots keep that lesson: a snapshot covers only the patches it includes, and live patches of the same text lineage that it left out still apply ([§22](#22-live-collaboration)).

### Encryption and integrity

SuperSync encrypts payloads with AES-256-GCM under an Argon2id key from a password, without associated data or padding, and keeps no signatures, hash chains or Merkle structures. Its documentation lists reorder and replay as open. v2 shares none of this design.

### Code a later implementation could adapt

v2 plans to adapt no SuperSync code. The units closest to v2's needs, all MIT at the commit above:

| File | Units | Possible use |
|---|---|---|
| `packages/sync-core/src/conflict-resolution.ts` | `isIdenticalConflict` | identical-branch rule |
| `packages/super-sync-server/src/sync/services/operation-download.service.ts` | `getOpsSinceWithSeq` | paging and compaction-jump structure |
| `packages/super-sync-server/src/sync/services/storage-quota.service.ts` | `deleteOldSyncedOpsForAllUsers` | retention cut behind a snapshot |
| `packages/sync-core/src/vector-clock.ts` | `compareVectorClocks`, `mergeVectorClocks` | cover comparisons |

An implementation that adapts any of them records the repository URL, exact commit, source path, license, destination, changes, notice obligations, reviewer and date under `docs/provenance/` before the code lands, and adds the MIT notice to `THIRD_PARTY_NOTICES.md`.

## 32. Fixtures

Fixtures live in `docs/workspace-format/fixtures/`. Each holds accepted cases and rejected ones: a bad signature, a wrong tag, a wrong version and a truncated field, plus the cases listed. Fixture keys are public test values and never secret.

Each file lists `records` and `cases`. A record gives a type, its bytes in hexadecimal, its fields as JSON when it takes 16 KiB or less, its digest, its signer from `keys`, and `expect`: `ok`, a decode error, or a failed signature. A rejected variant names the accepted record it comes `from` and the `mutation` that makes it: `truncate` drops final bytes, `append` adds bytes, `retag` replaces the leading tag, `version` rewrites that tag's version, and `flip_last_byte` flips the low bit of the signature's last byte. A case's `kind` names the check it runs, and its `expect` the outcome. Cases name account chain links by digest from the file's `links`, and account chains by their last link's digest from `chains`. The test `v2_fixtures` in `crates/sync-key-envelope` generates the files and fails when they differ from the code; run it with `NOURA_WRITE_FIXTURES=1` to write them. The `v2_fixtures` tests of `sync-protocol` and `v2_fixture_hpke` of `sync-key-envelope` run every record and case.

| Fixture | Pins |
|---|---|
| `sync-v2-encoding.json` | field types, tag bytes, decoding rules, `opt` discriminators, record tags checked against their position, every message body, error code, HTTP status and `detail` in [§26](#26-http-api), each pull page status with the other domains' pages still returned, every realtime frame including an error frame for refused presence and a refused subscribe, and a records frame applied, ignored or answered with a pull for each cursor position around `previous_seq` and inside the frame, the general field limits of [§4](#decoding-rules) and each field's limit, the `Noura-Restore-Generation` header, `checkpoint_due` in pages and frames, `not_found` for an absent head or checkpoint, `live` paging by `rid`, the bodies of `missing-blobs`, `checkpoint-chain` with its `to`, `keys/current`, `rewrap`, a re-wrap request's key entry, `accounts/chain`, `accounts/chains` with a retired chain, `account/reset`, and `policy` with its current account heads, the `account_changed` frame, and the `device_removed` detail; lists with a duplicate sort key, such as an `access` list naming one account twice, rejected; the RFC 8628 codes with status 400; an approval request for the account's own device, an invited device and a reset device, with only the first reaching `GET /v2/approval-requests`, and requests that name both an invitation and a workspace, neither with an account head, a workspace without one or an invitation without its proof, each `bad_request`; and a records frame whose `previous_seq` is 0 |
| `sync-v2-padding.json` | `padded_length` for boundary lengths, malformed padding |
| `sync-v2-ids.json` | remote ID format, the text forms of [§5](#5-identifiers): 16-byte values in 22 characters, 32-byte values and the session token in 43, decimal integers, a `live_cursor` as base64url of its encoding, device IDs derived under `noura.sync.device.id` from a card, and a card whose ID doesn't derive; card indices assigned at one change to cards of two accounts; an account fingerprint; and, from a test server secret, its key check under `noura.sync.v2 secret check` and the account ID of a mixed-case email under `noura.sync.v2 account id`, with non-ASCII emails whose non-ASCII letters keep their case, and the device page's confirmation under `noura.sync.v2 device confirmation`, which another code or expiry changes |
| `sync-v2-device.json` | `noura.sync.device.card` with an X-Wing key, device fingerprint, `noura.sync.device.enroll` (wrong origin, account and key), an enrollment that binds a new session to an enrolled card, one refused for a card that a link removed or a reset retired and for a device ID held with other bytes, a `device_removed` answer naming the removing link and a removed card reading its chain, and a pending card's sessions ended from an active device and not from a session alone |
| `sync-v2-account.json` | `noura.sync.account` and each link kind; chain links; a genesis accepted only for an account without a chain or with a reset code; authority failures: a link from a removed card, a recovery identity signing anything but `recover_device`, a second `recover_device` from one identity, a second active recovery identity, a 257th active card, an `add_device` of a device ID that a chain of the account listed, and a card naming another account; removal cascades: a removal two levels deep, a `keep` that spares a device and its cards, a `keep` naming a recovery identity, refused, a signer inside the removed subtree that stays with its cards, a self-removal that leaves the device's cards active, a kit replacement that takes down a device the old kit recovered unless kept, and a recovery that removes every other card; kept heads: a chain that extends the kept head, a fork at one seqno, a chain cut back below the kept head and a head recorded in a workspace off the kept chain, each `sync_account_forked` and stopping every workspace shared with that account; links after the kept head: one whose `previous` names another digest, right after the kept head or later, `sync_account_forked`, and one from a card that isn't active or that skips a seqno, `sync_account_unauthorized`, each stopping the workspaces shared with that account, or every workspace for the device's own account; resets: a new genesis refused until a workspace genesis, `add_member` or `accept_reset` names it, the alerts on your devices and on members' devices, the new fingerprint, and two workspaces naming different chains of one account, which raises the alert; the alert for each link you didn't witness; and an approval's `add_device` directly after the offer's `approver_head`, with one after another link refused with `account_conflict` |
| `sync-v2-approval.json` | `noura.sync.approval.commit`, `noura.sync.approval.offer` with one workspace-domain checkpoint anchor and its generation per workspace, an offer for 1,000 workspaces within 1 MiB, an anchor compared across generations, `noura.sync.approval.code`, a reveal that doesn't match its commitment, a second offer for an already revealed commitment, ground to match the approver's code, that the candidate refuses, an approver that ignores further requests while an attempt is open, `noura.sync.v2 invitation proof`, a commitment and offer carrying the candidate's account head, a swapped head refused at the reveal check, and an offer naming the approver's account head; a reset device's request naming a workspace, refused from an account whose chain that workspace already records, answered by an owner or admin of another account with an offer anchoring that workspace and committed as `accept_reset`, after which the reset device trusts the workspace, while before it, and for a forged genesis served under a known workspace ID, it reports `sync_workspace_unverified`; and the reset device's sibling, approved before the acceptance, which trusts the workspace once the reset device records the account's head there again; and your own new device's head recorded first by another device of your account, which the candidate accepts |
| `sync-v2-policy.json` | `noura.sync.policy` with its `heads`, each change kind, chain links, authority failures, a recovery identity signing a policy change, an `add_member` for an account that is already a member, a chain forked at one revision and served to two clients; account heads: a change whose heads lack the signer's account, record a head where the signer isn't active, record a head behind the workspace's or off the account's chain (`account_conflict`), or record another account outside `add_member`, `accept_reset` and `account_heads`; a sign-out head recorded by another member's device; an `account_heads` change that brings in a card without its envelopes, refused, and one that also takes a card out, which needs none in a domain its signer couldn't read and needs them in one it could; an interrupted approval, where a device approved a new card and was removed before it recorded the head in a workspace the new card never joined, and another member's device records the later head there; a domain at 512 readers where a later head that adds and removes one card commits and one that adds more than it removes is refused; a workspace log that nothing anchors, refused with `sync_workspace_unverified`, and one a device trusts through a later change that a device of its account, active at the head it keeps, signed; a backdated genesis or `account_heads` change that a card removed since signed, recording a head of the account at or after the reference head of a device that signed the account's genesis, refused by that device with `sync_workspace_unverified`; the second device of an invited account, a device approved in step 3 of a reset and one recovered from a kit exported then, each trusting the workspace once the joining or reset device records the account's head again; an account's head that another account's device recorded, recorded again by the account's own device once and refused with `forbidden` after that; an `account_heads` change that records no later head otherwise, refused; an `add_member` recording a head other than the one the request committed to, refused by the joining device; devices at a revision computed from recorded heads before and after a removal; the alert for a policy change other than `account_heads` that another device of your account signed, and the list of a removed card's policy changes with the undo the viewer's role allows; a removal recorded in one workspace and completed in the others by another device of the account; card indices across two accounts at one change, kept when a member leaves and comes back; a member removed and invited again whose devices continue their chains and who stays out of every restricted list until a manager grants access, and one invited again after a reset, whose `add_member` names its new chain and whose old cards leave the checkpoint heads; `accept_reset` by an owner and by an admin, as the commit of an approval that answers the reset account's request, refused without that request, from an editor and from the reset account, which takes the old cards out, drops restricted grants and carries the new cards' workspace-domain envelopes; a device that joins a workspace whose early changes a since-removed member signed and reads that member's chain up to its last recorded head, and a member removed with `remove_member` that reads the remover's chain to verify its removal; heads that would make one device ID active under two accounts; policy entries in every domain's log, a record after the entry that ends its device's write access, an entry missing from one log, an entry at a sequence other than its receipt's, an approval into a domain whose last entry is a checkpoint, and a re-checkpoint after the latest signer is demoted; a change that gives a domain a 513th reader or the workspace a 16,777,216th card; chain reads: a device that joined later reading a since-removed member's chain up to its last recorded head, a member removed with `remove_member` reading the remover's chain up to its removal, a current member's current chain in full to the workspace's devices, a device outside the workspace reading none, a member reading no chain of an account the workspace never recorded, and, while that account's approval request names an invitation to the workspace (`approval_request`), a member reading its current chain in full, and neither a removed member nor an account outside the workspace reading it; a reset device that trusts a second workspace through an `account_heads` change its sibling signed there; and a lagging head that ends in a removal, recorded in steps one card-adding link at a time |
| `sync-v2-keys.json` | `noura.sync.epoch`, the epoch bundle and `noura.sync.epoch.recipients`, an epoch entry that makes a checkpoint due as a replacement and one that doesn't as a rotation, the epoch record chain, `noura.sync.key-check` with `noura.sync.v2 key check`, lineage, `noura.sync.envelope`, HPKE `info` with X-Wing, a signer outside the recorded heads, the epoch rule (an old-epoch record after a newer epoch entry, a record between a rotation-requiring policy entry and the next epoch entry, and the same cases after a bootstrap), an epoch entry after a removal whose `policy_revision` predates the removal, signed by a former reader and by the removed device itself, which leaves the rotation pending in the log and after a bootstrap, an envelope that the removed device signed, which recipients refuse, and the remover's replacement; an epoch record or envelope from a device that can't read the domain, refused by the server; a replacement after an epoch whose lineage or key check fails, accepted while only its rotator used that epoch; a rotator's rewrite whose covered record was pruned, listed for review without a write, and one that also covers the rotator's later live operation; the same epoch after the rotator's member's second device wrote under it, where the replacement is refused with `epoch_conflict` while that member is active and accepted after its removal, and both devices' records under the skipped epoch count as unreadable; a replacement whose `previous` names the latest epoch, refused; a replacement that skips an honest rotator's epoch before anyone else wrote under it, after which the rotator writes its records under that epoch again, also after another device covered its state and patches there, where the rewrite stands as a conflict branch and as a snapshot that merges; another device's record under a skipped epoch, which holds the cursor; a re-wrap request answered by two readers, one of them with a failing envelope; a re-wrap schedule that the store conformance suite replays against each store, without records: a request for an epoch that isn't current (`epoch_stale`), a repeated request, refusals with `forbidden` for a device the server took no envelope for at that epoch and for one the policy doesn't let read the domain, `keys` and `keys/current` that list a request only to the domain's readers, three answers taken and a fourth and a second from one device refused, the request closed by the requester's `keys` fetch past its answers and by its `keys/current` read, asked again, left open while a rotation waits (`rotation_pending` to a new request), and closed when the next epoch commits, after which an answer at the old epoch is `epoch_stale` and one at the new epoch `forbidden`; a new card whose first envelope fails, which asks for a re-wrap at once, a newly added card that keeps a key holder's lone envelope after a malicious member's failing one, and a device that accepted an earlier epoch, which replaces it instead; a pending rotation judged by revision, with nothing left to rotate for a key holder after a rotation that committed at the change's revision before it saw the change, and the rotation still pending after a backdated epoch; an epoch entry and a record under it that arrive before the envelope and hold the cursor until it does; epoch records served without recipients and checked against the list a client computes; `keys/current`; an envelope that arrives through the key route before its epoch's entry, an epoch record with no rotation due, a restricted domain's epoch 1 from a device other than the `create_domain` signer, a grant without the new readers' envelopes; an `accept_reset`, a kit replacement, a recovery's head and an approval in a workspace where an earlier removal was never recorded, each taking a card out of a domain, committed with current-epoch envelopes for the new cards there when its signer could read the domain, accepted although the domain is due, and without them when it couldn't, with the rotation wrapping for them; a change that brings readers into a domain whose rotation was already pending, with their envelopes; a lone envelope in a pending domain, refused with `rotation_pending`; a kit replacement that another member records in a domain nobody writes, rotated by the key holder without a write; a sign-out by the last key holder of a pending domain, which rotates it first; a removal whose `account_heads` change the server keeps refusing, while the remover and a member device that learned the head encrypt, wrap and checkpoint nothing in the domains the removed card read until that change and a rotation commit; the same hold released instead by a `remove_member` or an `accept_reset` of the card's account, after which the device drops its pending push and the alert ends, and in a restricted domain alone by a `set_access` that drops the account there, each once an epoch of the domain at or above that change's revision commits; and a `recipient_limit` refusal that the verified policy confirms, after which they write as before; `noura.sync.v2 lineage key`, and cards a wrapping device must refuse: one outside the recorded heads, a card whose device ID the account chain lists with another HPKE key, and a device of an account the domain doesn't grant |
| `sync-v2-operation.json` | `noura.sync.op` with its `generation`, delete flag and checkpoint reference, a `generation` that decreases along a chain, `noura.sync.payload` for each of the four kinds, a delete flag that doesn't match the payload, AEAD associated data with the epoch record's digest, `noura.sync.v2 operation key`, path validation, an encoded payload of 262,144 bytes and one over it, `meta` entries in both text kinds, a payload whose kind doesn't match its class (a `file` that covers nothing on an existing `rid`, a `text_update` on a new `rid`, a delete that covers nothing), a counter that skips, and a `text_update` whose `load` is 0 or 2^29, unreadable |
| `sync-v2-chain.json` | schedules for gap, walks for checkpoints the log left out (two walked back through `previous`, one the server can't serve, a walk that misses a link, a served loop of `previous` links, and records naming a checkpoint below the floor, of an earlier generation or before the first), reorder, replay, fork, a retried upload, consecutive re-seal attempts served in either order, sibling attempts, an attempt that arrives after a successor of the other, a replay right after bootstrap, an unreadable record, a checkpoint that arrives after the client passed its `covered_seq`, a checkpoint entry left out of one client's log while other records still cross, a line of more than 4 re-seal attempts forced by rotations, a missing epoch entry, a record rejected with `snapshot_required`, `conflict_limit` or `quota_exceeded` and replaced at its position by a different record, a device returning after the floor passed its cursor, a removed device's head leaving the checkpoint heads, a device demoted, checkpointed, promoted and then bootstrapped past, whose chain continues from the head the checkpoint kept, a forked chain of a removed device resolved by the first checkpoint covering the removal's entry, keeping the branch served first, the one served second, the reader's own head, or a later sibling attempt of that head, a kept branch that forks below what the reader applied, which reverts the reader's records above the fork point, and refused when the kept head isn't among the records, sits at another sequence, or forks below the reader's latest accepted checkpoint, a chain that skips counters at each start, after a crash between raising the reservation and writing the journal, and after a journal rolled back, accepted; a skipping record whose predecessor the page left out, fetched as the device's next record and reported as a gap or reorder; and, after a restore: a writer that restarts its chain above every counter it signed, accepted by readers that rewound; a record from a device removed after the restore, refused by position; a record of a device whose write access a change ended before the restore, placed before that change's entry at a counter above the final head the client kept or re-signed at a counter below it, also with the re-signed record and the entry on different pages, where no file changes before the stop, and a checkpoint after that entry that lists another head, each `sync_restore_lost`; a generation change after which the server serves none of the checkpoints the client reached in a domain, where the client stops the workspace with `sync_restore_lost` and changes no file; a client that reached no checkpoint in the domain, which pulls from sequence 0 and fetches that device's records, counters 1 through the final head's, before it applies anything; a device with no record before the change, whose final head is zero; a device demoted, promoted and then removed, checked against both final heads; the same device's records accepted after a change grants it write access again; a pull page `head` below one seen in the same generation, `sync_server_rollback`, and a policy head or account head below the accepted one, `sync_policy_rollback` or `sync_account_forked`; and a removed member invited again, whose chain continues from the head the checkpoints kept |
| `sync-v2-live.json` | live sets and supersession over operation schedules (covers of covered operations, pruning, a lone delete left out at a checkpoint's `floor`, an edit against a delete), a state operation that leaves its own device's earlier state operation live, the conflict outcomes in [§16](#16-conflicts), path collisions including a file against a valid marker's folder, text lineage merges, and the idle and volume snapshot rules of [§22](#22-live-collaboration), including the idle snapshot that another writer writes after 5 minutes; a lone delete that a `floor` passed, then an edit on its `rid` that doesn't cover it, computed alike by server and client; an ordinary edit and a delete on a conflicted `rid`, which cover only the writer's branch; a reconciliation after a restore, where the server lacks a state that every writer applied and two devices write it again; and two identical live branches that one edit covers together; files matched without a journal after a lost journal, after Reset devices and in a folder connected to a workspace that was set up again, where a file that differs from every live state is held and uploads nothing until you keep the live state, keep this device's version, covering every live operation, or keep both under a new `rid` at `name (n).ext`; a text-mode note matched on its merged body, a managed note matched by ID at another path, which is held, and a conflict branch whose author device is gone, which connecting the folder keeps in the archive; and, after a restore, a file that differs from a rebuilt state the journal never applied, held the same way |
| `sync-v2-domains.json` | `.noura-domain` validity: planted, moved and evicted markers, a marker created inside a restricted root, a marker at a restricted domain's marker path, a later domain whose root holds an earlier one, an object of another domain under a restricted root written under ` (shared)`, a marker found in another domain's log, a marker with two live states, bootstraps that meet a moved, a deleted and a conflicted marker and record held roots, a file created during bootstrap before its domain's objects land, and a marker deleted locally that the client writes again, an object whose payload or merged `meta` path leaves its domain's root, kept at its last path and listed for review with no held root, a folder restricted while it holds workspace-domain files, which stay in place until their moves land, and an edit to a file under ` (shared)`, uploaded at its synced path; a folder synced without a journal, after a lost journal, Reset devices, Set up sync again and connecting a copied folder, whose local `.noura-domain` names a domain the device doesn't read or the policy doesn't list, held with every file beneath it and listed with the number of people who would see each, until its own domain's marker covers it or you confirm a domain; a held folder that an ancestor domain's marker, or another domain's marker at the same path, covers, which stays held; a hold that its own domain's marker ends, after which its files are matched before the domain's files are written there; and an object at a marker path that no domain names as its `marker_rid`, which the client refuses |
| `sync-v2-manifest.json` | `noura.sync.manifest.leaf` with sequences, the server's checks of a checkpoint at a replacement's entry, of one short of it and of one naming the epoch the replacement skipped, `.bucket`, `.node`, `.heads`, `noura.sync.checkpoint`, the heads digest check, a removed device leaving the heads and a demoted one staying, a member removed with `remove_member`, reset and invited again, whose old cards leave the heads, the same on each store for a demotion, a `remove_member`, an `accept_reset`, an `add_member` after a reset or on the same chain, and a member's removal of its own account that records a link removing one of its cards, and the heads rule on its own for a grantee cut back to read, `previous` links, `floor`, a `floor` that rises between two checkpoints and drops lone deletes from the root, a `covered_seq` equal to the previous checkpoint's entry sequence, roots for add, remove and change, a rolled-back checkpoint, a log-entry checkpoint whose signer was removed later, a bootstrap checkpoint whose signer was removed before its entry, refused, one whose signer was demoted after it, accepted, and one whose signer was removed after its `policy_revision`, refused until a later checkpoint commits, and a failed log-entry checkpoint that doesn't hold the cursor, a checkpoint whose `heads_digest` or `live_objects` differs from the server's, refused, and a bootstrap checkpoint that a removed device signed at an older revision and the server placed before the removal's entry, refused, as is one that doesn't descend from the anchor; `number` and `skip` links, a descent walk through `skip` to an old anchor, a checkpoint that races another under quota pressure and gets `checkpoint_stale`, a rewind after a restore to a checkpoint the device reached, which Recency would refuse because a removal the restore kept postdates it, followed by the due checkpoint a rewound writer signs, and a rewind refused to a checkpoint the device never reached; a bootstrap after a restore lost the approval's anchor, accepted under the other bootstrap checks, and a restore that lost the device's own latest accepted checkpoint, after which the device rewinds to an earlier checkpoint it reached |
| `sync-v2-blob.json` | `noura.sync.blob` header, the nonce prefix from `noura.sync.v2 blob nonce`, chunk nonces, lengths, `noura.sync.blob.parts`, a `plaintext_length` that differs from `L`, and a blob rebuilt byte for byte from its file and `blob_ref` |
| `sync-v2-collaboration.json` | `noura.sync.text.lineage` and the first snapshot of a lineage, written the same by two devices from one base operation and with different lineages from two, including managed notes with non-canonical frontmatter, a byte order mark and CRLF line endings; `meta` entries with a path and properties as canonical JSON, the YAML-to-JSON mapping, the file bytes they yield, and a note whose frontmatter falls outside the property rules, which stays out of text mode; concurrent writes to different keys, both kept, and to one key, won by the later; two writes of one key at one effective sequence, won by the higher record digest; a snapshot entry that keeps a covered write's sequence against a concurrent update it didn't see, a write a restore lost, written again as a new write; client IDs from a card index and a load number, one above 2^32 round-tripped through an update and the largest, 2^53 − 1, an update holding an item or a skip under another client ID and any record holding a GC struct, unreadable; raw checks: a client ID of 2^53, one of 2^53 plus an honest client ID, an 11-byte varint, a non-canonical varint, a clock past 2^32 − 1, a zero-length struct, a client listed twice in the struct section or the delete set, and invalid UTF-8, each unreadable, as are a trap in the verdict and a snapshot naming a card index above the workspace's count; canonical order: a snapshot that disagrees with an integrated update, with a pending one and with another snapshot, and one that covers an update whose elements it lacks, each rebuilt on the devices that held the other copy to the body a bootstrapping device computes, and two updates of one signer at the same clocks, the earlier one pending in another run, which every device resolves in chain order; frontmatter outside edits that change a value, remove a property, only reformat, or break parsing; yrs records near the verdict's fuel limit, which no record passes, and near and over its memory limit, with the same verdict and fuel under Cranelift and Pulley and on the previous and current release; the largest fuel per byte that legitimate records take, measured for the budget; base documents with records whose integration succeeds, overdraws a writer's allowance and defers its next record, traps, or sends the lineage to review; an update that depends on an unreadable one; records unreadable for a yrs item outside `content`, with a map key, an embed or a format attribute, and for a `meta` path that breaks the rules of [§13](#payload), an unknown key, a bad property name, a `p:` value that isn't canonical JSON, a `p:id` that differs from `object_id`, a removed `path`, `p:id` or `p:type`, a `p:` key on an unmanaged note, or an update entry with a nonzero sequence; a note's first text snapshot after its first `file` operation and after a `file` operation written because the file matched no live state, and a text-mode note moved to another domain as a `file`; a rebase at 16 MiB and a `file` operation at 4 MiB of text, with the record that jumps past 32 MiB stopping the lineage and the `file` operation that resolves it; a snapshot that leaves deferred and pending patches uncovered; a snapshot that covers a delete and shows its element as not deleted, rebuilt on a device that took in the delete, also after a restart; an item whose origin or right origin names its own clock or a later one, unreadable; an insert whose origin or right origin falls inside a surrogate pair and a delete range that starts or ends inside one, each integrating to U+FFFD in both halves alike on a device that integrates records as they arrive and on one that bootstraps from a snapshot, with the snapshot check finding no disagreement; an outside edit between two characters that share a high surrogate, diffed on code points; byte thresholds counted in ciphertext with blob-carried patches; and the hash of the verdict module |
| `sync-v2-presence.json` | `noura.sync.presence`, `noura.sync.presence.payload`, `noura.sync.presence.channel`, `noura.sync.v2 presence key`, the clock window |
| `sync-v2-profile.json` | `noura.sync.profile`, `noura.sync.profile.payload`, `noura.sync.v2 profile key`, a profile under an epoch that a removal it names requires rotating past, two profiles at one `profile_revision`, a profile whose signer a later change removed, refused, the profile signed again after that removal, and the server checks with their errors |
| `sync-v2-invitation.json` | `noura.sync.invite.token`, `noura.sync.invite.context`, `noura.sync.invite.payload`, `noura.sync.v2 invitation key` |
| `sync-v2-share.json` | `noura.sync.share.token`, `noura.sync.share` and its link signature, the fragment, `noura.sync.share.payload`, `noura.sync.v2 share key` |
| `sync-v2-recovery.json` | `noura.sync.recovery-kit`, `noura.sync.recovery-kit.payload` with its account head and the generation of its anchors, `noura.sync.v2 recovery kit`, the secret's base32 form, `noura.sync.recovery.proof` (wrong secret, tampered file), a proof refused for a recovery identity that a link removed, a recovery shown account links after the kit's head that a stolen device signed, a recovery that removes every other card and the identity, a workspace the kit doesn't list, accepted through a change that a device of your account, active at the head it keeps, signed; a change that a card step 5 removed signed in such a workspace, shown for your confirmation when `GET /v2/workspaces` lists the workspace during step 3 and refused with `sync_workspace_unverified` when first listed between steps 4 and 5 or served after the flow; such a workspace refused without an anchoring change or when a card removed after the kit was written and before the recovery signed it, as a backdated genesis or `account_heads` change, `sync_workspace_unverified`; and a kit whose identity is gone, which sends you to Reset devices; the server's checks of a `recovery_proof` against the session account's chain and enrolled card: an accepted proof, another origin, a proof from a card that isn't the recovery identity, a bad signature, another card than the session's, and an identity a link removed; and a recovery whose `remove` link commits before the recovered device enters its last workspace, with the head after `recover_device` adding the recovered card and removing none, and the head after the `remove` links only removing |

The store conformance suite runs `sync-v2-live.json` against both stores, so SQLite and PostgreSQL compute the same live sets as clients.

### Store and soak tests

Beyond the fixtures, the implementation tests these cases against both stores:

- two server instances racing pushes into one domain, with an injected commit delay, while every client receives every record;
- pushes into several domains racing policy changes, the collector, tus creation and share updates, which never deadlock;
- removing a device while it pushes a record every 500 ms, where every record after the removal's entry fails, on SQLite and on two PostgreSQL instances with injected delays between the policy read and the commit;
- 200 simulated devices in the workspace domain and 2 restricted domains, in a 24-hour soak at collaboration write volume with `retained_bytes` set to 64 MiB, that checks that the floor moves, that each domain's storage stays within the bound of [§19](#19-compaction) in the form given there, with the interval term measured from the largest entry and the writes during each wait, and that live-set bytes per text-mode note stay within one snapshot and 8 live updates once editing stops, also when the last editor disconnects right after its last edit; on SQLite it also checks that the server sustains 400 pushes a second, the 200 devices at the 500 ms tick, with no read transaction open across a pull's wait;
- a viewer that keeps pulling with an old cursor while the floor moves;
- a bootstrap during heavy checkpointing, and one whose checkpoint falls below the floor before it ends;
- `backup` while `serve` takes pushes, `restore` and `mark-restored`: `serve` refuses a stamped copy, and the generation rises above every earlier value; a `restore` into a fresh data directory with the secret `backup` wrote; every command refusing a missing or different secret, and `migrate` creating none for a database with a key check;
- a database put back without `restore` or `mark-restored`, by file copy and by failing over to an asynchronous PostgreSQL standby, where clients report `sync_server_rollback` or a chain gap and stop until `mark-restored`, then compare;
- a server restored from an older backup after further activity, in three runs. With only operations, checkpoints and blobs lost, including the checkpoint after a removal whose policy change the backup kept, devices rewind to a checkpoint they checked, write the lost content again from their files, including a record that every writer applied and whose author device is gone, sign the due checkpoint, and restart their chains above every counter they signed. With a lost account link, policy change, epoch record or envelope, including a lost removal of a device that wrote to the domain, every device that accepted it reports `sync_restore_lost` and writes nothing, devices that never saw it keep syncing, Set up sync again closes the old workspace and starts a new one that members join from their folders, and Reset devices starts a new chain that members see and an admin accepts. With two restores in a row, the second from an older backup, every device reaches the same stop or reconciliation as after one. Each run also checks that a share link revoked after the backup is revoked again;
- a restore whose blob store survived while the database lost the blob rows, one whose blob store lost objects that live operations list, where devices upload them again under the same IDs, and unknown store objects that move to quarantine and go 7 days later;
- a workspace over quota that accepts deletes, removals, checkpoints and an `account_heads` change for the signer's own account, through a lost device's replacement and a recovery with a kit, frees usage when a delete commits, rejects writes and additions that raise usage and uploads that don't fit, raises its floors only as far as it must while usage and history exceed the quota, and makes one checkpoint due per crossing;
- completed uploads that no push lists, collected after 24 hours, and a push racing the collector for one blob;
- a collaborator who sends near-limit updates every 500 ms while the others keep editing the note;
- a restricted domain's rotation left pending by a remover that can't read it, with no record or presence accepted there until a grantee device that holds the key rotates it after its wait, without writing;
- a tampering proxy for every row of [§30](#30-threats) marked as detected;
- a stored-data check that finds no per-record timestamp in database rows, no session expiry, and PostgreSQL's `track_commit_timestamp` off, and confirms that only the blob store's file times or `LastModified`, and the WAL and its archives of [§29](#29-what-the-server-stores), date anything;
- a member outside a restricted domain who receives no notify, records or presence frame for it;
- two PostgreSQL instances whose devices see each other's presence and records frames, with notifications sent after commit;
- an account chain append racing pushes that record that account's head, on SQLite and on two PostgreSQL instances, which never deadlock.

### Tag bytes

Each tag is the length byte, the ASCII domain string and `0002`:

| Domain string | Length | Tag bytes (hex) |
|---|---|---|
| `noura.sync.device.card` | 22 | `166e6f7572612e73796e632e6465766963652e636172640002` |
| `noura.sync.device.enroll` | 24 | `186e6f7572612e73796e632e6465766963652e656e726f6c6c0002` |
| `noura.sync.device.id` | 20 | `146e6f7572612e73796e632e6465766963652e69640002` |
| `noura.sync.account` | 18 | `126e6f7572612e73796e632e6163636f756e740002` |
| `noura.sync.approval.commit` | 26 | `1a6e6f7572612e73796e632e617070726f76616c2e636f6d6d69740002` |
| `noura.sync.approval.offer` | 25 | `196e6f7572612e73796e632e617070726f76616c2e6f666665720002` |
| `noura.sync.approval.code` | 24 | `186e6f7572612e73796e632e617070726f76616c2e636f64650002` |
| `noura.sync.policy` | 17 | `116e6f7572612e73796e632e706f6c6963790002` |
| `noura.sync.epoch` | 16 | `106e6f7572612e73796e632e65706f63680002` |
| `noura.sync.epoch.recipients` | 27 | `1b6e6f7572612e73796e632e65706f63682e726563697069656e74730002` |
| `noura.sync.key-check` | 20 | `146e6f7572612e73796e632e6b65792d636865636b0002` |
| `noura.sync.envelope` | 19 | `136e6f7572612e73796e632e656e76656c6f70650002` |
| `noura.sync.op` | 13 | `0d6e6f7572612e73796e632e6f700002` |
| `noura.sync.payload` | 18 | `126e6f7572612e73796e632e7061796c6f61640002` |
| `noura.sync.checkpoint` | 21 | `156e6f7572612e73796e632e636865636b706f696e740002` |
| `noura.sync.manifest.leaf` | 24 | `186e6f7572612e73796e632e6d616e69666573742e6c6561660002` |
| `noura.sync.manifest.bucket` | 26 | `1a6e6f7572612e73796e632e6d616e69666573742e6275636b65740002` |
| `noura.sync.manifest.node` | 24 | `186e6f7572612e73796e632e6d616e69666573742e6e6f64650002` |
| `noura.sync.manifest.heads` | 25 | `196e6f7572612e73796e632e6d616e69666573742e68656164730002` |
| `noura.sync.blob` | 15 | `0f6e6f7572612e73796e632e626c6f620002` |
| `noura.sync.blob.parts` | 21 | `156e6f7572612e73796e632e626c6f622e70617274730002` |
| `noura.sync.presence` | 19 | `136e6f7572612e73796e632e70726573656e63650002` |
| `noura.sync.presence.payload` | 27 | `1b6e6f7572612e73796e632e70726573656e63652e7061796c6f61640002` |
| `noura.sync.presence.channel` | 27 | `1b6e6f7572612e73796e632e70726573656e63652e6368616e6e656c0002` |
| `noura.sync.profile` | 18 | `126e6f7572612e73796e632e70726f66696c650002` |
| `noura.sync.profile.payload` | 26 | `1a6e6f7572612e73796e632e70726f66696c652e7061796c6f61640002` |
| `noura.sync.invite.token` | 23 | `176e6f7572612e73796e632e696e766974652e746f6b656e0002` |
| `noura.sync.invite.context` | 25 | `196e6f7572612e73796e632e696e766974652e636f6e746578740002` |
| `noura.sync.invite.payload` | 25 | `196e6f7572612e73796e632e696e766974652e7061796c6f61640002` |
| `noura.sync.share.token` | 22 | `166e6f7572612e73796e632e73686172652e746f6b656e0002` |
| `noura.sync.share` | 16 | `106e6f7572612e73796e632e73686172650002` |
| `noura.sync.share.payload` | 24 | `186e6f7572612e73796e632e73686172652e7061796c6f61640002` |
| `noura.sync.text.lineage` | 23 | `176e6f7572612e73796e632e746578742e6c696e656167650002` |
| `noura.sync.recovery-kit` | 23 | `176e6f7572612e73796e632e7265636f766572792d6b69740002` |
| `noura.sync.recovery-kit.payload` | 31 | `1f6e6f7572612e73796e632e7265636f766572792d6b69742e7061796c6f61640002` |
| `noura.sync.recovery.proof` | 25 | `196e6f7572612e73796e632e7265636f766572792e70726f6f660002` |

BLAKE3 `derive_key` contexts are the exact ASCII strings in [§3](#3-primitives), with no tag and no version bytes: `noura.sync.v2 operation key`, `noura.sync.v2 lineage key`, `noura.sync.v2 key check`, `noura.sync.v2 presence key`, `noura.sync.v2 profile key`, `noura.sync.v2 share key`, `noura.sync.v2 invitation key`, `noura.sync.v2 invitation proof`, `noura.sync.v2 recovery kit`, `noura.sync.v2 blob nonce`, `noura.sync.v2 account id` and `noura.sync.v2 secret check`.

### Example: a device card

With account `11` repeated 16 times, signing key `33` repeated 32 times and X-Wing public key `44` repeated 1,216 times, the device ID derives as `ab553dd81a57e9291f87d0b55e520c0b`, the first 16 bytes of `BLAKE3(tag(noura.sync.device.id) || 33 × 32)`, and the card is these 1,309 bytes:

```text
16 6e6f7572612e73796e632e6465766963652e63617264 0002   tag
11111111111111111111111111111111                        account
ab553dd81a57e9291f87d0b55e520c0b                        device
3333333333333333333333333333333333333333333333333333333333333333   signing_public
000004c0                                                hpke_public length, 1,216
44 repeated 1,216 times                                 hpke_public
```

## 33. Decisions at sign-off

These choices go beyond the list of topics the protocol had to cover. Each one says Confirmed, or Changed and what changed.

1. **Binary encoding.** Confirmed. Records use the fixed-width binary encoding in [§4](#4-encoding) instead of v1's JSON tuples, and the signed bytes are the sent bytes.
2. **New primitives.** Confirmed. XChaCha20-Poly1305 replaces AES-256-GCM, HPKE with the hybrid X-Wing key encapsulation replaces age and the custom web envelope, and BLAKE3 replaces SHA-256 for digests.
3. **History isolation is the server's job.** Confirmed. Lineage lets one envelope per domain open old epochs, so joining never re-encrypts objects. A removed card never returns to its account chain, and a member invited again reads what was written while they were away.
4. **Recovery kits are offline devices.** Changed: a kit's recovery identity is a card in your account chain that signs one `recover_device` link and no policy change. It stays current through rotations and works over quota, and a generated 160-bit secret that needs no key stretching protects it. A kit whose identity a device removed can't recover, and you reset your devices instead.
5. **Approval compares one 8-digit code.** Changed: the candidate's commitment also covers its account chain head, and accepting a reset is an approval whose offer anchors the workspace. The code derives from the approver's offer and a nonce the candidate committed to first, and both devices confirm it: one offer and one comparison per `nc`, and one outstanding offer per approving device.
6. **Policy changes enter every domain's log.** Confirmed. The server's commit order decides which records a removal cuts off, and server and clients check write access and epochs by log position. Only a device that can read a domain signs its epochs, and a replacement or a re-wrap gets past an epoch that clients reject.
7. **Deletes carry a cleartext flag.** Confirmed. The server can prune a tombstone once the retention floor passes it. The flag shows that an object was deleted and where in the log, never its type, creation time or size.
8. **Retention is measured in bytes.** Changed: the claim that storage stays bounded at any write rate is gone. Each domain keeps `retained_bytes` of log behind a checkpoint, 1 GiB by default, plus up to two checkpoint intervals, and the server stores no sequence-to-date table. An interval holds 64 MiB of weight plus the entry that crosses it and what writers append in the minute before the next checkpoint. Checkpoint records (376 bytes each) and the bucket hashes of servable checkpoints (128 KiB each) grow with the number of checkpoints instead ([§19](#19-compaction)). A device that falls further behind bootstraps.
9. **Limits.** Confirmed: 1 MiB bodies with every atomic change in one push, 256 KiB inline payloads, 2 GiB blobs, 256 covers per operation, 256 domains per workspace, 512 recipients per epoch. An approval offer anchors one checkpoint per workspace, so it fits too.
10. **Edit beats delete.** Confirmed. A conflict keeps the edit until you choose, and a returning device asks before deleting files the manifest lacks.
11. **Edit mode and size class stay visible.** Confirmed. Patches that cover nothing show text mode, and blob IDs show content above 256 KiB. Hiding them would mean dropping server-side compaction or blob authorization.
12. **Only the inviting device approves an invitee.** Changed: the inviting device's `add_member` records the invitee's account head, which the invitee's request committed to, so no separate device change exists and clients check one change.
13. **Sync rate limits count bytes per device.** Confirmed, with a bootstrap allowance and the defaults in [§26](#rate-limits); records delivered over the realtime channel don't count. Ticket 52 gives uploads a budget of their own.
14. **Rollback protection lives in the credential store.** Changed: the mark covers the device's own account chain head and its reserved load numbers, and the removed list is gone. A high-water mark per workspace keeps a restored or copied workspace folder from rolling a device back or reusing its counters, and the own-head entry keeps a rolled-back app data folder from accepting a shorter account chain.
15. **A text-mode note's path and properties travel beside its yrs body.** Changed: they left the yrs document. Each is a `meta` entry in the encrypted payload that merges per key by log position, with ties broken by record digest, so renames and frontmatter edits merge without review and the verdict judges every write from the record alone. A write that a restore lost comes back as a new write. A lineage derives from the first document's bytes, so two devices that start editing at once from the same state agree on it, and two that build different documents conflict.
16. **Every operation names its writer's latest checkpoint and the restore generation.** Confirmed. These 48 bytes per record keep a server from hiding a checkpoint from one client while records still cross.
17. **Devices live in one signed chain per account.** Changed: each account keeps one chain of its device cards and recovery identity, following Keybase's per-user sigchain, and workspace policies name accounts and record the chain heads they rely on, replacing introducers, per-workspace card copies, the sweep and the removed list. One `remove` link takes down a card and every card it added, directly or in turn, except devices you keep; each workspace follows by recording the new head, and the domains those cards read rotate. Another account removes a member, never one device. A reset account's new chain enters a workspace only through one owner or admin's `accept_reset`, the commit of an approval that answers the account's request, as a Keybase admin adds a reset user back to a team; every member's device shows it, and one acceptance stands in for every member's because the workspace's keys are shared ([§7](#resetting-your-devices)). Any member device records any later head of a member account, so a removal reaches every workspace, and an account's own device records its head again where another account's change recorded it, so the account's other devices trust the workspace.
18. **Quota counts live data, policy and keys.** Confirmed. A delete frees quota when it commits, history yields to the quota instead of counting toward it, and your own devices can still join a workspace over quota.
19. **Restores fail closed.** Changed: v2 repairs nothing after a restore. `restore` and `mark-restored` raise one server-wide generation, and clients catch restores without them as rollbacks. After a generation change each device compares what it accepted with what the server holds. A lost account link, policy change, epoch record or envelope stops the account or workspace until you reset your devices or an owner sets the workspace up again; when only operations, checkpoints and blobs went, devices rewind to a checkpoint they checked, or pull from sequence 0 in a domain where they reached none, and write lost content again from their files. When the server serves none of the checkpoints a device reached in a domain, the device stops the workspace. A starting checkpoint, record or head that disagrees with the final head of a writer whose access ended stops the workspace before any file changes. The server's secret stays so that account IDs survive a restore. Ticket 60 picks one writer per lost object.
20. **The collaboration sandbox judges a record by its bytes.** Changed: the guest checks the raw lib0 structure before yrs decodes it; a client ID is a card index from the policy times 2^29 plus a load number; an update holds structs only under its signer's client ID, a snapshot only under indexed cards', and no record holds a GC struct. A note's body merges its live records in a canonical order, which a device that meets a disagreeing snapshot rebuilds. One pinned guest module judges with fuel proportional to the record's size and no clock, and merging charges per-writer allowances as debt. The guest's yrs splits a surrogate pair as Yjs does, into two U+FFFD.
21. **Two IDs derive instead of being drawn.** Changed: an account's ID derives from its sign-up email under the server's secret, so Reset devices works after a restore lost the account, and a device's from its signing key. Workspace IDs are random, like domain and blob IDs, because no repair needs them, so only an approval, a kit or a change that a still-active device of your own account signed anchors a workspace's log.
22. **Chains may skip counters.** Confirmed. A device starts a new block of counters at each start, so neither a crash nor a rolled-back journal makes it sign a counter twice.
23. **The server publishes checkpoint due points.** Confirmed. Writers can't see blob weights inside payloads they can't read, and a quota crossing makes a checkpoint due once.

## Sign-off

| | |
|---|---|
| Signed off by | Claude, the orchestrating agent, under the owner's delegation of 2026-10-20 |
| Date | 2026-10-21 |
| Commit signed off | `ad46124` |
| Review | Ten adversarial rounds. The last full round found no critical issue and two high ones, each closed in `ad46124` and checked. Open medium and low findings are acceptance criteria in tickets 43 to 61. |
| Residual risks | Section 30 lists them. The largest: a thief who strikes first can remove your other devices, and you then reset your devices; a restore that loses device, policy or key state stops sync until you reset your devices or an owner sets the workspace up again. |
