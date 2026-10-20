# Encrypted sync protocol, version 2

Status: draft, waiting for the owner's sign-off. Nobody writes v2 code before the [sign-off record](#sign-off) holds a name and a date.

This document defines Noura's encrypted sync protocol for the Rust product: the records a desktop client signs and encrypts, the keys that let devices and members read them, the server's checks, and the HTTP API that carries them. It replaces [sync-v1.md](sync-v1.md), [checkpoints.md](checkpoints.md) and [collaboration.md](collaboration.md), which describe the legacy app.

The server stores ciphertext and the routing fields listed in [§29](#29-what-the-server-stores). It never needs plaintext workspace content to store, order, route or compact your data.

## 1. Goals

- Sync a workspace of 100,000 objects across 200 devices, with no request or response body above 1 MiB.
- Keep key rotation proportional to devices and key domains. Removing a device never re-encrypts objects.
- Let a client detect a server that drops, reorders, replays or forks a device's operations.
- Hide object types, creation times and exact sizes from the server.
- Let a new device download current state without replaying full history.
- Keep both versions when two devices change the same object.

Out of scope: hiding when you sync, how much you sync, your IP address, your account email, or who belongs to a workspace.

## 2. Version 1 is unsupported

Version 2 shares no record, route or fixture with version 1. A v2 client refuses a server that doesn't advertise protocol 2 ([§26](#26-http-api)), and a v2 server serves no `/v1` route. Nothing migrates: v1 workspaces, accounts, devices, keys, operations and recovery kits stay with the legacy app, and the hosted v1 database is wiped at cutover. To move a workspace from the legacy app, turn on v2 sync from its folder as if it were new.

## 3. Primitives

| Purpose | Primitive | Rust crate (license checked before adoption) |
|---|---|---|
| Signatures | Ed25519 (RFC 8032), verified with `verify_strict` | `ed25519-dalek` |
| Key wrapping | Hybrid Public Key Encryption (HPKE, RFC 9180) base mode: DHKEM(X25519, HKDF-SHA256) `0x0020`, HKDF-SHA256 `0x0001`, ChaCha20Poly1305 `0x0003` | `hpke` |
| Record encryption | XChaCha20-Poly1305 with a random 24-byte nonce | `chacha20poly1305` |
| Attachment encryption | STREAM (Hoang, Reyhanitabar, Rogaway and Vizár) over XChaCha20-Poly1305, 32-bit big-endian counter and last-chunk flag | RustCrypto STREAM |
| Digests, message authentication codes, key derivation | BLAKE3: `hash`, `keyed_hash`, `derive_key` | `blake3` |
| Recovery kit passphrase | Argon2id (RFC 9106), m = 65,536 KiB, t = 3, p = 4 | `argon2` |
| Randomness | the operating system's cryptographically secure random number generator (CSPRNG) | `getrandom` |

XChaCha20-Poly1305 removes the nonce-collision limit that AES-GCM's 96-bit random nonces impose on long-lived keys such as presence keys. Every random value in this document comes from the operating system's CSPRNG.

Each epoch key ([§9](#9-key-domains)) serves several purposes, so clients derive one subkey per purpose with BLAKE3 `derive_key(context, epoch_key)`:

| Context string | Subkey for |
|---|---|
| `noura.sync.v2 operation key` | operation payloads |
| `noura.sync.v2 lineage key` | the lineage field of an epoch record |
| `noura.sync.v2 presence key` | presence payloads |
| `noura.sync.v2 profile key` | the workspace profile |
| `noura.sync.v2 share box key` | share-link boxes |

Two contexts derive from secrets outside a domain: `noura.sync.v2 share key` derives the snapshot key from a share link's key, and `noura.sync.v2 invitation key` derives the context key from an invitation secret.

## 4. Encoding

Every signed record, hashed tuple, request and response uses one binary encoding. The bytes a device signs are the bytes it sends, so a verifier never rebuilds a tuple from parsed fields.

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

When a record carries ciphertext, its associated data for authenticated encryption (AEAD) is the body prefix that ends before its `nonce` field. Each layout below marks that point.

### Decoding rules

Decoders reject, with a structured error and without panicking:

- trailing bytes after the last field;
- a length above the limit for that field ([§27](#27-limits)) before allocating;
- an unknown tag, a version other than 2, or an unknown enum value;
- a list that this document requires sorted and unique but isn't;
- an all-zero `id` where this document forbids it;
- invalid UTF-8 in a `str`.

Lists of IDs sort by unsigned byte order. Lists of pairs sort by their first field, then their second.

## 5. Identifiers

Remote IDs are 16 random bytes from the operating system's CSPRNG. They carry no timestamp, version bits, type or local object ID, and the all-zero value is reserved. In URLs and headers a remote ID is base64url without padding (RFC 4648 §5), 22 characters.

| ID | Created by | Notes |
|---|---|---|
| Account ID | server | |
| Device ID | the device, at key generation | |
| Workspace ID | the creating device | unrelated to the local `workspace_...` ID in `.noura/workspace.yaml` |
| Domain ID | the device that creates the domain | |
| Remote object ID (`rid`) | the device that first syncs an object | stable across moves inside one domain; a move to another domain gets a new `rid` ([§9](#moving-objects-between-domains)) |
| Blob ID | the uploading device | |
| Transaction ID | the staging device | reused on retry |
| Invitation ID, invitation token | the inviting device | the server stores the token's hash |
| Share token | the sharing device | the server stores the token's hash |
| Presence session ID | the device, per connection | |

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

A device holds an Ed25519 signing key and an X25519 HPKE key pair in the operating system's credential store. Its card binds them to its account.

`card`, unsigned:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.device.card)` | |
| account | `id` | |
| device | `id` | |
| signing_public | `key` | Ed25519 public key |
| hpke_public | `key` | X25519 public key |

The device fingerprint is `BLAKE3(card)`. Apps show it as 64 lowercase hexadecimal characters in 16 groups of 4, and the device list shows every device's full fingerprint.

### Enrollment proof

A signed-in device enrolls by proving it holds its signing key. The server issues a fresh 32-byte challenge, valid for 10 minutes and usable once.

`enroll`, signed with the card's signing key:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.device.enroll)` | |
| origin | `str` | the server's public origin in ASCII: lowercase scheme and host, port only when not the default, no trailing slash, such as `https://sync.noura.app` |
| card | `bytes` | the encoded card |
| challenge | `key` | 32 bytes from the server |

The server verifies the signature with the card's `signing_public`, then checks that `origin` is its own configured origin, the card's account is the session's account, the challenge is current and unused, and the device ID is new. `sync-key-envelope` exports this verifier.

An account's first enrolled device is trusted when it enrolls: it can create workspaces. Every later device needs approval from a device the account already trusts ([§10](#10-approval)).

### Desktop sign-in

The desktop signs in with the OAuth 2.0 device authorization grant (RFC 8628). The device code is 32 random bytes in base64url, the user code is 8 characters from `BCDFGHJKLMNPQRSTVWXZ` shown as two groups of 4, codes expire after 10 minutes, and the app polls every 5 seconds, adding 5 seconds after each `slow_down`. A successful grant returns a session token of 32 random bytes, which the server stores only as its BLAKE3 hash and the device stores only in its credential store.

Signing out removes the device from every workspace's policy ([§8](#8-workspace-policy)), ends its sessions and deletes its keys. The device can't rotate keys for its own removal, so another device performs that rotation. Before you sign out the last trusted device of an account, the app warns that you'll need a recovery kit to read synced data again.

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
| cuts | `list<cut>` | sorted by device, then domain |
| sig | `sig` | by `signer` |

`cut` is `id device | id domain | u64 counter | digest op_digest`: the last operation the server accepted from a device in a domain at the moment that device lost write access there. A device that never wrote to the domain gets counter 0 and a zero digest.

| Kind | Name | Body |
|---|---|---|
| 1 | genesis | `id owner_account`, `bytes owner_card`, `id workspace_domain` |
| 2 | add_device | `bytes card`, `digest offer` (32 zero bytes when nobody compared an approval code: adding a recovery identity, or a device that a recovery identity signs in) |
| 3 | remove_device | `id device` |
| 4 | add_member | `id account`, `u8 role`, `id invitation` |
| 5 | set_role | `id account`, `u8 role` |
| 6 | remove_member | `id account` |
| 7 | create_domain | `id domain`, `u8 domain_kind` (2 folder, 3 restricted), `list<access>` |
| 8 | set_access | `id domain`, `list<access>` (replaces the domain's list) |
| 9 | delete_domain | `id domain` |
| 10 | transfer_owner | `id account` |
| 11 | admit_device | `id domain`, `id device`: makes a device eligible for a restricted domain ([§9](#granting-and-wrapping)) |

`access` is `id account | u8 level`, sorted by account, with levels 1 read, 2 write and 3 manage. A folder domain's list is empty in v2. Roles are 1 owner, 2 admin, 3 editor and 4 viewer.

A change that removes write access from a device in a domain must carry a cut for that pair. The server builds the expected cuts from its own chain heads and rejects a change whose cuts differ (`policy_cuts_stale`), returning its heads so the signer can retry. Before signing, the signer compares each cut with the records it holds for that device and domain, and asks the server again when a cut sits below a record it accepted.

The server and clients reject a record beyond a device's cut unless its `policy_revision` is at or after a later change that gave that device write access again in that domain (server error `beyond_cut`). An honest server reports true heads, so a client that already accepted a record beyond a new cut has caught the server lying: it accepts the policy change, reports `sync_policy_cut_invalid` for that device and domain, and holds that domain's cursor.

Removing a member requires a `remove_device` change for each of the member's devices earlier in the same transaction.

`delete_domain` deletes the domain's log, epochs, envelopes and checkpoints on the server, and the blobs that only its operations listed. Devices that held the domain keep their local files. Those files lose their marker's protection, so the rule in [§9](#moving-objects-between-domains) holds them for review instead of uploading them to a wider domain. An owner or admin can delete a restricted domain they can't read.

### Roles and authority

| Action | Who may sign it |
|---|---|
| genesis | the creating device, which becomes the owner's first device |
| add_device for your own account | any device of your account in the workspace |
| add_device for another account | an owner or admin device, only while that account has no device in the workspace (its first device, or re-enrollment after an owner or admin removed every lost device); admins can't add devices to the owner or to other admins |
| remove_device | a device of the same account, or a device whose role outranks the target account's role |
| add_member | owner (admin, editor or viewer); admin (editor or viewer); requires an invitation the account accepted |
| set_role | owner for any non-owner; admin between editor and viewer |
| remove_member | owner for any non-owner; admin for editors and viewers; the member themself |
| create_domain | owner or admin; a restricted domain's list must name at least one manager |
| set_access (restricted) | a manager of that domain for any change; an owner or admin only to remove entries |
| admit_device | a manager of that restricted domain |
| delete_domain | owner or admin, or a manager of that restricted domain |
| transfer_owner | owner; the old owner becomes an admin |

Roles grant these rights:

| Right | Owner | Admin | Editor | Viewer |
|---|---|---|---|---|
| Read workspace and folder domains | yes | yes | yes | yes |
| Write workspace and folder domains | yes | yes | yes | no |
| Invite and remove editors and viewers | yes | yes | no | no |
| Approve devices of new members | yes | yes | no | no |
| Create folder and restricted domains | yes | yes | no | no |
| Create and revoke share links | yes | yes | no | no |
| Change the workspace profile | yes | yes | no | no |
| Add or remove admins, transfer ownership | yes | no | no | no |

A restricted domain's access list alone decides who reads and writes it, whatever their workspace role. Grantees must be members.

### Verifying the chain

A client accepts change `r` only when its revision is one above the accepted head, its `previous` equals the head's digest, its signature verifies under a device that the state before `r` lists for the signer's account, and that account holds the authority above. A device trusts revision 1 through its approval anchor ([§10](#10-approval)) or because it created the workspace.

The client stores the accepted head durably and reports:

- `sync_policy_forked` when the server returns a different change for a revision the client already accepted;
- `sync_policy_rollback` when the server reports a head below the accepted one;
- `sync_policy_unauthorized` when a change fails the authority rules.

A record that names a policy revision above the client's accepted head (an operation, epoch record, checkpoint or purge) waits until the client has fetched and accepted that revision. If the server can't supply it, the client reports `sync_policy_rollback`.

Policy chains stay short (one change per membership or device event), so a new device replays the whole chain and v2 defines no policy snapshot.

## 9. Key domains

A key domain is a set of objects that share one symmetric key at a time. Each workspace has exactly one workspace domain and any number of folder and restricted domains.

| Domain | Who reads it | Objects |
|---|---|---|
| Workspace | every member | every object outside a folder or restricted domain, and the workspace profile |
| Folder | every member, inherited from the workspace | the objects under one folder |
| Restricted | only the members its access list names | the objects under one folder |

A folder domain inherits its audience. A restricted domain's audience comes only from its grants: the owner and admins don't read it unless a manager grants them access. Folder domains carry no extra permission in v2. They split a large workspace into units that rotate, checkpoint and bootstrap on their own, and they give a later version a place to add per-folder sync or guests.

### Domain roots

A folder or restricted domain covers the folder that holds its marker file, `.noura-domain`, and everything beneath it. The marker is an ordinary synced file inside the domain it names, so a member outside a restricted domain never receives it. Its content is one line, `domain: <base64url domain ID>`, followed by a line feed. Because the marker moves with its folder, moving the folder keeps the domain and no path serves as identity. Domain roots don't nest: a client refuses to create a marker inside another domain's root.

A marker is valid when its bytes are exactly `domain: `, the 22-character base64url domain ID and one line feed, the ID names a folder or restricted domain in the verified policy, and no ancestor folder holds another valid marker. An object's domain is the domain of the nearest ancestor folder holding a valid marker, or the workspace domain. The workspace format spec and its conformance fixtures gain this file in the same change that implements domain roots.

### Moving objects between domains

A move across a domain boundary writes a `delete` operation (reason 2, moved) in the source domain and a new state operation with a new `rid` in the target domain, whose payload names the source domain and `rid` in its `origin` field. Both commit atomically ([§15](#push)). The new `rid` keeps the server from linking the object across domains, and members of both domains still see one object because the local object ID travels inside both payloads.

A client never moves an object from a restricted domain to a wider audience on its own. When a file leaves a restricted root, or its marker disappears, the client holds those files for review and shows how many would become visible to more people. Moving into a restricted domain needs no confirmation.

### Epochs

Each domain's key changes in epochs, numbered from 1. An epoch key is 32 random bytes. An epoch record announces each epoch.

`epoch`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.epoch)` | |
| workspace | `id` | |
| domain | `id` | |
| epoch | `u64` | 1 for a new domain, then +1 |
| previous | `digest` | digest of the previous epoch record; 32 zero bytes at epoch 1 |
| policy_revision | `u64` | the policy head the rotating device saw; recipients are computed at this revision |
| rotating_device | `id` | |
| recipients | `list<id>` | sorted device IDs that receive this epoch's key |
| key_check | `digest` | `BLAKE3.keyed_hash(epoch_key, tag(noura.sync.key-check) \|\| workspace \|\| domain \|\| u64 epoch)` |
| *AEAD associated data ends here* | | |
| lineage_nonce | `nonce` | 24 zero bytes at epoch 1 |
| lineage | `bytes` | previous epoch key, XChaCha20-Poly1305 under the lineage subkey of this epoch's key; empty at epoch 1 |
| sig | `sig` | by `rotating_device` |

The lineage field links each epoch to the one before, so a device that holds the current key derives every earlier key by walking backward. A newly approved device therefore needs one envelope per domain, and objects written under old epochs stay readable without re-encryption.

`key_check` lets every recipient confirm it received the same key as the others. A rotating device can't hand different recipients different keys without failing that check.

A client accepts an epoch record when its signature verifies under the rotating device's pinned card, the rotating device could read the domain at `policy_revision`, every recipient could read it at that revision, the epoch is one above the previous accepted epoch, `previous` equals that record's digest, and the lineage decrypts to a key matching the previous record's `key_check`. Every key derived through lineage is checked against its own record's `key_check`. A failure reports `sync_key_mismatch`.

Epoch records form a hash chain per domain, and each checkpoint names the current epoch record ([§17](#17-manifest-and-checkpoints)). A rotating device that colludes with the server to show two recipients two different records for one epoch is caught when either recipient sees the other record, a checkpoint naming it, or an operation that fails to decrypt under its key (`sync_key_mismatch`).

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
| recipient_hpke_public | `key` | from the recipient's card |
| wrapping_device | `id` | |
| *HPKE `info` ends here* | | |
| enc | `key` | HPKE encapsulated key |
| ciphertext | `bytes` | 48 bytes: the epoch key and the 16-byte tag |
| sig | `sig` | by `wrapping_device` |

HPKE runs in base mode with the suite in [§3](#3-primitives). Its `info` is the envelope prefix from the tag through `wrapping_device`, its `aad` is empty, and its plaintext is the 32-byte epoch key. A recipient accepts an envelope only when the signature verifies under a card in its verified policy and the unwrapped key matches the epoch record's `key_check`. A key supplied by the server, or an envelope signed by an unpinned device, fails with `sync_key_mismatch`.

### Granting and wrapping

Granting decides who may read a domain; wrapping hands an authorized device the key.

- **Granting** happens only through policy changes, signed by the devices the authority table in [§8](#roles-and-authority) allows: membership changes for workspace and folder domains, and a manager's access-list changes for a restricted domain.
- **Wrapping** is mechanical. Any device that holds an epoch key may wrap it for a device that the current policy authorizes to read that domain. The server rejects an envelope for any other recipient.

A restricted domain adds one more condition, computed from the policy chain alone. A device of grantee account A is eligible for restricted domain R when one of these holds:

- it belonged to A when the change that made A a grantee of R committed;
- a device of A that was eligible for R at the time signed its `add_device`;
- a manager of R signed `admit_device` for it.

Only eligible devices receive R's key, as rotation recipients or through envelopes, and the server rejects any other recipient with `recipients_invalid`. A device that another account added (an admin re-enrolling a member) therefore gets no key for R, and nor do devices it adds, until a manager admits it. An `admit_device` change carries a fresh checkpoint of R, and the account's history floor in R becomes that checkpoint's `covered_seq`.

When an approver can't wrap a domain (an admin approving a member's new device outside a restricted domain the member belongs to), the new device's envelope stays missing. The server lists missing envelopes for eligible devices to every device that holds the key ([§26](#26-http-api)), and the first one to see the gap fills it.

### Rotation

A domain rotates when a device that could read it loses access:

| Domain | Rotates when |
|---|---|
| Workspace | a device is removed, or a member leaves or is removed |
| Folder | the same events as the workspace domain |
| Restricted | a grantee loses its grant, a grantee's device is removed, or a grantee leaves the workspace |

Exporting a recovery kit adds a device; revoking the kit removes it and rotates.

Rotating domain D at policy revision r means:

1. Generate a new epoch key.
2. Write an epoch record with `epoch` = current + 1, `policy_revision` ≥ r, `recipients` = every active device authorized to read D at that revision (for a restricted domain, every eligible device), and the lineage of the previous key.
3. Wrap the new key for every recipient.
4. Commit the policy change (if this device signs it), the epoch records and the envelopes in one transaction ([§20](#20-chunked-staging)).

The device that removes another device includes rotations for every domain it can read. A domain it can't read (a restricted domain it doesn't belong to) stays pending until one of that domain's grantee devices rotates it. Concurrent rotations race; the server accepts the first epoch record for each epoch and rejects the rest with `epoch_conflict`.

From revision r until the rotation commits, the server stops serving D's operations and keys to the removed devices and rejects writes to D with `rotation_pending`. A client that holds D's key rotates it before its next write to D. No device that has seen revision r encrypts under an epoch whose recipients include a device that no longer has access.

An operation is epoch-valid when its `epoch` is at least the first epoch of D whose record has `policy_revision` at or above the latest revision, at or below the operation's `policy_revision`, that required D to rotate. After the rotation commits, the server still accepts an operation under the previous epoch when its `policy_revision` is below r: its writer signed it before learning of the removal, and rejecting it would force a re-seal ([§14](#your-own-chain)).

**Cost.** A rotation writes one epoch record and one envelope per remaining authorized device, for each domain the removed device could read. Its size depends on devices and domains and never on objects. [§28](#28-worked-example-100000-objects) works through the numbers.

**What rotation protects.** A removed device keeps the keys and plaintext it already had. It can't decrypt anything encrypted under a later epoch.

### History floor

A domain key opens every earlier ciphertext in that domain through lineage, so the server decides how much history a new member sees. The server keeps a history floor per account and domain: it never serves operations with a sequence at or below the floor through pulls or chain fetches. Bootstrap ([§18](#18-bootstrap)) still serves the objects that are current, because a member sees current content.

| Event | Floor |
|---|---|
| A member joins | `covered_seq` of the checkpoint committed with the approval |
| A grantee joins a restricted domain | `covered_seq` of the checkpoint committed with the grant |
| A member's new device | the member's existing floor |
| An admin re-enrolls a member who lost every device | workspace and folder domains: `covered_seq` of the checkpoint committed with the approval; restricted domains: `covered_seq` of the checkpoint committed with `admit_device` |
| A device restored from the member's recovery kit | the member's existing floor |

An approval or grant that sets a floor carries a fresh checkpoint for each affected domain, signed by a device that can write it, and the floor equals that checkpoint's `covered_seq`. Records that land between signing and commit sit above the floor, so the new device pulls them normally. The server answers a pull or chain fetch below the floor with `before_history_floor`.

History isolation for new members depends on the server. [§30](#30-threats) records that limit.

## 10. Approval

Approval pins a new device to devices you already trust, and the reverse, with one comparison code shown on both screens.

### Approval offer

The approving device fetches the candidate's card from the server and sends a signed offer.

`offer`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.approval.offer)` | |
| approver_card | `bytes` | encoded card of the approving device |
| candidate_card_digest | `digest` | `BLAKE3(candidate card)` |
| nonce | `key` | 32 random bytes chosen by the approver after it received the candidate card |
| anchors | `list<anchor>` | sorted by workspace |
| sig | `sig` | by the approving device |

`anchor` is `id workspace | u64 revision | digest policy_head`: the approver's accepted policy head in each workspace the approval covers. Approving your own new device lists every workspace your account belongs to. Approving an invited member's device lists one workspace.

### Approval code

```text
offer_digest  = BLAKE3(offer body || sig)
code_input    = tag(noura.sync.approval.code) || offer_digest
approval_code = first 20 bytes of BLAKE3(code_input)
```

Both devices show the approval code as 40 lowercase hexadecimal characters in 10 groups of 4.

1. The candidate enrolls and waits.
2. The approving device shows the candidate's fingerprint, then sends the offer. Both devices show the approval code.
3. You compare the codes and confirm on both devices.
4. The approving device commits the approval: `add_device` changes (with `offer` set to `offer_digest`) in each anchored workspace, the envelopes the candidate needs, and, for a new member, `add_member` and a fresh checkpoint per domain it can read so the member's floor has a checkpoint to bootstrap from.
5. The candidate pins the approver's card and anchors. It accepts the approval only from that card, and verifies each workspace's chain from genesis to its anchor, then onward.

Before showing the code, the candidate checks that `candidate_card_digest` equals `BLAKE3` of its own card; otherwise it reports `sync_approval_mismatch` and shows nothing. After you confirm, the candidate verifies each workspace's chain up to its anchor and checks that the approver's card belongs to the policy at that anchor with the authority to sign the `add_device` that follows. For an invitation, the approver must be the inviting device: `BLAKE3(approver_card)` must equal the context's `inviter_fingerprint` ([§11](#11-invitations)).

A server that swaps either card produces different codes on the two screens. Passing the approver's offer for a substitute card through to the real candidate fails the digest check. To make the codes match otherwise, the server would have to find a second offer that hashes to the same 160-bit code after the approver chose its nonce, which takes about 2^160 work.

## 11. Invitations

An owner or admin invites someone by link. The link carries an invitation secret in its fragment, so the server never sees the workspace name or the inviter's details.

```text
{origin}/invite/{base64url(token)}#{base64url(secret)}
```

The inviting device generates the invitation ID (16 bytes), the token (16 bytes) and the secret (32 bytes). The server stores `BLAKE3(tag(noura.sync.invite.token) || token)`, the workspace, the role, the inviting account, the accepting account once known, the status, an expiry seven days out, and the context below. A workspace holds at most 100 active invitations.

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

The invitation page decrypts the context in `account-web` and shows the workspace name, the inviter and the role. A wrong secret or tampered context shows a generic page. You sign in or sign up (the server's email allowlist still applies) and accept. Acceptance binds the invitation to your account and grants nothing. The inviting device then approves your device with an approval code ([§10](#10-approval)), and only that commit adds you to the policy and wraps keys for you. To join, you open the same link in the desktop app (or paste it there). The app keeps the secret in memory, fetches the context by token, and checks that the approval offer comes from the card matching `inviter_fingerprint` and that the `add_member` change names this invitation. Only the inviting device can approve; if that device is lost, an owner or admin revokes the invitation and sends a new one. Revoking an invitation makes its link fail.

## 12. Recovery kit

A recovery kit is an offline device. Exporting a kit creates a recovery identity (a device card with its own signing and HPKE keys) and adds it to every workspace your account belongs to with `add_device`. When your account later creates or joins a workspace, your device adds the recovery identity there in the same transaction, and the app asks you to export a new kit so the kit lists that workspace. Recovery treats a workspace the kit doesn't list as first contact: the recovering device shows its name and owner fingerprint and joins only after you confirm. Rotations wrap keys for it like any other device, so the kit never goes stale. Removing the recovery identity from your device list revokes the kit.

The kit is a file you pick outside every workspace, written with mode 0600 on Unix:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.recovery-kit)` | |
| salt | `id` | 16 random bytes |
| m_cost_kib | `u32` | 65,536 |
| t_cost | `u32` | 3 |
| parallelism | `u32` | 4 |
| *AEAD associated data ends here* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | XChaCha20-Poly1305 under `Argon2id(passphrase, salt, m, t, p)`, Argon2 version `0x13`, 32-byte output |

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

`kit_workspace` is `id workspace | digest genesis | u64 revision | digest head`, sorted by workspace: the digest of the workspace's revision-1 policy change and the accepted policy head when the kit was written. The passphrase is at least 12 characters, normalized to Unicode NFC and encoded as UTF-8. Readers reject parameters below the values above, and a wrong passphrase or tampered file reports `sync_recovery_kit_invalid`.

To recover after losing every device, you sign in on a new computer, enroll it, and import the kit. The new device verifies the kit's card against its seeds, checks that each workspace's chain starts at the stored genesis digest and extends the stored head (a shorter chain reports `sync_policy_rollback`), and signs `add_device` for itself as the recovery identity. It unwraps the recovery identity's envelopes and wraps every key for itself. The kit restores keys, never the old device's signing identity or session. The app then asks you to remove the lost devices, which rotates every domain, and to export a new kit.

## 13. Operations

An operation records one change to one object.

### Operation record

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.op)` | |
| workspace | `id` | |
| domain | `id` | |
| device | `id` | the signer |
| counter | `u64` | position in this device's chain for this domain, from 1 |
| previous | `digest` | digest of the device's previous record in this domain; 32 zero bytes at counter 1 |
| epoch | `u64` | the epoch whose key encrypts the payload |
| policy_revision | `u64` | the policy head the device saw when it signed |
| previous_attempt | `digest` | digest of a record the server rejected at this chain position, when this record re-seals the same change ([§14](#your-own-chain)); otherwise 32 zero bytes |
| rid | `id` | |
| supersedes | `list<cover>` | sorted by device, one entry per device, at most 256 |
| blobs | `list<id>` | blob IDs the payload references, sorted, at most 4 |
| *AEAD associated data ends here* | | |
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
| 2 | delete | state | `str object_id`, `digest revision`, `u8 reason` (1 deleted, 2 moved to another domain) |
| 3 | text_snapshot | state | `str object_id`, `bytes path`, `digest revision`, `bytes properties`, `content`, `opt<origin>` |
| 4 | text_update | patch | `str object_id`, `digest base_snapshot`, `bytes yrs_update` |
| 5 | properties | patch | `str object_id`, `digest base_snapshot`, `list<property_change>` |
| 6 | move | patch | `str object_id`, `digest base_snapshot`, `bytes expected_path`, `bytes path` |

- `object_id` is the managed file's frontmatter ID, or empty for an ordinary file.
- `path` is a workspace-relative UTF-8 path of at most 4,096 bytes, with `/` between components. Clients reject absolute paths, `.` and `..` components, empty components, control characters, Windows reserved names and characters, trailing dots or spaces, and the reserved roots `.noura` and `.git` in any ASCII case. Paths compare byte for byte.
- `revision` is the BLAKE3 hash of the file's bytes as materialized; for a delete, of the deleted bytes, or zeros when unknown.
- `content` is `u8 form` then either form 1, `bytes inline`, or form 2, `blob_ref`.
- `blob_ref` is `id blob | u64 ciphertext_length | digest ciphertext_digest | key blob_key | u64 plaintext_length`.
- `origin` is `id domain | id rid`: where a cross-domain move came from.
- `property_change` is `str key | bytes expected | bytes value`, where each value is a canonical YAML scalar or collection and an empty value means absent.
- `base_snapshot` is the digest of the text snapshot a patch builds on.

An inline payload holds at most 262,144 bytes before padding. Larger content goes in a blob.

### Supersession and live operations

The server and the client compute the same live set for each `rid` from cleartext fields: an operation is live unless another operation on the same `rid` covers it. The server needs no plaintext to compute it, compact the log, or check a manifest.

- A state operation covers every operation live in the writer's view of that `rid`, naming each device's highest live counter. A delete covers everything, so the tombstone stays live alone. Covers name only live operations, which the server never prunes ([§19](#19-compaction-and-purge)).
- A patch covers nothing. It applies on top of the snapshot named by `base_snapshot`.

### Server checks on an operation

The server accepts an operation only when:

- the session's device is the signer, is active, and the signature verifies under its card;
- the device may write the domain at the current policy revision and at `policy_revision`, which is at most the current revision, and the record isn't beyond a cut (else `beyond_cut`);
- `epoch` is the domain's current epoch and no rotation is pending, or the previous epoch under the condition in [§9](#rotation);
- `counter` is one above the server's head for this device and domain and `previous` equals that head's digest (else `chain_conflict` with the head);
- `rid` is new, or already belongs to this domain;
- every cover names an existing operation on this `rid` with exactly that device and counter (else `supersedes_unknown`);
- every blob is a completed upload in this workspace (else `blob_incomplete`);
- the ciphertext length is a valid padded length (else `padding_invalid`).

A retried upload of an operation the server already holds, with the same digest, returns the original receipt. The same position with a different digest returns `chain_conflict`.

### Client checks on an operation

Before decrypting, the client verifies the signature under a pinned card from its verified policy, checks that the signer could write the domain at `policy_revision`, rejects an operation beyond the signer's cut, checks that the operation is epoch-valid ([§9](#rotation)), and runs the chain verifier ([§14](#14-device-chains)). It decrypts with the epoch key, unpads, parses, and checks that the payload's `rid` matches the header. A failure reports `sync_writer_not_authorized`, `sync_epoch_stale`, `sync_padding_invalid` or `sync_payload_invalid` and changes no file.

## 14. Device chains

Each device keeps one chain per domain it writes to: operations and purges ([§19](#19-compaction-and-purge)) share it. Every record carries the device's counter and the digest of its previous record under the signature, so a server can't remove, reorder or duplicate a record without breaking a link that the device signed.

Chains are per domain because a member outside a restricted domain never sees that domain's records. A single chain per device would show that member gaps it could never explain.

### Verifier

The client keeps, per domain and device, the accepted head `(counter, digest, sequence)` and the counter, digest and sequence of each accepted record above the domain's retention floor. It processes each pull page in sequence order. For a record with counter `c`, previous digest `p` and digest `h` at sequence `s`:

1. **Counter at or below the head.**
   - The client holds the record at `c` with digest `h` at sequence `s`: the page repeats a position the client already applied (for example after a restart), so the client ignores it.
   - The client holds the record at `c` with digest `h` at another sequence: `sync_chain_replayed`.
   - The client holds a different record at `c`, and the two are attempts at one position: one's `previous_attempt` names the other, or both name the same first attempt ([§14](#your-own-chain)). If a record at `c + 1` links to the held record, or `c` is below the head, the client ignores the newcomer. If `c` is the head, the client keeps whichever attempt a successor later links to, recomputing the object's live set when it switches. Either way it reports nothing.
   - The client holds a different record at `c` that isn't an attempt of the same position: `sync_chain_forked`.
   - The client no longer holds a record at `c` (it bootstrapped past it, or the record sits below the retention floor): every legitimate record at or below the head has a sequence at or below the head's, so `s` above the head's sequence reports `sync_chain_replayed`.
2. **Counter one above the head.** If `p` equals the head's digest, accept and advance the head. Otherwise the client fetches position `c - 1`: if the server returns a re-seal of the head that rule 1 accepts, the client replaces the head with it and accepts this record when `p` matches the re-seal. Otherwise: `sync_chain_forked`.
3. **Counter more than one above the head.** The page skipped records. The client fetches each missing position by chain position ([§26](#26-http-api)). A missing record that the server returns at a sequence above `s` reports `sync_chain_reordered`. A missing record at a sequence below `s`, or one the server can't return, reports `sync_chain_gap`.

On any of these errors the client applies nothing from the page, changes no file, keeps its cursor and reports the error with the device and domain. It retries on the next pull, so a transient server fault clears itself.

A retried upload never triggers an error: the server stores one copy, and a page that repeats a position at the same sequence is ignored.

### Suffix withholding

A chain proves what the server served, never that it served everything. A client learns a device's latest counter from three sources: later records from that device, the chain heads in a signed checkpoint ([§17](#17-manifest-and-checkpoints)), and, for its own chain, the receipts the server returned. A head above what the client holds, at a sequence the server claims to have delivered, reports `sync_chain_gap`. A server that withholds a device's records and every checkpoint mentioning them looks like a device that went offline; the app shows when it last heard from each device.

### Your own chain

A device writes each counter to its journal before signing and persists each signed record before sending it, so a crash never reuses a counter and a retry sends the same bytes. On startup, and on `chain_conflict`, it compares its journal with the server's head for its own chain:

- A server head above the journal means the journal was restored from a backup. The device verifies those records carry its own signature and continues from the server's head.
- A server head below a record the server acknowledged means the server lost acknowledged records. The device reports `sync_chain_gap` for its own chain and uploads its persisted records again, unchanged.

When the server rejects a record the device must change before it can succeed (`epoch_stale`, `rotation_pending`, `supersedes_unknown`, `blob_incomplete`, `beyond_cut`, `purge_invalid`), the device re-seals the change at the same counter with `previous_attempt` set to the digest of the first rejected attempt at that position; later attempts name the same first attempt. A rejected push names the failing record ([§26](#message-bodies)), and the device re-seals that record and every later record of the same chain in the batch, whose `previous` links change. A server that kept a rejected attempt and serves it shows readers two attempts at one position, which rule 1 above treats as a retry, so the verifier needs no decryption to tell them apart. A device whose write access is gone drops the record; its local file keeps the change.

## 15. Domain log, cursors and pulls

Each domain has its own log. The server assigns each accepted operation and purge a sequence number, increasing within the domain and not necessarily dense. Per-domain sequences keep a member outside a restricted domain from counting its activity through gaps.

### Push

`POST /v2/workspaces/{w}/ops` carries `list<record_ref>` ([§20](#20-chunked-staging)) holding operations and purges, at most 1 MiB, possibly spanning domains. The server commits the batch atomically or rejects all of it, and returns `list<receipt>`, where a receipt is `digest record_digest | id domain | u64 seq`, in request order. A cross-domain move fits one push; a larger atomic change uses a staged transaction ([§20](#20-chunked-staging)).

### Pull

`GET /v2/workspaces/{w}/domains/{d}/ops?after={seq}` returns a page:

| Field | Type | Notes |
|---|---|---|
| domain | `id` | |
| after | `u64` | the requested cursor |
| complete_through | `u64` | the page holds every record with a sequence in (after, complete_through] |
| head | `u64` | the domain's latest sequence |
| retention_floor | `u64` | [§19](#19-compaction-and-purge) |
| purge_horizon | `u64` | [§19](#19-compaction-and-purge) |
| checkpoint_due | `u8` | 1 when the server wants a writer to sign a checkpoint |
| entries | `list<entry>` | sorted by sequence |
| checkpoint | `opt<checkpoint_bundle>` | the latest checkpoint, when newer than `after` |

`entry` is `u64 seq | u8 record_type | bytes record`, with record types 1 operation and 7 purge. A page holds at most 1 MiB. A cursor below the retention floor returns `cursor_compacted`, and the client bootstraps ([§18](#18-bootstrap)).

The client advances a domain's cursor to `complete_through` only after every record in the page passes verification and every resulting file change reaches disk. Policy changes and keys have their own cursors ([§26](#26-http-api)). A pull may wait up to 25 seconds for new records, and the realtime channel ([§23](#23-presence-and-realtime-channel)) announces new heads; both only prompt a pull.

### Client journal

The client keeps its sync state in ordinary files under `.noura/sync/`, never only in SQLite, and never uploads them:

- the accepted policy chain and its head;
- epoch records and the envelopes addressed to this device, still wrapped;
- per domain: the cursor, the latest accepted checkpoint, the chain heads and recent records the verifier needs ([§14](#verifier)), and the cuts;
- this device's counters and every signed record not yet acknowledged, persisted before sending;
- the map from each `rid` to its local object, path and live operations;
- the review store for conflicts, orphans and held files.

Device signing and HPKE secrets live only in the operating system's credential store. Epoch keys exist only in memory: the client unwraps them from the stored envelopes with the device's HPKE secret. If the journal is lost, the client bootstraps again ([§18](#18-bootstrap)), matches local files to objects by path and content revision, and takes its own counters from the server's heads.

## 16. Conflicts

The client derives an object's state from its live set:

1. One live state operation: that state, plus the live patches whose `base_snapshot` is that operation, applied in sequence order.
2. Two or more live state operations: a conflict. Each one is a branch.
3. Patches whose base snapshot isn't live: orphans, kept for review.

Sync keeps both versions:

- The device whose local file matches a branch, or has local edits, keeps its file untouched.
- Every other branch goes to the local review store under `.noura/sync/conflicts/`, which never uploads.
- A device that holds neither materializes the branch with the lowest operation digest and stores the rest for review.
- Status counts the conflict until you choose.

Choosing writes a state operation that covers every branch.

Clients resolve these cases without asking:

- **Identical branches.** Branches whose decrypted path and revision match aren't a conflict.
- **Disjoint changes.** When one branch changed only the path and another only the content, relative to the common base operation the covers name, the client materializes the merged state. Any writer device that holds the base writes a merge operation covering both. Two devices writing the same merge produce identical branches, which the previous rule collapses.
- **Edit against delete.** The edit stays on disk and the conflict asks whether to delete it.

Two objects with different `rid`s can claim the same path, or paths that differ only in case or Unicode normalization. The object whose current state operation has the higher digest yields: its author's device moves it to `name (2).ext` with a normal operation, and other devices show it at that name without writing until the move arrives.

A property patch whose `expected` value doesn't match the current value is kept for review with the competing value, as text conflicts are.

## 17. Manifest and checkpoints

A domain's manifest is a Merkle tree over its live operations. Its root proves that a client holds every current object, and a new device bootstraps from it.

### Leaves and tree

Each `rid` with at least one live operation has a leaf:

```text
leaf_hash = BLAKE3(tag(noura.sync.manifest.leaf) || rid || list<ref>)
ref       = id device || u64 counter || digest op_digest    (sorted by device, then counter)
```

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
| covered_seq | `u64` | the manifest covers every record through this sequence |
| policy_revision | `u64` | |
| purge_horizon | `u64` | the horizon in force at `covered_seq` |
| live_objects | `u64` | number of leaves |
| manifest_root | `digest` | |
| heads_digest | `digest` | `BLAKE3(tag(noura.sync.manifest.heads) \|\| list<ref>)`: every device's chain head in this domain at `covered_seq`, sorted by device |
| epoch | `u64` | the domain's latest epoch the signer had accepted at `covered_seq` |
| epoch_record | `digest` | digest of that epoch record |
| signer | `id` | |
| sig | `sig` | |

A `checkpoint_bundle` is `bytes checkpoint | list<ref> heads`. A client accepts a checkpoint only when its signer can write the domain in the client's current verified policy and has no cut there after its latest grant, so a removed device can't sign checkpoints. The client checks `epoch_record` against its own accepted record for `epoch`, during bootstrap too. The server recomputes the root from its own log at `covered_seq` and rejects a mismatch with `manifest_mismatch`, so an honest server and an honest writer always agree. A writer signs a checkpoint when the server sets `checkpoint_due`: after 10,000 records since the last counted checkpoint, during a daily pass over every domain with records after its last counted checkpoint, and when the latest checkpoint's signer loses write access. A checkpoint the server accepts while `checkpoint_due` is set clears it and counts toward retention ([§19](#19-compaction-and-purge)). Checkpoints committed with an approval, grant or `admit_device` don't count, and the server rejects any other checkpoint with `checkpoint_not_due`.

A client that has applied through `covered_seq` compares the root and `epoch_record` with its own. A mismatch reports `sync_manifest_mismatch` or `sync_key_mismatch`, and the client walks the tree to find which objects differ.

## 18. Bootstrap

A new device, or one whose cursor fell below the retention floor, loads current state from the latest checkpoint instead of history:

1. Fetch and verify the policy chain from the approval anchor, and the epoch records and envelopes for each readable domain. Derive older epoch keys through lineage as live operations need them.
2. For each domain, fetch the latest checkpoint bundle and check its signer against the current verified policy ([§17](#checkpoint)) and its `covered_seq` against the device's history floor.
3. Fetch the operations live at `covered_seq` in pages ordered by `rid`. Verify each signature, compute each leaf, and check the root and `live_objects` against the checkpoint. A mismatch reports `sync_manifest_mismatch` and installs nothing.
4. Decrypt, materialize the files and record the chain heads from the bundle.
5. Pull from `covered_seq` with the chain verifier starting at those heads.

A returning device compares its journal with the manifest instead of replacing files. It never deletes a local file because the manifest lacks its object: it lists those files once for review ("deleted on another device while this one was away") and deletes them only when you confirm. A changed local copy uploads under a new `rid` unless you choose to delete it.

## 19. Compaction and purge

The server deletes operations no client needs.

Retention counts checkpoints instead of days, so the server needs no table that maps sequences to dates.

**Retention floor.** For each domain, the retention floor is the lower of: the lowest cursor acknowledged by any active device (a device acknowledges a cursor by pulling after it), and the `covered_seq` of the 30th most recent counted checkpoint, or 0 while fewer than 30 exist. A device stays active while it has pulled the domain since the 90th most recent counted checkpoint; while fewer than 90 exist, every device is active. The server records, per device and domain, its acknowledged cursor and the index of the latest counted checkpoint at its last pull. A domain without any checkpoint keeps every record, and a new device bootstraps it by pulling from sequence 0 with empty chain heads.

**Pruning.** The server may delete an operation whose sequence is at or below the retention floor when another operation whose sequence is also at or below the floor covers it. Every active device has seen both, so no writer can still name the deleted operation in a cover. The server deletes all such operations in one transaction, so a covering operation never disappears before the operations it covers, and the live set at every retained checkpoint stays complete.

**Purge.** A delete operation stays live as a tombstone until a writer purges it. The server publishes a `purge_horizon`: the lower of the retention floor and the `covered_seq` of the 90th most recent counted checkpoint, or 0 while fewer exist. An active device that hasn't pulled a tombstone yet holds the floor below it, so a purge never removes a deletion a device still needs. A writer device lists objects whose only live operation is a delete at or below the horizon and signs a purge:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.purge)` | |
| workspace | `id` | |
| domain | `id` | |
| device | `id` | |
| counter | `u64` | in the device's chain for this domain |
| previous | `digest` | |
| policy_revision | `u64` | |
| previous_attempt | `digest` | as in an operation |
| horizon | `u64` | at most the server's `purge_horizon` |
| rids | `list<id>` | sorted, at most 10,000 |
| sig | `sig` | |

The server checks the signer and chain as for an operation and that every listed `rid` belongs to the domain and has all its live operations at or below the horizon (else `purge_invalid`), then deletes every operation on those `rid`s. A client that holds a listed object with a live state other than a tombstone reports `sync_purge_live_object` and uploads its copy under a new `rid`. The server learns that those objects were deleted at some point before the horizon, which is less than a per-delete flag would reveal.

The server deletes a purge record once its sequence falls below the retention floor; readers above the floor have seen it, and readers below it bootstrap.

## 20. Chunked staging

A change too large for one request commits atomically through a staged transaction: key rotations in large workspaces, approvals into many domains, and moving many objects into a restricted domain.

1. `POST /v2/workspaces/{w}/txns` opens transaction `t` (a random ID) with its part count.
2. `PUT /v2/workspaces/{w}/txns/{t}/parts/{i}` uploads part `i`, at most 1 MiB. A part is `list<record_ref>`, where `record_ref` is `u8 record_type | bytes record` with types 1 operation, 2 policy change, 3 epoch record, 4 envelope, 5 checkpoint bundle, 6 profile and 7 purge. Re-uploading a part with the same bytes succeeds; different bytes fail.
3. `POST /v2/workspaces/{w}/txns/{t}/commit` carries the signed descriptor.

`txn`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.txn)` | |
| workspace | `id` | |
| txn | `id` | |
| device | `id` | |
| part_count | `u32` | at most 1,024 |
| parts_root | `digest` | `BLAKE3(tag(noura.sync.txn.parts) \|\| list<digest>)` over each part's `BLAKE3(part bytes)`, in part order |
| sig | `sig` | |

At commit the server checks the signature, that every part is present and matches `parts_root`, then validates every record in order against the current state plus the records before it, and commits all of them in one database transaction. Any failure rolls everything back and returns the first failing record's index and error. Committing a committed transaction returns the same result. Staged parts expire after 24 hours unused and count against the workspace quota until then. A transaction holds at most 250,000 records.

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

Every chunk holds 65,536 plaintext bytes except the last, which holds 1 to 65,536 and sets `last` to 1. The ciphertext length is `header length + P + 16 * ceil(P / 65,536)`, which lets the server check padding from the length alone. A blob's file content `L` is at most 2^31 bytes (2 GiB).

The receiver checks the complete ciphertext against `ciphertext_digest` (BLAKE3) before decrypting, writes the plaintext to a temporary file, checks `plaintext_length` and the file `revision`, and only then replaces the destination.

### Upload over a tus subset

Uploads use tus 1.0.0 core with the `creation`, `termination` and `expiration` extensions. The server advertises `Tus-Version: 1.0.0`, `Tus-Extension: creation,termination,expiration` and `Tus-Max-Size`.

| Request | Purpose |
|---|---|
| `POST /v2/workspaces/{w}/blobs` with `Upload-Length` and `Upload-Metadata: blob <base64>,digest <base64>,domain <base64>` (standard base64, as tus requires) | create; the device must be able to write that domain |
| `HEAD /v2/workspaces/{w}/blobs/{id}` | read `Upload-Offset` to resume |
| `PATCH /v2/workspaces/{w}/blobs/{id}` with `Upload-Offset` and `Content-Type: application/offset+octet-stream` | append at most 1 MiB |
| `DELETE /v2/workspaces/{w}/blobs/{id}` | abandon an incomplete upload |

The server flushes each part to durable storage before it reports the new offset. When the offset reaches `Upload-Length`, it checks the BLAKE3 digest, and on a mismatch discards the upload and answers `blob_digest_mismatch`. Incomplete uploads expire after 24 hours (`Upload-Expires`). A completed blob carries no timestamp.

`GET /v2/workspaces/{w}/blobs/{id}` with `Range` returns at most 1 MiB per response. The server serves a blob to a device whose account can read a domain holding a retained operation that lists the blob, and to its uploader before that operation exists. It deletes a blob once no retained operation lists it and a grace period longer than its backup retention has passed.

## 22. Live collaboration

A Markdown note enters text mode when a device writes a `text_snapshot` for it. Text mode keeps the note's body in a yrs document whose root text is named `content` (UTF-16 offsets, update format v1) and its frontmatter block in `properties`, the exact frontmatter bytes including delimiters and their line ending. The file's bytes are `properties` followed by the text of `content`.

- Each local edit sends a `text_update` carrying a yrs update.
- A frontmatter edit sends a `properties` patch with the expected and new value of each changed key; the workspace-format serializer rewrites the block.
- A rename sends a `move` patch with the expected and new path.
- A device writes a new snapshot, the encoded state of the same yrs document, when live patches on the note exceed 256 or 1 MiB. The snapshot covers each contributing device's latest counter.
- An outside edit to the file becomes a `text_update` computed from the difference, as the editor treats any outside edit.
- A `file` operation that covers every live operation takes the note out of text mode. Clients do this for files that turn binary or grow past 8 MiB of text.

Remote updates pass through the collaboration sandbox, which stops any update after 5 seconds or 512 MiB, before they reach the editor or the file. A snapshot or update larger than the inline limit travels as a blob. Text is limited to 8 MiB and an encoded yrs document to 32 MiB.

Edits made offline merge on reconnect: yrs applies concurrent updates in any order, and a snapshot never discards an update it didn't cover.

## 23. Presence and realtime channel

### Realtime channel

`GET /v2/workspaces/{w}/realtime` opens a WebSocket authenticated with the device's session token. Frames are binary, at most 64 KiB, each `u8 frame_type | bytes body`:

| Type | Direction | Body |
|---|---|---|
| 1 notify | server to client | `id domain`, `u64 head` |
| 2 presence | both | a presence record |
| 3 presence_gone | server to client | `id device`, `id session` |
| 4 policy_changed | server to client | `u64 revision` |
| 5 keys_changed | server to client | empty |

Notifications only prompt a pull, so a relay that forges or drops them can delay sync but can't change what the client accepts. The app sends a WebSocket ping every 20 seconds and reconnects with jittered backoff between 1 and 30 seconds. While the channel is up, the app still pulls each domain at least once a minute; while it's down, the app long-polls.

### Presence record

| Field | Type |
|---|---|
| tag | `tag(noura.sync.presence)` |
| workspace | `id` |
| domain | `id` |
| epoch | `u64` |
| device | `id` |
| session | `id` |
| sequence | `u64` |
| *AEAD associated data ends here* | |
| nonce | `nonce` |
| ciphertext | `bytes` |
| sig | `sig` |

The ciphertext uses the presence subkey of the domain's current epoch. Its padded plaintext:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.presence.payload)` |
| rid | `id` |
| state | `u8` (1 active, 2 idle) |
| selections | `list<selection>`, at most 8, each `bytes anchor`, `bytes head` (yrs relative positions, at most 256 bytes each) |

The object's `rid` stays inside the ciphertext, so the server learns which domain you're in but not which note. The server checks the signer's device, its read access to the domain and the current epoch, then forwards the record to the other connected devices that can read the domain. It never stores presence. A device sends at most 10 presence records a second. Receivers reject a sequence at or below the last one seen for that session and drop a session's cursors 30 seconds after its last record or when the server sends `presence_gone`.

## 24. Share links

An owner or admin shares a read-only snapshot of a note:

```text
{origin}/s/{base64url(token)}#{base64url(link_key)}
```

The sharing device generates a 16-byte token and a 32-byte link key. Browsers never send the fragment, so the server never receives the link key.

`share`, the snapshot the viewer fetches:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.share)` | |
| token_hash | `digest` | `BLAKE3(tag(noura.sync.share.token) \|\| token)` |
| snapshot_revision | `u64` | 1, then +1 on each Update link |
| *AEAD associated data ends here* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | under `derive_key("noura.sync.v2 share key", link_key)` |

Its padded plaintext is `tag(noura.sync.share.payload) | str title | bytes markdown`, at most 983,040 bytes (960 KiB) after padding, so the record fits a 1 MiB request. The viewer in `account-web` decrypts it under a content security policy that blocks inline scripts, renders the Markdown through a strict allowlist, and leaves images out. A tampered ciphertext or wrong key shows an error page and renders nothing.

`share_box`, which lets owner and admin devices list and update links:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.share.box)` |
| workspace | `id` |
| domain | `id` |
| epoch | `u64` |
| token_hash | `digest` |
| *AEAD associated data ends here* | |
| nonce | `nonce` |
| ciphertext | `bytes` |

The box uses the share box subkey of the shared note's domain at `epoch`. Its padded plaintext is `tag(noura.sync.share.box.payload) | id token | key link_key | id rid`, so only members who can read the note recover the link. The server stores the token hash, workspace, domain, expiry (1 to 30 days, 30 by default), the snapshot and the box.

Update link replaces the snapshot under the same token and key while the domain's current epoch equals the box's `epoch`. After a rotation, members who lost access may still hold the old link key, so Update link revokes the old link and creates a new one with a new token and key, and the app shows the new URL. Revoking or expiring a link deletes its snapshot and box, and the viewer then shows a not-found page.

## 25. Workspace profile

The workspace's display name travels encrypted, so a new device can list your workspaces by name and the server stores no plaintext name.

| Field | Type |
|---|---|
| tag | `tag(noura.sync.profile)` |
| workspace | `id` |
| profile_revision | `u64` |
| epoch | `u64` |
| signer | `id` |
| *AEAD associated data ends here* | |
| nonce | `nonce` |
| ciphertext | `bytes` |
| sig | `sig` |

The ciphertext uses the profile subkey of the workspace domain's `epoch` key. Its padded plaintext is `tag(noura.sync.profile.payload) | str name`. Clients accept a profile only when its signer is an owner or admin device in their current verified policy, and report `sync_profile_rollback` for a `profile_revision` below the last one they accepted.

## 26. HTTP API

Request and response bodies use the encoding in [§4](#4-encoding) with media type `application/vnd.noura.sync.v2`, except the tus requests in [§21](#21-attachments). Every route except the public share and invitation pages requires `Authorization: Bearer <session token>`. Errors return an HTTP status and the body `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. Messages never include database or operating system details, which stay in the server's local log.

| Route | Purpose |
|---|---|
| `GET /v2/limits` | protocol version (2) and the limits in [§27](#27-limits) |
| `POST /v2/device/authorize`, `POST /v2/device/token` | device authorization grant |
| `POST /v2/devices/challenge`, `POST /v2/devices` | enrollment challenge; enroll with an `enroll` record |
| `GET /v2/devices`, `DELETE /v2/devices/{device}` | list the account's devices; end a device's sessions |
| `GET /v2/approvals` | offers addressed to the calling device's card |
| `GET /v2/workspaces/{w}/pending-devices` | enrolled cards awaiting approval in this workspace |
| `POST /v2/approvals` | send an `offer` |
| `GET /v2/workspaces` | workspaces the account belongs to, with role and latest profile record |
| `POST /v2/workspaces` | create a workspace: genesis, workspace domain epoch, envelope and profile in one request |
| `GET /v2/workspaces/{w}/policy?after={revision}` | policy changes after a revision, with the head revision |
| `GET /v2/workspaces/{w}/keys?after={key_seq}` | epoch records of readable domains and envelopes addressed to this device, ordered by the server's key sequence |
| `GET /v2/workspaces/{w}/keys/missing` | authorized devices lacking an envelope for epochs this device holds |
| `POST /v2/workspaces/{w}/envelopes` | upload envelopes outside a transaction |
| `POST /v2/workspaces/{w}/ops` | push ([§15](#push)) |
| `GET /v2/workspaces/{w}/domains/{d}/ops?after={seq}&wait={seconds}` | pull ([§15](#pull)) |
| `GET /v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}` | one record by chain position, as an `entry` |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoint` | latest checkpoint bundle |
| `POST /v2/workspaces/{w}/domains/{d}/checkpoints` | submit a checkpoint bundle |
| `GET /v2/workspaces/{w}/domains/{d}/live?at={seq}&after_rid={b64}` | bootstrap page: operations live at `at`, ordered by `rid` |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&level={0..3}&index={j}` | 16 child hashes of a node, or a bucket's leaves at level 3 |
| `POST /v2/workspaces/{w}/txns` and the routes in [§20](#20-chunked-staging) | staged transactions |
| tus routes in [§21](#21-attachments), `GET /v2/workspaces/{w}/blobs/{id}` | attachments |
| `POST`, `GET /v2/workspaces/{w}/invitations`, `DELETE .../invitations/{id}` | create, list and revoke invitations |
| `GET /invite/{token}` | invitation page (HTML) |
| `GET /v2/invitations/{token}` | the invitation's context, for the page and the desktop app; no session required |
| `POST /v2/invitations/{token}/accept` | bind the invitation to the signed-in account |
| `POST`, `GET /v2/workspaces/{w}/shares`, `PUT`, `DELETE .../shares/{token_hash}` | create, list, update and revoke share links |
| `GET /s/{token}`, `GET /v2/shares/{token}` | share viewer page (HTML) and its snapshot record |
| `PUT /v2/workspaces/{w}/profile` | replace the profile record |
| `GET /v2/workspaces/{w}/realtime` | realtime channel |

### Message bodies

Each body below uses the types in [§4](#4-encoding) and carries no tag. A route not listed takes an empty request body and returns an empty response body on success. Timestamps in these bodies (`expires_at`) are Unix seconds as `u64`; they describe invitations and links, never objects.

| Route | Request | Response |
|---|---|---|
| `GET /v2/limits` | | `u16 protocol`, `u32 max_body`, `u32 max_inline`, `u64 max_blob`, `u32 max_txn_parts`, `u32 max_txn_records`, `u32 max_covers`, `u32 presence_per_second` |
| `POST /v2/device/authorize` | | `str device_code`, `str user_code`, `str verification_uri`, `u32 expires_in`, `u32 interval` |
| `POST /v2/device/token` | `str device_code` | `str session_token`; errors use the RFC 8628 codes `authorization_pending`, `slow_down`, `expired_token`, `access_denied` |
| `POST /v2/devices/challenge` | | `key challenge`, `u32 expires_in` |
| `POST /v2/devices` | `bytes enroll` | |
| `GET /v2/devices` | | `list<device_info>`: `bytes card`, `u8 state` (1 active, 2 removed) |
| `GET /v2/approvals` | | `list<bytes>` of offer records |
| `POST /v2/approvals` | `bytes offer` | |
| `GET .../pending-devices` | | `list<pending>`: `bytes card`, `opt<id> invitation` |
| `GET /v2/workspaces` | | `list<workspace_info>`: `id workspace`, `u8 role`, `opt<bytes> profile` |
| `POST /v2/workspaces` | `list<record_ref>`: genesis, the workspace domain's first epoch record, envelopes, profile, and the recovery identity's `add_device` with its envelope when you have a kit | `u64 policy_head` |
| `GET .../policy` | | `u64 head_revision`, `list<bytes>` of policy records |
| `GET .../keys` | | `u64 next_key_seq`, `list<key_entry>`: `u64 key_seq`, `u8 record_type` (3 or 4), `bytes record` |
| `GET .../keys/missing` | | `list<missing>`: `id domain`, `u64 epoch`, `bytes card` |
| `POST .../envelopes` | `list<bytes>` of envelope records | |
| `POST .../ops` | `list<record_ref>` | `list<receipt>` |
| `GET .../ops` | | pull page ([§15](#pull)) |
| `GET .../chain/{device}/{counter}` | | `entry` |
| `GET .../checkpoint` | | `checkpoint_bundle` |
| `POST .../checkpoints` | `checkpoint_bundle` | `u64 seq` |
| `GET .../live` | | `u64 at`, `opt<id> next_rid`, `list<entry>` |
| `GET .../manifest`, levels 0 to 2 | | `list<digest>` of the node's 16 children |
| `GET .../manifest`, level 3 | | `list<leaf>`: `id rid`, `list<ref>` |
| `POST .../txns` | `id txn`, `u32 part_count` | |
| `PUT .../txns/{t}/parts/{i}` | the part bytes | `digest part_digest` |
| `POST .../txns/{t}/commit` | `bytes txn` | `list<receipt>` for its operations and purges, `u64 policy_head` |
| `GET .../txns/{t}` | | `u8 state` (1 staged, 2 committed, 3 failed), `opt<error>` |
| `POST .../invitations` | `id invitation`, `digest token_hash`, `u8 role`, `bytes invite_context` | `u64 expires_at` |
| `GET /v2/invitations/{token}` | | `id workspace`, `id invitation`, `u8 role`, `u8 status`, `bytes invite_context` |
| `GET .../invitations` | | `list<invitation_info>`: `id invitation`, `u8 role`, `u8 status` (1 pending, 2 accepted, 3 approved, 4 revoked, 5 expired), `opt<id> accepted_by`, `u64 expires_at`, `bytes invite_context` |
| `POST .../shares` | `digest token_hash`, `id domain`, `u8 expires_in_days`, `bytes share`, `bytes share_box` | `u64 expires_at` |
| `GET .../shares` | | `list<share_info>`: `digest token_hash`, `id domain`, `u64 expires_at`, `bytes share_box` |
| `PUT .../shares/{token_hash}` | `bytes share` | |
| `GET /v2/shares/{token}` | | `bytes share` |
| `PUT .../profile` | `bytes profile` | |

`error` is `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. The `detail` of `chain_conflict` is `id domain | u64 counter | digest record_digest`, the server's head; of `policy_cuts_stale`, `list<cut>`; of a failed push or transaction, `u32 record_index | str code`.

Server error codes:

| Code | Meaning |
|---|---|
| `bad_request` | the body doesn't decode |
| `too_large` | the body or a field exceeds a limit |
| `unauthorized`, `forbidden`, `not_found` | no valid session; the device lacks the right; no such resource for this device |
| `rate_limited` | over a rate limit; `retry_after_seconds` says when to retry |
| `version_unsupported` | a tag's version isn't 2 |
| `signature_invalid` | a signature fails under the signer's card |
| `padding_invalid` | a ciphertext length isn't a padded length |
| `chain_conflict` | wrong counter or previous digest; `detail` holds the server's head |
| `supersedes_unknown` | a cover names no operation on this `rid` |
| `epoch_stale`, `epoch_conflict`, `rotation_pending` | the operation's epoch isn't current; another rotation took this epoch; the domain waits for a rotation |
| `policy_stale`, `policy_conflict`, `policy_cuts_stale` | a record names a revision above the head; another change took this revision; cuts differ from the server's heads, which `detail` holds |
| `recipients_invalid` | an epoch's recipients or an envelope's recipient don't match the authorized devices |
| `manifest_mismatch` | a checkpoint's root differs from the server's |
| `cursor_compacted` | the cursor is below the retention floor |
| `blob_incomplete`, `blob_digest_mismatch` | a referenced blob isn't complete; an upload's digest doesn't match |
| `txn_incomplete`, `txn_expired` | parts are missing or differ; the transaction expired |
| `quota_exceeded` | the workspace is over its storage quota |
| `id_conflict` | a new random ID already exists |
| `rid_domain_mismatch` | the `rid` belongs to another domain |
| `beyond_cut` | the record lies beyond its device's cut and names no later re-grant |
| `checkpoint_not_due` | a standalone checkpoint arrived while `checkpoint_due` was clear |
| `purge_invalid` | a purged `rid` is outside the domain or has a live operation above the horizon |
| `before_history_floor` | the request reaches below the account's history floor |
| `challenge_invalid`, `origin_mismatch` | the enrollment challenge is unknown, used or expired; the proof names another origin |
| `offer_expired`, `invitation_limit`, `invitation_invalid` | the approval offer is older than 10 minutes; the workspace already has 100 active invitations; the invitation is revoked, expired, used or unknown |

Client error codes: `sync_chain_gap`, `sync_chain_reordered`, `sync_chain_replayed`, `sync_chain_forked`, `sync_policy_forked`, `sync_policy_rollback`, `sync_policy_unauthorized`, `sync_policy_cut_invalid`, `sync_writer_not_authorized`, `sync_epoch_stale`, `sync_key_mismatch`, `sync_padding_invalid`, `sync_payload_invalid`, `sync_manifest_mismatch`, `sync_purge_live_object`, `sync_approval_mismatch`, `sync_recovery_kit_invalid`, `sync_profile_rollback`. Chain and record errors name the device and domain involved.

## 27. Limits

| Limit | Value |
|---|---|
| Request or response body, tus PATCH, range response | 1,048,576 bytes |
| Inline payload before padding | 262,144 bytes |
| Minimum padded length | 256 bytes |
| Blob file content | 2,147,483,648 bytes |
| Blob chunk | 65,536 bytes |
| Supersedes entries per operation | 256 |
| Blobs per operation | 4 |
| Staged transaction | 1,024 parts, 250,000 records, 24 hours |
| Purge | 10,000 `rid`s |
| Envelope ciphertext | 48 bytes |
| Epoch recipients | 4,096 |
| Presence | 10 a second per device, 30-second expiry, 8 KiB padded plaintext |
| Realtime frame | 65,536 bytes |
| Invitations | 7 days, 100 active per workspace |
| Share links | 1 to 30 days |
| Device authorization codes, enrollment challenges, approval offers | 10 minutes |
| Text in text mode, yrs document | 8 MiB, 32 MiB |
| Share snapshot, padded | 983,040 bytes |
| Retention | 30 counted checkpoints; devices idle for 90 counted checkpoints stop holding the floor; tombstones purge after 90 counted checkpoints |

The targets of 100,000 objects and 200 devices are tested sizes, not caps. No structure in this document stops at 1,000 objects.

## 28. Worked example: 100,000 objects

The workspace holds 95,000 notes averaging 3 KiB and 5,000 attachments averaging 2 MiB, across 200 devices. It has 12 domains: the workspace domain, 9 folder domains, and 2 restricted domains of 20 devices each.

**Record sizes** from the layouts above: an envelope is 274 bytes, an epoch record with 199 recipients 3,475 bytes, and an operation carrying a 3 KiB note and one cover 3,636 bytes (308 bytes of fields, AEAD tag and signature, and a 3,328-byte padded payload).

### Removing one device

The removed device could read all 12 domains. Its owner's other device signs the removal.

| Record | Count | Bytes |
|---|---|---|
| `remove_device` with 12 cuts | 1 | 1,041 |
| Envelopes, workspace and folder domains | 10 × 199 = 1,990 | 545,260 |
| Envelopes, restricted domains | 2 × 19 = 38 | 10,412 |
| Epoch records | 12 | 35,940 |
| Part framing, 5 bytes per record | 2,041 | 10,205 |
| Total | | 602,858 bytes |

The rotation fits one staged part and commits atomically. It re-encrypts no object; the 100,000 objects add nothing. In the worst case of 200 devices and 100 folder domains plus the workspace domain, the same removal writes 20,099 envelopes and 101 epoch records, about 6.0 MB in 6 parts.

### Joining

A new member's device joins with access to the workspace and folder domains.

1. **Approval transaction**, about 118 KB in one request: `add_member` (194 bytes), `add_device` (318 bytes), 10 envelopes (2,740 bytes) and 10 fresh checkpoint bundles, each about 11.5 KB with 200 chain heads.
2. **Policy and keys**: a policy chain of 3,000 changes is about 1.2 MB (2 pages). Walking lineage across 300 past epochs in 10 domains reads about 10 MB of epoch records (11 pages).
3. **Live operations**: 95,000 notes at 3,649 bytes per page entry, 287 per page, take 332 pages. The 5,000 attachment operations take 4 more.
4. **Attachments**: each 2 MiB attachment pads to a 2,163,269-byte blob, so the 5,000 blobs take 3 range requests each, 15,000 in all.
5. **Verification**: the device computes 100,000 leaf hashes and 4,369 bucket and node hashes per domain (43,690 for 10 domains) and compares each root. That takes milliseconds.

No request or response exceeds 1 MiB.

A sync rate limit counted only in requests would hold this download back: at 120 requests a minute, the attachment ranges alone take 125 minutes. The server should count bootstrap pages and range reads against a byte budget.

## 29. What the server stores

| Data | Fields | What it reveals |
|---|---|---|
| Accounts | account ID, email, passkey credentials, email confirmation state | who you are |
| Sessions, magic links, device grants, enrollment challenges | token hashes, account, expiry | that you signed in |
| Devices | card (account, device ID, public keys), active or removed, which sessions belong to which device | how many devices you enrolled; a card that never enrolls a session suggests a recovery identity |
| Pending approvals | enrolled cards awaiting approval, approval offers until they expire | that a device is waiting for approval |
| Workspaces | workspace ID, owner account, quota usage | that the workspace exists |
| Policy changes | full signed records | members, roles, device events, domain kinds and restricted grants, in order |
| Domains | domain ID, kind, current epoch, rotation state, history floors | how many domains exist and who reads each |
| Device progress | per device and domain: acknowledged cursor and the index of the latest checkpoint at its last pull | how far each device has synced, in checkpoint units |
| Epoch records and envelopes | full signed records, the key sequence, which authorized devices still lack an envelope | when each domain rotated and which devices hold its key |
| Operations | domain, sequence, device, counter, previous digest, epoch, policy revision, `rid`, supersedes, blob IDs, nonce, padded ciphertext, signature, digest | which device changed which random ID and how often; concurrent edits and incremental updates; padded payload size |
| Purges | full signed records | that listed objects were deleted before a horizon at least 90 counted checkpoints old |
| Checkpoints | full signed records and chain heads | the number of live objects per domain |
| Staged transactions | parts, descriptor, expiry | size of large pending changes |
| Blobs | blob ID, domain, uploading device, padded ciphertext length, ciphertext digest, upload state, referencing operations; expiry only while incomplete | padded attachment size, and that an operation refers to a large file |
| Share links | token hash, workspace, domain, expiry, padded snapshot, box | that a domain has a shared item, and from its expiry roughly when someone shared it |
| Invitations | ID, token hash, workspace, role, inviter, accepting account, status, expiry, padded context | who invited whom, with which role, and from the expiry roughly when |
| Workspace profile | signed encrypted record | when the name changed |
| Rate counters | key and counter | request volume |
| Presence | nothing stored | |

No stored field holds an object type, a creation time or an unpadded size.

- **Object types.** `rid`s are random, payload kinds are encrypted, and paths, file names and frontmatter stay inside ciphertext. Two patterns show how an object is edited, not what it is: operations that cover nothing mark a note in text mode, which any Markdown type (note, task, project, chat) can enter, and a blob ID marks content above 256 KiB, a size class the padded length would show anyway.
- **Creation times.** Operations, `rid`s and completed blobs carry no timestamp, and retention counts checkpoints, so the server keeps no table that turns a sequence into a date. The server sees when an operation arrives while it handles the request, and it doesn't store that time. A `rid` first appears when an object first syncs: for files that existed before you turned sync on, that's the day you turned it on. Share and invitation expiries date the share or invitation, never an object.
- **Sizes.** Every ciphertext the server stores is Padmé-padded, and the server rejects unpadded lengths. Policy, epoch and envelope sizes depend on member, device and domain counts, never on content.

Logs keep route templates, request IDs, status codes and account IDs. They never hold record bodies, tokens, fragments, `rid`s or ciphertext.

## 30. Threats

### What a malicious server or relay can do, and what the client detects

| The server | Detected? | How |
|---|---|---|
| Drops a record inside a device chain | yes | `sync_chain_gap` |
| Reorders a device's records | yes | `sync_chain_reordered` |
| Replays a record at a new position | yes | `sync_chain_replayed` |
| Serves two different records at one chain position | yes | `sync_chain_forked` |
| Drops the newest records of a device | when evidence arrives | later records, checkpoint heads, your own receipts |
| Forges an operation, policy change, epoch record, envelope, checkpoint or presence record | yes | signatures under cards pinned through the policy chain |
| Inserts its own device key | yes | `add_device` needs an authorized signer, and approval needs matching codes |
| Swaps a card during approval | yes | the two approval codes differ |
| Returns an older or different policy chain | yes, after first contact | `sync_policy_rollback`, `sync_policy_forked` |
| Gives recipients of one epoch different keys (with a malicious member) | yes | `key_check` |
| Lists an unauthorized recipient in an epoch | yes | recipients checked against the verified policy |
| Serves a manifest that omits an object | yes, with an honest signer | the recomputed root differs from the signed checkpoint |
| Shows two recipients two different epoch records for one epoch (with the rotating member) | when either sees the other | the epoch record chain, checkpoints naming the epoch record, decryption failures |
| Accepts writes from a device that lost write access | yes | cuts |
| Forwards an old presence record | yes | per-session sequence |
| Forges or drops realtime notifications | harmless | notifications only prompt verified pulls |

### What the client can't prevent

- **Withheld service.** The server can refuse requests, freeze a domain at an old state, or delete ciphertext. Your files stay on your devices.
- **Split views.** The server can show different clients different subsets of records. Clients detect the split once any evidence crosses it (a record, a checkpoint, a receipt). A server that partitions clients completely and forever goes unnoticed.
- **Timing and traffic.** The server sees when you sync, how many operations you send, padded sizes, which random IDs change together, your IP address and your account email.
- **Membership.** Every member and the server see the policy: members, roles, devices, domain kinds and restricted grants. Members outside a restricted domain learn that it exists and who holds it, never its folder name or contents.
- **History for new members.** A new member's domain keys open older ciphertext in those domains through lineage. The history floor keeps the server from serving it, and a malicious server could serve it anyway.
- **Pending rotations.** Until a grantee device rotates a restricted domain, the server alone keeps its ciphertext from a removed device. An operation signed before its writer saw a removal can still use the old epoch.
- **Old share snapshots.** A viewer can't tell a current snapshot from an older one the server kept after Update link.
- **Served web code.** The server serves `account-web`. A modified viewer or invitation page could read the fragment and leak a share link's key or an invitation's context. Self-hosting removes that risk; the desktop app doesn't depend on served code.
- **Blame for forks.** A server can't fake a fork from a re-seal, because `previous_attempt` marks it as a retry. Two records at one position with different changes come from the device's key; clients can't tell whether the device misbehaved or a stolen key did.
- **A colluding writer.** An editor who colludes with the server can sign a checkpoint that omits objects, and a new device bootstrapping from it misses them. Current devices detect the mismatch against their own root; returning devices hold missing files for review instead of deleting them.
- **Enrollment in another account's name.** An owner or admin can remove a member's devices and approve a new device for that member's account. The server checks that the member's own sign-in enrolled the card; clients can't check that. The member sees the device in their device list, and the device gets no restricted key until a manager admits it ([§9](#granting-and-wrapping)).
- **Recovery after first contact.** A recovering device trusts the policy head stored in its kit. A server can withhold changes made after the kit was written, such as the kit's own revocation, until another device's records reveal them.

### Members and devices

- A member writes within their role. A malicious editor can delete objects or purge tombstones; other devices keep their local copies and report a purge of a live object.
- A removed member or device keeps everything it received and can read nothing encrypted after the rotation.
- A device that forks its own chain halts that domain for readers until an owner or admin removes it; the removal's cuts tell readers which records to keep.
- Anyone holding a recovery kit and its passphrase can act as your device until you remove the recovery identity.

## 31. Comparison with SuperSync

SuperSync is the sync service of Super Productivity (MIT, `LICENSE` at the repository root, copyright 2018 Johannes Millan). This comparison reads [johannesjo/super-productivity](https://github.com/johannesjo/super-productivity) at commit `78139c531c808bab7c46ef96fd0fbb0a3b6f7d6d`.

### Operation log

SuperSync stores operations in PostgreSQL with a dense per-user `server_seq` (`packages/super-sync-server`). An operation carries a UUID v7 ID, a client ID, action, operation and entity types, entity IDs, a vector clock, a wall-clock timestamp and a schema version, all in cleartext; only the payload can be encrypted (`packages/shared-schema/src/supersync-http-contract.ts`). Uploads carry up to 100 operations, and the server deduplicates by operation ID and by a request ID cached for 5 minutes.

v2 adopts the server-ordered append-only log, batched uploads, idempotent retries and a cursor written after durable application. It differs in four ways: the log is per domain, an operation's identity is its chain position and digest instead of a time-ordered UUID, every type and entity ID stays inside the ciphertext, and each operation is signed and chained. A push is atomic, so the request-ID cache has no job.

### Cursors

SuperSync clients pull with `GET /ops?sinceSeq&limit` and page with `hasMore`. The server reports `gapDetected` when the client is ahead, the range was purged, or a page has a hole, and the client restarts from zero (`operation-download.service.ts`, `getOpsSinceWithSeq`). A cursor behind compaction jumps to the newest full-state operation.

v2 adopts paging from a cursor and the snapshot-plus-tail recovery. It differs because a server-computed gap flag asks the client to trust the server: v2 clients detect gaps from signed chains, and a cursor behind compaction gets `cursor_compacted` and bootstraps from a signed checkpoint whose Merkle root the client verifies.

### Conflicts

SuperSync detects conflicts per entity with vector clocks, on the server at upload and on the client against an entity frontier (`conflict.ts`, `entity-frontier.ts`). The server rejects concurrent uploads. The client resolves by last write wins with a merge of disjoint fields, and the losing version is overwritten (`conflict-resolution.ts`, `planLwwConflictResolutions`). Clocks are pruned to 20 entries (`vector-clock.ts`, `limitVectorClockSize`), and the project tracks a case where pruning hides a real conflict.

v2 can't detect conflicts on the server, because entity IDs are encrypted. Supersession vectors name only the heads an operation covers, so they need no pruning, and both versions stay for review. v2 adopts the merge of disjoint changes, the rule that identical branches aren't a conflict (`isIdenticalConflict`), and a deterministic tie order, using operation digests where SuperSync uses client IDs.

### Compaction

SuperSync treats a full-state import or repair as a snapshot operation. A daily job deletes history before the newest snapshot once it is older than 45 days, and a 100 MB quota per user deletes old snapshots (`storage-quota.service.ts`, `deleteOldSyncedOpsForAllUsers`). Clients don't create periodic snapshots. A new device downloads the newest snapshot and the tail.

v2 adopts a retention window behind a snapshot boundary and the snapshot-plus-tail bootstrap. It differs in what a snapshot is and how the window is measured: writer devices sign periodic manifest checkpoints, the server deletes only covered operations below a retention floor that waits for every active device and 30 checkpoints, and tombstones leave through signed purges. v2 counts checkpoints instead of days so the server keeps no dates. Text snapshots play the role of SuperSync's full-state operations for one note.

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

An implementation that adapts any of them records the repository, commit, path, license and changes under `docs/provenance/` before the code lands, and adds the MIT notice to `THIRD_PARTY_NOTICES.md`.

## 32. Fixtures

Fixtures live in `docs/workspace-format/fixtures/`. Each holds accepted cases and rejected ones: a bad signature, a wrong tag, a wrong version and a truncated field, plus the cases listed. Fixture keys are public test values and never secret.

| Fixture | Pins |
|---|---|
| `sync-v2-encoding.json` | field types, tag bytes, decoding rules |
| `sync-v2-padding.json` | `padded_length` for boundary lengths, malformed padding |
| `sync-v2-ids.json` | remote ID format, base64url forms |
| `sync-v2-device.json` | `noura.sync.device.card`, device fingerprint, `noura.sync.device.enroll` (wrong origin, account and key) |
| `sync-v2-approval.json` | `noura.sync.approval.offer`, `noura.sync.approval.code` |
| `sync-v2-policy.json` | `noura.sync.policy`, each change kind, cuts, chain links, authority failures |
| `sync-v2-keys.json` | `noura.sync.epoch`, the epoch record chain, `noura.sync.key-check`, lineage, `noura.sync.envelope`, HPKE `info`, unpinned signer, `noura.sync.v2 lineage key` |
| `sync-v2-operation.json` | `noura.sync.op`, `noura.sync.payload` for each kind, AEAD associated data, `noura.sync.v2 operation key` |
| `sync-v2-chain.json` | schedules for gap, reorder, replay, fork, a retried upload, re-seals with `previous_attempt` served in either order, and a record beyond a cut |
| `sync-v2-manifest.json` | `noura.sync.manifest.leaf`, `.bucket`, `.node`, `.heads`, `noura.sync.checkpoint`, roots for add, remove and change |
| `sync-v2-purge.json` | `noura.sync.purge` |
| `sync-v2-txn.json` | `noura.sync.txn`, `noura.sync.txn.parts` |
| `sync-v2-blob.json` | `noura.sync.blob` header, chunk nonces, lengths |
| `sync-v2-presence.json` | `noura.sync.presence`, `noura.sync.presence.payload`, `noura.sync.v2 presence key` |
| `sync-v2-profile.json` | `noura.sync.profile`, `noura.sync.profile.payload`, `noura.sync.v2 profile key` |
| `sync-v2-invitation.json` | `noura.sync.invite.token`, `noura.sync.invite.context`, `noura.sync.invite.payload`, `noura.sync.v2 invitation key` |
| `sync-v2-share.json` | `noura.sync.share.token`, `noura.sync.share`, `noura.sync.share.payload`, `noura.sync.share.box`, `noura.sync.share.box.payload`, `noura.sync.v2 share key`, `noura.sync.v2 share box key` |
| `sync-v2-recovery.json` | `noura.sync.recovery-kit`, `noura.sync.recovery-kit.payload` (wrong passphrase, low parameters) |

### Tag bytes

Each tag is the length byte, the ASCII domain string and `0002`:

| Domain string | Length | Tag bytes (hex) |
|---|---|---|
| `noura.sync.device.card` | 22 | `166e6f7572612e73796e632e6465766963652e636172640002` |
| `noura.sync.device.enroll` | 24 | `186e6f7572612e73796e632e6465766963652e656e726f6c6c0002` |
| `noura.sync.approval.offer` | 25 | `196e6f7572612e73796e632e617070726f76616c2e6f666665720002` |
| `noura.sync.approval.code` | 24 | `186e6f7572612e73796e632e617070726f76616c2e636f64650002` |
| `noura.sync.policy` | 17 | `116e6f7572612e73796e632e706f6c6963790002` |
| `noura.sync.epoch` | 16 | `106e6f7572612e73796e632e65706f63680002` |
| `noura.sync.key-check` | 20 | `146e6f7572612e73796e632e6b65792d636865636b0002` |
| `noura.sync.envelope` | 19 | `136e6f7572612e73796e632e656e76656c6f70650002` |
| `noura.sync.op` | 13 | `0d6e6f7572612e73796e632e6f700002` |
| `noura.sync.payload` | 18 | `126e6f7572612e73796e632e7061796c6f61640002` |
| `noura.sync.purge` | 16 | `106e6f7572612e73796e632e70757267650002` |
| `noura.sync.checkpoint` | 21 | `156e6f7572612e73796e632e636865636b706f696e740002` |
| `noura.sync.manifest.leaf` | 24 | `186e6f7572612e73796e632e6d616e69666573742e6c6561660002` |
| `noura.sync.manifest.bucket` | 26 | `1a6e6f7572612e73796e632e6d616e69666573742e6275636b65740002` |
| `noura.sync.manifest.node` | 24 | `186e6f7572612e73796e632e6d616e69666573742e6e6f64650002` |
| `noura.sync.manifest.heads` | 25 | `196e6f7572612e73796e632e6d616e69666573742e68656164730002` |
| `noura.sync.txn` | 14 | `0e6e6f7572612e73796e632e74786e0002` |
| `noura.sync.txn.parts` | 20 | `146e6f7572612e73796e632e74786e2e70617274730002` |
| `noura.sync.blob` | 15 | `0f6e6f7572612e73796e632e626c6f620002` |
| `noura.sync.presence` | 19 | `136e6f7572612e73796e632e70726573656e63650002` |
| `noura.sync.presence.payload` | 27 | `1b6e6f7572612e73796e632e70726573656e63652e7061796c6f61640002` |
| `noura.sync.profile` | 18 | `126e6f7572612e73796e632e70726f66696c650002` |
| `noura.sync.profile.payload` | 26 | `1a6e6f7572612e73796e632e70726f66696c652e7061796c6f61640002` |
| `noura.sync.invite.token` | 23 | `176e6f7572612e73796e632e696e766974652e746f6b656e0002` |
| `noura.sync.invite.context` | 25 | `196e6f7572612e73796e632e696e766974652e636f6e746578740002` |
| `noura.sync.invite.payload` | 25 | `196e6f7572612e73796e632e696e766974652e7061796c6f61640002` |
| `noura.sync.share.token` | 22 | `166e6f7572612e73796e632e73686172652e746f6b656e0002` |
| `noura.sync.share` | 16 | `106e6f7572612e73796e632e73686172650002` |
| `noura.sync.share.payload` | 24 | `186e6f7572612e73796e632e73686172652e7061796c6f61640002` |
| `noura.sync.share.box` | 20 | `146e6f7572612e73796e632e73686172652e626f780002` |
| `noura.sync.share.box.payload` | 28 | `1c6e6f7572612e73796e632e73686172652e626f782e7061796c6f61640002` |
| `noura.sync.recovery-kit` | 23 | `176e6f7572612e73796e632e7265636f766572792d6b69740002` |
| `noura.sync.recovery-kit.payload` | 31 | `1f6e6f7572612e73796e632e7265636f766572792d6b69742e7061796c6f61640002` |

BLAKE3 `derive_key` contexts are the exact ASCII strings in [§3](#3-primitives), with no tag and no version bytes: `noura.sync.v2 operation key`, `noura.sync.v2 lineage key`, `noura.sync.v2 presence key`, `noura.sync.v2 profile key`, `noura.sync.v2 share box key`, `noura.sync.v2 share key` and `noura.sync.v2 invitation key`.

### Example: a device card

With account `11` repeated 16 times, device `22` repeated 16 times, signing key `33` repeated 32 times and HPKE key `44` repeated 32 times, the card is these 121 bytes:

```text
16 6e6f7572612e73796e632e6465766963652e63617264 0002   tag
11111111111111111111111111111111                        account
22222222222222222222222222222222                        device
3333333333333333333333333333333333333333333333333333333333333333   signing_public
4444444444444444444444444444444444444444444444444444444444444444   hpke_public
```

## 33. Decisions to confirm at sign-off

These choices go beyond the list of topics the protocol had to cover. Confirm or change each one:

1. **Binary encoding.** Records use the fixed-width binary encoding in [§4](#4-encoding) instead of v1's JSON tuples, and the signed bytes are the sent bytes.
2. **New primitives.** XChaCha20-Poly1305 replaces AES-256-GCM, HPKE replaces age and the custom web envelope, and BLAKE3 replaces SHA-256 for digests.
3. **Folder domains carry no permission.** They inherit the workspace audience and exist as rotation, checkpoint and bootstrap units. The alternative is to drop them from v2.
4. **History isolation is the server's job.** Lineage lets one envelope per domain open old epochs, so joining never re-encrypts objects.
5. **Recovery kits are offline devices** protected by an Argon2id passphrase of at least 12 characters, and they stay current through rotations.
6. **Approval compares one 160-bit code** on both devices, confirmed on both.
7. **Tombstones leave through signed purges** after 90 checkpoints, instead of a cleartext delete flag on each operation.
8. **Retention counts checkpoints, not days**, so the server stores no sequence-to-date table: 30 checkpoints of dead operations, 90 for idle devices and tombstones.
9. **Limits**: 1 MiB bodies, 256 KiB inline payloads, 2 GiB blobs, 256 covers per operation.
10. **Edit beats delete** in a conflict until you choose, and a returning device asks before deleting files the manifest lacks.
11. **Edit mode and size class stay visible.** Patches that cover nothing show text mode, and blob IDs show content above 256 KiB. Hiding them would mean dropping server-side compaction or blob authorization.
12. **Only the inviting device approves an invitee**, and a device that another account enrolled gets no restricted key until a manager signs `admit_device`.
13. **Bootstrap rate budget**: the sync rate limit counts bootstrap pages and range reads in bytes.

## Sign-off

| | |
|---|---|
| Owner | pending |
| Date | pending |
| Commit signed off | pending |
