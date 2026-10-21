# Encrypted sync protocol, version 2

Status: draft, waiting for sign-off. On 2026-10-20 the owner delegated sign-off to Claude, the orchestrating agent. Nobody writes v2 code before the [sign-off record](#sign-off) holds a name and a date.

This document defines Noura's encrypted sync protocol for the Rust product: the records a desktop client signs and encrypts, the keys that let devices and members read them, the server's checks, and the HTTP API that carries them. It replaces [sync-v1.md](sync-v1.md), [checkpoints.md](checkpoints.md) and [collaboration.md](collaboration.md), which describe the legacy app.

The server stores ciphertext and the routing fields listed in [§29](#29-what-the-server-stores). It never needs plaintext workspace content to store, order, route or compact your data.

## 1. Goals

- Sync a workspace of 100,000 objects across 200 devices, with no request or response body above 1 MiB.
- Keep key rotation proportional to devices and key domains. Removing a device never re-encrypts objects.
- Let a client detect a server that drops, reorders, replays or forks a device's operations, or that forks or rolls back the policy, the keys or the manifest.
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
| Key wrapping | Hybrid Public Key Encryption (HPKE, RFC 9180) base mode: the X-Wing key encapsulation mechanism (ML-KEM-768 combined with X25519, draft-connolly-cfrg-xwing-kem-10) `0x647a` as draft-ietf-hpke-pq-04 defines it for HPKE, HKDF-SHA256 `0x0001`, ChaCha20Poly1305 `0x0003` | `hpke` 0.14.1 with the features `mlkem` (which brings `x-wing` and `ml-kem`), `hkdfsha2` and `chacha`; `mlkem` doesn't enable `hkdfsha2`, so the build names it; MIT or Apache-2.0 |
| Record encryption | XChaCha20-Poly1305 with a random 24-byte nonce | `chacha20poly1305` 0.11.0, MIT or Apache-2.0 |
| Attachment encryption | STREAM (Hoang, Reyhanitabar, Rogaway and Vizár) over XChaCha20-Poly1305, 32-bit big-endian counter and last-chunk flag | `aead-stream` 0.6.0, type `StreamBE32<XChaCha20Poly1305>`; MIT or Apache-2.0 |
| Digests, message authentication codes, key derivation | BLAKE3: `hash`, `keyed_hash`, `derive_key` | `blake3` 1.8.7, CC0-1.0 or Apache-2.0 |
| Randomness | the operating system's cryptographically secure random number generator (CSPRNG) | `getrandom` 0.4.3, MIT or Apache-2.0 |

Ticket 43 pins these versions in `Cargo.lock`, which today still holds older ones (`hpke` 0.12.0, `ed25519-dalek` 2.2.0, `chacha20poly1305` 0.10.1) for the legacy app. From then on, a version change updates this table. Continuous integration runs the published test vectors for each primitive: for the HPKE suite above, the base-mode vectors of the draft-ietf-hpke-pq reference implementation (`hpkewg/hpke-pq` at commit `6433c8f`, shipped with `hpke` 0.14.1 as `test-vectors/pq-6433c8f.json`), which cover X-Wing with HKDF-SHA256 and ChaCha20Poly1305; Wycheproof vectors for XChaCha20-Poly1305, Ed25519 and X25519 (low-order points included); and the BLAKE3 reference vectors. RFC 9180's own vectors cover no X-Wing suite.

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

Seven contexts derive from other inputs. `noura.sync.v2 share key` derives the snapshot key from a share link's key. `noura.sync.v2 invitation key` derives the context key, and `noura.sync.v2 invitation proof` the proof key, from an invitation secret. `noura.sync.v2 recovery kit` derives the kit key from the kit's secret and salt ([§12](#12-recovery-kit)). `noura.sync.v2 blob nonce` derives a blob's nonce prefix from its key and ID ([§21](#blob-format)). `noura.sync.v2 commit tag` and `noura.sync.v2 account id` derive two server keys from the server's secret ([§14](#after-a-server-restore)).

## 4. Encoding

Every signed record, hashed tuple, request and response uses one binary encoding. The bytes a device signs are the bytes it sends, so a verifier never rebuilds a tuple from parsed fields. `sync-protocol` owns this encoding, and `sync-key-envelope` builds the device card, the enrollment proof and the envelope on top of it, so both crates use one encoder.

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
- a list that this document requires sorted and unique but isn't;
- an all-zero `id` where this document forbids it;
- invalid UTF-8 in a `str`;
- a card whose device ID doesn't derive from its signing key, or a genesis whose workspace ID doesn't derive from its body ([§5](#5-identifiers)).

A field that [§27](#27-limits) gives no limit of its own takes two general ones: a `str` holds at most 4,096 bytes, and a `bytes` length or `list` count can't exceed the bytes left in the body, each list element counted at its smallest encoding. `sync-v2-encoding.json` pins the limit of every variable field.

Lists of IDs sort by unsigned byte order. Lists of pairs sort by their first field, then their second.

## 5. Identifiers

Remote IDs are 16 bytes, random from the operating system's CSPRNG except where the table derives them. They carry no timestamp, version bits, type or local object ID, and the all-zero value is reserved.

In URL paths, query parameters and headers, binary values are base64url without padding (RFC 4648 §5): a 16-byte ID or token takes 22 characters, and a 32-byte value (a digest, an offer or checkpoint digest, a token hash, a session token) takes 43. Integers (`after`, `at`, `level`, `index`, `epoch`, `counter`, a revision or key sequence) are unsigned decimal with no sign or leading zeros. A structured value passed in a query, such as a `live_cursor` ([§26](#message-bodies)), is base64url of its encoding. The header `Authorization: Bearer <token>` carries the session token's 43 characters.

| ID | Created by | Notes |
|---|---|---|
| Account ID | server | derived from the email the account signs up with, under a key from the server's secret, so signing up again after a restore gets the same ID ([§14](#after-a-server-restore)) |
| Device ID | derived | the first 16 bytes of `BLAKE3(tag(noura.sync.device.id) \|\| signing_public)`, so no other key pair can claim it |
| Workspace ID | derived | the first 16 bytes of `BLAKE3(tag(noura.sync.workspace.id) \|\| owner_card \|\| workspace_domain)` from its genesis ([§8](#policy-change-record)), so only the owner's first device can create it; unrelated to the local `workspace_...` ID in `.noura/workspace.yaml` |
| Domain ID | the device that creates the domain | |
| Remote object ID (`rid`) | the device that first syncs an object | stable across moves inside one domain; a move to another domain gets a new `rid` ([§9](#moving-objects-between-domains)) |
| Text lineage ID | derived, not random: from the `rid`, the base operation and the starting text | names the yrs document of a text-mode note, so two devices that start one from the same state agree on it ([§22](#22-live-collaboration)) |
| Blob ID | the uploading device | |
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

A device whose session ended without its device being revoked, such as after a server restore lost the session's row ([§14](#after-a-server-restore)), signs in again and enrolls with its existing card and a fresh proof. When the server holds that card byte for byte and hasn't revoked the device, it binds the new session to the existing device instead of answering `id_conflict`; a card the restore lost enrolls as new. A card whose device ID the server holds with other bytes, or whose device it revoked, fails with `id_conflict`. A restore that lost the account itself costs nothing more: you sign up again with the same email, which gets the same account ID ([§14](#after-a-server-restore)).

An account's first enrolled device is trusted when it enrolls: it can create workspaces. Every later device needs approval from a device the account already trusts ([§10](#10-approval)), except a device you recover with a kit ([§12](#12-recovery-kit)).

### Pinned cards

Each device keeps a list of its own account's pinned cards in the app's application data, outside every workspace, under a BLAKE3 keyed hash with a key it keeps in the credential store, as it does for its journal ([§15](#client-journal)). A card of your account enters the list when the device:

- creates it: its own card, and a recovery identity it exports ([§12](#12-recovery-kit));
- completes an approval code with it, as approver or as candidate ([§10](#10-approval));
- imports it from a recovery kit;
- verifies, in a workspace's policy chain, an `add_device` for it that a device it already pinned signed.

An `add_device` that another account signed for your account pins nothing, and a card that only `GET /v2/devices` lists never enters the list. Every card in an `add_device` with a zero `offer` comes byte for byte from the signer's pinned cards. A device of your account that the signer hasn't pinned joins that workspace later, when a device that pinned it adds it, or through its own approval.

A card leaves the list when the device verifies, in any workspace's policy chain, a `remove_device` for it that a device or recovery identity of your account signed, and when you remove, sign out or revoke that card on this device. The device then records the card's digest in a removed list in the same keyed store. Nothing pins a card on that list again: no approval, kit import or `add_device`. A zero-offer `add_device` never names such a card. From then on the device accepts no change that the card signed, in any chain, unless it had accepted that change already or a later change in the same chain, signed by a card it pinned, removes the card. A chain that a thief's card built or extended therefore stays untrusted, while a workspace that the removal flow has already cleaned up stays trusted ([Removing a device](#removing-a-device)).

### Device list

The app builds your device list from the verified policy chains of your workspaces and its pinned cards. It reads `GET /v2/devices` only to show which devices hold a session, never as a source of cards. For each of your devices it shows which device and which account signed its `add_device`.

Each device alerts you when it verifies a change for your account that it didn't witness: an `add_device` of either kind for a card that wasn't on its pinned list before and that it didn't add through an approval code it took part in, or a `remove_device` it didn't sign. The alert names the signing device, its account, the card's fingerprint, and whether the card is a device or a recovery identity. Your devices adding each other to a new workspace raise no alert, because each already pinned the others. An invited member's first device raises none on itself, because it created its own card.

### Removing a device

Removing a device of your own account is account-wide. The app signs a `remove_device` in every workspace whose policy lists the card, one push per workspace, and revokes the device on the server ([Desktop sign-in](#desktop-sign-in)). Each of your devices that sees such a removal in one workspace signs the same removal in every other workspace that still lists the card, and treats the card as removed meanwhile, so a flow that stops halfway still completes.

The flow also removes every card the removed device added for your account, devices and recovery identities alike, and every card those added in turn. A card counts as added by a device when the device running the flow pinned it through that device: through an approval code that device took part in as approver, or through an `add_device` that device signed ([Pinned cards](#pinned-cards)). A card your device had pinned earlier another way doesn't count, so a device that added your other devices to a new workspace takes none of them down with it. You may keep a device you recognize, but the flow never offers to keep a recovery identity that the removed device added. A thief who added a device or a recovery identity from your stolen laptop therefore loses it with the laptop, and the rotations that follow wrap no key for it. The app then asks you to export a new recovery kit ([§12](#12-recovery-kit)).

### Desktop sign-in

The desktop signs in with the OAuth 2.0 device authorization grant (RFC 8628). The device code is 32 random bytes in base64url, the user code is 8 characters from `BCDFGHJKLMNPQRSTVWXZ` shown as two groups of 4, codes expire after 10 minutes, and the app polls every 5 seconds, adding 5 seconds after each `slow_down`. A successful grant returns a session token of 32 random bytes, which the server stores only as its BLAKE3 hash and the device stores only in its credential store. A session has no expiry. It ends when its device is revoked: when you sign out, or when you remove the device from another device ([Removing a device](#removing-a-device)). The server never revokes a device on its own. The server deletes a device grant, an enrollment challenge or a magic link when it's used, or 10 minutes after it issued it.

Revoking takes a signed record, which `DELETE /v2/devices/{device}` carries:

`revoke`, signed:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.device.revoke)` | |
| origin | `str` | as in the enrollment proof |
| account | `id` | |
| device | `id` | the device to revoke |
| signer | `id` | |
| sig | `sig` | by `signer` |

The server accepts it from a session of that account when the signer is the revoked device itself, a recovery identity of the account, or a device of the account that some workspace's policy lists as active, and it verifies the signature under the signer's card from that policy chain, or under the enrolled card when a device revokes itself. A session alone revokes nothing, so a magic link, a phished device code or an enrolled device that nobody approved can't revoke your devices, and neither can a device after its removal.

A policy change that removes another account's device, or a member, from one workspace ([§8](#8-workspace-policy)) ends that access to that workspace alone. Removing a device of your own account takes it out of every workspace and revokes it ([Removing a device](#removing-a-device)). A device that belongs to no workspace can still create one.

Signing out removes the device from every workspace's policy, revokes it on the server, which ends its sessions, and deletes its keys. A revoked device ID never enrolls or binds a session again. The device can't rotate keys for its own removal, so each domain it read rotates when another device next writes or sends presence there ([§9](#rotation)). Before you sign out the last trusted device of an account, the app warns that you'll need a recovery kit to read synced data again.

## 8. Workspace policy

A workspace's membership, roles, devices and key domains live in its policy: a hash chain of signed changes that every member device verifies.

### Policy change record

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.policy)` | |
| workspace | `id` | |
| revision | `u64` | 1 for genesis, then +1 per change |
| previous | `digest` | digest of revision − 1; 32 zero bytes at revision 1 |
| signer | `id` | device ID |
| kind | `u8` | table below |
| body | kind-specific | table below |
| sig | `sig` | by `signer` |

| Kind | Name | Body |
|---|---|---|
| 1 | genesis | `id owner_account`, `bytes owner_card`, `id workspace_domain`; the record's `workspace` derives from `owner_card` and `workspace_domain` ([§5](#5-identifiers)) |
| 2 | add_device | `u8 device_kind` (1 device, 2 recovery identity), `bytes card`, `digest offer` (32 zero bytes when nobody compared an approval code, which the signer may do only for a card of its own account that it pinned ([§7](#pinned-cards)): adding a recovery identity, adding your other devices to a workspace you created or joined, or a device that a recovery identity signs in) |
| 3 | remove_device | `id device` |
| 4 | add_member | `id account`, `u8 role`, `id invitation` |
| 5 | set_role | `id account`, `u8 role` |
| 6 | remove_member | `id account` |
| 7 | create_domain | `id domain`, `id marker_rid`, `list<access>`: creates a restricted domain |
| 8 | set_access | `id domain`, `list<access>` (replaces the domain's list) |
| 9 | delete_domain | `id domain` |
| 10 | transfer_owner | `id account` |

`access` is `id account | u8 level`, sorted by account, with levels 1 read, 2 write and 3 manage. Each level includes the ones below it: a manager writes and reads, and a writer reads. Roles are 1 owner, 2 admin, 3 editor and 4 viewer.

Every route under `/v2/workspaces/{w}/` serves only a session whose device is active in w's policy, or a recovery identity's proven session ([§12](#12-recovery-kit)); repair also serves a device that its own request makes active ([§14](#after-a-server-restore)). When a change commits, the server stops serving this workspace to the devices it removes, whatever their account: a pull returns status 4 for every domain, and every other request fails with `forbidden`, except that a removed device may still fetch the policy chain up to and including the change that removed it, so it can verify its removal ([§15](#pull)). A change that takes read access from a device requires rotations, which follow it ([§9](#rotation)).

A `remove_member`, whoever signs it, removes every device of the account from this workspace and drops the account from every restricted domain's access list, as a grantee and as a manager. An account invited again reads a restricted domain only after a manager checks with that person and grants it again with `set_access`. A restricted domain whose last manager leaves keeps its other grants, and an owner or admin can still delete it.

Once a change removes a card, through `remove_device`, `remove_member` or signing out, no later change adds that card to the workspace again: the server rejects such an `add_device` with `forbidden`, and clients report `sync_policy_unauthorized`.

Device IDs are unique within a workspace's chain. Clients reject, with `sync_policy_unauthorized`, an `add_device` whose device ID the chain already names, under any account and in any state, and one whose device ID matches a card on their pinned or removed list ([§7](#pinned-cards)) with different bytes. The server rejects such an `add_device` with `id_conflict`, recovery identities included. A device ID derives from the card's signing key ([§5](#5-identifiers)), so nobody can take another device's ID with a card of their own.

An account holds at most one active recovery identity in a workspace: an `add_device` of kind 2 is valid only while the chain lists no active recovery identity of that account, so a new kit removes the old identity before it adds its own ([§12](#12-recovery-kit)). No domain may have more than 512 active readers, recovery identities included ([§27](#27-limits)). The server rejects a change that breaks either rule, with `forbidden` or `recipient_limit`, and clients report it as `sync_policy_unauthorized`.

`delete_domain` deletes the domain's log, epochs, envelopes and checkpoints on the server, and its blobs, and the server rejects any later record for that domain with `not_found`. Devices that held the domain keep their local files. Those files lose their marker's protection, so the rule in [§9](#domain-roots) holds them for review instead of uploading them to a wider domain. An owner or admin can delete a restricted domain they can't read.

### Roles and authority

| Action | Who may sign it |
|---|---|
| genesis | the creating device, which becomes the owner's first device |
| add_device for your own account | any device of your account in the workspace; a recovery identity only for one new device of its account per workspace |
| add_device for another account | an owner or admin device, only at the revision right after that account's `add_member`, signed by the device that signed the `add_member`; one per `add_member` |
| remove_device | for your own account, any device or the recovery identity of your account; for another account, an owner device when that account is an admin, editor or viewer, or an admin device when it is an editor or viewer |
| add_member | owner (admin, editor or viewer); admin (editor or viewer); requires an account that isn't a member and an invitation it accepted |
| set_role | owner for any non-owner; admin between editor and viewer |
| remove_member | owner for any non-owner; admin for editors and viewers; the member themself |
| create_domain | owner or admin; the access list must name the signer's account as a manager |
| set_access | a manager of that domain for any change; an owner or admin only to remove entries |
| delete_domain | owner or admin, or a manager of that domain |
| transfer_owner | owner; the old owner becomes an admin |

A recovery identity signs no other kind of change, whatever its account's role.

Clients check the rule for another account's device from the chain alone: the `add_device` must sit at revision r + 1 when r is that account's `add_member`, carry the same signer as r, and be the only change for another account's card that r allows. Any other `add_device` that one account signs for another's card reports `sync_policy_unauthorized`. Because `add_member` takes only an account that isn't a member, an owner or admin can't plant a device in an account while it stays a member: they would have to remove the member first, which removes its devices and drops its restricted grants, and a manager grants those again only after checking with the person. The joining device checks that the `add_device` at r + 1 adds its own card. The server also accepts such an `add_device` only when that account enrolled the card with its own session, and demands an enrollment proof for every card except a recovery identity that a device of its own account adds.

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

A restricted domain's access list alone decides who reads and writes it, whatever their workspace role. Grantees must be members, and every active device of an account on the list reads the domain.

### Policy entries

When a change commits, the server appends an entry for it, `u64 revision | digest head`, to every domain's log in the same database transaction, under the locks that [§15](#store-transactions) orders. A domain that the change creates gets that entry first. The entry fixes where the change takes effect in each log. A client checks each entry's `head` against its own change at that revision, fetching the chain first when the revision lies above its accepted head, and reports `sync_policy_forked` on a mismatch.

Server and clients check one rule for every record a device signs into a domain's log, an operation or a checkpoint: a record of device X in domain D is valid only when X could write D at the latest policy entry before the record, and also at the record's own `policy_revision`, which is at most that entry's revision. The server checks a new record against its current policy, because the record goes after the latest entry, and rejects it with `forbidden` or `policy_stale` ([§13](#server-checks-on-an-operation)). The server's commit order settles any race between a writer and the change that removes its write access: every record that lands after the change's entry fails, so a removal needs no coordination with the devices it removes. A client checks the same rule as it reads the log and reports `sync_writer_not_authorized` for a record that breaks it.

A client that accepted change `r` requires `r`'s entry in each log it reads, at or below the `head` of the first page it requests afterwards, and reports `sync_chain_gap` when its cursor passes that head without the entry. Epoch entries follow the same rule ([§9](#rotation)). A client that bootstrapped a domain from a checkpoint needs no entry there for a change at or below the checkpoint's `policy_revision`, or for an epoch at or below its `epoch`: those entries sit at or below `covered_seq`, and the checkpoint stands in for them. The device that committed `r` also holds a receipt with each entry's sequence ([§15](#push)) and requires the entry at exactly that sequence, so in the remover's view the server can't move a removal past the removed device's later records.

The entries of a restricted domain reach only its readers, and the server already knows the policy, so they add no metadata.

### Verifying the chain

A client accepts change `r` only when its revision is one above the accepted head, its `previous` equals the head's digest, its signature verifies under a device that the state before `r` lists for the signer's account, and that account holds the authority above. The server checks every signed record a device pushes the same way, under the signer's card in the policy chain, never under an enrolled card. A device trusts revision 1 through its approval anchor ([§10](#10-approval)), its recovery kit ([§12](#12-recovery-kit)), or because it created the workspace.

A device of your account that creates or joins a workspace adds, in the creation request or its first push after approval, every device and recovery identity of your account that it pinned ([§7](#pinned-cards)) and the chain never added. A card the chain added and later removed stays out. Each device of your account does the same when it first verifies a workspace's chain. Another device of your account then trusts that workspace's chain through the `add_device` that names it: a device of its own account, whose card it pinned, must have signed that change, and the change's `previous` digest fixes the prefix, which the device then verifies from genesis.

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

A marker is valid when the live set of `marker_rid` in the named domain's own log holds exactly one operation, a state operation whose decrypted path ends in `/.noura-domain`, and the policy still lists the domain. A deleted marker, or one in conflict with two live states, isn't valid. A `.noura-domain` file found in any other log, or written by hand, is an ordinary file. On a device, a marker is also invalid when it sits at or beneath the recorded root of a domain whose `create_domain` has a lower policy revision, or at the same path as that domain's marker. The earlier root wins, so someone outside restricted domain R who signs a marker inside R's folder can't redirect R's files on the devices of R's grantees.

The journal records each domain root as (domain, folder, marker `rid`) when the client creates or applies a valid marker, and that record decides where a new file goes: the domain of the innermost recorded root that holds it, or the workspace domain. Roots nest only when a later domain's root holds an earlier one, and the earlier root keeps its own files. A client applies a domain's marker before any other object of that domain, during bootstrap too ([§18](#18-bootstrap)), so the root exists before the first file beneath it. During a bootstrap or a journal rebuild, the client holds every new local file until it has applied the marker of every restricted domain it reads. When a marker disappears or stops being valid, or a marker's move would change the domain of existing objects, the client keeps the recorded root, holds the affected files and any new files under it for review, and uploads none of them to another domain until you confirm. An iCloud placeholder of the marker, `.noura-domain.icloud`, counts as present. Restoring the marker from synced state ends the hold.

A client holding live objects of a restricted domain that no recorded root covers records a held root for each folder that directly contains such an object, before it writes the object's file. That happens only when a bootstrap or a journal rebuild meets a marker that is moved, deleted or in conflict. The client holds every new file at or beneath a held root and uploads none of them to any domain until a valid marker covers that folder or you confirm a domain for them.

While a domain's marker is valid, an object of that domain whose path leaves the root, whether from its payload or from a text-mode note's merged `meta` ([§22](#22-live-collaboration)), records no held root. The client keeps the file at its last path inside the root, or doesn't write it when it has none there, lists the change for review and reports it. A device never writes such a path itself: a rename that would take a file across a root boundary is a move between domains ([Moving objects between domains](#moving-objects-between-domains)).

A client never uploads a delete of a marker. When you delete a marker file locally, the client writes it again from its journal; only `delete_domain` retires a marker. A client that receives a delete of `marker_rid`, or a move of it to a path that doesn't end in `/.noura-domain`, keeps its recorded root, holds files under it as above, and reports the record.

A device writes an object of another domain under `<root> (shared)`, the root folder's name followed by ` (shared)`, when the object's path lies at or beneath the root of a restricted domain the device reads, and reports it. It never classifies such an object into that root, and new files you create under the root still go to the root's domain. The relocation is local: an edit to a file under `<root> (shared)` uploads to that object's own domain at its synced path.

Restricting a folder that already holds files makes one exception. The creating device writes `create_domain` and the marker, then moves each workspace-domain object under the new root into the domain, one object pair at a time ([Moving objects between domains](#moving-objects-between-domains)). A workspace-domain object whose latest state operation sits before the `create_domain` entry in the workspace domain's log stays at its path on every device until its move arrives, instead of going under ` (shared)`. A grantee device holds your edits to it for review and uploads none of them until the move lands. If the creating device stops before it finishes, any device of a manager of the domain moves the rest.

Sync covers every file that the workspace scanner reports ([v1.md](v1.md#local-metadata)) and passes the path rules in [§13](#payload), plus each valid `.noura-domain` marker, the one dotfile sync reads. It skips the manifest's `ignore` patterns, symlinks, empty folders, and files whose names aren't valid UTF-8 or break the path rules; the client reports each file it skips. v1.md and its conformance fixtures gain the marker exception in the change that implements domain roots.

### Moving objects between domains

A move across a domain boundary writes a new state operation with a new `rid` in the target domain, whose payload names the source domain and `rid` in its `origin` field, and a `delete` operation (reason 2, moved) in the source domain, whose payload names the target. Both commit atomically in one push ([§15](#push)), so the server can link the two `rid`s ([§29](#29-what-the-server-stores)). A blob serves only operations of the domain it was created for, so a moved attachment gets a new blob: the client encrypts it again under a new key and blob ID and uploads it. Members of both domains see one object, because the local object ID travels inside both payloads. The new state operation is always a `file`, so a text-mode note leaves text mode when it changes domain and enters it again on its next edit ([§22](#entering-text-mode)). A move inside one domain is a state operation on the same `rid` with the new path, and it reuses the `blob_ref`.

A client applies the create before the delete. When it can read the target domain, it defers a reason-2 delete until it has pulled the target domain past the move; a device that can't read the target applies the delete at once.

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

An epoch travels as an epoch bundle, `bytes epoch | list<id> recipients`, and the server keeps every bundle whole. When an epoch commits, the server also appends its epoch record, without recipients, to that domain's log as an entry, in the same database transaction ([§15](#15-domain-log-cursors-and-pulls)). A reader verifies that entry as an epoch record, and the entry fixes where the epoch starts in the log ([§9](#rotation)).

The server accepts an epoch record only for epoch 1 of a domain, committed with genesis or `create_domain`; for a rotation that a committed policy change requires and that hasn't committed yet, with a `policy_revision` at or above the latest change that requires it; or for the one rotation per domain that falls due when a restore's quiet period ends ([§14](#after-a-server-restore)), with a `policy_revision` at or above the policy head at that moment. It rejects any other epoch record with `rotation_not_due`. A member therefore can't rotate a domain at will, and only a required rotation makes writers re-seal.

The lineage field links each epoch to the one before, so a device that holds the current key derives every earlier key by walking backward. A newly approved device therefore needs one envelope per domain, and objects written under old epochs stay readable without re-encryption. A key that a newer key's lineage yields needs no signature check of its own record: the lineage AEAD authenticates it, and the device checks it against that record's `key_check`.

`key_check` lets every recipient confirm it received the same key as the others. A rotating device can't hand different recipients different keys without failing that check.

A client accepts an epoch record when:

- its signature verifies under the rotating device's pinned card;
- the rotating device was active and could read the domain at `policy_revision`; epoch 1 of a restricted domain comes from the device that signed its `create_domain`;
- `policy_revision` is at least the previous record's, and `policy_head` matches the client's change at that revision;
- the recipient list matches `recipients_digest` and holds exactly the active devices that could read the domain at that revision;
- the epoch is one above the previous accepted epoch and `previous` equals that record's digest;
- the lineage decrypts to a key matching the previous record's `key_check`; at epoch 1 the lineage is empty.

Every key derived through lineage is checked against its own record's `key_check`. A failure reports `sync_key_mismatch`.

**Epoch authority.** An epoch's entry never tells a client that a rotation happened. Server and clients judge every rotation by revision: a rotation of D is pending while a policy change above the latest epoch record's `policy_revision` requires D to rotate, wherever that epoch's entry sits in the log. Suppose a device that lost access to D at revision r, or any former reader, signs an epoch record at a revision below r, and a server commits it after r's entry. The record's recipients are the readers before r, the lost device among them, yet the rotation that r requires stays pending: no writer encrypts under that epoch, every operation under it after r's entry is epoch-invalid ([§9](#rotation)), and the device that committed r still rotates D. When the signer is the device that lost access, its envelopes reach no recipient that accepted r either: such a recipient takes an envelope only from a device that could read D at its accepted head or at the latest policy entry before the epoch's entry ([§9](#envelopes)), and the signer could do neither. A checkpoint applies the same revision rule at `covered_seq`, so a client that follows the log and one that bootstraps from a checkpoint agree about which rotations are pending.

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

HPKE runs in base mode with the suite in [§3](#3-primitives). Its `info` is the envelope prefix from the tag through `wrapping_device`, its `aad` is empty, and its plaintext is the 32-byte epoch key. A recipient accepts an envelope only when the unwrapped key matches the epoch record's `key_check` and the signature verifies under the card of a device that could read the domain at one of two points: at the latest policy entry before the epoch's entry in the domain's log, or at the recipient's accepted head. Until the recipient reaches the epoch's entry, only the second test applies, and an envelope that fails it waits for the entry. A key supplied by the server, or an envelope signed by an unpinned device or by one that lost access before the epoch's entry, fails with `sync_key_mismatch`.

The server deletes an epoch's envelopes once the next epoch of that domain commits. A device that lacks a key gets an envelope for the current epoch and reaches older keys through lineage.

### Granting and wrapping

Granting decides who may read a domain; wrapping hands an authorized device the key.

- **Granting** happens only through policy changes, signed by the devices the authority table in [§8](#roles-and-authority) allows: membership changes for the workspace domain, and a manager's access-list changes for a restricted domain.
- **Wrapping** is mechanical. Any device that holds an epoch key may wrap it for a device that its own verified policy authorizes to read that domain. The wrapping device takes the recipient's card byte for byte from the `add_device` change in its verified policy, and wraps only for a device that is active at its accepted head and authorized for the domain. It refuses otherwise and never wraps for a card the server supplies.

The server accepts a push that makes a device a reader of a domain (`add_device`, a `set_access` that grants, `create_domain` or a rotation) only when the push carries that domain's current-epoch envelope for every new reader, and answers `rotation_pending` while that domain's rotation is pending. It rejects an epoch or envelope whose recipients differ from the devices its current policy authorizes to read the domain, with `recipients_invalid`. Each device therefore receives every key its account reads when it's added, and at every rotation or grant, and with an honest server no device ever lacks an envelope. The signer of a granting change holds every key it must wrap: a manager holds its restricted domain's key, and every member's device holds the workspace domain's.

### Rotation

A domain rotates when a device that could read it loses access:

| Domain | Rotates when |
|---|---|
| Workspace | a device is removed, or a member leaves or is removed |
| Restricted | a grantee loses its grant, a grantee's device is removed, or a grantee leaves the workspace |

Exporting a recovery kit adds a device; revoking the kit removes it and rotates.

Rotating domain D at policy revision r means:

1. Generate a new epoch key.
2. Write an epoch record with `epoch` = current + 1, `policy_revision` ≥ r and at least the previous record's, a recipient list of every active device authorized to read D at that revision, and the lineage of the previous key.
3. Wrap the new key for every recipient, with cards from the verified policy.
4. Commit the epoch bundle and the envelopes in one push ([§20](#20-atomic-changes-and-quota)).

The device that commits a change requiring rotation then rotates every domain it can read, one domain per push. Any other pending domain, such as a restricted domain the remover doesn't read, or every domain a signed-out device read, rotates when a device that holds its key next writes or sends presence there. Until then the server accepts no record or presence in that domain and answers `rotation_pending`, so that device rotates first. Concurrent rotations race: the server accepts the first epoch record for each epoch and rejects the rest with `epoch_conflict`. While a rotation is pending, nothing is written or forwarded in that domain, so rotating late exposes nothing.

A restricted domain's epoch 1 commits with its `create_domain`, from the device that signed it, whose account the access list names as a manager. Clients reject an epoch 1 from any other device.

From revision r until the rotation commits, the server also stops serving D's operations and keys to the removed devices. A device keeps at most one push in flight per workspace. Every push response carries the server's policy head revision ([§15](#push)), and a device fetches the policy chain and keys before its next push whenever a push response, a pull or the realtime channel named a revision above its accepted head. When it accepts a change that requires D to rotate, it re-seals every record in its outbox that it hasn't sent and that uses an epoch the change invalidated. No device that has seen revision r encrypts under an epoch whose recipients include a device that no longer has access.

**Epoch validity.** Server and clients check one rule: an operation of domain D is epoch-valid only when its `epoch` is that of the latest epoch entry before it in D's log, and no policy entry before it requires D to rotate with a revision above that epoch record's `policy_revision`. A writer encrypts under the latest epoch it accepted only while no change in its accepted policy above that epoch's `policy_revision` requires D to rotate; otherwise it rotates D first, or waits for a device that can. The server checks a new operation against its current state and rejects anything else with `epoch_stale` or `rotation_pending`, and the writer re-seals under the new epoch ([§14](#your-own-chain)). A keystroke batch in flight across a rotation therefore costs one re-seal round trip. A client that meets an operation breaking the rule, such as an old-epoch record after a newer epoch entry, treats it like any other record an honest server would have rejected: it reports `sync_epoch_stale` and holds the domain's cursor ([§13](#client-checks-on-an-operation)).

After a bootstrap, the checkpoint stands in for the entries at or below its `covered_seq`: `epoch_record` names the latest epoch entry, `policy_revision` names the latest policy entry, and a rotation is pending at `covered_seq` when a change above the epoch record's `policy_revision`, up to the checkpoint's, requires D to rotate ([§17](#checkpoint)). That is the rule above, applied at `covered_seq`.

A reader that learns epoch e from the key route before its log entry requires that entry in D's log at or below the `head` of the first page it requests afterwards. It reports `sync_chain_gap` when its cursor passes that head without the entry, or when a record under e arrives before it.

With an honest server, nothing after a new epoch's entry uses a key that a removed device holds. A server that hides a removal from a writer lets the removed device read that writer's new records until one of them, or a later checkpoint, reaches a device that accepted the removal ([§30](#30-threats)).

**Cost.** A rotation writes one epoch record and one envelope per remaining authorized device, for the workspace domain and each restricted domain the removed device could read. Its size depends on devices and domains and never on objects. [§28](#28-worked-example-100000-objects) works through the numbers.

**What rotation protects.** A removed device keeps the keys and plaintext it already had. It can't decrypt anything encrypted under a later epoch, and its card never rejoins the workspace ([§8](#policy-change-record)). Lineage gives every holder of a current key every earlier key, so a member that loses access and later gets it back, on new devices, can read what was written while it was away, if it can get that ciphertext. The app warns whoever invites a member again, or grants a restricted domain to an account again, that the account will be able to read what was written while it was away.

### History floor

A domain key opens every earlier ciphertext in that domain through lineage, so the server decides how much history a new member sees. The server keeps a history floor per account and domain. It never serves an entry with a sequence at or below the floor through pulls or chain fetches. `live` and `manifest` answer only at a checkpoint whose `covered_seq` is at or above the floor, and there they serve every operation live at that checkpoint, whatever its sequence, because a member sees current content ([§18](#18-bootstrap)). The server serves a blob only when an operation that lists it sits above the floor or is live at the checkpoint the device reads.

| Event | Floor |
|---|---|
| A member joins | `covered_seq` of the checkpoint committed with the approval |
| A grantee joins a restricted domain | `covered_seq` of the checkpoint committed with the grant |
| A member's new device | the member's existing floor |
| A device restored from the member's recovery kit | the member's existing floor |

An approval or grant that sets a floor carries a fresh checkpoint for each affected domain, signed by a device that can write it, and the floor equals that checkpoint's `covered_seq`. That `covered_seq` may equal the sequence of the domain's latest checkpoint entry, so a quiet domain still takes one ([§17](#checkpoint)). Records that land between signing and commit sit above the floor, so the new device pulls them normally. A change that repair brings back after a restore carries no checkpoint, so its floor is the first checkpoint that commits after its entry, which the server makes due at once ([§17](#checkpoint)). The server answers a request below the floor with `before_history_floor`, and the client bootstraps.

History isolation for new members depends on the server. [§30](#30-threats) records that limit.

## 10. Approval

Approval pins a new device to devices you already trust, and the reverse, with one short code shown on both screens. The code mixes in a secret that the candidate commits to before it sees the offer, so a party in between can't steer the code.

### Approval request

The candidate enrolls and, for each attempt, draws a fresh 32-byte nonce `nc` and posts a request (`POST /v2/approval-requests`) carrying its commitment:

```text
commit = BLAKE3(tag(noura.sync.approval.commit) || candidate_card_digest || nc)
```

where `candidate_card_digest` is `BLAKE3(candidate card)`. For an invitation, the request also names the invitation and carries the invitation proof ([§11](#11-invitations)). A request expires 10 minutes after the candidate posts it. Approvers see the card and the request through `GET /v2/approval-requests` for devices of their own account, or `GET /v2/workspaces/{w}/pending-devices` for invited accounts.

### Approval offer

The approving device catches up on every domain the approval covers, then sends a signed offer.

`offer`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.approval.offer)` | |
| approver_card | `bytes` | encoded card of the approving device |
| candidate_card_digest | `digest` | `BLAKE3(candidate card)` |
| candidate_commit | `digest` | the request's `commit` |
| nonce | `key` | 32 random bytes chosen by the approver after it received the request |
| anchors | `list<anchor>` | sorted by workspace |
| sig | `sig` | by the approving device |

`anchor` is `id workspace | u64 revision | digest policy_head | u64 generation | list<checkpoint_anchor>`, and `checkpoint_anchor` is `id domain | u64 covered_seq | digest checkpoint`, sorted by domain: the approver's accepted policy head in each workspace the approval covers, the restore generation in which it accepted the checkpoints that follow ([§14](#after-a-server-restore)), and its latest accepted checkpoint in each domain the candidate will read. Approving your own new device lists every workspace your account belongs to. Approving an invited member's device lists one workspace.

A restore hands out again sequences that clients saw, so a device compares a checkpoint anchor with a checkpoint only when both come from the same generation. When a restore lost the anchor itself, the device accepts the server's checkpoint under the other bootstrap checks and shows the restore alert of [§14](#after-a-server-restore).

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
5. The approving device commits the approval with one push per anchored workspace: the `add_device` change (with `offer` set to `offer_digest`) and a current-epoch envelope for every domain the candidate's account reads ([§9](#granting-and-wrapping)), and, for a new member, `add_member` at the revision before the `add_device` ([§8](#roles-and-authority)) and a fresh checkpoint of the workspace domain, so the member's floor has a checkpoint to bootstrap from.
6. The candidate pins the approver's card and anchors. It accepts the approval only from that card, and verifies each workspace's chain from genesis to its anchor, then onward.

Before revealing `nc`, the candidate checks that `candidate_card_digest` equals `BLAKE3` of its own card and `candidate_commit` equals its own commitment; otherwise it reports `sync_approval_mismatch` and reveals nothing. Each `nc` serves one offer and one comparison: the candidate never shows a code for any offer but the first. When a second offer names the same commitment before you confirm, whether it arrives before or after the reveal, the candidate ends the attempt with `sync_approval_mismatch`, discards `nc`, and shows no code; a new attempt needs a new request with a fresh `nc`. A second offer that arrives after you confirm changes nothing, and the candidate reports it. If two of your devices both answer a request, the app asks you to approve from one of them. An approving device sends an offer only for a request you selected on it, keeps at most one offer outstanding, and shows a code only for that offer; it ignores other requests until that attempt ends. After you confirm, the candidate verifies each workspace's chain up to its anchor and checks that the approver's card belongs to the policy at that anchor with the authority to sign the `add_device` that follows. It bootstraps each domain only from its checkpoint anchor or a checkpoint that descends from it ([§17](#checking-a-checkpoint)), else `sync_manifest_rollback`. For an invitation, the approver must be the inviting device: `BLAKE3(approver_card)` must equal the context's `inviter_fingerprint` ([§11](#11-invitations)). An offer expires 10 minutes after the approver sends it.

A server that swaps either card must show each device an offer or a commitment of its own choosing. The candidate's code depends on `nc`, which the server learns only after it has fixed the one offer the candidate will ever use with that `nc`. A server that learns `nc` and then grinds further offers toward the approver's code gains nothing, because the candidate ends the attempt at the second offer. The approver's code depends on an offer that already commits to the request, so the server can't change it after the approver signs. The approver offers only on the request you selected and keeps one offer outstanding, so the server learns one approver code per attempt you start and can't collect codes from requests it posted itself. The two codes therefore match with probability 10^-8 per attempt, and every attempt asks you to confirm. Passing the approver's offer for a substitute card through to the real candidate fails the digest check.

## 11. Invitations

An owner or admin invites someone by link. The link carries an invitation secret in its fragment, so the server never sees the workspace name or the inviter's details.

```text
{origin}/invite/{base64url(token)}#{base64url(secret)}
```

The inviting device generates the invitation ID (16 bytes), the token (16 bytes) and the secret (32 bytes), and keeps the secret until the invitation ends. While the invitation is open, the server stores `BLAKE3(tag(noura.sync.invite.token) || token)`, the workspace, the role, the inviting account, the accepting account once known, the status, an expiry seven days out, and the context below. It deletes the row once the invitation is approved, revoked or expired. A workspace holds at most 100 open invitations.

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

Your device's approval request names the invitation and carries `invite_proof = keyed_hash(derive_key("noura.sync.v2 invitation proof", secret), card)`. The inviting device checks the proof before it sends an offer, so a server that never saw the secret can't put its own card in your place. The inviting device then approves your device with an approval code ([§10](#10-approval)), which you compare with the inviter over a channel you both trust, such as a call. Only that commit adds you to the policy and wraps keys for you. Your app checks that the approval offer comes from the card matching `inviter_fingerprint`, that the `add_member` change names this invitation, and that the change right after it adds your own card. Only the inviting device can approve; if that device is lost, an owner or admin revokes the invitation and sends a new one. Revoking an invitation makes its link fail.

## 12. Recovery kit

A recovery kit is an offline device. Exporting a kit creates a recovery identity, a device card with its own signing and HPKE keys, and adds it with `add_device` (`device_kind` 2) to every workspace your account belongs to. When your account creates or joins a workspace, your device adds the recovery identity there, if it pinned that identity's card ([§7](#pinned-cards)), in the creation request or its first push after approval; otherwise another device of your account that pinned it adds it ([§8](#verifying-the-chain)). The app then asks you to export a new kit so the kit lists that workspace. Rotations wrap keys for the recovery identity like any other device, so the kit never goes stale. Removing the recovery identity from your device list revokes the kit, and exporting a new kit removes the previous recovery identity before it adds the new one, so one kit works at a time ([§8](#policy-change-record)).

A kit is also a standing key. Because every rotation wraps the new key for the recovery identity, the kit file and its secret together decrypt every current and future record in every domain your account reads, for anyone who also gets the ciphertext: the server's operator, a copy of a server backup, or someone holding your account session. That reader needs no device, and nothing alerts you. When you export a kit, the app says so and asks you to keep the file and the secret in different places. It offers to revoke the kit and export a new one after you remove a device, and whenever you suspect the kit was exposed.

A recovery identity can sign two kinds of change ([§8](#roles-and-authority)): `add_device` for one new device of its own account per workspace, and `remove_device` for devices of its own account. Clients reject any other change it signs.

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
| card | `bytes` |
| signing_seed | `key` |
| hpke_secret | `key` |
| workspaces | `list<kit_workspace>` |

`kit_workspace` is `id workspace | digest genesis | u64 revision | digest head | u64 generation | list<checkpoint_anchor>`, sorted by workspace: the digest of the workspace's revision-1 policy change, the accepted policy head, the restore generation of the checkpoints that follow, and the latest accepted checkpoint per domain when the kit was written. The device compares a checkpoint anchor only with a checkpoint of the same generation ([§10](#approval-offer)). `hpke_secret` is the 32-byte X-Wing seed. A wrong secret or tampered file reports `sync_recovery_kit_invalid`.

To recover after losing every device, you sign in on a new computer, enroll it, import the kit and type the secret. The new device:

1. Verifies the kit's card against its seeds.
2. Proves to the server that it holds the recovery identity's signing key. It signs a `recovery_proof` and posts it to `POST /v2/recovery`; for the rest of that session the server lets it read the envelopes addressed to the recovery identity and submit changes the recovery identity signs.
3. Checks that each listed workspace's chain starts at the stored genesis digest and extends the stored head (a shorter chain reports `sync_policy_rollback`), and refuses a checkpoint below the kit's checkpoint anchors. It shows you every policy change after the kit's head, with its signer, and you confirm them before it goes on.
4. Signs, as the recovery identity, `add_device` for itself in each workspace, unwraps the recovery identity's envelopes and wraps every key for itself.
5. In the same flow, removes every other device of your account, unless you keep one you still hold, and the recovery identity, which rotates every domain. Removing the recovery identity never takes the recovering device with it, although the identity added it ([§7](#removing-a-device)). It lists the members added after the kit's head for you to review, then exports a new kit with a new secret.

The kit's head is the recovering device's only floor against rollback. A server can show it the chain up to that head, extended with changes that a device removed after the kit was written still signs, such as your stolen laptop adding devices. Removing every other device of your account by default keeps the rotations in step 5 from wrapping keys for those. To keep the floor recent, the app asks you to export a new kit after each device removal.

`recovery_proof`, signed with the recovery identity's signing key:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.recovery.proof)` | |
| origin | `str` | as in the enrollment proof |
| recovery_device | `id` | the recovery identity's device ID |
| new_card_digest | `digest` | `BLAKE3` of the new device's enrolled card |
| challenge | `key` | a fresh enrollment challenge |

The server checks the signature under the recovery identity's card, that the card belongs to the session's account and isn't removed, that the new card is the session's enrolled device, and that the challenge is current and unused.

The recovered device becomes a trusted device of your account: it can approve your later devices. The kit restores keys, never the old device's signing identity or session.

A workspace the kit doesn't list, such as one you joined after writing the kit, is accepted only when the change that added the recovery identity there was signed by a device of your account that the new device verified in a listed workspace's chain, directly or through your account's `add_device` changes after the stored head. Otherwise the new device refuses that workspace with `sync_workspace_unverified` and tells you it can't verify it.

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
| 3 | text_snapshot | state | `str object_id`, `digest revision`, `id text_lineage`, `content` holding an encoded yrs document, `opt<origin>` |
| 4 | text_update | patch | `str object_id`, `id text_lineage`, `content` holding a yrs update |

- `object_id` is the managed file's frontmatter ID, or empty for an ordinary file.
- `path` is a workspace-relative UTF-8 path of at most 4,096 bytes, with `/` between components. Clients reject absolute paths, `.` and `..` components, empty components, control characters, Windows reserved names and characters, trailing dots or spaces, and the reserved roots `.noura` and `.git` in any ASCII case. Paths compare byte for byte; [§16](#16-conflicts) defines when two paths collide.
- `revision` is the BLAKE3 hash of the file's bytes as materialized; for a delete, of the deleted bytes, or zeros when unknown.
- `content` is `u8 form` then either form 1, `bytes inline`, or form 2, `blob_ref`.
- `blob_ref` is `id blob | u64 ciphertext_length | digest ciphertext_digest | key blob_key | u64 plaintext_length` ([§21](#21-attachments)).
- `origin` is `id domain | id rid`: in a state operation, where a cross-domain move came from; in a reason-2 delete, where it went.
- `text_lineage` names the yrs document of a text-mode note ([§22](#22-live-collaboration)). That document holds the note's path and properties too, so a text-mode note's path comes from the document, not from the payload. The collaboration sandbox applies the path rules above to every path the document sets ([§22](#collaboration-sandbox)).

An inline payload holds at most 262,144 bytes before padding. Larger content goes in a blob.

### Supersession and live operations

The server and the client compute the same live set for each `rid` from cleartext fields: an operation is live unless another operation on the same `rid` covers it. The server needs no plaintext to compute it, compact the log, or check a manifest.

- A state operation covers the writer's view of that `rid`, naming each covered device's highest live counter, its own device's included. While the `rid` holds one live state operation, that view is every live operation. While it holds two or more, a conflict ([§16](#16-conflicts)), an ordinary edit or delete covers only the branch the writer's file matched, with that branch's patches and its own device's earlier operations, so the other branches stay live; only a choice between branches, or the merge of disjoint changes, covers every branch. A delete sets `flags` to 1, and once it covers every live operation the tombstone stays live alone until compaction drops it ([§19](#19-compaction)). A delete that covers nothing on an existing `rid` counts as a patch, and clients treat it as unreadable ([Client checks](#client-checks-on-an-operation)). Covers name only live operations, and the server prunes nothing live except such a lone delete behind the retention floor.
- A patch covers nothing. It applies to the text lineage it names ([§22](#22-live-collaboration)).

The server counts an operation that covers nothing on an existing `rid` as a patch, and any other operation as a state operation. When a state operation would need more than 256 covers, it covers the 256 devices with the most live operations on that `rid`, its own device among them, and the rest stay live.

### Server checks on an operation

The server accepts an operation only when:

- the session's device is the signer and is active in this workspace's policy, and the signature verifies under its card in the policy chain;
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

Before decrypting, the client verifies the signature under a pinned card from its verified policy, checks `policy_head` against its own change at `policy_revision`, checks that the signer could write the domain at the latest policy entry before the record and at `policy_revision`, which is at most that entry's revision ([§8](#policy-entries)), checks that neither `policy_revision` nor `generation` decreased along the chain, checks that the operation is epoch-valid ([§9](#rotation)), runs the chain verifier ([§14](#14-device-chains)), and checks the checkpoint it names ([§17](#checkpoint-chain)). A failure reports `sync_writer_not_authorized`, `sync_policy_forked`, `sync_policy_regressed`, `sync_epoch_stale` or a chain error, changes no file, and holds the domain's cursor. An honest server rejects such a record, so the failure shows a lying server or a misused key.

A record that passes those checks but fails to decrypt, unpad, parse, match its header's `rid` and delete flag, match the class the server gave it, or pass the collaboration sandbox ([§22](#22-live-collaboration)) came from its signer's key, and the server couldn't see the problem. A payload matches its class when a `file`, `delete` or `text_snapshot` sits in an operation the server counts as a state operation and a `text_update` in one it counts as a patch ([Supersession and live operations](#supersession-and-live-operations)). The client counts it as applied: its chain head, cursor, live set and manifest include it, and a later state operation can supersede it. The client keeps the object's last good state on disk, lists the record in the review store as an unreadable version attributed to its signer, and reports `sync_record_unreadable` with the device and domain. A record encrypted under a different epoch record for the same epoch fails to decrypt the same way ([§4](#records-signatures-and-digests)), and checkpoints then expose the split as `sync_key_mismatch` ([§17](#17-manifest-and-checkpoints)).

## 14. Device chains

Each device keeps one chain of operations per domain it writes to. Every record carries the device's counter and the digest of its previous record under the signature, so a server can't remove, reorder or duplicate a record without breaking a link that the device signed.

Chains are per domain because a member outside a restricted domain never sees that domain's records. A single chain per device would show that member gaps it could never explain.

### Verifier

The client keeps, per domain and device, the accepted head `(counter, digest, sequence)` and the counter, digest and sequence of each accepted record above its latest accepted checkpoint in that domain; below that checkpoint it keeps only heads. Its verifier state therefore stays within one checkpoint interval per domain ([§17](#checkpoint)). Heads taken from a checkpoint bundle carry their sequences ([§17](#17-manifest-and-checkpoints)), so these checks work right after bootstrap. The client processes each pull page in sequence order. For a record with counter `c`, previous digest `p` and digest `h` at sequence `s`:

1. **Counter at or below the head.**
   - The client holds the record at `c` with digest `h` at sequence `s`: the page repeats a position the client already applied (for example after a restart), so the client ignores it.
   - The client holds the record at `c` with digest `h` at another sequence: `sync_chain_replayed`.
   - The client holds a different record at `c`, and one of the two names the other in `previous_attempt`, directly or through other attempts at `c` that the client has seen or the page carries: the two are attempts at one position ([§14](#your-own-chain)), and the one whose links reach the other is the later. If the client holds the later attempt, it ignores the earlier. If it holds the earlier attempt as its head, with no successor applied, it switches to the later attempt and recomputes the object's live set. If the earlier attempt has a successor that the client applied, it reports `sync_chain_forked`.
   - The client holds a different record at `c` and neither names the other, or two records at `c` name the same `previous_attempt`: `sync_chain_forked`. Attempts at one position form a single line, each naming the one before, and their number has no cap when newer epochs or policy changes force the re-seals ([§14](#your-own-chain)).
   - The client holds no record at `c` (it bootstrapped past it, the record sits below its latest accepted checkpoint, or the device skipped that counter): every legitimate record at or below the head has a sequence at or below the head's, so `s` above the head's sequence reports `sync_chain_replayed`.
2. **Counter above the head, and `p` equals the head's digest** (32 zero bytes when the client knows no record of that device in the domain): accept and advance the head. A counter more than one above the head skips counters the device reserved and never sent, or signed before a server restore lost them ([§14](#your-own-chain)); the device's signature over `previous` shows that it skipped them.
3. **Counter above the head, and `p` differs from the head's digest.** The page left records out, the record follows a later attempt of the head, or the chain forked. The client first fetches the head's own position: when the server returns a later attempt of the head that rule 1 accepts, the client replaces the head with it and judges the record again. Otherwise it fetches the device's records after its head in chain order (`GET .../chain/{device}/{counter}` returns the record at that counter or, when the device skipped it, the device's next record above it, [§26](#26-http-api)) and judges each by these rules until it reaches the record. A fetched record whose `previous` names neither the head nor the record fetched before it reports `sync_chain_forked`. A fetched record at a sequence above `s` reports `sync_chain_reordered`. One at a sequence below `s`, or an answer that leaves the chain short of the record, reports `sync_chain_gap`. A `pruned` answer means the client's head fell below the retention floor, and the client bootstraps ([§18](#18-bootstrap)).

On any of these errors the client applies nothing from the page, changes no file, keeps its cursor and reports the error with the device and domain. It retries on the next pull, so a transient server fault clears itself.

A device that forks its own chain halts that domain for its readers until someone removes its write access. A reader holding `sync_chain_forked` for device X in domain D keeps its cursor until it accepts a policy change r that ends X's write access to D. It then fetches checkpoints from the latest back through `previous` to the first whose `policy_revision` is at or above r, which is the first to cover r's entry, keeps the branch of X whose head that checkpoint lists, ignores the other branch's records, and resumes. The checkpoint still gets its full check when the reader reaches it in its log ([§17](#checking-a-checkpoint)).

A retried upload never triggers an error: the server stores one copy, and a page that repeats a position at the same sequence is ignored.

### Suffix withholding

A chain proves what the server served, never that it served everything. A client learns a device's latest counter from three sources: later records from that device, the chain heads in each checkpoint, which reach every puller as log entries ([§17](#17-manifest-and-checkpoints)), and, for its own chain, the receipts the server returned. A head above what the client holds, at a sequence the server claims to have delivered, reports `sync_chain_gap`. Checkpoints form a chain, and every record names the latest checkpoint its writer reached ([§17](#checkpoint-chain)). A server that hides a checkpoint from one client must therefore also hide every later checkpoint and every record whose writer reached it, and the split shows once one such record or checkpoint crosses to that client. A server that withholds a device's records from everyone, and every checkpoint mentioning them, looks like a device that went offline; the app shows when it last heard from each device.

### Your own chain

A device persists each signed record before sending it, so a retry sends the same bytes. The credential store holds a reserved counter per domain ([§15](#client-journal)), and the device signs no counter above it: it first raises the reservation by a block of 1,024, so the credential store takes one write per 1,024 records. Each time the app starts, the device raises every reservation by one more block and signs its next new record in each domain at the start of that block, with `previous` naming its last record there ([Verifier](#verifier) rule 2). The counters it skips were either never signed, or signed into a journal that a crash or a restored backup rolled back, so the device never signs one counter twice, whatever order a crash leaves the journal and the credential store in. On startup, and on `chain_conflict`, the device compares its journal's head with the server's head for its own chain (`GET .../heads/{device}`):

- A server head above the journal's means the journal lost records that the device sent. The device verifies that the server's records above its journal carry its own signature, takes them into its journal, and continues after them.
- A server head below a record the server acknowledged, or below the device's head in its latest accepted checkpoint, in the same restore generation, means the server went back: the device reports `sync_server_rollback` ([§15](#pull)). After a generation change, [After a server restore](#after-a-server-restore) applies.
- A journal without a reserved counter in the credential store means the keys are new: the machine enrolls as a new device ([§7](#device-card)).

When the server rejects a record for a reason the device must fix (`epoch_stale`, `rotation_pending`, `supersedes_unknown`, `blob_incomplete`, `policy_regressed`), the device first checks the reason against data it can verify:

- for `epoch_stale` and `rotation_pending`, it fetches policy and keys, and re-seals only after accepting a newer epoch or a change that requires one; a `rotation_pending` with no change behind it is legitimate once per domain after each restore's quiet period, and the device then rotates the domain ([After a server restore](#after-a-server-restore));
- for `supersedes_unknown`, it fetches each covered operation by chain position and drops the covers the server can't return;
- for `blob_incomplete`, it reads the upload's offset;
- for `policy_regressed`, it rereads its accepted policy and its own chain.

When the check contradicts the server, the device doesn't re-seal: it retries the same bytes later and reports the server's error. Otherwise it re-seals the change at the same counter with `previous_attempt` set to the digest of the attempt the server rejected last at that position. A re-seal that follows a newer epoch or a rotation-requiring policy change the device accepted has no cap. Any other re-seal counts toward a cap of 3 per position. At the cap the device reports `sync_reseal_limit` for that domain, signs no other attempt at that position, and stops writing the domain: it retries the last attempt's bytes later, and its local files keep their changes. A rejected push names the failing record ([§26](#message-bodies)), and the device re-seals that record and every later record of the same chain in the batch, whose `previous` links change; each of those names the attempt it replaces at its own position. A server that kept a rejected attempt and serves it shows readers two consecutive attempts, which rule 1 above resolves without decryption. A device whose write access is gone drops the record; its local file keeps the change.

Three rejections concern the record's object, not the record: `snapshot_required`, `conflict_limit` and `quota_exceeded`. For these the device puts a different record at the same position instead of the same change, so one busy object never stalls the rest of the domain. For `snapshot_required` it writes a text snapshot that includes the rejected patch ([§22](#22-live-collaboration)). Otherwise it sends the next record in its outbox, such as a delete you made to free space, or retries later when the outbox holds nothing else, and keeps the rejected change locally: it retries a `quota_exceeded` change once space frees up, and shows a `conflict_limit` change as a conflict for you to resolve, which the server accepts because a resolution covers every live branch. The replacement names the rejected attempt in `previous_attempt` and doesn't count toward the cap.

### After a server restore

A restore from a backup brings back older server state. The server forgets the policy changes, epochs, envelopes, records, chain heads, checkpoints, accounts, sessions, revocations, share links and blob rows created after the backup. Devices hold most of what it lost: the policy and keys in their journals, and the content in their files.

**Server secret.** The server keeps a 32-byte secret outside its database: a file in its data directory that the first `migrate` creates, or a file that every instance of a PostgreSQL deployment reads. A database backup never contains it, and a restore leaves it alone. The server derives two keys from it with BLAKE3 `derive_key`. The `noura.sync.v2 account id` key gives each account its ID: the first 16 bytes of `BLAKE3.keyed_hash(key, email)`, over the email address the account signed up with, in lowercase. v2 has no route that changes that address, so signing up again after a restore recreates the account under the ID its policy chains name, and nobody else can claim that ID. The `noura.sync.v2 commit tag` key tags each policy change, epoch bundle and envelope the server commits:

```text
commit_tag = BLAKE3.keyed_hash(commit tag key, tag(noura.sync.commit) || workspace || u64 generation || digest record)
```

where `generation` is the restore generation at commit and `record` the record's digest, or for an epoch bundle its epoch record's. Push, workspace creation and repair return the generation and tag of each policy change and epoch bundle they commit, and `policy` and `keys` serve them with every record. Clients keep them with the record and never check them: a tag only proves to the server that it committed that record before.

**Restore generation.** The server keeps one restore generation for all its workspaces. `backup` copies a running SQLite database with `VACUUM INTO`, which reads one snapshot beside `serve`, then stamps the copy, and `serve` refuses to start on a stamped database. While the copy runs, the write-ahead log grows by everything the server writes meanwhile, because no WAL checkpoint can pass the copy's snapshot. To restore SQLite, the operator runs `restore` with the backup file; it replaces the database alone. After restoring PostgreSQL with its own tools, or failing over to a standby that may lack commits, the operator runs `mark-restored` before `serve`. Both raise the generation to the larger of the stored generation plus 1 and the current Unix time in seconds, so a restored copy never repeats a value clients saw after an earlier restore. Both clear the stamp and check the blob store against the database ([§19](#19-compaction)). The server reports the generation in the `Noura-Restore-Generation` header of every response, in every pull page, and in a `restored` realtime frame ([§23](#realtime-channel)), so a device learns of a restore from its next request of any kind. Clients can't check the generation: it is the server's claim, and every change of it shows you an alert that the server reports a restore from a backup. A generation below one the client saw reports `sync_server_rollback`.

**Restores the server didn't record.** A database put back any other way, such as a file copy, a virtual machine or filesystem snapshot, point-in-time recovery or a failover to an asynchronous standby, hands out again, in the same generation, sequences that clients already passed. The server catches most of these: once a minute an instance raises a counter in the database and writes the counter and the generation to a heartbeat file beside its secret, and it refuses to start, or stops serving, while the database shows that generation with a lower counter. Clients catch the rest: a pull page `head` or `checkpoint_seq`, a `heads/{device}` answer or a checkpoint `covered_seq` below one the client saw in the same generation reports `sync_server_rollback`. The client then stops advancing and writing that workspace and alerts you until the generation changes. A PostgreSQL failover target therefore replicates synchronously, and any restore or failover that can lose commits needs `mark-restored`.

**Quiet period.** For 7 days after a generation change, push takes no policy change, epoch bundle or envelope, and answers `restore_quiet`. These records come back only through repair, so no change signed since the restore can take the revision of one that a device is about to bring back. A domain whose required rotation the restore lost takes no writes until the period ends. When it ends, every domain's rotation falls due once ([§9](#epochs)): the server answers `rotation_pending` until one epoch per domain commits with a `policy_revision` at or above the policy head at that moment. That rotation wraps the newest key for every device the policy authorizes, so the recovery identity and the devices whose envelopes the restore lost read again, and lineage gives them every earlier key.

**Repair.** A client that sees a new generation resubmits through `POST .../repair`, with the generation and commit tag it holds for each record:

1. the accepted policy changes the server lacks, in revision order;
2. the epoch bundles it accepted that the server lacks, in epoch order per domain, and the envelopes addressed to it.

While it resubmits, a server policy head below the client's own reports no rollback. Repair takes a record only when its commit tag verifies for a generation below the current one (else `commit_unknown`), so it takes back what the server committed before the restore and never a record signed since. It checks each record against the chain at its own revision: its signature, its chain links, and its signer's authority in the state before it. It waives the checks that tie such a record to rows the restore lost or to the push that first carried it:

- the coupling of another account's `add_device` with its `add_member`, which clients still check in the chain ([§8](#roles-and-authority));
- the accepted invitation behind `add_member` and the enrollment behind a card;
- the envelopes that a granting change must carry, and epoch 1 committing with its `create_domain`;
- `rotation_not_due`, and the check of recipients against the current policy; repair checks them at the epoch's own revision instead.

A record whose revision or epoch another record already holds fails with `policy_conflict` or `epoch_conflict`. Repair has no deadline: after the quiet period, a tagged record still lands while its position is free. Repair serves a session whose device is active once the request's own changes apply, so a device approved after the backup can bring back the chain that adds it, and it serves a recovery identity's proven session. When the backup predates the workspace, the first repair request starts with its genesis; the tag and the derived workspace ID ([§5](#5-identifiers)) keep anyone else from creating a workspace under that ID first. Repair returns receipts and new commit tags, so a later restore can repair again.

Clients judge epochs by revision ([§9](#epochs)), so a resubmitted epoch whose entry lands after later replayed changes reads as it did before. An `add_member` or a grant that repair brings back sets that account's history floor at the first checkpoint that commits afterwards in each domain it reads; the server makes that checkpoint due at once ([§17](#checkpoint)) and serves the account nothing in the domain until it commits.

**Writing after a restore.** After a generation change, a client writes nothing to the workspace, no record, presence, upload or policy change, until the server's policy head reaches the client's accepted head and the server holds every epoch record the client accepted. A device that holds a removal therefore brings it back before it writes again, and never writes under a key the removed device holds. When another record already holds a revision or epoch the client accepted, the client reports `sync_policy_forked` or `sync_key_mismatch` for that workspace and lists the changes the server lost, removals first. The app offers to enroll the machine as a new device, and you make those changes again. Whatever the server shows, the client accepts no change that a card on its removed list signed, as [§7](#pinned-cards) describes.

Each device also signs again a revocation for every card of its account on its removed list ([§7](#desktop-sign-in)), and revokes again each share link it revoked before ([§24](#24-share-links)). An invitation that the restore brings back stays harmless: only the inviting device approves, and it discarded the secret when it revoked the invitation.

**Reconciling content.** Repair carries no operations or blobs. Each device's files hold the content, and devices write what the restore lost as new operations. Once the server holds its accepted policy and epochs, a client takes each domain through these steps:

1. It rebuilds the domain from the server as a returning device does ([§18](#18-bootstrap)), from the server's latest checkpoint, which must be one the client accepted or descend from one ([§17](#checking-a-checkpoint)). Its chain heads, verifier state and live set become the server's, so the records the server lost leave them.
2. It finds each `rid` whose server state lies behind its journal: one the server lacks, or one whose live operations on the server the journal's live operations all cover. It waits a random 0 to 60 seconds, then, for each such `rid` that still lags, writes the journal's last applied state again as a new operation covering the server's live operations: the file or delete, reusing its `blob_ref` and uploading the blob again byte for byte when the server lacks it ([§21](#blob-format)), or, for a text-mode note, a `text_snapshot` of its document under the same lineage. When another device wrote the same state first, the two branches are identical and merge ([§16](#16-conflicts)), as do two snapshots of one lineage. For a text-mode note whose lineage the server still holds live, the client sends as a `text_update` what its document holds beyond the server's state.
3. It drops each signed record in its outbox whose `previous` the server doesn't hold; the change stays in the file.
4. It signs its next record in the domain above every counter it signed there, with `previous` naming the server's head for its chain ([Verifier](#verifier) rule 2).

Any other difference between a file and the rebuilt state then counts as a local edit, as after any bootstrap. When the steps finish, the app lists the objects the device wrote again. A lost change comes back from any device that applied it, even when its author device no longer exists. A change that no surviving device applied is gone, and its object returns to its state in the backup.

**Removed devices.** A server that lost a removal accepts the removed device's new records until repair brings the removal back, and those records sit before the replayed change's entry, where the positional rule allows them. A client that accepted, before it saw the generation change, a change ending device X's write access to domain D treats as unreadable ([§13](#client-checks-on-an-operation)) every record of X in D above X's bound: X's head in the first checkpoint covering that change's entry, if the client accepted one, or else the highest counter it held from X. The client keeps these bounds through the rebuild. It counts each such record as applied, writes none of its content, lists it for review and reports `sync_writer_not_authorized`. A client that learns of the change only after the generation change can't tell those records from honest ones; the devices that knew raise the alert ([§30](#30-threats)).

**Recovering devices.** A recovering device can't resubmit changes after its kit's head, because the kit holds only that head's digest. When a restored server's head lies below the kit's, the device waits for another device to bring the changes back, and reports `sync_policy_rollback` for that workspace if none does by the end of the quiet period. When nothing can close a gap, an owner device can turn sync on for its folder as a new workspace.

## 15. Domain log, cursors and pulls

Each domain has its own log. The server assigns each accepted operation, checkpoint, epoch entry ([§9](#epochs)) and policy entry ([§8](#policy-entries)) a sequence number, increasing within the domain and not necessarily dense. Per-domain sequences keep a member outside a restricted domain from counting its activity through gaps.

Records become visible in sequence order: the server assigns a domain's sequences inside the committing database transaction, under the locks that [Store transactions](#store-transactions) orders.

### Push

`POST /v2/workspaces/{w}/ops` carries `list<record_ref>`, at most 1 MiB, where `record_ref` is `u8 record_type | bytes record` with types 1 operation, 2 policy change, 3 epoch bundle, 4 envelope, 5 checkpoint bundle and 6 profile. A push may span domains. The server decodes it and checks its signatures, padding and ciphertext lengths before it opens its database transaction, then validates the records in order against the current state plus the records before them, and commits all of them in that one transaction or none. It returns its policy head revision, `list<receipt>`, one receipt per log entry the push created, in request order, with a policy change's entries in domain-ID order, and `list<commit>`, one per policy change and epoch bundle, in request order. A receipt is `digest record_digest | id domain | u64 seq`, where `record_digest` is the record's digest, and for an epoch or checkpoint bundle the digest of the epoch or checkpoint record inside it. A commit is `digest record_digest | u64 generation | digest commit_tag` ([§14](#after-a-server-restore)). Like every response, a push response carries the restore generation, and a device that sees a new one stops writing and follows [§14](#after-a-server-restore) before it signs anything else. A device keeps at most one push in flight per workspace and fetches the policy chain before its next push when the returned revision is above its accepted head ([§9](#rotation)). Every atomic change fits one push ([§20](#20-atomic-changes-and-quota)). Besides workspace creation and repair, push is the only route that writes records, so the one in-flight rule covers every write.

### Store transactions

Both stores commit a push the same way, so no record lands after a change that forbids it and no two commits deadlock:

- **Lock order.** Each push and each repair request takes, in this order: the workspace's policy lock, shared for a request without a policy change and exclusive for one with a policy change; the workspace's key-sequence row, when the request carries an epoch bundle or envelope; the head rows of the domains it touches, in domain-ID order; then the rows of the blobs it lists, in blob-ID order. In PostgreSQL the policy lock is a transaction-level advisory lock keyed by the first 8 bytes of the workspace ID, `pg_advisory_xact_lock_shared` or `pg_advisory_xact_lock`, so concurrent pushes don't lock one hot row; in SQLite the single writer stands in for it. The collector, tus creation and the share routes, which change a domain's usage, lock that domain's head row before any blob row. Commits into different domains of one workspace proceed in parallel unless one of them changes the policy.
- **Reads after locks.** The server reads policy, epoch, rotation, checkpoint and quota state only after it holds those locks, under READ COMMITTED in PostgreSQL and inside `BEGIN IMMEDIATE` in SQLite, so it never validates a record against a policy older than the one it commits after.
- **Sequences.** The server assigns a domain's sequences while it holds that domain's head row, so no record commits below a sequence a reader has already passed. Key sequences follow the same rule on the key-sequence row.
- **Quota.** Each domain's head row carries the domain's usage and history weight ([§20](#20-atomic-changes-and-quota)), and the quota check sums the workspace's domains, so pushes into different domains don't queue on one quota row. The sum may miss a concurrent push into another domain.
- **Reads.** A pull, `live` or `manifest` response comes from one snapshot: a read-only REPEATABLE READ transaction in PostgreSQL, one read transaction in SQLite. `complete_through`, `retention_floor` and the entries therefore agree while pruning runs. No read transaction stays open across a pull's wait: the server waits outside the database and takes the snapshot when it has something to return.
- **Pruning.** One instance at a time prunes a domain, under a per-domain advisory lock in PostgreSQL. It persists the new floor before it deletes anything, then deletes in batches in ascending sequence order ([§19](#19-compaction)).
- **SQLite.** SQLite allows one writer, so commits from every workspace on a server queue behind each other. The server holds the writer only while it validates and commits pushes it has already decoded and checked. It may commit several queued pushes in one transaction with a savepoint per push, so one push's failure rolls back only that push and one fsync covers the group, and it runs WAL checkpoints itself so the write-ahead log stays bounded. SQLite mode suits a self-hosted server for one team; a hosted service with many tenants runs PostgreSQL.

### Pull

`POST /v2/workspaces/{w}/pull` takes `list<pull_cursor>`, each `id domain | u64 after`, and a wait of up to 25 seconds, and returns a page for each domain with records after its cursor, at most 1 MiB in all. It waits while none has any. A client pulls every domain it reads through this one request, so a device makes one pull per round however many domains exist.

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

`entry` is `u64 seq | u8 record_type | bytes record`, with record types 1 operation, 5 checkpoint bundle, 8 epoch record ([§9](#epochs)) and 9 policy entry, whose record is `u64 revision | digest head` ([§8](#policy-entries)). A page holds at most 1 MiB. A page whose status isn't 1 holds no entries, its `complete_through` equals `after`, and `head` is 0; status 2 still carries `retention_floor` and `checkpoint_seq`, and statuses 3 to 5 set both to 0; `checkpoint_due` is 0 for every status but 1. One domain's status never fails the request, so the other domains keep syncing. The client bootstraps a domain whose page says 2 or 3 ([§18](#18-bootstrap)). For 4 or 5 it fetches the policy and stops pulling that domain once a change it accepted took its read access or deleted the domain; until then it reports the status and retries. A device that a change removed from the workspace gets status 4 on every page and the policy chain up to that change ([§8](#policy-change-record)). Once it verifies the removal, it stops syncing the workspace and shows that the device was removed, by which device and account, and that its files stay as they are.

The client advances a domain's cursor to `complete_through` only after every record in the page passes the checks in [§13](#client-checks-on-an-operation) and [§14](#14-device-chains) and every resulting file change reaches disk. A text-mode record that the collaboration sandbox is still decoding counts once the record itself reaches the journal on disk ([§22](#22-live-collaboration)). Policy changes and keys have their own cursors ([§26](#26-http-api)). The realtime channel ([§23](#23-presence-and-realtime-channel)) delivers small records and announces new heads.

The server never publishes a retention floor below one it published before, unless its restore generation changed ([§19](#19-compaction)). A `retention_floor` below one the server reported before in the same generation reports `sync_manifest_rollback`. A `head` or `checkpoint_seq` below one the server reported before in the same generation reports `sync_server_rollback` ([§14](#after-a-server-restore)). For a page with status 2, the client reports `sync_manifest_rollback` when its `retention_floor` is below one reported before in the same generation, or when its cursor isn't below that floor. Otherwise it bootstraps, and it requires the checkpoint it bootstraps from to have a `covered_seq` at or above both its cursor and its latest accepted checkpoint's, else `sync_manifest_rollback`. An honest server passes both checks, because the floor never passes the latest checkpoint.

### Client journal

The client keeps its sync state in ordinary files under `.noura/sync/<device-id>/`, never only in SQLite, and never uploads them. It ignores a journal named for another device, and locks its own while the app runs. The journal holds:

- the accepted policy chain and its head;
- epoch bundles and the envelopes addressed to this device, still wrapped;
- the generation and commit tag of each policy change, epoch bundle and envelope it holds ([§14](#after-a-server-restore));
- per domain: the cursor, the latest accepted checkpoint, the digest, `covered_seq` and sequence of the checkpoint it bootstrapped from and of each it reached since ([§17](#checkpoint-chain)), the last retention floor the server reported, and the chain heads and records the verifier needs ([§14](#verifier));
- per domain, for every record above the latest verified checkpoint, its sequence, `rid`, chain position, digest and covers, and the same for each operation such a record covered, so the client can compute its live set at any later `covered_seq`; and every live lone delete, indexed by sequence, so the client can drop the ones a rising checkpoint `floor` passes ([§17](#leaves-and-tree));
- this device's counters, and every signed record not yet acknowledged, persisted before sending;
- the map from each `rid` to its local object, path, live operations and text-mode state;
- the domain roots ([§9](#domain-roots));
- the replica ID and the last restore generation it saw;
- the review store for conflicts, unreadable records, held files, and text updates whose lineage isn't live.

Each journal file carries a BLAKE3 keyed hash under a 32-byte journal key that the device keeps in the credential store, and the client refuses a file whose hash fails.

The credential store also holds a high-water mark per workspace: the accepted policy revision and head digest, the latest accepted epoch record and checkpoint of each domain, and this device's reserved counter per domain ([§14](#your-own-chain)). The client raises a reserved counter before it signs past it, one block of 1,024 at a time and one more block at each start, and writes the policy, epoch and checkpoint parts no more often than it flushes its journal, so typing costs no credential store write per keystroke batch. On startup it refuses a journal that sits below the mark and reports `sync_journal_rollback`. It then bootstraps ([§18](#18-bootstrap)), accepting no policy head, epoch or checkpoint below the mark, and handles its own counters as [§14](#your-own-chain) describes. A server restore that raised the generation is the one exception for heads and checkpoints, as [§14](#after-a-server-restore) describes; it never excuses a fork of what the mark records.

A device syncs one folder per remote workspace. The app keeps a registry in its application data, outside every workspace, that maps each account and remote workspace to one folder and a random replica ID, which the journal also stores. A folder whose journal holds another replica ID, such as a copy you made in Finder or a folder another Mac opened through a cloud drive, doesn't sync until you connect it, and connecting it discards its journal and bootstraps.

Device signing and HPKE secrets live only in the operating system's credential store. Epoch keys exist only in memory: the client unwraps them from the stored envelopes with the device's HPKE secret.

Within one scan, a deleted ordinary file and a created one with the same content revision, and the same operating system file ID where the platform reports one, count as a move: the client writes a state operation on the same `rid` with the new path and reuses the `blob_ref`. Two files that share one frontmatter ID don't sync as that object until you resolve the local `identity_conflict` ([v1.md](v1.md#classification)).

If the journal is lost, the client bootstraps again ([§18](#18-bootstrap)) and matches local files to objects: managed files by frontmatter ID first, ordinary files by content revision, then by path. A matched file whose revision differs from the live state counts as a local edit and forms a conflict branch. With no journal, the client continues its chains from the server's heads, at counters above its reserved ones ([§14](#your-own-chain)).

## 16. Conflicts

The client derives an object's state from its live set:

1. One live state operation: that state, plus the live text updates of its text lineage, which merge in any order ([§22](#22-live-collaboration)).
2. Two or more live state operations: a conflict, and each one is a branch, except live text snapshots of one text lineage, which merge ([§22](#22-live-collaboration)).
3. Text updates whose lineage isn't live: kept for review as text.

Sync keeps both versions:

- The device whose local file matches a branch, or has local edits, keeps its file untouched.
- Every other branch goes to the local review store under `.noura/sync/<device-id>/conflicts/`, which never uploads.
- A device that holds neither materializes the branch with the lowest operation digest and stores the rest for review.
- Status counts the conflict until you choose.

Choosing writes a state operation that covers every branch. Any other edit you make to a conflicted object covers only the branch your file matched ([§13](#supersession-and-live-operations)), so the conflict stays, with your edit as that branch's new state, until you choose.

Clients resolve these cases without asking:

- **Identical branches.** Branches whose decrypted path and revision match aren't a conflict.
- **Disjoint changes.** When one branch changed only the path and another only the content, relative to the common base operation the covers name, the client materializes the merged state. Any writer device that holds the base writes a merge operation covering both. Two devices writing the same merge produce identical branches, which the previous rule collapses.
- **Edit against delete.** The edit stays on disk and the conflict asks whether to delete it. An edit to an object whose delete a checkpoint's `floor` has already dropped from the live set ([§17](#leaves-and-tree)) revives the object without asking.

Two objects with different `rid`s collide when their paths are equal after NFC normalization and Unicode simple case folding, or when one object's path equals a folder prefix of the other's. A valid domain marker ([§9](#domain-roots)) never yields, so the other object yields, even when its path is only the marker folder's prefix, such as a file `Projects` against `Projects/.noura-domain`. Otherwise the object whose current state operation has the higher digest yields. Its author's device moves it to the first free name `name (n).ext`, with n counting up from 2, with a normal operation, and other devices show it at that name without writing until the move arrives. A text-mode note moves by a text update that sets its path ([§22](#22-live-collaboration)).

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

The heads are the chain head, as a `ref` with its sequence, of every device with at least one accepted record in the domain at `covered_seq`, sorted by device. A device leaves the list only once its card is gone from the workspace: when a change at or below the previous checkpoint's `policy_revision` removed it, through `remove_device`, `remove_member` or signing out. A removed card never returns, so a device that left the list never writes the domain again. A device that only lost write access, such as an editor demoted to viewer or a grantee cut back to read, stays listed with its last head, so a client that bootstraps meanwhile still knows where its chain stopped if write access comes back. No device leaves the first checkpoint of a domain, and the first checkpoint that covers a removal's entry still lists the removed device's final head. Signer and readers derive the list from the log and the policy chain alone, so they agree on it.

A `checkpoint_bundle` is `bytes checkpoint | list<ref> heads`. The server checks:

- that the signer can write the domain at the latest policy entry ([§8](#policy-entries)), else `forbidden`;
- that `policy_revision` and `policy_head` name the latest policy entry at or below `covered_seq`, and `epoch` and `epoch_record` the latest epoch entry at or below it;
- that `previous` names the domain's latest checkpoint, else `checkpoint_stale`;
- that `covered_seq` sits at or above that checkpoint's entry sequence and at or above the retention floor;
- that `floor` lies between the previous checkpoint's `floor` and the current retention floor;
- that a checkpoint is due and `covered_seq` reaches the due point it published, or that the checkpoint comes in the same push as an approval or grant that sets a history floor ([§9](#history-floor)), else `checkpoint_not_due`.

Before it opens the committing transaction, the server recomputes, at `covered_seq`, the root, the number of leaves and the chain heads, from the bucket hashes and heads it stored for the previous checkpoint, the records between the two, and the lone deletes that the rising `floor` drops ([§17](#leaves-and-tree)). It rejects a root, `live_objects` or `heads_digest` that differs with `manifest_mismatch`, so an honest server and an honest writer always agree, and a bootstrapping device never starts from heads a writer made up. The records at or below `covered_seq` are already committed and pruning can't reach them, because the floor never passes the previous checkpoint, so this work needs no lock; the transaction then checks `previous` again under its locks. An accepted checkpoint becomes a log entry with its own sequence.

The server decides when a checkpoint falls due and publishes the due point as `checkpoint_due` in pull pages and in notify and records frames ([§23](#realtime-channel)): the sequence that the next checkpoint's `covered_seq` must reach, or 0 while none is due. A checkpoint falls due:

- at the entry that brings the domain's log to 10,000 entries or 64 MiB of weight ([§19](#19-compaction)) since the last checkpoint entry;
- at a policy entry that ends the write access of the latest checkpoint's signer or of any device with a chain head in the domain;
- at the entry of an `add_member` or a grant that repair brought back after a restore ([§14](#after-a-server-restore)), so the account's history floor has a checkpoint;
- at the next entry of each domain with entries after its latest checkpoint, when its workspace's usage and history together come to exceed its quota ([§20](#20-atomic-changes-and-quota)).

Writers don't compute due points: blob weights hide inside payloads they may not read, and a bootstrapped device doesn't know which blobs pruned history listed. A writer device that has applied the domain past the published due point waits a random 0 to 60 seconds, then signs a checkpoint if none has arrived since. The server keeps the first one and answers the rest with `checkpoint_stale`. A writer that gets `checkpoint_not_due` drops its checkpoint and waits for the next due point. Every policy change adds an entry to each log, so even a quiet domain gains an entry after a demotion, and the fresh checkpoint has something to cover.

### Checkpoint chain

Each checkpoint names the one before it in its domain, and every operation names the latest checkpoint its writer reached (`checkpoint_seq` and `checkpoint`, [§13](#operation-record)). A server that leaves a checkpoint out of one client's log therefore can't hide it once a later checkpoint, or a record whose writer reached it, crosses to that client. The entry of the checkpoint a client bootstrapped from reaches it again in its log, and the client skips it. A client fetches a checkpoint by digest (`GET .../checkpoints/{digest}`) in two cases:

- a checkpoint entry in its log whose `previous` isn't the last checkpoint the client reached or bootstrapped from in that domain;
- a record of the current restore generation whose `checkpoint_seq` sits at or above both the retention floor and the `covered_seq` of the checkpoint the client bootstrapped from in this generation (0 if it pulled from the start), when the client hasn't reached `checkpoint`.

The client fetches the named checkpoint, then each one its `previous` names, back to the last checkpoint it reached. If the server can't serve one, or the walk passes the client's last checkpoint without meeting it, the client reports `sync_chain_gap` for the domain. Otherwise the client checks the fetched checkpoints as log entries, oldest first, as if its pages had carried them. An honest server never causes a fetch. A record gets its sequence after the checkpoint its writer reached, so a client that pulls in sequence order reaches that checkpoint first. A writer that lags names a checkpoint the client already reached, or one below the client's bootstrap point or the floor, which needs no check.

### Checking a checkpoint

A client checks each checkpoint it reaches in its log after it has applied every record through `covered_seq`, and accepts it when:

- `BLAKE3(tag(noura.sync.manifest.heads) || heads)` equals `heads_digest`, and every head's sequence is at or below `covered_seq`;
- `previous` names the last checkpoint the client reached in that domain;
- its signer could write the domain at the latest policy entry before the checkpoint's entry ([§8](#policy-entries)) and at its `policy_revision`;
- `policy_revision` is the revision of the latest policy entry at or below `covered_seq`, at least the previous checkpoint's, and `policy_head` matches the client's change at that revision;
- `epoch_record` is the digest of the latest epoch entry at or below `covered_seq`, an epoch record the client accepted for `epoch`, else `sync_key_mismatch`;
- `floor` is at least the previous checkpoint's;
- `covered_seq` lies above that of the latest checkpoint the client accepted, each device listed in both has a head there at or above its earlier head, and each device listed only in the earlier one may leave under the heads rule above, else `sync_manifest_rollback`.

The client then computes its live set and chain heads at `covered_seq` from its journal ([§15](#client-journal)) and compares them with the checkpoint. A root mismatch reports `sync_manifest_mismatch`, and the client walks the tree to find which objects differ. A checkpoint head above the client's own reports `sync_chain_gap`, and one below it reports `sync_manifest_mismatch`.

A checkpoint in the log that fails a check is reported and never becomes the client's latest accepted checkpoint, an approval or kit anchor, or a source of state. It doesn't hold the cursor: each record around it passed its own chain checks, and the next checkpoint still names it as `previous`.

A checkpoint a device bootstraps from ([§18](#18-bootstrap)) passes the checks above that need no local history: the heads digest, `policy_head` and the epoch record. It also passes three more:

- **Descent.** It is the device's anchor, or descends from it through `previous` links. The anchor is the device's latest accepted checkpoint in the domain, or else the checkpoint anchor of its approval or kit. The device fetches the records between them with `GET .../checkpoint-chain` and checks each link back to the anchor, within the anchor's generation; when a restore lost the anchor, [§10](#approval-offer) applies. A walk that fails reports `sync_manifest_rollback`. The server keeps every checkpoint record while the domain exists ([§19](#19-compaction)), so the walk reaches an old kit's anchor too. A device with no anchor in the domain, such as one granted a domain after its approval, skips this check and relies on the two below.
- **Signer.** Its signer could write the domain at the latest policy entry before the checkpoint's entry and at its `policy_revision`. `GET .../checkpoint` returns the entry's sequence with the bundle, and before installing anything the device pulls the log from `covered_seq` up to that entry, applying nothing, to learn the policy entries it needs for the check.
- **Recency.** No change in the device's accepted chain above the checkpoint's `policy_revision` removes its signer or any device in its heads from the workspace, through `remove_device`, `remove_member` or signing out. The server places entries, so it could slip a checkpoint that a removed device signed at an older revision in front of the removal's entry; the device has verified the whole chain, so it knows every removal and refuses such a checkpoint.

A checkpoint whose signer was only demoted after its `policy_revision` still serves, so a domain whose remaining grantees only read can still take new devices; a demoted signer that colludes with the server is a colluding writer ([§30](#30-threats)). When the latest checkpoint fails, the device reports the error, shows the domain as waiting for a checkpoint, and tries again on each pull. A removal makes a checkpoint due at its entry, so a checkpoint that passes follows once a writer signs it; a domain that lost every writer waits until a manager grants someone write access.

## 18. Bootstrap

A new device, or one whose cursor fell below the retention floor, loads current state from the latest checkpoint instead of history:

1. Fetch and verify the policy chain from the approval anchor, and the current epoch bundle and envelopes of each domain it reads. Fetch older epoch records only when a live operation names an older epoch, and walk back to them through `previous` digests and lineage; `GET .../epochs/{epoch}` returns as many earlier bundles as fit one response.
2. For each domain, fetch the latest checkpoint bundle with its entry's sequence. Check it as in [§17](#checking-a-checkpoint): its descent from the device's anchor, its signer by position and the recency of its policy. Check its `covered_seq` against the device's history floor, its checkpoint anchors and the latest checkpoint the device accepted (`sync_manifest_rollback`).
3. Fetch the 4,096 bucket hashes at `covered_seq` in one request and check them against `manifest_root`.
4. For a restricted domain, apply the marker first. Fetch the leaf refs of the bucket that holds `marker_rid` (level 3 of `manifest`), check them against that bucket's hash, fetch the marker's live operations (`live` with `rid`), check their digests against the refs, and apply them. Record the domain root when the marker is valid ([§9](#domain-roots)); otherwise the client records held roots as it writes the domain's files.
5. Fetch the operations live at `covered_seq` in pages ordered by `rid`, device and counter. Run the checks in [§13](#client-checks-on-an-operation) on each operation, except the chain verifier and the positional rules of [§8](#policy-entries) and [§9](#rotation), which the checkpoint's signer applied, and compute the leaves. As each bucket completes, check its hash, decrypt and materialize its files, and record the checkpoint and the next bucket in the journal, so a restart resumes at that bucket. A bucket that fails its hash, or a `live_objects` count that fails after the last bucket, reports `sync_manifest_mismatch` and installs nothing more.
6. Record the chain heads from the bundle, with their sequences, and the checkpoint as the start of the domain's checkpoint chain ([§17](#checkpoint-chain)).
7. Pull from `covered_seq` with the chain verifier starting at those heads.

A checkpoint stays servable while its `covered_seq` is at or above the retention floor. When the floor passes the device's checkpoint before the bootstrap ends, `live` and `manifest` answer `cursor_compacted`, and the device restarts from the latest checkpoint, keeping the buckets whose hashes match.

A returning device, one whose cursor fell below the floor while it kept its journal, downloads only what it lacks. In step 5 it compares each bucket hash with the one its journal yields and, for each bucket that differs, fetches the bucket's leaf refs (level 3 of `manifest`) and then, through `live` with `rid`, only the objects whose refs name an operation its journal lacks. When more than half the buckets differ, it fetches `live` pages as a new device does. Any bootstrapping device downloads a blob only when it holds no file at the payload's `revision`. A returning device also compares its journal with the manifest instead of replacing files. It never deletes a local file because the manifest lacks its object: it lists those files once for review ("deleted on another device while this one was away") and deletes them only when you confirm. A changed local copy uploads under a new `rid` unless you choose to delete it.

## 19. Compaction

The server deletes operations no client needs, and bounds each domain's retained log by volume instead of time, so it needs no table that maps sequences to dates.

**Log weight.** An entry's weight is its record's length plus, for an operation, the ciphertext length of each blob it lists that no earlier operation in the domain listed. A move that reuses a `blob_ref` adds little, while a new attachment, or a text snapshot carried as a blob, counts in full. Retention and the checkpoint interval both count weight.

**Retention floor.** A domain's retention floor is the `covered_seq` of the newest checkpoint that has at least `retained_bytes` of weight appended after its entry. `retained_bytes` is a server setting, 1 GiB by default, that `GET /v2/limits` reports. While its workspace's usage and history together exceed the quota, a domain's floor is instead the `covered_seq` of its latest checkpoint, so history goes before live data ([§20](#20-atomic-changes-and-quota)). The floor is 0 while no checkpoint qualifies, and it never passes the latest checkpoint, so a device that falls behind it always has a checkpoint to bootstrap from. The server persists, per domain, the highest floor it has published, and never publishes a lower one unless its restore generation changes ([§14](#after-a-server-restore)). A domain without any checkpoint keeps every record, and a new device bootstraps it by pulling from sequence 0 with empty chain heads. A device whose cursor falls below the floor gets a compacted page and bootstraps, and its own changes upload as local edits or conflicts ([§18](#18-bootstrap)).

**Pruning.** The server prunes in batches, in ascending sequence order, deleting:

- each operation at or below the floor that another operation, also at or below the floor, covers;
- each delete (`flags` 1) that a checkpoint's `floor` dropped from the live set ([§17](#leaves-and-tree)), whatever operations on its `rid` follow it, once it sits at or below the floor and at or below the `floor` field of every checkpoint the server still serves.

A cover names an operation the server already held, so a covered operation always has a lower sequence than the one covering it. Deleting in ascending order therefore never removes a covering operation before the operations it covers, and a crash between batches leaves the live set at every servable checkpoint complete. A writer whose view is older than the floor can still name a pruned operation in a cover; the server answers `supersedes_unknown`, and the writer drops that cover ([§14](#your-own-chain)). An edit against a delete keeps both operations live, so neither is pruned until you choose ([§16](#16-conflicts)). The server keeps one chain-head row per device and domain, holding counter, digest and sequence, which pruning never deletes while the domain exists. A fetch of a pruned chain position returns `pruned`. The server deletes checkpoint, epoch and policy entries once their sequence falls below the floor, and keeps the bundles and bucket hashes of every checkpoint whose `covered_seq` is at or above it. It keeps every checkpoint record itself, without heads or bucket hashes, about 300 bytes each, while the domain exists, so a bootstrapping device can walk `previous` links back to its anchor ([§17](#checking-a-checkpoint)). A checkpoint stays servable to `live` and `manifest` while its `covered_seq` is at or above the floor.

`delete_domain` marks the domain deleted in its own transaction, and the server answers `not_found` for the domain from then on. It reclaims the domain's log, checkpoints and blobs afterwards, in batches, so no transaction holds the store for the size of a domain.

**Blobs.** A blob passes through three states:

- *Uploading*, until its offset reaches `Upload-Length`. An incomplete upload expires 24 hours after creation (`Upload-Expires`).
- *Completed*, listed by no operation yet. The server deletes it 24 hours after completion unless a push lists it first.
- *Listed*, while a retained operation lists it. When its last listing operation leaves, through pruning or `delete_domain`, the collector deletes it.

The collector deletes a blob in three steps: it marks the row deleted, deletes the store objects, then deletes the row. A crash between steps leaves a marked row that it finishes later. A push that lists a blob and the collector both take the blob's row lock, after the domain's head row ([§15](#store-transactions)), so the collector never deletes a blob that a committing push lists, and a push never lists a marked blob (`blob_incomplete`).

`restore` and `mark-restored` run while `serve` is stopped and check the blob store against the restored database. They delete each store object whose blob the database doesn't know, since the operations that listed it are gone and devices upload what they still need again ([§14](#after-a-server-restore)). They give each completed blob that no operation lists a fresh 24 hours. A listed blob whose store objects are gone becomes missing: its row returns to an upload at offset 0, `GET` answers `blob_incomplete`, and `GET .../missing-blobs` lists it. A device that holds the file at a listing payload's `revision` uploads it again under the same ID, byte for byte ([§21](#blob-format)). An operator whose blob store lost objects while the database kept its rows runs `mark-restored` too.

**Bound.** While checkpoints keep committing, each domain's storage stays within the live set at its floor checkpoint, the log above the floor, 128 KiB of bucket hashes for each servable checkpoint, and one checkpoint record for each checkpoint it ever took. The log above the floor weighs at most `retained_bytes` plus two checkpoint intervals, one between the floor checkpoint's `covered_seq` and its entry and one between that entry and the next checkpoint's. An interval weighs at most 64 MiB, plus the weight of the entry that crosses the threshold (up to 1 MiB of record and 4 new blobs of 2 GiB), plus what writers append during the 0 to 60 second wait after a due point. The live set at the floor checkpoint exceeds the current live set only by what was deleted or replaced since, which the server keeps until the floor passes those changes. Nothing bounds the log while no checkpoint commits, for example while every writer is offline or disagrees with the server's root, and the quota can't reclaim history without a checkpoint either ([§20](#20-atomic-changes-and-quota)). Each device's verifier state stays within one checkpoint interval per domain ([§14](#verifier)).

## 20. Atomic changes and quota

Every atomic change is one push of at most 1 MiB ([§15](#push)), which the server commits whole or not at all. The changes a requirement needs to be atomic all fit:

| Change | Size |
|---|---|
| An approval: policy changes and at most 256 envelopes | about 350 KB |
| A new member's approval: `add_member`, `add_device`, one envelope and a checkpoint of the workspace domain | about 16 KB with 200 chain heads |
| `create_domain` with its epoch 1 bundle, envelopes and marker | about 715 KB for 512 recipients and 200 accounts on its list |
| A rotation for up to 512 recipients, with its epoch bundle and envelopes | about 711 KB |
| A move between domains: the new object's state operation and the old one's delete | two operations |

A move of many objects commits one object pair at a time, with many pairs per push, so each file sits in exactly one domain throughout.

**Quota.** The operator sets each workspace's storage quota with the server setting `workspace_quota_bytes`, which applies to every workspace and is unlimited by default. `GET /v2/workspaces` reports each workspace's quota, usage and history. Usage counts live data and the records the server keeps for the workspace's whole life: the operations live at each domain's head, the blobs they list, uploads in progress or completed but not yet listed, share snapshots, policy changes and epoch bundles. A delete therefore frees its object's usage when it commits. History is everything else each domain retains: the log above its floor that no live operation accounts for, the blobs only that log lists, and checkpoint data. History counts nothing toward usage but yields to the quota. When usage and history together come to exceed the quota, a checkpoint falls due in each domain ([§17](#checkpoint)), and while they stay above it, each domain's floor follows its latest checkpoint, so pruning reclaims history first ([§19](#19-compaction)). While checkpoints keep committing, a workspace's storage therefore stays within its quota plus about one checkpoint interval per domain, and an account owns at most 100 workspaces ([§27](#27-limits)).

While usage exceeds the quota, the server accepts envelopes, checkpoints, epoch bundles, policy changes that add nobody (any change but `add_device`, `add_member`, `create_domain` and a `set_access` that grants), and any operation that doesn't raise usage, such as a delete or a smaller replacement. A workspace over quota can therefore still remove a device, rotate and free space, while churn in its membership can't grow its policy chain past the quota. The server rejects other records with `quota_exceeded`, and the writer moves its next outbox record into that position ([§14](#your-own-chain)). A tus creation request whose `Upload-Length` would take usage above the quota fails with `quota_exceeded` too ([§21](#21-attachments)).

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

`nonce_prefix` is the first 19 bytes of `derive_key("noura.sync.v2 blob nonce", blob_key || blob)`. Each blob key is fresh and random, so no key meets one nonce twice, and a device that holds the file at the payload's `revision` can rebuild the blob byte for byte, which repair after a restore relies on ([§14](#after-a-server-restore)). A device rebuilds a blob only from bytes whose BLAKE3 hash equals that `revision`, and never encrypts other bytes under an existing blob key.

Every chunk holds 65,536 plaintext bytes except the last, which holds 1 to 65,536 and sets `last` to 1. The ciphertext length is `header length + P + 16 * ceil(P / 65,536)`, which lets the server check padding from the length alone. A blob's file content `L` is at most 2^31 bytes (2 GiB).

`ciphertext_digest` is `BLAKE3(tag(noura.sync.blob.parts) || list<digest>)` over the BLAKE3 hash of each 1 MiB segment of the ciphertext, in order; the last segment may be shorter. Any server instance can then check each segment as it arrives, and the final check costs one hash per segment.

The receiver hashes each segment as it downloads, checks the segment hashes against `ciphertext_digest` before decrypting, writes the plaintext to a temporary file, checks `plaintext_length` and the file `revision`, and only then replaces the destination.

### Upload over a tus subset

Uploads use tus 1.0.0 core with the `creation`, `termination` and `expiration` extensions. The server advertises `Tus-Version: 1.0.0`, `Tus-Extension: creation,termination,expiration` and `Tus-Max-Size`.

| Request | Purpose |
|---|---|
| `POST /v2/workspaces/{w}/blobs` with `Upload-Length` and `Upload-Metadata: blob <base64>,digest <base64>,domain <base64>` (standard base64, as tus requires) | create; the device must be able to write that domain, the upload must fit the workspace's quota (else `quota_exceeded`, [§20](#20-atomic-changes-and-quota)), and only operations of that domain may list the blob (else `blob_domain_mismatch`) |
| `HEAD /v2/workspaces/{w}/blobs/{id}` | read `Upload-Offset` to resume |
| `PATCH /v2/workspaces/{w}/blobs/{id}` with `Upload-Offset` and `Content-Type: application/offset+octet-stream` | append one segment: exactly 1 MiB at an offset that is a multiple of 1 MiB, or the rest of the upload |
| `DELETE /v2/workspaces/{w}/blobs/{id}` | abandon an incomplete upload |

The server stores each segment as its own object, records its hash, and flushes it to durable storage before it reports the new offset. When the offset reaches `Upload-Length`, it computes `ciphertext_digest` from the stored segment hashes, and on a mismatch discards the upload and answers `blob_digest_mismatch`. Incomplete uploads expire 24 hours after creation (`Upload-Expires`), and completed uploads that no operation lists 24 hours after completion ([§19](#19-compaction)). A filesystem store's file times (ctime, birth time) and an S3-compatible store's `LastModified` date each segment's upload, because no program can set ctime or birth time ([§29](#29-what-the-server-stores)).

`GET /v2/workspaces/{w}/blobs/{id}` with `Range` returns at most 1 MiB per response, as raw octets with `Content-Range`, like any HTTP range response. The server serves a blob to a device that is active in the workspace's policy and can read the domain the blob was created for, when a retained operation of that domain lists the blob and sits above the account's history floor or is live at a checkpoint the device reads, and to its uploading device before that operation exists. It deletes blobs as [§19](#19-compaction) describes. `GET /v2/workspaces/{w}/missing-blobs` lists the blobs, in domains the device can write, that a restore left missing ([§19](#19-compaction)); after a generation change each device fetches it once and uploads again each listed blob whose file it holds at a listing payload's `revision`.

## 22. Live collaboration

A Markdown note in text mode lives in a yrs document, update format v1, with two roots: the text `content`, with UTF-16 offsets, and the map `meta`. `meta` holds the note's path under the key `path`, and for a managed note each frontmatter property under `p:` followed by the property's name, as the RFC 8785 canonical JSON of its value ([v1.md](v1.md#managed-markdown)). Every `meta` value is a yrs `Any::String`, and `meta` holds no other key; the sandbox refuses a record that breaks these rules ([Collaboration sandbox](#collaboration-sandbox)). A managed note's file bytes are the frontmatter that the workspace-format serializer writes from the `p:` entries, followed by the text of `content`. An unmanaged note keeps its whole file, frontmatter included, in `content`, and `meta` holds only its path. Renames and property edits travel as yrs updates like text, so devices merge them the way they merge text, and every device writes the note to the same file. A rename that would take the note across a domain root is a move between domains ([§9](#moving-objects-between-domains)), never a `path` change, and a merged `path` outside the note's domain root goes to review ([§9](#domain-roots)).

### Entering text mode

A device puts a note into text mode when it first sends an edit you made to that note in the editor, once the note has a live state operation: a new note first syncs as a `file` operation, and so does a note that moves to another domain ([§9](#moving-objects-between-domains)). It writes a `text_snapshot` whose lineage and first document derive from the note's current state, so two devices that start at once from the same state write the same snapshot:

- the text lineage ID is the first 16 bytes of `BLAKE3(tag(noura.sync.text.lineage) || rid || base || revision)`, where `base` is the digest of the live state operation the note's file matched and `revision` the BLAKE3 hash of the starting file bytes;
- the first document holds one transaction by yrs client ID 0, which sets `meta`'s `path`, then its `p:` entries in byte order of their keys, then inserts the starting text into `content`, encoded with garbage collection on.

The first snapshot covers the base operation and holds the starting state alone; your edits follow as text updates under the device's own client ID. Identical first snapshots merge to the starting state once. Two devices that start from different states write different lineages, which conflict as two branches ([§16](#16-conflicts)).

### Updates

- The editor batches local edits into at most one `text_update` every 500 ms while you type.
- A frontmatter edit to a managed note sets the changed `p:` entries, and a rename or move, in Noura or outside it, sets `path`.
- An outside edit to the file becomes a `text_update` computed from the difference, as the editor treats any outside edit. For a managed note, the client parses the new frontmatter, sets each `p:` entry whose value changed, removes each property that disappeared, and diffs the body into `content`. An outside edit that only reformats the frontmatter changes no value and sends nothing; your file keeps its bytes until another change rewrites it in canonical form. An outside edit that leaves the frontmatter unparseable, adds frontmatter to an unmanaged note or removes it from a managed one becomes a `file` operation instead, which ends text mode.
- A `file` operation that covers every live operation takes the note out of text mode. A device writes one when the file turns binary, or when its own edit takes the note's text past 4 MiB, counted as the UTF-8 bytes of `content`.

**State.** The note's document is the merge of every live `text_snapshot` and every live `text_update` of its text lineage, applied in any order, because yrs updates commute. Two live snapshots of one lineage merge without a conflict prompt, and so do their paths and properties.

### Snapshots

A device writes a new snapshot, the encoded state of the yrs document with garbage collection on, in three cases:

- **Idle.** When a note has had no edit for 30 seconds, or when you close it, the device that wrote its latest text update writes a snapshot if the note holds more than 8 live patches. When the note still holds more than 8 live patches after 5 minutes without an edit, because that device went to sleep or away, any writer device that holds the note's document writes one; two such snapshots of one lineage merge, and the 10-second limit below keeps the race cheap.
- **Volume.** When the note's live patches exceed 512 records, or a byte threshold of the larger of 1 MiB and a quarter of the last snapshot's encoded size, capped at 2 MiB, the device with the lowest device ID among the devices that hold a live patch of the note writes one. Another device writes it only when the live patches pass twice either threshold, at most 1,024 records or 4 MiB, which stays below the server's limits of 2,048 records and 8 MiB ([§13](#server-checks-on-an-operation)). Byte thresholds count what the server counts: the ciphertext length of the live patch operations, blob ciphertext included.
- **Required.** When the server answers `snapshot_required` ([§14](#your-own-chain)).

Each device writes at most one snapshot of a note every 10 seconds, unless the server requires one. At 20 typists sending a patch every 500 ms, the designated device snapshots about every 13 seconds. The idle rule leaves a note you edited and walked away from with one snapshot and at most 8 live patches ([§28](#28-worked-example-100000-objects)). For each device whose patches it includes, a snapshot covers the highest counter below that device's lowest live patch the document hasn't integrated: one deferred under a writer's budget, or one pending on a dependency that hasn't arrived. Patches it didn't include therefore stay live and still apply. A patch judged unreadable, or one that depends on such a patch, merges on no device, so a snapshot may cover it. A reader that already holds every covered patch, and whose text matches the snapshot's `revision`, skips downloading the snapshot's blob.

**Rebase.** When the encoded document reaches 16 MiB, counted as the length of its update format v1 encoding with garbage collection on, the device that the volume rule designates writes a snapshot with a new lineage whose first document holds the current state alone, derived as on entering text mode, with `base` the note's live `text_snapshot` at the highest log sequence. Text updates of the old lineage that arrive afterward go to review as text. Two devices that rebase from different states write different lineages, which conflict.

### The document on a device

Each device holds one yrs document per open note, inside the collaboration sandbox. The editor sends local edits into it as transactions and receives encoded updates and text changes back; it never inserts remote text again as a local edit. Relative positions for presence encode and decode inside the sandbox, and undo tracks only transactions of local origin. A device draws a fresh random 53-bit yrs client ID, never 0, each time it loads a document. The journal keeps each text-mode note's live snapshots and patches with the sandbox's verdict on each, and the device rebuilds the document from them after a restart, skipping the records it judged unreadable.

### Collaboration sandbox

Remote snapshots and updates pass through the collaboration sandbox, a Wasmtime instance, before they reach the editor or the file. The sandbox works in two stages, and no clock takes part in either.

1. **Verdict.** The sandbox parses and validates each record's yrs bytes in a fresh instance that holds no document, with a fuel budget of 1,000,000 units plus 1,000 per byte of the decrypted yrs bytes, which are the inline content or the blob's file, and 512 MiB of linear memory. It also checks every write the record makes to `meta`. A record is unreadable when it fails to parse, passes either limit, or writes to `meta`:
   - `path`, with a value that isn't a string passing the path rules of [§13](#payload);
   - a `p:` key whose name isn't a valid property name ([v1.md](v1.md#managed-markdown)), or a `p:` value that isn't a string holding RFC 8785 canonical JSON;
   - `p:id`, with anything but the payload's `object_id`;
   - any other key.

   An unreadable record counts as applied, its content never reaches the document, and it goes to review ([§13](#client-checks-on-an-operation)). The verdict depends on the bytes alone, so every device reaches the same one. To keep it that way, one guest module judges every record: `sync-v2-collaboration.json` records the BLAKE3 hash of its `.wasm` file and the Wasmtime operator costs its fuel count assumes, and the app runs no other module for the verdict. The budget is a protocol constant, set far above the largest legitimate update per byte that ticket 54 measures. A change to the module, the budget or the operator costs is a protocol change, not a release choice. Continuous integration runs the near-limit fixtures under Cranelift and Pulley, on the previous and the current Noura release, and fails when their verdicts differ.
2. **Integration.** The sandbox then merges the parsed record into the note's document. The cost depends on the document, so integration gives no protocol verdict. Each writer gets 2,000,000,000 units of fuel per note per minute across both stages; past that, the device defers the writer's further records to the next minute and keeps merging the others, so a collaborator who floods a note slows only their own edits. When one record's integration alone passes 10,000,000,000 units, or would take the document past 32 MiB or its text past 8 MiB, the device stops merging that lineage. Ordinary growth never reaches those sizes, because a note rebases at 16 MiB and leaves text mode at 4 MiB, so only a record that jumps past them triggers the stop. The stop is local, and other devices may keep merging. The device keeps the note's last good state on disk, writes no snapshot or update for that lineage, lists it for review as text, and shows the note as waiting for your choice: keeping its last good state writes a `file` operation that covers every live operation of the note. That `file` operation, or a rebase from any device, ends the lineage on every device.

yrs holds an update whose dependencies haven't arrived as pending until they do. An update that depends on a record judged unreadable never merges, and the device lists the lineage for review as text when it holds such updates.

A device counts a `text_snapshot` or `text_update` as applied for its chain head, cursor, live set and manifest once the record passes the checks in [§13](#client-checks-on-an-operation) and reaches the journal, and runs the sandbox in the background. Meanwhile the note stays editable on its last merged state: your edits queue for the document and apply after the records ahead of them, and the file keeps its last merged state. On a slow machine the note catches up later, and the verdict doesn't change. A snapshot or update larger than the inline limit travels as a blob.

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

The channel carries no writes besides presence and subscriptions: clients push only through `POST .../ops` and pull only through `POST .../pull` ([§15](#15-domain-log-cursors-and-pulls)).

After a push commits, and never before, the server tells every connected device that can read a domain about that domain's new entries, the pushing device included. In PostgreSQL mode the instance sends `NOTIFY` with the workspace, domain and new head after the commit succeeds, from a separate statement outside the push's transaction, and the other instances read the entries from the database. A `NOTIFY` inside the transaction would take PostgreSQL's database-wide notification lock through the commit and make every push in every workspace commit one at a time. A push that rolls back wakes nobody, and a crash that loses a notification costs at most the minute until the next workspace pull. When the entries fit one frame, it forwards them in a records frame: `entries` holds every entry of the domain from the one after `previous_seq` through the frame's last, in sequence order, each as `u64 seq | u8 record_type | bytes record` with its log position, and `previous_seq` is the sequence of the domain's entry just before the first. Otherwise it sends notify with the new head, at most one notify per domain every 250 ms on a connection.

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

The device that creates a link keeps the link's token, key and signing seed in its credential store, and it is the only device that can update that link. The server stores the token hash, workspace, domain, creating device, expiry (1 to 30 days, 30 by default) and the snapshot. It accepts a new link only from an owner or admin device that is active in the workspace's policy, and an update only from the creating device's session while that device stays active and its account an owner or admin. Other owner and admin devices list links by token hash, domain and expiry, and can revoke them. The server never learns which note a link shares, so only the creating device, which records that in its own application data, lists a note's links in the note's share dialog. A member learns a link key only from its URL, so a rotation needs no link handling. A workspace holds at most 1,000 active share links.

Update link replaces the snapshot under the same token and keys. Revoking or expiring a link deletes its snapshot, and the viewer then shows a not-found page. A link whose creating device was removed can't change any more, and stays until it expires or someone revokes it. A device keeps the token hash of each link it revoked until the link would have expired, and revokes it again after a server restore ([§14](#after-a-server-restore)).

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

- its signer is the session's device, an owner or admin device active in the policy, and the signature verifies under its card in the chain (else `forbidden`);
- `policy_revision` is at most the current revision (else `policy_stale`) and `policy_head` equals the server's change there (else `policy_conflict`);
- `epoch` is the workspace domain's current epoch and no rotation is pending there (else `epoch_stale` or `rotation_pending`);
- `profile_revision` is one above the server's latest profile (else `profile_conflict`).

Clients accept a profile only when its signer is an owner or admin device in their verified policy at `policy_revision`, `policy_head` matches their change at that revision, `policy_revision` is at least the previous profile's, and its epoch passes the writer rule of [§9](#rotation) at that revision: no change at or below `policy_revision` requires the workspace domain to rotate past it. A server that accepts a profile from an admin who hadn't seen a removal could still show the new name to the removed device; an honest server refuses it with `epoch_stale`. They report `sync_profile_rollback` for a `profile_revision` below the last one they accepted, or for a profile that differs from the one they accepted at the same revision.

## 26. HTTP API

Request and response bodies use the encoding in [§4](#4-encoding) with media type `application/vnd.noura.sync.v2`, except the tus requests in [§21](#21-attachments). Every route requires `Authorization: Bearer <session token>` except `GET /v2/limits`, `POST /v2/device/authorize`, `POST /v2/device/token`, `GET /invite/{token}`, `GET /v2/invitations/{token}`, `GET /s/{token}` and `GET /v2/shares/{token}`. Routes under `/v2/workspaces/{w}/` serve only active devices of w, as [§8](#policy-change-record) describes. Every response, errors included, carries the server's restore generation in the header `Noura-Restore-Generation` ([§14](#after-a-server-restore)). Errors return an HTTP status and the body `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. Messages never include database or operating system details, which stay in the server's local log.

Every route that creates something under a client-chosen ID (a workspace, blob, invitation, share link or approval offer) returns the original result when a client retries it with identical bytes, and `id_conflict` when the bytes differ.

| Route | Purpose |
|---|---|
| `GET /v2/limits` | protocol version (2), restore generation, and the limits in [§27](#27-limits) |
| `POST /v2/device/authorize`, `POST /v2/device/token` | device authorization grant |
| `POST /v2/devices/challenge`, `POST /v2/devices` | enrollment challenge; enroll with an `enroll` record, or bind a new session to an enrolled card ([§7](#enrollment-proof)) |
| `GET /v2/devices`, `DELETE /v2/devices/{device}` | list the account's devices that aren't revoked, with their session counts; revoke a device with a signed `revoke` record, which ends its sessions ([§7](#desktop-sign-in)) |
| `POST /v2/approval-requests`, `GET /v2/approval-requests` | post a candidate's request; list requests of the account's own new devices |
| `GET /v2/workspaces/{w}/pending-devices` | requests of invited accounts awaiting approval in this workspace |
| `POST /v2/approvals`, `GET /v2/approvals` | send an `offer`; offers addressed to the calling device's card |
| `POST`, `GET /v2/approvals/{offer_digest}/reveal` | reveal the candidate's nonce; read it |
| `POST /v2/recovery` | prove possession of a recovery identity ([§12](#12-recovery-kit)) |
| `GET /v2/workspaces` | workspaces the account belongs to, with role, quota, usage and latest profile record |
| `POST /v2/workspaces` | create a workspace: genesis, the workspace domain's first epoch, envelopes and profile in one request |
| `GET /v2/workspaces/{w}/policy?after={revision}` | policy changes after a revision, with the head revision; for a removed device, only up to the change that removed it |
| `GET /v2/workspaces/{w}/keys?after={key_seq}&recipient={device}` | epoch bundles of readable domains and envelopes addressed to `recipient`, which is the session's device unless the session proved a recovery identity, ordered by the server's key sequence |
| `GET /v2/workspaces/{w}/domains/{d}/epochs/{epoch}` | that epoch bundle and the ones before it, newest first, as many as fit one response, for walking lineage |
| `POST /v2/workspaces/{w}/ops` | push, the only route besides workspace creation and repair that writes records ([§15](#push)) |
| `POST /v2/workspaces/{w}/pull` | pull every listed domain in one request, the only pull route ([§15](#pull)) |
| `GET /v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}` | the device's record at that counter, or its next record above it when the device skipped the counter, as an `entry` |
| `GET /v2/workspaces/{w}/domains/{d}/heads/{device}` | the server's chain head for a device; `not_found` when the device has no record in the domain |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoint` | latest checkpoint bundle, with its entry's sequence; `not_found` before the first |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoints/{digest}` | one checkpoint by digest, as an `entry` ([§17](#checkpoint-chain)) |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoint-chain?from={digest}` | checkpoint records from that digest back through `previous`, newest first, as many as fit one response ([§17](#checking-a-checkpoint)) |
| `GET /v2/workspaces/{w}/domains/{d}/live?at={seq}&after={cursor}` or `?at={seq}&rid={rid}&after={cursor}` | bootstrap page: operations live at `at`, ordered by `rid`, device and counter, or those of one `rid` |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&level={0..3}&index={j}&after={cursor}` | 16 child hashes of a node, or a bucket's leaf refs at level 3 |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&buckets=all` | all 4,096 bucket hashes |
| `POST /v2/workspaces/{w}/repair` | bring back policy changes, epoch bundles and envelopes after a server restore, starting with the genesis for a workspace the server doesn't hold ([§14](#after-a-server-restore)) |
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

Four routes page, each with its own cursor: `policy` by revision (`after`, the last revision received), `keys` by key sequence (`after`, the last key sequence received), and `live`, in both forms, and `manifest` level 3 by `live_cursor` (`after`, the `next` of the previous page). The client asks again until `policy` reaches `head_revision`, `keys` reaches `head_key_seq`, or `next` is absent. `epochs` and `checkpoint-chain` return as many records as fit, and the client asks again from the last one it got. Every other list fits one response within the limits of [§27](#27-limits).

| Route | Request | Response |
|---|---|---|
| `GET /v2/limits` | | `u16 protocol`, `u64 restore_generation`, `u32 max_body`, `u32 max_inline`, `u64 max_blob`, `u32 max_covers`, `u32 max_recipients`, `u32 presence_per_second`, `u64 retained_bytes` |
| `POST /v2/device/authorize` | | `str device_code`, `str user_code`, `str verification_uri`, `u32 expires_in`, `u32 interval` |
| `POST /v2/device/token` | `str device_code` | `str session_token`; errors use the RFC 8628 codes `authorization_pending`, `slow_down`, `expired_token`, `access_denied` |
| `POST /v2/devices/challenge` | | `key challenge`, `u32 expires_in` |
| `POST /v2/devices` | `bytes enroll` | |
| `GET /v2/devices` | | `list<device_info>`: `bytes card`, `u32 sessions` |
| `DELETE /v2/devices/{device}` | `bytes revoke` | |
| `POST /v2/approval-requests` | `digest commit`, `opt<id> invitation`, `opt<digest> invite_proof` | |
| `GET /v2/approval-requests`, `GET .../pending-devices` | | `list<pending>`: `bytes card`, `digest commit`, `opt<id> invitation`, `opt<digest> invite_proof` |
| `POST /v2/approvals` | `bytes offer` | |
| `GET /v2/approvals` | | `list<bytes>` of offer records |
| `POST /v2/approvals/{offer_digest}/reveal` | `key nc` | |
| `GET /v2/approvals/{offer_digest}/reveal` | | `key nc` |
| `POST /v2/recovery` | `bytes recovery_proof` | |
| `GET /v2/workspaces` | | `list<workspace_info>`: `id workspace`, `u8 role`, `u64 used_bytes`, `u64 history_bytes`, `opt<u64> quota_bytes` (absent when unlimited), `opt<bytes> profile` |
| `POST /v2/workspaces` | `list<record_ref>`: genesis, the workspace domain's first epoch bundle, envelopes, profile, and `add_device` changes, with their envelopes, for the devices and recovery identity of your account that the creating device pinned ([§7](#pinned-cards)), never for a removed card or one only `GET /v2/devices` lists | `u64 policy_revision`, `list<receipt>`, `list<commit>` |
| `GET .../policy` | | `u64 head_revision`, `list<tagged>` of the policy records after `after`, in revision order, at most 1 MiB; `tagged` is `bytes record`, `u64 generation`, `digest commit_tag` |
| `GET .../keys` | | `u64 head_key_seq`, `list<key_entry>` after `after`, in key-sequence order, at most 1 MiB: `u64 key_seq`, `u8 record_type` (3 epoch bundle or 4 envelope), `bytes record`, `u64 generation`, `digest commit_tag` |
| `GET .../epochs/{epoch}` | | `list<bytes>` of epoch bundles, newest first, at most 1 MiB |
| `POST .../ops` | `list<record_ref>` | `u64 policy_revision`, `list<receipt>`, `list<commit>` |
| `POST .../pull` | `list<pull_cursor>`, `u32 wait_seconds` | `list<page>` of pull pages ([§15](#pull)) |
| `GET .../chain/{device}/{counter}` | | `entry` |
| `GET .../heads/{device}` | | `u64 counter`, `digest record_digest`, `u64 seq` |
| `GET .../checkpoint` | | `u64 seq` of the checkpoint's entry, `checkpoint_bundle` |
| `GET .../checkpoints/{digest}` | | `entry` |
| `GET .../checkpoint-chain` | | `list<bytes>` of checkpoint records, newest first, at most 1 MiB |
| `GET .../live` | | `u64 at`, `opt<live_cursor> next`, `list<entry>`; `live_cursor` is `id rid`, `id device`, `u64 counter`, the last entry returned, and pages exclude it |
| `GET .../manifest`, levels 0 to 2 | | `list<digest>` of the node's 16 children |
| `GET .../manifest`, level 3 | | `list<leaf_ref>`: `id rid`, `ref`, sorted by `rid`, device and counter; then `opt<live_cursor> next` |
| `GET .../manifest`, `buckets=all` | | `list<digest>` of the 4,096 bucket hashes |
| `POST .../repair` | `list<repair_ref>`: `u8 record_type` (2 policy change, 3 epoch bundle or 4 envelope), `bytes record`, `u64 generation`, `digest commit_tag` | `list<receipt>`, `list<commit>` |
| `GET .../missing-blobs` | | `list<id>` |
| `POST .../invitations` | `id invitation`, `digest token_hash`, `u8 role`, `bytes invite_context` | `u64 expires_at` |
| `GET /v2/invitations/{token}` | | `id workspace`, `id invitation`, `u8 role`, `u8 status`, `bytes invite_context` |
| `GET .../invitations` | | `list<invitation_info>`: `id invitation`, `u8 role`, `u8 status` (1 pending, 2 accepted), `opt<id> accepted_by`, `u64 expires_at`, `bytes invite_context` |
| `POST .../shares` | `digest token_hash`, `id domain`, `u8 expires_in_days`, `bytes share` | `u64 expires_at` |
| `GET .../shares` | | `list<share_info>`: `digest token_hash`, `id domain`, `u64 expires_at` |
| `PUT .../shares/{token_hash}` | `bytes share` | |
| `GET /v2/shares/{token}` | | `bytes share` |

`error` is `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. The `detail` of `chain_conflict` is `id domain | u64 counter | digest record_digest`, the server's head; of `cursor_compacted` from `live` or `manifest`, `u64 retention_floor | u64 checkpoint_covered_seq` (a pull reports both conditions per page instead, [§15](#pull)); of a failed push or repair, `u32 record_index | str code | opt<bytes> record_detail`, where `record_detail` holds the failing record's own detail, such as a `chain_conflict` head.

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
| `epoch_stale`, `epoch_conflict`, `rotation_pending` | 409 | the operation's epoch isn't the latest epoch entry's; another rotation took this epoch; the domain waits for a rotation, so it takes no record, presence or new reader |
| `policy_stale`, `policy_conflict` | 409 | a record names a revision above the head; another change took this revision, or `policy_head` differs from it |
| `policy_regressed` | 422 | a record's `policy_revision` or `generation` is below the previous record's in its chain |
| `recipients_invalid` | 422 | an epoch's recipients or an envelope's recipient don't match the authorized devices |
| `manifest_mismatch` | 422 | a checkpoint's root, `live_objects` or `heads_digest` differs from the server's |
| `cursor_compacted`, `pruned` | 410 | a `live` or `manifest` request names a checkpoint below the retention floor; the chain position was pruned |
| `blob_incomplete`, `blob_digest_mismatch`, `blob_domain_mismatch` | 409, 422, 422 | a referenced blob isn't complete; an upload's digest doesn't match; the blob was created for another domain |
| `quota_exceeded` | 507 | the workspace is over its storage quota and the record would raise usage, or an upload wouldn't fit ([§20](#20-atomic-changes-and-quota)) |
| `id_conflict` | 409 | a client-chosen or derived ID already exists with other bytes, an `add_device` reuses a device ID the chain names, or an enrolled card's device was revoked |
| `rid_domain_mismatch` | 409 | the `rid` belongs to another domain |
| `checkpoint_not_due` | 409 | a standalone checkpoint arrived while none was due, or its `covered_seq` sits below the due point |
| `checkpoint_stale`, `checkpoint_unknown` | 409 | a checkpoint's `previous` isn't the domain's latest checkpoint; a record names a checkpoint this domain doesn't hold at that `covered_seq` |
| `rotation_not_due` | 409 | an epoch record arrived with no new domain or pending rotation behind it |
| `restore_quiet`, `commit_unknown` | 409, 422 | the server was restored less than 7 days ago and takes policy changes and keys only through repair; a repaired record's commit tag doesn't verify for an earlier generation |
| `recipient_limit` | 409 | the change would give a domain more than 512 active readers |
| `profile_conflict` | 409 | a profile's `profile_revision` isn't one above the server's latest |
| `workspace_limit` | 409 | the account already owns 100 workspaces |
| `snapshot_required`, `conflict_limit` | 409 | the `rid` holds too many live patches, so the client writes a snapshot first; the `rid` holds 64 live state operations, so the client resolves first |
| `supersedes_incomplete` | 409 | a state operation leaves its own device's earlier state operation on the `rid` live |
| `domain_limit` | 409 | the workspace already holds 256 domains |
| `before_history_floor` | 403 | the request reaches below the account's history floor |
| `challenge_invalid`, `origin_mismatch` | 400 | the enrollment challenge is unknown, used or expired; the proof names another origin |
| `offer_expired`, `invitation_limit`, `invitation_invalid`, `share_limit` | 410, 409, 404, 409 | the approval offer is older than 10 minutes; the workspace already has 100 open invitations; the invitation is revoked, expired, used or unknown; the workspace already has 1,000 active share links |

Client error codes: `sync_chain_gap`, `sync_chain_reordered`, `sync_chain_replayed`, `sync_chain_forked`, `sync_policy_forked`, `sync_policy_rollback`, `sync_policy_unauthorized`, `sync_policy_regressed`, `sync_writer_not_authorized`, `sync_epoch_stale`, `sync_key_mismatch`, `sync_record_unreadable`, `sync_manifest_mismatch`, `sync_manifest_rollback`, `sync_journal_rollback`, `sync_approval_mismatch`, `sync_recovery_kit_invalid`, `sync_workspace_unverified`, `sync_profile_rollback`, `sync_reseal_limit`, `sync_server_rollback`. Chain and record errors name the device and domain involved. A client that gets `before_history_floor` or `cursor_compacted`, or a pull page with status 2 or 3, bootstraps that domain ([§18](#18-bootstrap)).

### Rate limits

The server limits sync traffic per device and in bytes, with a separate allowance for bootstrap pages and range reads, and limits sign-in per client IP. Records that reach a device through the realtime channel don't count. A request over a limit gets `rate_limited` with `retry_after_seconds`. The defaults, which a server setting changes:

| Traffic | Default per device |
|---|---|
| Pushes | 10 a second, which leaves room for the 500 ms text tick, presence and rotations |
| Push bodies and blob uploads (tus `PATCH`) | 64 MiB a minute |
| Pulls, chain and checkpoint fetches, policy and keys | 128 MiB a minute |
| Bootstrap allowance: `live`, `manifest` and blob range reads | 512 MiB a minute |
| Other sync requests | 600 a minute |
| Sign-in requests (device grant, magic link, passkey) | 30 a minute per client IP |

At these defaults, the join in [§28](#28-worked-example-100000-objects) downloads its 10 GB of attachments in about 20 minutes.

## 27. Limits

| Limit | Value |
|---|---|
| Request or response body, push, tus PATCH, range response | 1,048,576 bytes |
| Inline payload before padding | 262,144 bytes |
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
| Device authorization codes, enrollment challenges, magic links, approval requests and offers | 10 minutes |
| Sessions | no expiry; a session ends when its device is revoked, at sign-out or from another device |
| Enrolled devices that aren't revoked | 256 per account |
| Workspaces | 1,000 per account, 100 of them owned |
| Active recovery identities | 1 per account and workspace |
| Pending approval requests | 32 per account, 100 per workspace |
| Re-seals per chain position that no newer epoch or policy change forces | 3 |
| Text mode | the writer's device leaves it at 4 MiB of UTF-8 text; a note rebases at 16 MiB of encoded document; a record that jumps past 8 MiB of text or 32 MiB of document stops the lineage on that device |
| Collaboration sandbox verdict, per record | 1,000,000 fuel units plus 1,000 per byte of decrypted yrs bytes, 512 MiB of memory, one pinned guest module |
| Collaboration sandbox integration | 2,000,000,000 fuel units per writer, note and minute; 10,000,000,000 for one record |
| Share snapshot, padded | 983,040 bytes |
| Retained log per domain | `retained_bytes` of weight, a server setting, 1 GiB by default, less while the workspace's usage and history exceed its quota |
| Checkpoint interval | 10,000 log entries or 64 MiB of weight, at due points the server publishes; a writer waits a random 0 to 60 seconds after a due point |
| Workspace quota | `workspace_quota_bytes` of live data, policy changes and epoch bundles, a server setting, unlimited by default; history yields to it |
| Completed upload that no operation lists | 24 hours |
| Sync rate limits | [§26](#rate-limits) |
| Quiet period after a server restore, without policy or key pushes | 7 days |

The targets of 100,000 objects and 200 devices are tested sizes, not caps. No structure in this document stops at 1,000 objects.

## 28. Worked example: 100,000 objects

The workspace holds 95,000 notes averaging 3 KiB and 5,000 attachments averaging 2 MiB, across 200 devices. It has 3 domains: the workspace domain and 2 restricted domains of 20 devices each.

**Record sizes** from the layouts above: an envelope is 1,366 bytes; an epoch record is 351 bytes, and its bundle with 199 recipients 3,543 bytes; an operation carrying a 3 KiB note and one cover is 3,717 bytes (389 bytes of fields, AEAD tag and signature, and a 3,328-byte padded payload); a checkpoint bundle with 200 chain heads is 13,144 bytes; a policy entry is 40 bytes.

### Removing one device

The removed device could read all 3 domains. Its owner's other device signs and pushes the removal, and the server adds the removal's entry to each of the 3 logs. The rotations follow, one push per domain: about 276 KB for the workspace domain and about 27 KB for each restricted domain.

| Record | Count | Bytes |
|---|---|---|
| `remove_device` | 1 | 173 |
| Epoch bundle, workspace domain | 1 | 3,543 |
| Envelopes, workspace domain | 199 | 271,834 |
| Epoch bundles, restricted domains | 2 | 1,326 |
| Envelopes, restricted domains | 2 × 19 = 38 | 51,908 |
| Record framing, 5 bytes per record | 241 | 1,205 |
| Total | | 329,989 bytes |

The removal re-encrypts no object; the 100,000 objects add nothing. Each domain where the removed device wrote then takes a fresh checkpoint of about 13 KB from whichever writer signs first. The worst case is a workspace whose 256 domains all reach all 200 devices: 256 rotations of about 276 KB each, about 71 MB in all, each in its own push.

### Joining

A new member's device joins with access to the workspace domain.

1. **Approval push**, about 16 KB in one request: `add_member` (190 bytes), `add_device` (1,503 bytes), one envelope for the workspace domain (1,366 bytes), and a fresh checkpoint bundle of 13,144 bytes.
2. **Policy and keys**: a policy chain of 3,000 changes is about 2.6 MB (3 pages), commit tags included, if half are `add_device` changes and half `remove_device` changes. The device fetches the current epoch bundle of the workspace domain; walking lineage back across 300 past epochs, when live operations name them, reads about 1.1 MB of epoch bundles in two requests.
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
| Accounts | account ID, derived from the sign-up email, email, passkey credentials, email confirmation state | who you are |
| Sessions | token hash, account, device; no expiry | that a device signed in |
| Magic links, device grants, enrollment challenges | token hashes, account, expiry, deleted when used or after 10 minutes | that you're signing in |
| Devices | card (account, device ID, public keys), revoked or not with the signed revocation, which sessions belong to which device; whether a device is active in a workspace comes from that workspace's policy | how many devices you enrolled; a card that never enrolls a session suggests a recovery identity |
| Pending approvals | approval requests with their commitments, offers and reveals until they expire | that a device is waiting for approval |
| Server | the restore generation, the end of the quiet period, the heartbeat counter, and the stamp `backup` writes into a copy; outside the database, the server secret and the heartbeat file | when the server was last restored, since the generation is a Unix time |
| Workspaces | workspace ID, owner account, per-domain usage and history | that the workspace exists, and how much data it holds |
| Policy changes | full signed records, also as entries in every domain's log | members, roles, device events, restricted domains and their grants, in order |
| Domains | domain ID, kind (workspace or restricted), marker `rid`, current epoch, rotation state, history floors, the highest published retention floor | how many domains exist and who reads each |
| Chain heads | per device and domain: counter, digest, sequence | the last position each device wrote |
| Epoch records and envelopes | full signed bundles with their recipient lists, also as log entries in the domains they cover; envelopes of current epochs; the key sequence | when each domain rotated, in log order, and which devices held its key |
| Operations | domain, sequence, device, counter, previous digest, epoch, policy revision and head, restore generation, the checkpoint the writer had reached, `rid`, the delete flag, supersedes, blob IDs, nonce, padded ciphertext, signature, digest | which device changed which random ID and how often; which operations delete an object; concurrent edits and incremental updates; padded payload size; how far each writer lagged behind the latest checkpoint |
| Checkpoints | full signed records, with the previous checkpoint's digest and the floor, and chain heads, as log entries; bundles and bucket hashes of every checkpoint at or above the retention floor; every checkpoint record while the domain exists | the number of live objects per domain, and how many checkpoints the domain took |
| Blobs | blob ID, the domain it was created for, uploading device, padded ciphertext length, ciphertext digest and segment hashes, upload state, referencing operations; a deletion time only while the upload is incomplete or unlisted after completion ([§19](#19-compaction)); the blob store's file times or `LastModified` | padded attachment size; that an operation refers to a large file; when each attachment was uploaded |
| Share links | token hash, workspace, domain, creating device, expiry, signed padded snapshot | that a domain has a shared item, which device shared it, and from its expiry roughly when |
| Invitations | while open: ID, token hash, workspace, role, inviter, accepting account, status, expiry, padded context | who is inviting whom, with which role, until the invitation ends |
| Workspace profile | signed encrypted record | when the name changed |
| Rate counters | key and counter | request volume |
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
| Forges an operation, policy change, epoch record, envelope, checkpoint, presence record or share snapshot | yes | signatures under cards pinned through the policy chain, or the link's verify key |
| Inserts its own device key | yes | `add_device` needs an authorized signer, and approval needs matching codes |
| Plants a device in a member's account, with an owner or admin | yes | another account's `add_device` is valid only right after that account's `add_member`, signed by the same device, `sync_policy_unauthorized`; the member's devices alert on any card of their account they didn't witness |
| Reuses a device ID under another card | yes | device IDs are unique in a chain, `sync_policy_unauthorized` |
| Lists an extra card in `GET /v2/devices` | yes | a zero-offer `add_device` takes cards only from the signer's pinned cards |
| Brings back a card removed earlier, or a chain a removed card signed | yes | each device keeps removed cards on its removed list and pins or trusts none of them; clients reject an `add_device` for a card the chain removed |
| Swaps a card during approval | yes, except with probability 10^-8 per attempt | the candidate's committed nonce keeps the two codes independent, and the candidate uses each nonce for one offer |
| Returns an older or different policy chain | yes, after first contact | `sync_policy_rollback`, `sync_policy_forked` |
| Shows two devices different changes at one policy revision | when a record crosses | `policy_head` in every record, `sync_policy_forked` |
| Gives recipients of one epoch different keys (with a malicious member) | yes | `key_check` |
| Lists an unauthorized recipient in an epoch | yes | recipients checked against the verified policy |
| Serves a manifest that omits an object | yes, with an honest signer | the recomputed root differs from the signed checkpoint |
| Rolls a device back to an older checkpoint | yes | `sync_manifest_rollback`, against the latest accepted checkpoint and the anchors in approvals and kits |
| Seeds a new device from a checkpoint that a removed device signed, placed before the removal's entry, or from a checkpoint off the anchor's chain | yes | the bootstrap checkpoint must descend from the device's anchor, and no removal in the device's verified chain may postdate its `policy_revision` for its signer or a device in its heads |
| Claims compaction or lowers the retention floor to force a bootstrap from an old checkpoint | yes | the published floor never drops within a generation, and the bootstrap checkpoint must cover the client's cursor |
| Serves a live operation at different log positions to different clients | when a checkpoint arrives | sequences in leaf refs make the roots differ |
| Shows two recipients two different epoch records for one epoch (with the rotating member) | when either sees the other | the epoch record chain, checkpoints naming the epoch record, records that stop decrypting |
| Accepts writes from a device that lost write access | yes, for a record after the change's entry; for a record the server places before that entry in one client's view, when the remover's receipts or a checkpoint covering the entry reach that client | the policy entry in each log marks where write access ends, `sync_writer_not_authorized`; the remover's receipts and the final head that the first checkpoint covering the entry lists, `sync_manifest_mismatch` (see split views below) |
| Commits after a removal an epoch record that a former reader, or the removed device, signed at an older revision | yes | rotations are judged by revision, so the rotation stays pending and the remover still rotates; recipients refuse its envelopes, `sync_key_mismatch` |
| Leaves a policy change's entry out of one log | yes | a client that accepted the change requires its entry, `sync_chain_gap` |
| Keeps writers from seeing a removal, so a removed device keeps reading their new records | only after the leak, when one of those records or a later checkpoint reaches a device that accepted the removal | the epoch rule, `sync_epoch_stale`; until then, each device's last-heard-from time |
| Claims a restore so clients accept lower heads and checkpoints | shown to you | every generation change raises an alert; clients rebuild only from a checkpoint they accepted or one descending from it, still refuse a fork of the policy or an epoch, check every record's write access by position, write lost content again from their files, and list for review a removed device's records above its bound on a device that accepted the removal before the restore |
| Goes back to an older state without raising the generation | yes | `sync_server_rollback` when a head, `checkpoint_seq`, chain head or checkpoint falls below one the client saw in that generation |
| Takes over an ID after a restore: an account, a device or a workspace | yes | account IDs derive from the sign-up email, device IDs from the signing key and workspace IDs from the genesis, and repair takes back only records carrying the server's commit tag |
| Forwards an old presence record | yes | `minute`, the current epoch, the per-session sequence |
| Forges or drops realtime notifications or records frames | harmless | both feed the same verification as pulls, and a gap sends the client to pull |

### What the client can't prevent

- **Withheld service.** The server can refuse requests, freeze a domain at an old state, or delete ciphertext. Your files stay on your devices.
- **Split views.** The server can show different clients different subsets of records. Clients detect the split once any evidence crosses it: a record, which names its writer's policy head and latest checkpoint, a checkpoint, a receipt or a policy head. A server that partitions clients completely and forever goes unnoticed. While the server keeps a writer's records from every device that accepted a removal, the removed device reads them without limit. Detection fires only when one of those records, or a later checkpoint, reaches such a device, which is after the leak; during the partition, the app's last-heard-from time for each device is the only signal. In the same way, only the remover's receipts fix where a removal's entry sits. The server can place a removed device's later records, signed at an older revision, before that entry in another client's view, and that client applies them until the checkpoint that covers the entry, signed by a device that saw the true order, reaches it and lists a different final head. Withholding that checkpoint means withholding every later checkpoint and every record whose writer reached it.
- **Timing and traffic.** The server sees when you sync, how many operations you send, padded sizes, which random IDs change together, your IP address and your account email. Text updates show which `rid` each writer edits, to the 500 ms tick, and presence channels show which devices have the same note open.
- **Membership.** Every member and the server see the policy: members, roles, devices, domain kinds and restricted grants. Members outside a restricted domain learn that it exists and who holds it, never its folder name or contents.
- **History for new members.** A new member's domain keys open older ciphertext in those domains through lineage. The history floor keeps the server from serving it, and a malicious server could serve it anyway. A text-mode note also reaches a new member with its live patches, up to the snapshot thresholds of [§22](#snapshots), and those can hold text deleted since. A file you move into a restricted domain leaves its earlier versions in the source domain's log, under keys that the source domain's current and future members hold, until the floor passes them; the app warns you when you move it ([§9](#moving-objects-between-domains)).
- **Access granted again.** A member who loses access and later gets it back, on new devices, can read what was written while they were away ([§9](#rotation)). The app warns whoever invites or grants again. A removed device never gets back in.
- **Pending rotations.** While a rotation is pending, the server accepts no record or presence in that domain, so nothing new uses the old key. Until a device rotates it, the server alone keeps the domain's existing ciphertext and keys from the removed device.
- **Old share snapshots.** The viewer warns when a later visit shows a lower `snapshot_revision`, but a first visit can't tell a current snapshot from an older one the server kept after Update link.
- **Served web code.** The server serves `account-web`. A modified viewer page could read the fragment and leak a share link's keys. A modified invitation page could read the secret of an invitation link you opened in a browser, and with it the workspace and inviter names and the means to forge an invitation proof; the approval code still stands between it and your keys. The desktop app handles invitations without the browser and doesn't depend on served code.
- **Blame for forks.** A server can't fake a fork from a re-seal, because each attempt names the attempt it replaced. Two records at one position that aren't consecutive attempts come from the device's key; clients can't tell whether the device misbehaved or a stolen key did.
- **A colluding writer.** An editor who colludes with the server can sign a checkpoint that omits objects, and a new device bootstrapping from it misses them. Current devices reach that checkpoint as a log entry and detect the mismatch against their own root; returning devices hold missing files for review instead of deleting them.
- **A new member's first device.** An owner or admin adds a device to another account only right after that account's `add_member`, signed by the same device, and `add_member` takes only an account that isn't a member; clients check both in the chain. The server checks that the member's own sign-in enrolled the card; clients can't check that, so everyone relies on the inviter's code comparison with the new member, and the member's own device checks that the added card is its own. A member who lost every device and their kit is removed and invited again; removal drops their devices, grants and manager entries, so a manager grants restricted access again only after checking with them.
- **A stolen device.** Until you remove it, a stolen unlocked device, or malware on it, holds the same authority as your other devices. It can add devices and recovery identities to your account, and your other devices pin them through it. Each of your devices alerts you about every such card ([§7](#device-list)), and removing the stolen device removes every card it added, directly or in turn ([§7](#removing-a-device)), before the rotations wrap any key. The thief can also strike first: remove your other devices and your recovery identity from every workspace and revoke them on the server, leaving the stolen device as your account's only one. When your account owns a workspace, the thief then owns it, and you keep only your files, from which you can turn sync on as a new workspace. v2 gives a recovery identity no precedence over your devices, because a policy chain has no clock that could delay a removal; remove a stolen device as soon as you can.
- **Recovery after first contact.** A recovering device trusts the policy head stored in its kit. A server can withhold changes made after the kit was written, such as the kit's own revocation or the removal of a stolen device, and show changes that the stolen device signs instead, until another device's records reveal them. In that view the stolen device still holds its role, so the recovery flow shows you every change after the kit's head, removes every other device of your account before it rotates, and lists members added after the head for review ([§12](#12-recovery-kit)). Exporting a new kit after each device removal keeps the stored head past it.
- **A restored machine.** A restore of a whole machine from backup can roll back both the journal and the credential store, and the device then can't tell. The server's heads and the checkpoints are its only check. The device may then sign a counter again whose first record never reached the server, and a server that kept that first record can later show readers a fork. A journal rolled back alone costs nothing, because the device starts a new counter block at each start ([§14](#your-own-chain)).
- **A restore you learn of late.** A client that accepts a change ending a device's write access only after it sees a restore can't tell that device's records, which a server that lost the change accepted, from honest ones, and applies them. Devices that accepted the change before the restore list those records for review and alert you ([§14](#after-a-server-restore)).
- **A removal that a restore lost for good.** Repair brings a lost removal back as soon as any device that accepted it comes online during the 7-day quiet period. When every such device stays away longer, the removed device counts as active again, gets the rotation that ends the quiet period, and can push changes, including the removal of your devices. Each device that held the removal refuses that chain and alerts you when it returns ([§14](#after-a-server-restore)), but devices that never saw the removal follow the server.
- **Logs and backups.** PostgreSQL's write-ahead log and its archives date every push to the second, and a streamed SQLite replica's object times date batches of commits ([§29](#29-what-the-server-stores)). Whoever holds them learns when each record arrived.

### Members and devices

- A member writes within their role. A malicious editor can delete objects or write records nobody can read; other devices keep their local copies and list unreadable records for review. A member can't rotate a domain outside a required rotation.
- A removed device keeps everything it received, can read nothing encrypted after the rotation, and never rejoins the workspace. A removed member invited again reads, through lineage, what was written while they were away.
- A device that forks its own chain halts that domain for readers until someone removes its write access; readers then keep the branch whose head the first checkpoint covering the removal's entry lists ([§14](#verifier)).
- Anyone holding a recovery kit and its secret can read, with no device and no alert, every current and future record of every domain your account reads, given the ciphertext from the server, a server backup or your account session, because rotations keep wrapping keys for the recovery identity. They can also add one device of your account to each workspace. That device is a full device: it reads, approves your later devices, adds others, removes your devices and the recovery identity, and revokes them on the server. All of this lasts until you revoke the kit by removing the recovery identity and remove what it added ([§12](#12-recovery-kit)).

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

| Fixture | Pins |
|---|---|
| `sync-v2-encoding.json` | field types, tag bytes, decoding rules, `opt` discriminators, record tags checked against their position, every message body, error code, HTTP status and `detail` in [§26](#26-http-api), each pull page status with the other domains' pages still returned, every realtime frame including an error frame for refused presence and a refused subscribe, and a records frame applied, ignored or answered with a pull for each cursor position around `previous_seq` and inside the frame, the general field limits of [§4](#decoding-rules) and each field's limit, the `Noura-Restore-Generation` header, `checkpoint_due` in pages and frames, `not_found` for an absent head or checkpoint, `live` paging by `rid`, and the bodies of `revoke`, `missing-blobs`, `checkpoint-chain`, repair and commits |
| `sync-v2-padding.json` | `padded_length` for boundary lengths, malformed padding |
| `sync-v2-ids.json` | remote ID format, the text forms of [§5](#5-identifiers): 16-byte values in 22 characters, 32-byte values and the session token in 43, decimal integers, a `live_cursor` as base64url of its encoding, device and workspace IDs derived from a card and a genesis, and a card or genesis whose ID doesn't derive |
| `sync-v2-device.json` | `noura.sync.device.card` with an X-Wing key, device fingerprint, `noura.sync.device.enroll` (wrong origin, account and key), an enrollment that binds a new session to an enrolled card, and one refused for a revoked device or a device ID held with other bytes, and `noura.sync.device.revoke` signed by the device itself, a recovery identity and an active device, and refused from a removed device, an unapproved device and a session alone |
| `sync-v2-approval.json` | `noura.sync.approval.commit`, `noura.sync.approval.offer` with checkpoint anchors and their generation, an anchor compared across generations, `noura.sync.approval.code`, a reveal that doesn't match its commitment, a second offer for an already revealed commitment, ground to match the approver's code, that the candidate refuses, an approver that ignores further requests while an attempt is open, `noura.sync.v2 invitation proof` |
| `sync-v2-policy.json` | `noura.sync.policy`, each change kind and both device kinds, chain links, authority failures, a recovery identity signing a forbidden change, an `add_device` for another account at a later revision than its `add_member`, two such changes after one `add_member`, one signed by a device other than the `add_member`'s signer, an `add_member` for an account that is already a member, an `add_device` whose device ID the chain already names under another account or as a recovery identity, removing a device that added another device and a recovery identity, with the removal flow listing both and the alerts on your other devices, a removed card added again, a member removed and added again who stays out of every restricted list until a manager grants access, a zero offer from another account, a card that only `GET /v2/devices` lists and that a creating device must not add, a chain forked at one revision and served to two clients; pinned cards across a workspace created after a removal, a first verification of a chain that removed a card, a join after recovery, and a fabricated chain signed by a removed card; policy entries in every domain's log, a record after the entry that ends its device's write access, an entry missing from one log, an entry at a sequence other than its receipt's, an approval into a domain whose last entry is a checkpoint, a re-checkpoint after the latest signer is demoted, and an admin's removal of another account's device in one workspace that leaves it active in another; an editor's `remove_device` for a viewer's device, refused; a second active recovery identity, refused, and a new one after the old one's removal, accepted; a change that gives a domain a 513th reader; a removal of your own device signed in one workspace only, which your other devices complete, with chains through the removed card's earlier `add_device` changes still trusted once a pinned card removes it; a removal flow that leaves out a card the removed device only copied with a zero offer, and a recovery that keeps the recovering device |
| `sync-v2-keys.json` | `noura.sync.epoch`, the epoch bundle and `noura.sync.epoch.recipients`, the epoch record chain, `noura.sync.key-check` with `noura.sync.v2 key check`, lineage, `noura.sync.envelope`, HPKE `info` with X-Wing, an unpinned signer, the epoch rule (an old-epoch record after a newer epoch entry, a record between a rotation-requiring policy entry and the next epoch entry, and the same cases after a bootstrap), an epoch entry after a removal whose `policy_revision` predates the removal, signed by a former reader and by the removed device itself, which leaves the rotation pending in the log and after a bootstrap, an envelope that the removed device signed, which recipients refuse, an envelope that arrives through the key route before its epoch's entry, an epoch record with no rotation due, a restricted domain's epoch 1 from a device other than the `create_domain` signer, a grant without the new readers' envelopes, `noura.sync.v2 lineage key`, and cards a wrapping device must refuse: one absent from the policy, a pinned device ID with another HPKE key, and a device of an account the domain doesn't grant, the rotation that ends a restore's quiet period, accepted once per domain, and the commit tags of epoch bundles and envelopes |
| `sync-v2-operation.json` | `noura.sync.op` with its `generation`, delete flag and checkpoint reference, a `generation` that decreases along a chain, `noura.sync.payload` for each of the four kinds, a delete flag that doesn't match the payload, AEAD associated data with the epoch record's digest, `noura.sync.v2 operation key`, path validation, a payload whose kind doesn't match its class (a `file` that covers nothing on an existing `rid`, a `text_update` on a new `rid`, a delete that covers nothing), and a counter that skips |
| `sync-v2-chain.json` | schedules for gap, reorder, replay, fork, a retried upload, consecutive re-seal attempts served in either order, sibling attempts, an attempt that arrives after a successor of the other, a replay right after bootstrap, an unreadable record, a checkpoint that arrives after the client passed its `covered_seq`, a checkpoint entry left out of one client's log while other records still cross, a line of more than 4 re-seal attempts forced by rotations, a missing epoch entry, a record rejected with `snapshot_required`, `conflict_limit` or `quota_exceeded` and replaced at its position by a different record, a device returning after the floor passed its cursor, a removed device's head leaving the checkpoint heads, a device demoted, checkpointed, promoted and then bootstrapped past, whose chain continues from the head the checkpoint kept, a forked chain of a removed device resolved by the first checkpoint covering the removal's entry, a chain that skips counters at each start, after a crash between raising the reservation and writing the journal, and after a journal rolled back, accepted; a skipping record whose predecessor the page left out, fetched as the device's next record and reported as a gap or reorder; and, after a restore: a writer that restarts its chain above every counter it signed, accepted by readers that rebuilt; a record from a device removed after the restore, refused by position; a removed device's records beyond the head the client held, listed for review; and a pull page `head` below one seen in the same generation, `sync_server_rollback` |
| `sync-v2-live.json` | live sets and supersession over operation schedules (covers of covered operations, pruning, a lone delete left out at a checkpoint's `floor`, an edit against a delete), a state operation that leaves its own device's earlier state operation live, the conflict outcomes in [§16](#16-conflicts), path collisions including a file against a valid marker's folder, text lineage merges, and the idle and volume snapshot rules of [§22](#22-live-collaboration), including the idle snapshot that another writer writes after 5 minutes; a lone delete that a `floor` passed, then an edit on its `rid` that doesn't cover it, computed alike by server and client; an ordinary edit and a delete on a conflicted `rid`, which cover only the writer's branch; and a reconciliation after a restore, where the server lacks a state that every writer applied and two devices write it again |
| `sync-v2-domains.json` | `.noura-domain` validity: planted, moved and evicted markers, a marker created inside a restricted root, a marker at a restricted domain's marker path, a later domain whose root holds an earlier one, an object of another domain under a restricted root written under ` (shared)`, a marker found in another domain's log, a marker with two live states, bootstraps that meet a moved, a deleted and a conflicted marker and record held roots, a file created during bootstrap before its domain's objects land, and a marker deleted locally that the client writes again, an object whose payload or merged `meta` path leaves its domain's root, kept at its last path and listed for review with no held root, a folder restricted while it holds workspace-domain files, which stay in place until their moves land, and an edit to a file under ` (shared)`, uploaded at its synced path |
| `sync-v2-manifest.json` | `noura.sync.manifest.leaf` with sequences, `.bucket`, `.node`, `.heads`, `noura.sync.checkpoint`, the heads digest check, a removed device leaving the heads and a demoted one staying, `previous` links, `floor`, a `floor` that rises between two checkpoints and drops lone deletes from the root, a `covered_seq` equal to the previous checkpoint's entry sequence, roots for add, remove and change, a rolled-back checkpoint, a log-entry checkpoint whose signer was removed later, a bootstrap checkpoint whose signer was removed before its entry, refused, one whose signer was demoted after it, accepted, and one whose signer was removed after its `policy_revision`, refused until a later checkpoint commits, and a failed log-entry checkpoint that doesn't hold the cursor, a checkpoint whose `heads_digest` or `live_objects` differs from the server's, refused, and a bootstrap checkpoint that a removed device signed at an older revision and the server placed before the removal's entry, refused, as is one that doesn't descend from the anchor |
| `sync-v2-blob.json` | `noura.sync.blob` header, the nonce prefix from `noura.sync.v2 blob nonce`, chunk nonces, lengths, `noura.sync.blob.parts`, and a blob rebuilt byte for byte from its file and `blob_ref` |
| `sync-v2-collaboration.json` | `noura.sync.text.lineage` and the first document of a lineage, written the same by two devices from one state and differently from two states; the `meta` map with a path and properties as canonical JSON, and the file bytes it yields; two concurrent snapshots that each carry a different property or path change, merged; frontmatter outside edits that change a value, remove a property, only reformat, or break parsing; yrs records near the verdict's fuel and memory limits and over them, with the same verdict under Cranelift and Pulley and on the previous and current release; base documents with records whose integration succeeds, is deferred past a writer's allowance, or sends the lineage to review; and an update that depends on an unreadable one; `meta` writes of a path that breaks the rules of [§13](#payload), a non-string value, a bad property name, a `p:` value that isn't canonical JSON, a `p:id` that differs from `object_id` and an unknown key, each unreadable; a note's first text snapshot after its first `file` operation, and a text-mode note moved to another domain as a `file`; a rebase at 16 MiB and a `file` operation at 4 MiB of text, with the record that jumps past 32 MiB stopping the lineage and the `file` operation that resolves it; a snapshot that leaves deferred and pending patches uncovered; byte thresholds counted in ciphertext with blob-carried patches; and the hash of the verdict module |
| `sync-v2-presence.json` | `noura.sync.presence`, `noura.sync.presence.payload`, `noura.sync.presence.channel`, `noura.sync.v2 presence key`, the clock window |
| `sync-v2-profile.json` | `noura.sync.profile`, `noura.sync.profile.payload`, `noura.sync.v2 profile key`, a profile under an epoch that a removal it names requires rotating past, two profiles at one `profile_revision`, and the server checks with their errors |
| `sync-v2-invitation.json` | `noura.sync.invite.token`, `noura.sync.invite.context`, `noura.sync.invite.payload`, `noura.sync.v2 invitation key` |
| `sync-v2-share.json` | `noura.sync.share.token`, `noura.sync.share` and its link signature, the fragment, `noura.sync.share.payload`, `noura.sync.v2 share key` |
| `sync-v2-recovery.json` | `noura.sync.recovery-kit`, `noura.sync.recovery-kit.payload` with the generation of its anchors, `noura.sync.v2 recovery kit`, the secret's base32 form, `noura.sync.recovery.proof` (wrong secret, tampered file), and a recovery shown changes after the kit's head that a removed device signed |

The store conformance suite runs `sync-v2-live.json` against both stores, so SQLite and PostgreSQL compute the same live sets as clients.

### Store and soak tests

Beyond the fixtures, the implementation tests these cases against both stores:

- two server instances racing pushes into one domain, with an injected commit delay, while every client receives every record;
- pushes into several domains racing policy changes, the collector, tus creation and share updates, which never deadlock;
- removing a device while it pushes a record every 500 ms, where every record after the removal's entry fails, on SQLite and on two PostgreSQL instances with injected delays between the policy read and the commit;
- 200 simulated devices in the workspace domain and 2 restricted domains, in a 24-hour soak at collaboration write volume with `retained_bytes` set to 64 MiB, that checks that the floor moves, that each domain's storage stays within the bound of [§19](#19-compaction) in the form given there, with the interval term measured from the largest entry and the writes during each wait, and that live-set bytes per text-mode note stay within one snapshot and 8 live updates once editing stops, also when the last editor disconnects right after its last edit; on SQLite it also checks that the server sustains 400 pushes a second, the 200 devices at the 500 ms tick, with no read transaction open across a pull's wait;
- a viewer that keeps pulling with an old cursor while the floor moves;
- a bootstrap during heavy checkpointing, and one whose checkpoint falls below the floor before it ends;
- `backup` while `serve` takes pushes, `restore` and `mark-restored`: `serve` refuses a stamped copy, and the generation rises above every earlier value;
- a database put back without `restore` or `mark-restored`, by file copy and by failing over to an asynchronous PostgreSQL standby: the heartbeat stops `serve` when the copy predates its last beat, and clients report `sync_server_rollback` otherwise;
- a server restored from an older backup after further activity, with a lost removal, records under since-rotated epochs, a workspace, an account and blobs created after the backup: repair from several devices brings the policy and keys back, a removed device that pushes a policy change during the quiet period gets `restore_quiet`, the devices write the lost content again from their files, including a record that every writer applied and whose author device is gone, and the rotation that ends the quiet period lets an offline device and the recovery identity read again;
- after a restore, a removed device whose session came back, which every other device revokes again, and a share link revoked after the backup, which its device revokes again;
- a restore whose blob store survived while the database lost the blob rows, and one whose blob store lost objects that live operations list, where devices upload them again under the same IDs;
- a removed device that pushes after a restore and before its removal is resubmitted, whose records the devices that accepted the removal list for review while no domain stalls;
- a workspace over quota that accepts deletes, removals and checkpoints, frees usage when a delete commits, rejects writes and additions that raise usage and uploads that don't fit, and moves its floors to the latest checkpoints while usage and history exceed the quota;
- completed uploads that no push lists, collected after 24 hours, and a push racing the collector for one blob;
- a collaborator who sends near-limit updates every 500 ms while the others keep editing the note;
- a restricted domain's rotation left pending by a remover that can't read it, with no record or presence accepted there until a grantee device writes and rotates it;
- a tampering proxy for every row of [§30](#30-threats) marked as detected;
- a stored-data check that finds no per-record timestamp in database rows, no session expiry, and PostgreSQL's `track_commit_timestamp` off, and confirms that only the blob store's file times or `LastModified`, and the WAL and its archives of [§29](#29-what-the-server-stores), date anything;
- a member outside a restricted domain who receives no notify, records or presence frame for it;
- two PostgreSQL instances whose devices see each other's presence and records frames, with notifications sent after commit.

### Tag bytes

Each tag is the length byte, the ASCII domain string and `0002`:

| Domain string | Length | Tag bytes (hex) |
|---|---|---|
| `noura.sync.device.card` | 22 | `166e6f7572612e73796e632e6465766963652e636172640002` |
| `noura.sync.device.enroll` | 24 | `186e6f7572612e73796e632e6465766963652e656e726f6c6c0002` |
| `noura.sync.device.id` | 20 | `146e6f7572612e73796e632e6465766963652e69640002` |
| `noura.sync.device.revoke` | 24 | `186e6f7572612e73796e632e6465766963652e7265766f6b650002` |
| `noura.sync.workspace.id` | 23 | `176e6f7572612e73796e632e776f726b73706163652e69640002` |
| `noura.sync.commit` | 17 | `116e6f7572612e73796e632e636f6d6d69740002` |
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

BLAKE3 `derive_key` contexts are the exact ASCII strings in [§3](#3-primitives), with no tag and no version bytes: `noura.sync.v2 operation key`, `noura.sync.v2 lineage key`, `noura.sync.v2 key check`, `noura.sync.v2 presence key`, `noura.sync.v2 profile key`, `noura.sync.v2 share key`, `noura.sync.v2 invitation key`, `noura.sync.v2 invitation proof`, `noura.sync.v2 recovery kit`, `noura.sync.v2 blob nonce`, `noura.sync.v2 commit tag` and `noura.sync.v2 account id`.

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

## 33. Decisions to confirm at sign-off

These choices go beyond the list of topics the protocol had to cover. Confirm or change each one:

1. **Binary encoding.** Records use the fixed-width binary encoding in [§4](#4-encoding) instead of v1's JSON tuples, and the signed bytes are the sent bytes.
2. **New primitives.** XChaCha20-Poly1305 replaces AES-256-GCM, HPKE with the hybrid X-Wing key encapsulation replaces age and the custom web envelope, and BLAKE3 replaces SHA-256 for digests.
3. **History isolation is the server's job.** Lineage lets one envelope per domain open old epochs, so joining never re-encrypts objects. A removed card never rejoins a workspace, and a member invited again reads what was written while they were away.
4. **Recovery kits are offline devices** with limited signing rights, protected by a generated 160-bit secret that needs no key stretching, and they stay current through rotations.
5. **Approval compares one 8-digit code** derived from the approver's offer and a nonce the candidate committed to first, confirmed on both devices: one offer and one comparison per `nc`, and one outstanding offer per approving device.
6. **Policy changes enter every domain's log**, so the server's commit order decides which records a removal cuts off, and server and clients check write access and epochs by log position.
7. **Deletes carry a cleartext flag**, so the server can prune a tombstone once the retention floor passes it. The flag shows that an object was deleted and where in the log, never its type, creation time or size.
8. **Retention is measured in bytes**: each domain keeps `retained_bytes` of log behind a checkpoint, 1 GiB by default, so storage stays bounded at any write rate and the server stores no sequence-to-date table. A device that falls further behind bootstraps.
9. **Limits**: 1 MiB bodies with every atomic change in one push, 256 KiB inline payloads, 2 GiB blobs, 256 covers per operation, 256 domains per workspace, 512 recipients per epoch.
10. **Edit beats delete** in a conflict until you choose, and a returning device asks before deleting files the manifest lacks.
11. **Edit mode and size class stay visible.** Patches that cover nothing show text mode, and blob IDs show content above 256 KiB. Hiding them would mean dropping server-side compaction or blob authorization.
12. **Only the inviting device approves an invitee**, and an owner or admin adds a device to another account only in the change right after that account's `add_member`, which clients check in the chain.
13. **Sync rate limits count bytes per device**, with a bootstrap allowance and the defaults in [§26](#rate-limits), and records delivered over the realtime channel don't count.
14. **Rollback protection lives in the credential store.** A high-water mark per workspace keeps a restored or copied workspace folder from rolling a device back or reusing its counters.
15. **A text-mode note's path and properties live in its yrs document**, so renames and frontmatter edits merge like text instead of going to review, and a lineage derives from the note's state, so two devices that start editing at once agree on it.
16. **Every operation names its writer's latest checkpoint and the restore generation**, 48 bytes per record, so a server can't hide a checkpoint from one client while records still cross.
17. **Removing your own device is account-wide.** It leaves every workspace and loses its sessions through a signed revocation that no bare session can forge. Removing another account's device, or a member, stays per workspace.
18. **Quota counts live data, policy and keys.** A delete frees quota when it commits, and history yields to the quota instead of counting toward it.
19. **Restores are explicit, and repair carries only policy and keys.** `restore` and `mark-restored` raise one server-wide generation, and a heartbeat and the clients catch restores without them. Repair takes back only records that carry the server's commit tag, push takes no policy or keys for 7 days, and devices write lost content again from their files and restart their chains above every counter they signed. Every domain rotates when the quiet period ends.
20. **The collaboration sandbox judges a record by its bytes alone**, with one pinned guest module and fuel proportional to its size, while merging into a note's document runs under per-writer budgets that each device keeps for itself.
21. **Three IDs derive instead of being drawn**: an account's from its sign-up email under the server's secret, a device's from its signing key, and a workspace's from its genesis, so nobody can claim them after a restore.
22. **Chains may skip counters.** A device starts a new block of counters at each start, so neither a crash nor a rolled-back journal makes it sign a counter twice.
23. **The server publishes checkpoint due points**, because writers can't see blob weights inside payloads they can't read.

## Sign-off

| | |
|---|---|
| Signed off by | pending (Claude, the orchestrating agent, under the owner's delegation of 2026-10-20) |
| Date | pending |
| Commit signed off | pending |
