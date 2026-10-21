# Encrypted sync protocol, version 2

Status: draft, waiting for sign-off. On 2026-10-20 the owner delegated sign-off to Claude, the orchestrating agent. Nobody writes v2 code before the [sign-off record](#sign-off) holds a name and a date.

This document defines Noura's encrypted sync protocol for the Rust product: the records a desktop client signs and encrypts, the keys that let devices and members read them, the server's checks, and the HTTP API that carries them. It replaces [sync-v1.md](sync-v1.md), [checkpoints.md](checkpoints.md) and [collaboration.md](collaboration.md), which describe the legacy app.

The server stores ciphertext and the routing fields listed in [§29](#29-what-the-server-stores). It never needs plaintext workspace content to store, order, route or compact your data.

## 1. Goals

- Sync a workspace of 100,000 objects across 200 devices, with no request or response body above 1 MiB.
- Keep key rotation proportional to devices and key domains. Removing a device never re-encrypts objects.
- Let a client detect a server that drops, reorders, replays or forks a device's operations, or that forks or rolls back the policy, the keys or the manifest.
- Keep object types, creation times and exact sizes out of the data the server stores. The server still sees when requests arrive, and its stored data dates a record to about a day ([§29](#29-what-the-server-stores)).
- Let a new device download current state without replaying full history.
- Keep both versions when two devices change the same object.

Out of scope: hiding when you sync, how much you sync, your IP address, your account email, or who belongs to a workspace.

## 2. Version 1 is unsupported

Version 2 shares no record, route or fixture with version 1. A v2 client refuses a server that doesn't advertise protocol 2 ([§26](#26-http-api)), and a v2 server serves no `/v1` route. Nothing migrates: v1 workspaces, accounts, devices, keys, operations and recovery kits stay with the legacy app, and the hosted v1 database is wiped at cutover. To move a workspace from the legacy app, turn on v2 sync from its folder as if it were new.

## 3. Primitives

| Purpose | Primitive | Rust crate, version and license |
|---|---|---|
| Signatures | Ed25519 (RFC 8032), verified with `verify_strict` by the client, the server and `account-web` alike | `ed25519-dalek` 3.0.0, BSD-3-Clause |
| Key wrapping | Hybrid Public Key Encryption (HPKE, RFC 9180) base mode: the X-Wing key encapsulation mechanism (ML-KEM-768 combined with X25519, draft-connolly-cfrg-xwing-kem) `0x647a`, HKDF-SHA256 `0x0001`, ChaCha20Poly1305 `0x0003` | `hpke` 0.14.1 with its `mlkem` feature, which brings `x-wing` and `ml-kem`; MIT or Apache-2.0 |
| Record encryption | XChaCha20-Poly1305 with a random 24-byte nonce | `chacha20poly1305` 0.11.0, MIT or Apache-2.0 |
| Attachment encryption | STREAM (Hoang, Reyhanitabar, Rogaway and Vizár) over XChaCha20-Poly1305, 32-bit big-endian counter and last-chunk flag | `aead-stream` 0.6.0, type `StreamBE32<XChaCha20Poly1305>`; MIT or Apache-2.0 |
| Digests, message authentication codes, key derivation | BLAKE3: `hash`, `keyed_hash`, `derive_key` | `blake3` 1.8.7, CC0-1.0 or Apache-2.0 |
| Recovery kit | Argon2id (RFC 9106), m = 65,536 KiB, t = 3, p = 4, over a generated 160-bit secret | `argon2` 0.6.0, MIT or Apache-2.0 |
| Randomness | the operating system's cryptographically secure random number generator (CSPRNG) | `getrandom` 0.4.3, MIT or Apache-2.0 |

`Cargo.lock` pins these versions, and a version change updates this table. Continuous integration runs the published test vectors for each primitive: the RFC 9180 vectors for the HPKE key schedule and the X-Wing draft's vectors, Wycheproof vectors for XChaCha20-Poly1305, Ed25519 and X25519 (low-order points included), and the RFC 9106 vectors for Argon2id.

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
| `noura.sync.v2 share box key` | share-link boxes |

Four contexts derive from other inputs. `noura.sync.v2 folder key` derives a folder domain's epoch key from the workspace domain's epoch key and the folder domain's ID ([§9](#9-key-domains)). `noura.sync.v2 share key` derives the snapshot key from a share link's key. `noura.sync.v2 invitation key` derives the context key, and `noura.sync.v2 invitation proof` the proof key, from an invitation secret.

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

When a record carries ciphertext, its associated data for authenticated encryption (AEAD) is the body prefix that ends before its `nonce` field. Each layout below marks that point. A record encrypted under a subkey of an epoch key (an operation, presence record, profile or share box) appends to that prefix the digest of the epoch record that announced the epoch. The record doesn't carry that digest: writer and reader each take it from their accepted epoch chain, and for a folder domain they take the workspace domain's record. A writer and a reader that hold different epoch records for one epoch therefore can't read each other's records.

### Decoding rules

Decoders reject, with a structured error and without panicking:

- trailing bytes after the last field;
- a length above the limit for that field ([§27](#27-limits)) before allocating, and a `bytes` field whose fixed length this document states at any other length;
- an unknown tag, a version other than 2, or an unknown enum value;
- a record whose tag isn't the tag of the type its position names, such as a `record_ref` type, a route or a field;
- an `opt` discriminator other than 0 or 1;
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
| Text lineage ID | the device that puts a note into text mode or rebases it | names the yrs document of a text-mode note ([§22](#22-live-collaboration)) |
| Blob ID | the uploading device | |
| Transaction ID | the staging device | reused on retry |
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
| device | `id` | |
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

The server verifies the signature with the card's `signing_public`, then checks that `origin` is its own configured origin, the card's account is the session's account, the challenge is current and unused, and the device ID is new. `sync-key-envelope` exports this verifier.

An account's first enrolled device is trusted when it enrolls: it can create workspaces. Every later device needs approval from a device the account already trusts ([§10](#10-approval)), except a device you recover with a kit ([§12](#12-recovery-kit)).

### Pinned cards

Each device keeps a list of its own account's pinned cards in the app's application data, outside every workspace, under a BLAKE3 keyed hash with a key it keeps in the credential store, as it does for its journal ([§15](#client-journal)). A card of your account enters the list when the device:

- creates it: its own card, and a recovery identity it exports ([§12](#12-recovery-kit));
- completes an approval code with it, as approver or as candidate ([§10](#10-approval));
- imports it from a recovery kit;
- verifies, in a workspace's policy chain, an `add_device` for it that a device it already pinned signed.

An `add_device` that another account signed for your account pins nothing, and a card that only `GET /v2/devices` lists never enters the list. Every card in an `add_device` with a zero `offer` comes byte for byte from the signer's pinned cards. A device of your account that the signer hasn't pinned joins that workspace later, when a device that pinned it adds it, or through its own approval.

### Device list

The app builds your device list from the verified policy chains of your workspaces and its pinned cards, and uses `GET /v2/devices` only for sessions, never as a source of cards. For each of your devices it shows which device and which account signed its `add_device`, and it alerts you when another account signed an `add_device` for your account.

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
| 2 | add_device | `u8 device_kind` (1 device, 2 recovery identity), `bytes card`, `digest offer` (32 zero bytes when nobody compared an approval code, which the signer may do only for a card of its own account that it pinned ([§7](#pinned-cards)): adding a recovery identity, adding your other devices to a workspace you created or joined, or a device that a recovery identity signs in) |
| 3 | remove_device | `id device` |
| 4 | add_member | `id account`, `u8 role`, `id invitation` |
| 5 | set_role | `id account`, `u8 role` |
| 6 | remove_member | `id account` |
| 7 | create_domain | `id domain`, `u8 domain_kind` (2 folder, 3 restricted), `id marker_rid`, `list<access>` |
| 8 | set_access | `id domain`, `list<access>` (replaces the domain's list) |
| 9 | delete_domain | `id domain` |
| 10 | transfer_owner | `id account` |
| 11 | admit_device | `id domain`, `id device`: makes a device eligible for a restricted domain ([§9](#granting-and-wrapping)) |
| 12 | amend_cut | `u64 revision`, `list<cut>`: raises cuts of the change at that revision |

`access` is `id account | u8 level`, sorted by account, with levels 1 read, 2 write and 3 manage. A folder domain's list is empty in v2. Roles are 1 owner, 2 admin, 3 editor and 4 viewer.

A change that removes write access from a device in a domain must carry a cut for that pair. Before signing it, the signer freezes the devices that lose access (`POST /v2/workspaces/{w}/freeze`): the server rejects their new records in those domains with `device_frozen` and returns their heads, and the signer builds the cuts from those heads. The server accepts a freeze of a device in a domain only from a device whose account could sign a change that removes that write access: a `remove_device`, `set_role`, `remove_member` or `set_access` under the authority rules below. A viewer can therefore freeze nobody but its own devices. The server records the freezing device and names it in the `detail` of `device_frozen`, and the frozen device's app shows who froze it. A freeze ends when a change that removes that write access commits, or after 10 minutes. Only the freezing device can renew it, only after its push of the removing change failed during the freeze, and at most 3 times. The freeze only keeps a writing target from racing its own removal; a server that ignores it reports false heads, which the cuts expose. The server builds the expected cuts from its own chain heads and rejects a change whose cuts differ (`policy_cuts_stale`), returning its heads so the signer can retry. Before signing, the signer compares each cut with the records it holds for that device and domain, and asks the server again when a cut sits below a record it accepted.

A change that removes access commits by itself, in one push ([§15](#push)), and the rotations it requires follow ([§9](#rotation)). When it commits, the server ends every session of the devices it removes, whatever their account.

The server and clients reject a record beyond a device's cut unless its `policy_revision` is at or after a later change that gave that device write access again in that domain (server error `beyond_cut`). An honest server reports true heads, so a client that already accepted a record beyond a new cut has caught the server lying: it accepts the policy change, reports `sync_policy_cut_invalid` for that device and domain, and holds that domain's cursor.

A cut that the server's false heads set too low stays in the signed chain. A device that could sign the original change corrects it with `amend_cut`, which raises each cut to a record signed by the removed device whose `policy_revision` sits below the amended change's revision. The server checks that each new cut names a pair the amended change cut, sits at or above the old cut, and names a record it holds. A reader holding a domain on `sync_policy_cut_invalid` resumes once an accepted amendment covers every record it held beyond the cut.

Removing a member requires a `remove_device` change for each of the member's devices earlier in the same push or transaction. A `remove_member`, whoever signs it, also drops the account from every restricted domain's access list in the policy state. An account added again reads a restricted domain only after a manager grants it again with `set_access`. A restricted domain whose last manager leaves keeps its other grants, and an owner or admin can still delete it.

`delete_domain` deletes the domain's log, epochs, envelopes and checkpoints on the server, and the blobs that only its operations listed. Devices that held the domain keep their local files. Those files lose their marker's protection, so the rule in [§9](#domain-roots) holds them for review instead of uploading them to a wider domain. An owner or admin can delete a restricted domain they can't read.

### Roles and authority

| Action | Who may sign it |
|---|---|
| genesis | the creating device, which becomes the owner's first device |
| add_device for your own account | any device of your account in the workspace; a recovery identity only for one new device of its account per workspace |
| add_device for another account | an owner or admin device, only while that account has no device in the workspace (its first device, or re-enrollment after an owner or admin removed every lost device); admins can't add devices to the owner or to other admins |
| remove_device | a device of the same account, a recovery identity of the same account, or a device whose role outranks the target account's role |
| add_member | owner (admin, editor or viewer); admin (editor or viewer); requires an invitation the account accepted |
| set_role | owner for any non-owner; admin between editor and viewer |
| remove_member | owner for any non-owner; admin for editors and viewers; the member themself |
| create_domain | owner or admin; a restricted domain's list must name the signer's account as a manager |
| set_access (restricted) | a manager of that domain for any change; an owner or admin only to remove entries |
| admit_device | a manager of that restricted domain |
| delete_domain | owner or admin, or a manager of that restricted domain |
| transfer_owner | owner; the old owner becomes an admin |
| amend_cut | a device that may sign the change it amends, at the current revision |

A recovery identity signs no other kind of change, whatever its account's role. The server accepts an `add_device` for a card of another account only when that account enrolled the card with its own session, and demands an enrollment proof for every card except a recovery identity that a device of its own account adds.

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

A client accepts change `r` only when its revision is one above the accepted head, its `previous` equals the head's digest, its signature verifies under a device that the state before `r` lists for the signer's account, and that account holds the authority above. A device trusts revision 1 through its approval anchor ([§10](#10-approval)), its recovery kit ([§12](#12-recovery-kit)), or because it created the workspace.

A device of your account that creates or joins a workspace adds, in the creation request or its first push or transaction after approval, every device and recovery identity of your account that it pinned ([§7](#pinned-cards)) and the chain lacks. Each device of your account does the same when it first verifies a workspace's chain. Another device of your account then trusts that workspace's chain through the `add_device` that names it: a device of its own account, whose card it pinned, must have signed that change, and the change's `previous` digest fixes the prefix, which the device then verifies from genesis.

Records name the policy by revision and digest. A record that carries `policy_revision` (an operation, purge, epoch record, checkpoint, profile or transaction descriptor) also carries `policy_head`, the digest of the change at that revision. The client compares it with its own accepted change at that revision.

The client stores the accepted head durably, in its journal and in its high-water mark ([§15](#client-journal)), and reports:

- `sync_policy_forked` when the server returns a different change for a revision the client already accepted, or a record's `policy_head` differs from the client's change at that revision;
- `sync_policy_rollback` when the server reports a head below the accepted one;
- `sync_policy_unauthorized` when a change fails the authority rules.

A record that names a policy revision above the client's accepted head waits until the client has fetched and accepted that revision. If the server can't supply it, the client reports `sync_policy_rollback`.

Policy chains stay short (one change per membership or device event), so a new device replays the whole chain and v2 defines no policy snapshot.

## 9. Key domains

A key domain is a set of objects that share one symmetric key at a time. Each workspace has exactly one workspace domain and any number of folder and restricted domains, at most 256 domains in all.

| Domain | Who reads it | Objects | Keys |
|---|---|---|---|
| Workspace | every member | every object outside a folder or restricted domain, and the workspace profile | its own epochs |
| Folder | every member, inherited from the workspace | the objects under one folder | derived from the workspace domain's epochs |
| Restricted | only the members its access list names | the objects under one folder | its own epochs |

A folder domain inherits its audience. A restricted domain's audience comes only from its grants: the owner and admins don't read it unless a manager grants them access. Folder domains carry no extra permission in v2. They split a large workspace into logs that checkpoint and bootstrap on their own, and they give a later version a place to add per-folder sync or guests.

A folder domain has no epochs of its own. Its key for workspace epoch `e` is `derive_key("noura.sync.v2 folder key", workspace epoch key e || domain)`, and its records name the workspace domain's epoch number. One rotation of the workspace domain therefore covers every folder domain, and a device needs no envelope for a folder domain.

### Domain roots

A folder or restricted domain covers the folder that holds its marker file, `.noura-domain`, and everything beneath it. The marker is the synced object whose `rid` the domain's `create_domain` names, and it lives in that domain's log, so a member outside a restricted domain never receives it. Its content is one line, `domain: <base64url domain ID>`, followed by a line feed; clients write exactly that and rewrite the file when an outside edit changes it. Because the marker moves with its folder, moving the folder keeps the domain and no path serves as identity. A client refuses to create a marker inside another domain's root. The creating device writes the marker in the same push or transaction as `create_domain`.

A marker is valid when the live set of `marker_rid` in the named domain's own log holds exactly one operation, a state operation whose decrypted path ends in `/.noura-domain`, and the policy still lists the domain. A deleted marker, or one in conflict with two live states, isn't valid. A `.noura-domain` file found in any other log, or written by hand, is an ordinary file. An object belongs to the domain of the nearest ancestor folder holding a valid marker, or to the workspace domain, so an outer marker never takes objects from an inner root.

The journal records each domain root as (domain, folder, marker `rid`) when the client creates or applies the marker, and that record decides where a new file goes: the client classifies every new file under a recorded root into that domain. A client applies a domain's marker before any other object of that domain, during bootstrap too ([§18](#18-bootstrap)), so the root exists before the first file beneath it. During a bootstrap or a journal rebuild, the client holds every new local file until it has applied the marker of every folder and restricted domain it reads. When a marker disappears or stops being valid, or a marker's move would change the domain of existing objects, the client keeps the recorded root, holds the affected files and any new files under it for review, and uploads none of them to another domain until you confirm. An iCloud placeholder of the marker, `.noura-domain.icloud`, counts as present. Restoring the marker from synced state ends the hold. This holds for every domain kind.

A client holding live objects of a folder or restricted domain that no recorded root covers records a held root for each folder that directly contains such an object, before it writes the object's file. That happens after a bootstrap or a journal rebuild meets a marker that is moved, deleted or in conflict, and when objects of a domain sit outside its root. The client holds every new file at or beneath a held root and uploads none of them to any domain until a valid marker covers that folder or you confirm a domain for them.

A client never uploads a delete of a marker. When you delete a marker file locally, the client writes it again from its journal; only `delete_domain` retires a marker. A client that receives a delete of `marker_rid`, or a move of it to a path that doesn't end in `/.noura-domain`, keeps its recorded root, holds files under it as above, and reports the record.

When a grantee receives an object of a wider domain whose path falls under a restricted root, it writes the object outside the root, under the root folder's name followed by ` (shared)`, and reports it. It never moves the object into the restricted domain.

Sync covers every file that the workspace scanner reports ([v1.md](v1.md#local-metadata)) and passes the path rules in [§13](#payload), plus each valid `.noura-domain` marker, the one dotfile sync reads. It skips the manifest's `ignore` patterns, symlinks, empty folders, and files whose names aren't valid UTF-8 or break the path rules; the client reports each file it skips. v1.md and its conformance fixtures gain the marker exception in the change that implements domain roots.

### Moving objects between domains

A move across a domain boundary writes a new state operation with a new `rid` in the target domain, whose payload names the source domain and `rid` in its `origin` field, and a `delete` operation (reason 2, moved) in the source domain, whose payload names the target. Both commit atomically ([§15](#push)), so the server can link the two `rid`s ([§29](#29-what-the-server-stores)). An attachment moved between the workspace domain and a folder domain, or between two folder domains, keeps its blob: those domains share one audience, and their operations may list each other's blobs ([§21](#21-attachments)). An attachment moved into or out of a restricted domain gets a new blob: the client encrypts it again under a new key and blob ID and uploads it. Members of both domains see one object, because the local object ID travels inside both payloads.

A client applies the create before the delete. When it can read the target domain, it defers a reason-2 delete until it has pulled the target domain past the move; a device that can't read the target applies the delete at once.

A client never moves an object from a restricted domain to a wider audience on its own. When a file leaves a restricted root, or its marker disappears, the client holds those files for review and shows how many would become visible to more people. Moving files into a restricted domain yourself needs no confirmation; a marker change that would move existing objects goes to review as above. A move of more objects than one staged transaction holds splits into ordered transactions ([§20](#20-chunked-staging)).

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
| lineage | `bytes` | previous epoch key, XChaCha20-Poly1305 under the lineage subkey of this epoch's key; empty at epoch 1 and at a lineage break |
| sig | `sig` | by `rotating_device` |

An epoch travels as an epoch bundle, `bytes epoch | list<id> recipients`. The server keeps an epoch's recipient list while the epoch is current or named by a retained counted checkpoint ([§19](#19-compaction-and-purge)), and serves the record alone after that. When an epoch commits, the server also appends its epoch record, without recipients, to that domain's log as an entry, and for a workspace epoch to every folder domain's log too, in the same database transaction ([§15](#pull)). A reader verifies that entry as an epoch record, and the entry fixes where the epoch starts in each log ([§9](#rotation)).

The server accepts an epoch record only for epoch 1 of a domain, committed with genesis or `create_domain`; for a rotation that a committed policy change requires and that hasn't committed yet; or for a lineage break signed by a device whose account could sign a change that grants access to that domain. It rejects any other epoch record with `rotation_not_due`. A member therefore can't rotate a domain at will, and only a required rotation or an authorized lineage break makes writers re-seal.

The lineage field links each epoch to the one before, so a device that holds the current key derives every earlier key by walking backward. A newly approved device therefore needs one envelope per workspace or restricted domain, and objects written under old epochs stay readable without re-encryption. A key that a newer key's lineage yields needs no signature check of its own record: the lineage AEAD authenticates it, and the device checks it against that record's `key_check`.

`key_check` lets every recipient confirm it received the same key as the others. A rotating device can't hand different recipients different keys without failing that check.

A client accepts an epoch record when:

- its signature verifies under the rotating device's pinned card;
- the rotating device was active and could read the domain at `policy_revision`; for epoch 1 of a restricted domain it must be eligible ([§9](#granting-and-wrapping));
- `policy_revision` is at least the previous record's, and `policy_head` matches the client's change at that revision;
- the recipient list matches `recipients_digest`, and every recipient could read the domain at that revision;
- the epoch is one above the previous accepted epoch and `previous` equals that record's digest;
- the lineage decrypts to a key matching the previous record's `key_check`, or the lineage is empty and marks a break (below).

Every key derived through lineage is checked against its own record's `key_check`. A failure reports `sync_key_mismatch`.

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
| recipient_hpke_digest | `digest` | `BLAKE3` of the `hpke_public` bytes in the recipient's card |
| wrapping_device | `id` | |
| *HPKE `info` ends here* | | |
| enc | `bytes` | HPKE encapsulated key, exactly 1,120 bytes |
| ciphertext | `bytes` | 48 bytes: the epoch key and the 16-byte tag |
| sig | `sig` | by `wrapping_device` |

HPKE runs in base mode with the suite in [§3](#3-primitives). Its `info` is the envelope prefix from the tag through `wrapping_device`, its `aad` is empty, and its plaintext is the 32-byte epoch key. A recipient accepts an envelope only when the signature verifies under the card of a device active at the epoch's `policy_revision` or at the recipient's accepted head, and the unwrapped key matches the epoch record's `key_check`. A key supplied by the server, or an envelope signed by an unpinned or long-removed device, fails with `sync_key_mismatch`.

The server deletes an epoch's envelopes once the next epoch of that domain commits. A device that lacks a key gets an envelope for the current epoch and reaches older keys through lineage.

### Granting and wrapping

Granting decides who may read a domain; wrapping hands an authorized device the key.

- **Granting** happens only through policy changes, signed by the devices the authority table in [§8](#roles-and-authority) allows: membership changes for workspace and folder domains, and a manager's access-list changes for a restricted domain.
- **Wrapping** is mechanical. Any device that holds an epoch key may wrap it for a device that its own verified policy authorizes to read that domain. The wrapping device takes the recipient's card byte for byte from the `add_device` change in its verified policy, and wraps only for a device that is active at its accepted head and authorized for the domain, and for a restricted domain also eligible. It refuses otherwise and never wraps for a card the server supplies. The server also rejects an envelope for any other recipient.

A restricted domain adds one more condition, computed from the policy chain alone. A device is foreign when another account signed its `add_device`, unless the same approval added its account with `add_member` and the account had never been a member of the workspace before, or when a foreign device signed it. A device approved when its account rejoins the workspace is therefore foreign. A device of grantee account A is eligible for restricted domain R when one of these holds:

- it isn't foreign, and it belonged to A when the change that last made A a grantee of R committed;
- a device of A that was eligible for R at the time signed its `add_device`;
- a manager of R signed `admit_device` for it.

Only eligible devices receive R's key, as rotation recipients or through envelopes, and the server rejects any other recipient with `recipients_invalid`. A device that another account added (an admin re-enrolling a member) therefore gets no key for any restricted domain, under past grants or later ones, and nor do devices it adds, until a manager admits it. An `admit_device` change carries a fresh checkpoint of R, and the account's history floor in R becomes that checkpoint's `covered_seq`.

When an approver can't wrap a domain (an admin approving a member's new device outside a restricted domain the member belongs to), the new device's envelope stays missing. The server lists each eligible device that lacks an envelope for the current epoch, by device ID, to one device that holds the key at a time ([§26](#duties)), which fills the gap.

### Rotation

A domain rotates when a device that could read it loses access:

| Domain | Rotates when |
|---|---|
| Workspace | a device is removed, or a member leaves or is removed |
| Folder | never on its own: it uses the workspace domain's epochs |
| Restricted | a grantee loses its grant, a grantee's device is removed, or a grantee leaves the workspace |

Exporting a recovery kit adds a device; revoking the kit removes it and rotates. A device that grants access again can also rotate a domain to break its lineage (below).

Rotating domain D at policy revision r means:

1. Generate a new epoch key.
2. Write an epoch record with `epoch` = current + 1, `policy_revision` ≥ r and at least the previous record's, a recipient list of every active device authorized to read D at that revision (for a restricted domain, every eligible device), and the lineage of the previous key.
3. Wrap the new key for every recipient, with cards from the verified policy.
4. Commit the epoch bundle and the envelopes in one push or staged transaction ([§20](#20-chunked-staging)).

The device that removes another device commits the policy change first, then rotates each domain it can read, one domain per push or transaction. A domain it can't read (a restricted domain it doesn't belong to) stays pending until one of that domain's grantee devices rotates it. Any device that can read a pending domain may rotate it, at the pace [§26](#duties) sets; concurrent rotations race, and the server accepts the first epoch record for each epoch and rejects the rest with `epoch_conflict`.

A restricted domain's epoch 1 commits with its `create_domain`, from a device of the signer, whom the access list names as a manager. Clients reject an epoch 1 whose rotating device isn't eligible for the domain.

From revision r until the rotation commits, the server stops serving D's operations and keys to the removed devices and rejects writes to D with `rotation_pending`. A writer that gets `rotation_pending` and holds D's key rotates D at once. A device keeps at most one push in flight per workspace. Every push response carries the server's policy head revision ([§15](#push)), and a device fetches the policy chain and keys before its next push whenever a push response, a pull or the realtime channel named a revision above its accepted head. When it accepts a change that requires D to rotate, it re-seals every record in its outbox that it hasn't sent and that uses an epoch the change invalidated. A client that holds D's key rotates it before its next write to D. No device that has seen revision r encrypts under an epoch whose recipients include a device that no longer has access.

An operation is epoch-valid when its `epoch` is at least the first epoch of D whose record has `policy_revision` at or above the latest revision, at or below the operation's `policy_revision`, that required D to rotate; for a folder domain, D's epochs are the workspace domain's, and a pending workspace rotation holds its folder domains too. After the rotation commits, the server still accepts an operation under the previous epoch with `policy_revision` below r, but only in a push, never in a staged transaction, and only from a device to which it hasn't yet served revision r: such a record may have been signed or sent before its writer learned of the removal. That push's response tells the writer about r, and the writer fetches r before its next push, so an honest server lets through at most one push of such records per device after the new epoch's log entry.

A reader applies those records, and reports `sync_writer_stale` for a device when, after the log entry of an epoch e of D, that device's records under an epoch below e form more than one run of consecutive log entries, or one run of more than 1 MiB. Any other entry ends a run: another device's record, a record of the same device under e or later, a checkpoint, a purge or an epoch entry. Records of an older restore generation, which repair resubmits ([§14](#after-a-server-restore)), don't count. The check needs no checkpoint. A reader that learns e from the key route before its log entry requires that entry in D's log at or below the `head` of the first page it requests afterwards. It reports `sync_chain_gap` when its cursor passes that head without the entry, or when a record under e arrives before it. On `sync_writer_stale` the app alerts you, naming the device, the domain and the policy revision its records name, and asks an owner or admin to remove that device or stop syncing through this server. Owner and admin devices also list, for each device, the highest policy revision its records name.

**Cost.** A rotation writes one epoch record and one envelope per remaining authorized device, for the workspace domain and each restricted domain the removed device could read. Folder domains add nothing. Its size depends on devices and domains and never on objects. [§28](#28-worked-example-100000-objects) works through the numbers.

**What rotation protects.** A removed device keeps the keys and plaintext it already had. It can't decrypt anything encrypted under a later epoch. Lineage gives every holder of a current key every earlier key, so a device or member that loses access and later gets it back can read what was written while it was excluded, if it can get that ciphertext.

**Lineage break.** A device that grants access again to an account that lost it may break the lineage instead, if its account could sign that grant: the next epoch record carries an empty lineage, and that device re-seals every live object of the domain (for the workspace domain, of every folder domain too) under the new epoch with state operations covering their live operations, in as many transactions as that takes, before the grant commits. The account then reads nothing written while it was away, at a cost proportional to the domain's objects. Devices that held the earlier keys keep them. Without a break, the app warns whoever grants the access that the account will be able to read what was written while it was away.

### History floor

A domain key opens every earlier ciphertext in that domain through lineage, so the server decides how much history a new member sees. The server keeps a history floor per account and domain: it never serves operations with a sequence at or below the floor through pulls, chain fetches, `live` or `manifest` requests, and it serves a blob only when an operation that lists it sits above the floor or is live at the checkpoint the device reads. Bootstrap ([§18](#18-bootstrap)) still serves the objects that are current, because a member sees current content.

| Event | Floor |
|---|---|
| A member joins | `covered_seq` of the checkpoint committed with the approval |
| A grantee joins a restricted domain | `covered_seq` of the checkpoint committed with the grant |
| A member's new device | the member's existing floor |
| An admin re-enrolls a member who lost every device | workspace and folder domains: `covered_seq` of the checkpoint committed with the approval; restricted domains: `covered_seq` of the checkpoint committed with `admit_device` |
| A device restored from the member's recovery kit | the member's existing floor |

An approval or grant that sets a floor carries a fresh checkpoint for each affected domain, signed by a device that can write it, and the floor equals that checkpoint's `covered_seq`. Records that land between signing and commit sit above the floor, so the new device pulls them normally. The server answers a request below the floor with `before_history_floor`, and the client bootstraps.

History isolation for new members depends on the server. [§30](#30-threats) records that limit.

## 10. Approval

Approval pins a new device to devices you already trust, and the reverse, with one short code shown on both screens. The code mixes in a secret that the candidate commits to before it sees the offer, so a party in between can't steer the code.

### Approval request

The candidate enrolls and, for each attempt, draws a fresh 32-byte nonce `nc` and posts a request (`POST /v2/approval-requests`) carrying its commitment:

```text
commit = BLAKE3(tag(noura.sync.approval.commit) || candidate_card_digest || nc)
```

where `candidate_card_digest` is `BLAKE3(candidate card)`. For an invitation, the request also names the invitation and carries the invitation proof ([§11](#11-invitations)). Approvers see the card and the request through `GET /v2/approval-requests` for devices of their own account, or `GET /v2/workspaces/{w}/pending-devices` for invited accounts.

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

`anchor` is `id workspace | u64 revision | digest policy_head | list<checkpoint_anchor>`, and `checkpoint_anchor` is `id domain | u64 covered_seq | digest checkpoint`, sorted by domain: the approver's accepted policy head in each workspace the approval covers, and its latest accepted checkpoint in each domain the candidate will read. Approving your own new device lists every workspace your account belongs to. Approving an invited member's device lists one workspace.

### Approval code

```text
offer_digest  = BLAKE3(offer body || sig)
code_input    = tag(noura.sync.approval.code) || offer_digest || nc
approval_code = (u64 from the first 8 bytes of BLAKE3(code_input), big-endian) mod 100,000,000
```

Both devices show the approval code as 8 decimal digits, with leading zeros, in two groups of 4.

1. The candidate enrolls, posts its request and shows its own fingerprint.
2. The approving device shows the candidate's fingerprint, catches up, and sends the offer.
3. The candidate takes the first offer that names its commitment, checks it, then reveals `nc` for that offer alone (`POST /v2/approvals/{offer_digest}/reveal`). The approving device fetches `nc` and checks it against `candidate_commit`. Both devices show the code.
4. You compare the codes and confirm on both devices. A mismatch, or a cancel on either device, ends the attempt: both discard the offer and `nc`, and a new attempt starts from a new request.
5. The approving device commits the approval: `add_device` changes (with `offer` set to `offer_digest`) in each anchored workspace, the envelopes the candidate needs, and, for a new member, `add_member` and a fresh checkpoint per domain it can read so the member's floor has a checkpoint to bootstrap from.
6. The candidate pins the approver's card and anchors. It accepts the approval only from that card, and verifies each workspace's chain from genesis to its anchor, then onward.

Before revealing `nc`, the candidate checks that `candidate_card_digest` equals `BLAKE3` of its own card and `candidate_commit` equals its own commitment; otherwise it reports `sync_approval_mismatch` and reveals nothing. Each `nc` serves one offer and one comparison: the candidate never shows a code for any offer but the first. When a second offer names the same commitment before you confirm, whether it arrives before or after the reveal, the candidate ends the attempt with `sync_approval_mismatch`, discards `nc`, and shows no code; a new attempt needs a new request with a fresh `nc`. A second offer that arrives after you confirm changes nothing, and the candidate reports it. If two of your devices both answer a request, the app asks you to approve from one of them. An approving device sends at most one offer per request. After you confirm, the candidate verifies each workspace's chain up to its anchor and checks that the approver's card belongs to the policy at that anchor with the authority to sign the `add_device` that follows. It refuses to bootstrap a domain from a checkpoint below its checkpoint anchor (`sync_manifest_rollback`). For an invitation, the approver must be the inviting device: `BLAKE3(approver_card)` must equal the context's `inviter_fingerprint` ([§11](#11-invitations)). An offer expires 10 minutes after the approver sends it.

A server that swaps either card must show each device an offer or a commitment of its own choosing. The candidate's code depends on `nc`, which the server learns only after it has fixed the one offer the candidate will ever use with that `nc`. A server that learns `nc` and then grinds further offers toward the approver's code gains nothing, because the candidate ends the attempt at the second offer. The approver's code depends on an offer that already commits to the request, so the server can't change it after the approver signs. The two codes therefore match with probability 10^-8 per attempt, and every attempt asks you to confirm. Passing the approver's offer for a substitute card through to the real candidate fails the digest check.

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

Your device's approval request names the invitation and carries `invite_proof = keyed_hash(derive_key("noura.sync.v2 invitation proof", secret), card)`. The inviting device checks the proof before it sends an offer, so a server that never saw the secret can't put its own card in your place. The inviting device then approves your device with an approval code ([§10](#10-approval)), which you compare with the inviter over a channel you both trust, such as a call. Only that commit adds you to the policy and wraps keys for you. Your app checks that the approval offer comes from the card matching `inviter_fingerprint` and that the `add_member` change names this invitation. Only the inviting device can approve; if that device is lost, an owner or admin revokes the invitation and sends a new one. Revoking an invitation makes its link fail.

## 12. Recovery kit

A recovery kit is an offline device. Exporting a kit creates a recovery identity, a device card with its own signing and HPKE keys, and adds it with `add_device` (`device_kind` 2) to every workspace your account belongs to. When your account creates or joins a workspace, your device adds the recovery identity there, if it pinned that identity's card ([§7](#pinned-cards)), in the creation request or its first push after approval; otherwise another device of your account that pinned it adds it ([§8](#verifying-the-chain)). The app then asks you to export a new kit so the kit lists that workspace. Rotations wrap keys for the recovery identity like any other device, so the kit never goes stale. Removing the recovery identity from your device list revokes the kit.

A recovery identity can sign two kinds of change ([§8](#roles-and-authority)): `add_device` for one new device of its own account per workspace, and `remove_device` for devices of its own account. Clients reject any other change it signs.

The kit is a file you pick outside every workspace, written with mode 0600 on Unix, plus a secret the app generates: 20 random bytes, shown once as 32 characters of RFC 4648 base32 in 8 groups of 4, for you to write down. Readers ignore spaces, hyphens and letter case when you type it back. With 160 random bits, an offline guess at the secret is out of reach, and Argon2id adds cost to each guess.

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.recovery-kit)` | |
| salt | `id` | 16 random bytes |
| m_cost_kib | `u32` | 65,536 |
| t_cost | `u32` | 3 |
| parallelism | `u32` | 4 |
| *AEAD associated data ends here* | | |
| nonce | `nonce` | |
| ciphertext | `bytes` | XChaCha20-Poly1305 under `Argon2id(secret, salt, m, t, p)`, where `secret` is the 20 secret bytes; Argon2 version `0x13`, 32-byte output |

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

`kit_workspace` is `id workspace | digest genesis | u64 revision | digest head | list<checkpoint_anchor>`, sorted by workspace: the digest of the workspace's revision-1 policy change, the accepted policy head, and the latest accepted checkpoint per domain when the kit was written. `hpke_secret` is the 32-byte X-Wing seed. Readers reject parameters below the values above, or above m = 1,048,576 KiB, t = 10 and p = 16, and a wrong secret or tampered file reports `sync_recovery_kit_invalid`.

To recover after losing every device, you sign in on a new computer, enroll it, import the kit and type the secret. The new device:

1. Verifies the kit's card against its seeds.
2. Proves to the server that it holds the recovery identity's signing key. It signs a `recovery_proof` and posts it to `POST /v2/recovery`; for the rest of that session the server lets it read the envelopes addressed to the recovery identity and submit changes the recovery identity signs.
3. Checks that each listed workspace's chain starts at the stored genesis digest and extends the stored head (a shorter chain reports `sync_policy_rollback`), and refuses a checkpoint below the kit's checkpoint anchors.
4. Signs, as the recovery identity, `add_device` for itself in each workspace, unwraps the recovery identity's envelopes and wraps every key for itself.
5. In the same flow, removes the recovery identity, which rotates every domain, asks you to remove the lost devices, and exports a new kit with a new secret.

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
| counter | `u64` | position in this device's chain for this domain, from 1 |
| previous | `digest` | digest of the device's previous record in this domain; 32 zero bytes at counter 1 |
| epoch | `u64` | the epoch whose key encrypts the payload; for a folder domain, the workspace domain's epoch |
| policy_revision | `u64` | the policy head the device saw when it signed; at least the previous record's in this chain |
| policy_head | `digest` | digest of the change at `policy_revision` |
| previous_attempt | `digest` | digest of the attempt the server last rejected at this chain position, when this record re-seals the same change ([§14](#your-own-chain)); otherwise 32 zero bytes |
| generation | `u64` | the server's restore generation when the device signed ([§14](#after-a-server-restore)) |
| checkpoint_seq | `u64` | `covered_seq` of the latest checkpoint the device reached in this domain, from its log, its own receipt or bootstrap; 0 before the first |
| checkpoint | `digest` | digest of that checkpoint; 32 zero bytes before the first ([§17](#checkpoint-chain)) |
| rid | `id` | |
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
| 3 | text_snapshot | state | `str object_id`, `bytes path`, `digest revision`, `id text_lineage`, `bytes properties`, `content`, `opt<origin>` |
| 4 | text_update | patch | `str object_id`, `id text_lineage`, `content` holding a yrs update |
| 5 | properties | patch | `str object_id`, `id text_lineage`, `list<property_change>` |
| 6 | move | patch | `str object_id`, `id text_lineage`, `bytes expected_path`, `bytes path` |

- `object_id` is the managed file's frontmatter ID, or empty for an ordinary file.
- `path` is a workspace-relative UTF-8 path of at most 4,096 bytes, with `/` between components. Clients reject absolute paths, `.` and `..` components, empty components, control characters, Windows reserved names and characters, trailing dots or spaces, and the reserved roots `.noura` and `.git` in any ASCII case. Paths compare byte for byte; [§16](#16-conflicts) defines when two paths collide.
- `revision` is the BLAKE3 hash of the file's bytes as materialized; for a delete, of the deleted bytes, or zeros when unknown.
- `content` is `u8 form` then either form 1, `bytes inline`, or form 2, `blob_ref`.
- `blob_ref` is `id blob | u64 ciphertext_length | digest ciphertext_digest | key blob_key | u64 plaintext_length` ([§21](#21-attachments)).
- `origin` is `id domain | id rid`: in a state operation, where a cross-domain move came from; in a reason-2 delete, where it went.
- `text_lineage` names the yrs document of a text-mode note ([§22](#22-live-collaboration)).
- `property_change` is `str key | bytes expected | bytes value`, where each value is the RFC 8785 canonical JSON of the property's JSON-compatible value ([v1.md](v1.md#managed-markdown)) and an empty value means absent. Clients skip the expected-value check for `updated` and keep the later of the two RFC 3339 instants.

An inline payload holds at most 262,144 bytes before padding. Larger content goes in a blob.

### Supersession and live operations

The server and the client compute the same live set for each `rid` from cleartext fields: an operation is live unless another operation on the same `rid` covers it. The server needs no plaintext to compute it, compact the log, or check a manifest.

- A state operation covers every operation live in the writer's view of that `rid`, naming each device's highest live counter. A delete covers everything, so the tombstone stays live alone. Covers name only live operations, which the server never prunes ([§19](#19-compaction-and-purge)).
- A patch covers nothing. It applies to the text lineage it names ([§22](#22-live-collaboration)).

When a state operation would need more than 256 covers, it covers the 256 devices with the most live operations on that `rid`, and the rest stay live.

### Server checks on an operation

The server accepts an operation only when:

- the session's device is the signer, is active and isn't frozen in this domain (else `device_frozen`), and the signature verifies under its card;
- the device may write the domain at the current policy revision and at `policy_revision`, which is at most the current revision and at least the previous record's in this chain (else `policy_regressed`), `policy_head` equals the server's change at that revision (else `policy_conflict`), and the record isn't beyond a cut (else `beyond_cut`);
- `epoch` is the domain's current epoch and no rotation is pending, or the previous epoch under the condition in [§9](#rotation);
- `generation` is at most the server's current restore generation, and when it equals it and `checkpoint_seq` is at or above the retention floor, `checkpoint` names a checkpoint of this domain with that `covered_seq` (else `checkpoint_unknown`);
- `counter` is one above the server's head for this device and domain and `previous` equals that head's digest (else `chain_conflict` with the head);
- `rid` is new, or already belongs to this domain;
- every cover names an existing operation on this `rid` with exactly that device and counter (else `supersedes_unknown`);
- after this operation the `rid` holds at most 2,048 live patches with at most 8 MiB of ciphertext (else `snapshot_required`) and at most 64 live state operations (else `conflict_limit`);
- every blob is a completed upload created for this domain, or, when this is the workspace domain or a folder domain, for any of those domains (else `blob_incomplete` or `blob_domain_mismatch`);
- the ciphertext length is a valid padded length (else `padding_invalid`).

A retried upload of an operation the server already holds, with the same digest, returns the original receipt. The same position with a different digest returns `chain_conflict`.

### Client checks on an operation

Before decrypting, the client verifies the signature under a pinned card from its verified policy, checks `policy_head` against its own change at `policy_revision`, checks that the signer could write the domain at `policy_revision` and that `policy_revision` didn't decrease along the chain, rejects an operation beyond the signer's cut, checks that the operation is epoch-valid ([§9](#rotation)), runs the chain verifier ([§14](#14-device-chains)), and checks the checkpoint it names ([§17](#checkpoint-chain)). A failure reports `sync_writer_not_authorized`, `sync_policy_forked`, `sync_policy_regressed`, `sync_epoch_stale` or a chain error, changes no file, and holds the domain's cursor. An honest server rejects such a record, so the failure shows a lying server or a misused key.

A record that passes those checks but fails to decrypt, unpad, parse, match its header's `rid`, or pass the collaboration sandbox ([§22](#22-live-collaboration)) came from its signer's key, and the server couldn't see the problem. The client counts it as applied: its chain head, cursor, live set and manifest include it, and a later state operation can supersede it. The client keeps the object's last good state on disk, lists the record in the review store as an unreadable version attributed to its signer, and reports `sync_record_unreadable` with the device and domain. A record encrypted under a different epoch record for the same epoch fails to decrypt the same way ([§4](#records-signatures-and-digests)), and checkpoints then expose the split as `sync_key_mismatch` ([§17](#17-manifest-and-checkpoints)).

## 14. Device chains

Each device keeps one chain per domain it writes to: operations and purges ([§19](#19-compaction-and-purge)) share it. Every record carries the device's counter and the digest of its previous record under the signature, so a server can't remove, reorder or duplicate a record without breaking a link that the device signed.

Chains are per domain because a member outside a restricted domain never sees that domain's records. A single chain per device would show that member gaps it could never explain.

### Verifier

The client keeps, per domain and device, the accepted head `(counter, digest, sequence)` and the counter, digest and sequence of each accepted record above the domain's retention floor. Heads taken from a checkpoint bundle carry their sequences ([§17](#17-manifest-and-checkpoints)), so these checks work right after bootstrap. The client processes each pull page in sequence order. For a record with counter `c`, previous digest `p` and digest `h` at sequence `s`:

1. **Counter at or below the head.**
   - The client holds the record at `c` with digest `h` at sequence `s`: the page repeats a position the client already applied (for example after a restart), so the client ignores it.
   - The client holds the record at `c` with digest `h` at another sequence: `sync_chain_replayed`.
   - The client holds a different record at `c`, and one of the two names the other in `previous_attempt`, directly or through other attempts at `c` that the client has seen or the page carries: the two are attempts at one position ([§14](#your-own-chain)), and the one whose links reach the other is the later. If the client holds the later attempt, it ignores the earlier. If it holds the earlier attempt as its head, with no successor applied, it switches to the later attempt and recomputes the object's live set. If the earlier attempt has a successor that the client applied, it reports `sync_chain_forked`.
   - The client holds a different record at `c` and neither names the other, or two records at `c` name the same `previous_attempt`: `sync_chain_forked`. Attempts at one position form a single line, each naming the one before, and their number has no cap when newer epochs or policy changes force the re-seals ([§14](#your-own-chain)).
   - The client no longer holds a record at `c` (it bootstrapped past it, or the record sits below the retention floor): every legitimate record at or below the head has a sequence at or below the head's, so `s` above the head's sequence reports `sync_chain_replayed`.
2. **Counter one above the head.** If `p` equals the head's digest, accept and advance the head. Otherwise the client fetches position `c - 1`: if the server returns a later attempt of the head that rule 1 accepts, the client replaces the head with it and accepts this record when `p` matches that attempt. Otherwise: `sync_chain_forked`.
3. **Counter more than one above the head.** The page skipped records. The client fetches each missing position by chain position ([§26](#26-http-api)). A missing record that the server returns at a sequence above `s` reports `sync_chain_reordered`. A missing record at a sequence below `s`, or one the server can't return, reports `sync_chain_gap`. A `pruned` answer means the client's head fell below the retention floor, and the client bootstraps ([§18](#18-bootstrap)).

On any of these errors the client applies nothing from the page, changes no file, keeps its cursor and reports the error with the device and domain. It retries on the next pull, so a transient server fault clears itself.

A retried upload never triggers an error: the server stores one copy, and a page that repeats a position at the same sequence is ignored.

### Suffix withholding

A chain proves what the server served, never that it served everything. A client learns a device's latest counter from three sources: later records from that device, the chain heads in each checkpoint, which reach every puller as log entries ([§17](#17-manifest-and-checkpoints)), and, for its own chain, the receipts the server returned. A head above what the client holds, at a sequence the server claims to have delivered, reports `sync_chain_gap`. Checkpoints form a chain, and every record names the latest checkpoint its writer reached ([§17](#checkpoint-chain)). A server that hides a checkpoint from one client must therefore also hide every later checkpoint and every record whose writer reached it, and the split shows once one such record or checkpoint crosses to that client. A server that withholds a device's records from everyone, and every checkpoint mentioning them, looks like a device that went offline; the app shows when it last heard from each device.

### Your own chain

A device writes each counter to its journal and to its counter high-water mark in the credential store ([§15](#client-journal)) before signing, and persists each signed record before sending it, so a crash never reuses a counter and a retry sends the same bytes. On startup, and on `chain_conflict`, it compares its journal and high-water mark with the server's head for its own chain (`GET .../heads/{device}`) and with the heads in its latest accepted checkpoint:

- A server head above the journal, at or below the high-water mark, means the journal was restored from a backup. The device verifies those records carry its own signature and continues from the server's head.
- A server head below the high-water mark, below a record the server acknowledged, or below the head in the device's latest accepted checkpoint means the server lost or hides records. The device uploads the persisted records it still holds, unchanged, and never signs at or below its high-water mark. When it no longer holds them, it reports `sync_chain_gap` for its own chain and stops writing that domain, and the app offers to enroll the machine as a new device.
- A journal without a high-water mark in the credential store means the keys are new: the machine enrolls as a new device ([§7](#device-card)).

When the server rejects a record for a reason the device must fix (`epoch_stale`, `rotation_pending`, `supersedes_unknown`, `blob_incomplete`, `beyond_cut`, `policy_regressed`, `purge_invalid`), the device first checks the reason against data it can verify:

- for `epoch_stale` and `rotation_pending`, it fetches policy and keys, and re-seals only after accepting a newer epoch or a change that requires one;
- for `supersedes_unknown`, it fetches each covered operation by chain position and drops the covers the server can't return;
- for `blob_incomplete`, it reads the upload's offset;
- for `beyond_cut`, `policy_regressed` and `purge_invalid`, it rereads its accepted policy, its own chain and the published horizon.

When the check contradicts the server, the device doesn't re-seal: it retries the same bytes later and reports the server's error. Otherwise it re-seals the change at the same counter with `previous_attempt` set to the digest of the attempt the server rejected last at that position. A re-seal that follows a newer epoch or a rotation-requiring policy change the device accepted has no cap. Any other re-seal counts toward a cap of 3 per position. At the cap the device reports `sync_reseal_limit` for that domain, signs no other attempt at that position, and stops writing the domain: it retries the last attempt's bytes later, and its local files keep their changes. A rejected push names the failing record ([§26](#message-bodies)), and the device re-seals that record and every later record of the same chain in the batch, whose `previous` links change; each of those names the attempt it replaces at its own position. A server that kept a rejected attempt and serves it shows readers two consecutive attempts, which rule 1 above resolves without decryption. A device whose write access is gone drops the record; its local file keeps the change.

### After a server restore

An operator who restores the server from a backup raises `restore_generation`. The server reports the generation in `GET /v2/limits`, in every pull page and in a `restored` realtime frame ([§23](#realtime-channel)), and opens repair (`POST /v2/workspaces/{w}/repair`) for 14 days after the restore. Outside that window repair answers `repair_closed`.

Clients can't check the generation: it is the server's claim. A client that sees a new generation alerts you that the server reports a restore from a backup, then:

1. resubmits through repair, in revision order, the accepted policy changes the server lacks;
2. resubmits the epoch bundles it accepted and the envelopes addressed to it that the server lacks, in epoch order per domain;
3. resubmits, in chain order per device and domain, its own signed records and any other device's records it holds in full that the server lacks;
4. drops the sequences it stored, keeps counters and digests, and pulls each domain from the latest checkpoint at or below the server's head, matching the records it holds by digest. That checkpoint restarts the client's checkpoint chain for the domain ([§17](#checkpoint-chain)).

Repair runs every push check, with these changes:

- The server accepts a correctly signed record from any device the policy lists, whatever the session's device, and ignores freezes and pending rotations.
- It checks an operation's epoch with the epoch-validity rule in [§9](#rotation) at the operation's own `policy_revision`, instead of requiring the current epoch.
- When a policy change carries a cut above the server's head for that device and domain, the server records the cut as the head and keeps the range between its old head and the cut open: it accepts the missing records of that range through repair when they link in chain order and the last one matches the cut's digest.
- A record of an older generation skips the `checkpoint` check ([§13](#server-checks-on-an-operation)).
- The server assigns new sequences throughout.

After a generation change the client accepts the server's lower chain heads and checkpoints, and a lower policy head only while it resubmits the changes it holds above it. It never accepts a different policy change at a revision it accepted, a different record at a chain position it holds, or an epoch record that differs from one it accepted. For a removed device whose records the server lost, the client takes the device's latest cut as its head. When repair ends, the app lists, for each device and domain, the records the server lost that no device resubmitted.

The journal keeps the policy chain, epoch bundles and envelopes anyway ([§15](#client-journal)). A client also keeps its own signed records until a counted checkpoint covers them whose `counted` index sits at least 14 below the domain's latest ([§17](#checkpoint)), up to 64 MiB per workspace; past that cap it drops its oldest kept records first. A backup up to about two weeks old therefore loses nothing that a surviving device holds, unless that device wrote more than the cap since. Edits from a device that no longer exists survive only as files on other devices. When resubmission can't close a gap, the client reports `sync_chain_gap` or `sync_policy_rollback` as before, and an owner device can turn sync on for its folder as a new workspace.

## 15. Domain log, cursors and pulls

Each domain has its own log. The server assigns each accepted operation, purge, checkpoint and epoch entry ([§9](#epochs)) a sequence number, increasing within the domain and not necessarily dense. Per-domain sequences keep a member outside a restricted domain from counting its activity through gaps.

Records become visible in sequence order. The server assigns a domain's sequences while it holds that domain's head row locked inside the committing database transaction, so no record commits below a sequence a reader has already passed. Key sequences follow the same rule per workspace.

### Push

`POST /v2/workspaces/{w}/ops` carries `list<record_ref>` ([§20](#20-chunked-staging)), at most 1 MiB, possibly spanning domains: operations and purges, and also policy changes, epoch bundles, envelopes and checkpoint bundles. The server validates the records in order, as at a transaction commit, commits the batch atomically or rejects all of it, and returns its policy head revision and `list<receipt>`, where a receipt is `digest record_digest | id domain | u64 seq`, in request order. A device keeps at most one push in flight per workspace and fetches the policy chain before its next push when the returned revision is above its accepted head ([§9](#rotation)). A cross-domain move fits one push; a larger atomic change uses a staged transaction ([§20](#20-chunked-staging)).

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
| restore_generation | `u64` | [§14](#after-a-server-restore) |
| policy_revision | `u64` | the server's policy head revision ([§9](#rotation)) |
| duties | `u8` | duty bits for this device ([§26](#duties)): 1 sign a checkpoint, 2 purge, 4 rotate this domain, 8 fill missing envelopes, 16 the checkpoint due counts toward retention |
| entries | `list<entry>` | sorted by sequence |

`entry` is `u64 seq | u8 record_type | bytes record`, with record types 1 operation, 5 checkpoint bundle, 7 purge and 8 epoch record ([§9](#epochs)). A page holds at most 1 MiB. A cursor below the retention floor returns `cursor_compacted`, and the client bootstraps ([§18](#18-bootstrap)).

`POST /v2/workspaces/{w}/pull` takes `list<pull_cursor>`, each `id domain | u64 after`, and a wait of up to 25 seconds, and returns pages for the domains with records after their cursor, at most 1 MiB in all. It waits while none has any. A client pulls every domain it reads through this one request, so a device makes one pull per round however many domains exist.

The client advances a domain's cursor to `complete_through` only after every record in the page passes the checks in [§13](#client-checks-on-an-operation) and [§14](#14-device-chains) and every resulting file change reaches disk. A text-mode record that the collaboration sandbox is still decoding counts once the record itself reaches the journal on disk ([§22](#22-live-collaboration)). Policy changes and keys have their own cursors ([§26](#26-http-api)). The realtime channel ([§23](#23-presence-and-realtime-channel)) delivers small records and announces new heads.

The server never publishes a retention floor below one it published before, unless its restore generation changed ([§19](#19-compaction-and-purge)). A `retention_floor` below one the server reported before in the same generation reports `sync_manifest_rollback`. The `detail` of `cursor_compacted` carries the domain's current retention floor and the `covered_seq` of its latest checkpoint. The client reports `sync_manifest_rollback` when that floor is below one reported before in the same generation, or when its cursor isn't below that floor. Otherwise it bootstraps, and it requires the checkpoint it bootstraps from to have a `covered_seq` at or above both its cursor and its latest accepted checkpoint's, else `sync_manifest_rollback`. An honest server passes both checks, because the floor never passes the latest checkpoint.

### Client journal

The client keeps its sync state in ordinary files under `.noura/sync/<device-id>/`, never only in SQLite, and never uploads them. It ignores a journal named for another device, and locks its own while the app runs. The journal holds:

- the accepted policy chain and its head;
- epoch bundles and the envelopes addressed to this device, still wrapped;
- per domain: the cursor, the latest accepted checkpoint, the digest, `covered_seq` and sequence of each checkpoint it bootstrapped from or reached since, down to the retention floor ([§17](#checkpoint-chain)), the last retention floor the server reported, the chain heads and recent records the verifier needs ([§14](#verifier)), and the cuts;
- per domain, for every record above the latest verified checkpoint, its sequence, `rid`, chain position, digest and covers, and the same for each operation such a record covered, so the client can compute its live set at any later `covered_seq`;
- this device's counters, every signed record not yet acknowledged, persisted before sending, and the records it keeps for a server restore ([§14](#after-a-server-restore));
- the map from each `rid` to its local object, path, live operations and text-mode state;
- the domain roots ([§9](#domain-roots));
- the replica ID and the last restore generation it saw;
- the review store for conflicts, orphans, unreadable records and held files.

Each journal file carries a BLAKE3 keyed hash under a 32-byte journal key that the device keeps in the credential store, and the client refuses a file whose hash fails.

The credential store also holds a high-water mark per workspace: the accepted policy revision and head digest, the latest accepted epoch record and checkpoint of each domain, and this device's highest signed counter per domain. The client updates the mark before it acts on what the mark records. On startup it refuses a journal that sits below the mark and reports `sync_journal_rollback`. It then bootstraps ([§18](#18-bootstrap)), accepting no policy head, epoch or checkpoint below the mark and signing no counter at or below it. A server restore that raised the generation is the one exception for heads and checkpoints, as [§14](#after-a-server-restore) describes; it never excuses a fork of what the mark records.

A device syncs one folder per remote workspace. The app keeps a registry in its application data, outside every workspace, that maps each account and remote workspace to one folder and a random replica ID, which the journal also stores. A folder whose journal holds another replica ID, such as a copy you made in Finder or a folder another Mac opened through a cloud drive, doesn't sync until you connect it, and connecting it discards its journal and bootstraps.

Device signing and HPKE secrets live only in the operating system's credential store. Epoch keys exist only in memory: the client unwraps them from the stored envelopes with the device's HPKE secret.

Within one scan, a deleted ordinary file and a created one with the same content revision, and the same operating system file ID where the platform reports one, count as a move: the client writes a state operation on the same `rid` with the new path and reuses the `blob_ref`. Two files that share one frontmatter ID don't sync as that object until you resolve the local `identity_conflict` ([v1.md](v1.md#classification)).

If the journal is lost, the client bootstraps again ([§18](#18-bootstrap)) and matches local files to objects: managed files by frontmatter ID first, ordinary files by content revision, then by path. A matched file whose revision differs from the live state counts as a local edit and forms a conflict branch. The client takes its own counters from its high-water mark.

## 16. Conflicts

The client derives an object's state from its live set:

1. One live state operation: that state, plus the live patches that apply to it. Text updates of its text lineage merge in any order ([§22](#22-live-collaboration)); properties and move patches apply in sequence order, which checkpoints commit to through the sequence in each leaf ref ([§17](#17-manifest-and-checkpoints)).
2. Two or more live state operations: a conflict, and each one is a branch, except live text snapshots of one text lineage, which merge ([§22](#22-live-collaboration)).
3. Properties and move patches whose text lineage isn't live: orphans, kept for review. Text updates whose lineage isn't live go to review as text.

Sync keeps both versions:

- The device whose local file matches a branch, or has local edits, keeps its file untouched.
- Every other branch goes to the local review store under `.noura/sync/<device-id>/conflicts/`, which never uploads.
- A device that holds neither materializes the branch with the lowest operation digest and stores the rest for review.
- Status counts the conflict until you choose.

Choosing writes a state operation that covers every branch.

Clients resolve these cases without asking:

- **Identical branches.** Branches whose decrypted path and revision match aren't a conflict.
- **Disjoint changes.** When one branch changed only the path and another only the content, relative to the common base operation the covers name, the client materializes the merged state. Any writer device that holds the base writes a merge operation covering both. Two devices writing the same merge produce identical branches, which the previous rule collapses.
- **Edit against delete.** The edit stays on disk and the conflict asks whether to delete it.

Two objects with different `rid`s collide when their paths are equal after NFC normalization and Unicode simple case folding, or when one object's path equals a folder prefix of the other's. A valid domain marker never yields. Otherwise the object whose current state operation has the higher digest yields: its author's device moves it to the first free name `name (n).ext`, with n counting up from 2, with a normal operation, and other devices show it at that name without writing until the move arrives.

A property patch whose `expected` value doesn't match the current value is kept for review with the competing value, as text conflicts are.

## 17. Manifest and checkpoints

A domain's manifest is a Merkle tree over its live operations. Its root proves that a client holds every current object, and a new device bootstraps from it.

### Leaves and tree

Each `rid` with at least one live operation has a leaf:

```text
leaf_hash = BLAKE3(tag(noura.sync.manifest.leaf) || rid || list<ref>)
ref       = id device || u64 counter || u64 seq || digest op_digest    (sorted by device, then counter)
```

Each ref carries the sequence the server assigned, so the root also commits to the order in which clients apply patches ([§16](#16-conflicts)).

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
| covered_seq | `u64` | the manifest covers every record through this sequence; above the sequence of the previous checkpoint's own entry, and at or above the retention floor |
| counted | `u64` | the number of counted checkpoints in the domain through this one: the previous checkpoint's value, plus 1 when this one counts ([§19](#19-compaction-and-purge)) |
| heads_floor | `u64` | the retention floor as the signer last read it; at least the previous checkpoint's, at most `covered_seq` |
| policy_revision | `u64` | the signer's accepted policy head when it signs |
| policy_head | `digest` | digest of the change at `policy_revision` |
| purge_horizon | `u64` | the server's purge horizon as the signer last read it, at most `covered_seq` |
| live_objects | `u64` | number of leaves |
| manifest_root | `digest` | |
| heads_digest | `digest` | `BLAKE3(tag(noura.sync.manifest.heads) \|\| list<ref>)` over the heads below |
| epoch | `u64` | the domain's latest epoch the signer had accepted at `covered_seq`; for a folder domain, the workspace domain's |
| epoch_record | `digest` | digest of that epoch record |
| signer | `id` | |
| sig | `sig` | |

The heads are the chain head, as a `ref` with its sequence, of every device with at least one accepted record in the domain at `covered_seq`, sorted by device. A device whose write access there ended with a cut at or below the checkpoint's `policy_revision`, with no later grant, leaves the list once its head's sequence sits at or below `heads_floor`. Its cut in the policy chain already fixes its last position, and a client that needs that head takes it from the cut, with `heads_floor` as its sequence. Signer and readers derive the list from the same signed `heads_floor`, so they agree on it.

A `checkpoint_bundle` is `bytes checkpoint | list<ref> heads`. The server checks:

- the signer's write access and `policy_head`;
- that `previous` names the domain's latest checkpoint, else `checkpoint_stale`;
- that `covered_seq` sits above that checkpoint's entry sequence and at or above the retention floor;
- that `counted` is the previous value plus 1 exactly when the server asked for a counted checkpoint;
- that `heads_floor` lies between the previous checkpoint's and the current retention floor, and `purge_horizon` between the previous checkpoint's and the current horizon.

It recomputes the root at `covered_seq` from the bucket hashes it stored for the previous checkpoint plus the records between the two, outside the write path, and rejects a mismatch with `manifest_mismatch`, so an honest server and an honest writer always agree. An accepted checkpoint becomes a log entry with its own sequence.

The server asks for a checkpoint by setting the checkpoint duty for one writer ([§26](#duties)), with bit 16 when the checkpoint counts: after 10,000 records since the last checkpoint, once a day for each domain with records after its last counted checkpoint, and when the latest checkpoint's signer loses write access. Only the daily checkpoint counts toward retention ([§19](#19-compaction-and-purge)), so a domain gets at most one counted checkpoint a day. Checkpoints committed with an approval, grant or `admit_device` don't count, and the server rejects any other checkpoint with `checkpoint_not_due`.

### Checkpoint chain

Each checkpoint names the one before it in its domain, and every operation and purge names the latest checkpoint its writer reached (`checkpoint_seq` and `checkpoint`, [§13](#operation-record)). A server that leaves a checkpoint out of one client's log therefore can't hide it once a later checkpoint, or a record whose writer reached it, crosses to that client. The entry of the checkpoint a client bootstrapped from reaches it again in its log, and the client skips it. A client fetches a checkpoint by digest (`GET .../checkpoints/{digest}`) in two cases:

- a checkpoint entry in its log whose `previous` isn't the last checkpoint the client reached or bootstrapped from in that domain;
- a record of the current restore generation whose `checkpoint_seq` sits at or above both the retention floor and the `covered_seq` of the checkpoint the client bootstrapped from in this generation (0 if it pulled from the start), when the client hasn't reached `checkpoint`.

The client fetches the named checkpoint, then each one its `previous` names, back to the last checkpoint it reached. If the server can't serve one, or the walk passes the client's last checkpoint without meeting it, the client reports `sync_chain_gap` for the domain. Otherwise the client checks the fetched checkpoints as log entries, oldest first, as if its pages had carried them. An honest server never causes a fetch. A record gets its sequence after the checkpoint its writer reached, so a client that pulls in sequence order reaches that checkpoint first. A writer that lags names a checkpoint the client already reached, or one below the client's bootstrap point or the floor, which needs no check.

### Checking a checkpoint

A client checks each checkpoint it reaches in its log after it has applied every record through `covered_seq`, and accepts it when:

- `BLAKE3(tag(noura.sync.manifest.heads) || heads)` equals `heads_digest`, and every head's sequence is at or below `covered_seq`;
- `previous` names the last checkpoint the client reached in that domain;
- its signer could write the domain at the checkpoint's `policy_revision`, which is at least the previous checkpoint's;
- `policy_head` matches the client's change at `policy_revision`, and the client waits when that revision is above its head;
- the client holds an accepted epoch record for `epoch` whose digest equals `epoch_record`, else `sync_key_mismatch`;
- `counted` equals the previous checkpoint's value or one more, and `heads_floor` is at least the previous checkpoint's;
- `covered_seq` lies above that of the latest checkpoint the client accepted, each device listed in both has a head there at or above its earlier head, and each device listed only in the earlier one may leave under the heads rule above, else `sync_manifest_rollback`.

The client then computes its live set and chain heads at `covered_seq` from its journal ([§15](#client-journal)) and compares them with the checkpoint. A root mismatch reports `sync_manifest_mismatch`, and the client walks the tree to find which objects differ. A checkpoint head above the client's own reports `sync_chain_gap`, and one below it reports `sync_manifest_mismatch`.

A checkpoint in the log that fails a check is reported and never becomes the client's latest accepted checkpoint, an approval or kit anchor, or a source of state. It doesn't hold the cursor: each record around it passed its own chain checks, and the next checkpoint still names it as `previous`.

A checkpoint a device bootstraps from ([§18](#18-bootstrap)) passes the checks above that need no local history: the heads digest, `policy_head` and the epoch record, with `covered_seq` at or above the latest checkpoint the device accepted and its anchors. It must also pass one more: its signer can write the domain in the client's current verified policy and has no cut there after its latest grant, so a removed device can't seed a new device's state.

## 18. Bootstrap

A new device, or one whose cursor fell below the retention floor, loads current state from the latest checkpoint instead of history:

1. Fetch and verify the policy chain from the approval anchor, and the current epoch bundle and envelopes of each workspace or restricted domain it reads. Fetch older epoch records only when a live operation names an older epoch, and walk back to them through `previous` digests and lineage.
2. For each domain, fetch the latest checkpoint bundle, which leases that checkpoint to the device for 24 hours, renewed by each `live` request. Check it as in [§17](#checking-a-checkpoint), and check its `covered_seq` against the device's history floor, against the checkpoint anchors of its approval or kit, and against the latest checkpoint the device accepted (`sync_manifest_rollback`).
3. Fetch the 4,096 bucket hashes at `covered_seq` in one request and check them against `manifest_root`.
4. For a folder or restricted domain, apply the marker first. Fetch the leaf refs of the bucket that holds `marker_rid` (level 3 of `manifest`), check them against that bucket's hash, fetch the marker's live operations (`live` with `rid`), check their digests against the refs, and apply them. Record the domain root when the marker is valid ([§9](#domain-roots)); otherwise the client records held roots as it writes the domain's files.
5. Fetch the operations live at `covered_seq` in pages ordered by `rid`, device and counter. Run the checks in [§13](#client-checks-on-an-operation) on each operation, except the chain verifier, and compute the leaves. As each bucket completes, check its hash, decrypt and materialize its files, and record the checkpoint and the next bucket in the journal, so a restart resumes at that bucket. A bucket that fails its hash, or a `live_objects` count that fails after the last bucket, reports `sync_manifest_mismatch` and installs nothing more.
6. Record the chain heads from the bundle, with their sequences, and the checkpoint as the start of the domain's checkpoint chain ([§17](#checkpoint-chain)).
7. Pull from `covered_seq` with the chain verifier starting at those heads.

A checkpoint whose lease lapsed returns `checkpoint_expired`, and the device starts again from the latest checkpoint.

A returning device compares its journal with the manifest instead of replacing files. It never deletes a local file because the manifest lacks its object: it lists those files once for review ("deleted on another device while this one was away") and deletes them only when you confirm. A changed local copy uploads under a new `rid` unless you choose to delete it.

## 19. Compaction and purge

The server deletes operations no client needs.

Retention counts the server's daily counted checkpoints instead of dates, so the server needs no table that maps sequences to dates. A domain gets at most one counted checkpoint a day, and only on days it changes, so each window below lasts at least as many days as it counts.

**Retention floor.** For each domain, the retention floor is the higher of two values: the `covered_seq` of the 90th most recent counted checkpoint (0 while fewer exist), and the lower of the lowest cursor acknowledged by any retaining device and the `covered_seq` of the 30th most recent counted checkpoint (0 while fewer than 30 exist). A device acknowledges a cursor by pulling after it, and the server keeps the highest cursor each device acknowledged. A device is retaining while its acknowledged cursor has advanced since the 90th most recent counted checkpoint; while fewer than 90 exist, every device is retaining. The server persists, per domain, the highest retention floor it has published, and never publishes a lower one unless its restore generation changes ([§14](#after-a-server-restore)). It may therefore hold acknowledged cursors in memory and persist them later: a lost update lowers only the floor it computes, which delays pruning, while the published floor stays where it was. A domain without any checkpoint keeps every record, and a new device bootstraps it by pulling from sequence 0 with empty chain heads.

**Pruning.** The server may delete an operation whose sequence is at or below the retention floor when another operation whose sequence is also at or below the floor covers it. Every retaining device has seen both, so no writer can still name the deleted operation in a cover. The server deletes all such operations in one transaction, so a covering operation never disappears before the operations it covers, and the live set at the latest checkpoint stays complete. The server keeps one chain-head row per device and domain, holding counter, digest and sequence, which pruning never deletes while the domain exists. A fetch of a pruned chain position returns `pruned`. The server deletes a checkpoint entry, like a purge record, once its sequence falls below the floor, and keeps the bundles of retained counted checkpoints.

**Purge.** A delete operation stays live as a tombstone until a writer purges it. The server publishes a `purge_horizon`: the lower of the retention floor and the `covered_seq` of the 90th most recent counted checkpoint, or 0 while fewer exist. A retaining device that hasn't pulled a tombstone yet holds the floor below it, and a device that lags behind the 90th counted checkpoint bootstraps, so a purge never removes a deletion a device still needs. A writer device lists objects whose only live operation is a delete at or below the horizon and signs a purge:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.purge)` | |
| workspace | `id` | |
| domain | `id` | |
| device | `id` | |
| counter | `u64` | in the device's chain for this domain |
| previous | `digest` | |
| policy_revision | `u64` | |
| policy_head | `digest` | |
| previous_attempt | `digest` | as in an operation |
| generation | `u64` | as in an operation |
| checkpoint_seq | `u64` | as in an operation |
| checkpoint | `digest` | as in an operation |
| horizon | `u64` | at most the server's `purge_horizon` |
| rids | `list<id>` | sorted, at most 10,000 |
| sig | `sig` | |

The server checks the signer, chain and checkpoint reference as for an operation, and that every listed `rid` belongs to the domain and has all its live operations at or below the horizon (else `purge_invalid`). The listed `rid`s then leave the live set. The server deletes their operations once neither the latest checkpoint nor a leased one ([§18](#18-bootstrap)) has them live. A client that holds a listed object with a live state other than a tombstone reports `sync_purge_live_object` and uploads its copy under a new `rid`. A purge marks each listed `rid`'s tombstone, whose cleartext fields the server already holds. The server therefore learns which operation deleted each object and from which device, and through counted checkpoints about which day, at least 90 counted checkpoints after the delete.

The server deletes a purge record once its sequence falls below the retention floor; readers above the floor have seen it, and readers below it bootstrap.

**Quota.** A workspace over its storage quota frees space only as counted checkpoints pass: a covered operation leaves once the retention floor passes it, after 30 to 90 counted checkpoints; a tombstone leaves through a purge, after at least 90; and an unreferenced blob leaves once the backup retention passes. While it's over quota, the server still accepts the records that free space ([§20](#20-chunked-staging)).

## 20. Chunked staging

A change too large for one request commits atomically through a staged transaction: approvals into many domains, rotations of large domains, and moves of many objects into a restricted domain.

1. `POST /v2/workspaces/{w}/txns` opens transaction `t` (a random ID) with its part count.
2. `PUT /v2/workspaces/{w}/txns/{t}/parts/{i}` uploads part `i`, numbered from 0, at most 1 MiB. A part is `list<record_ref>`, where `record_ref` is `u8 record_type | bytes record` with types 1 operation, 2 policy change, 3 epoch bundle, 4 envelope, 5 checkpoint bundle, 6 profile and 7 purge. Re-uploading a part with the same bytes succeeds; different bytes fail. The server decodes each part and checks its signatures, padding and ciphertext lengths when the part arrives, outside the database writer.
3. `POST /v2/workspaces/{w}/txns/{t}/commit` carries the signed descriptor. The server answers `202 Accepted`, and the client reads the outcome with `GET /v2/workspaces/{w}/txns/{t}`.

`txn`:

| Field | Type | Notes |
|---|---|---|
| tag | `tag(noura.sync.txn)` | |
| workspace | `id` | |
| txn | `id` | |
| device | `id` | |
| policy_revision | `u64` | the policy head the device saw |
| policy_head | `digest` | digest of the change at `policy_revision` |
| part_count | `u32` | at most 64 |
| parts_root | `digest` | `BLAKE3(tag(noura.sync.txn.parts) \|\| list<digest>)` over each part's `BLAKE3(part bytes)`, in part order |
| sig | `sig` | |

At commit the server checks the signature, `policy_head`, and that every part is present and matches `parts_root`, then validates every record in order against the current state plus the records before it, and commits all of them in one database transaction. Any failure rolls everything back and records the first failing record's index and error. Committing a committed transaction returns the same result. The server keeps a committed transaction's outcome, its state and receipts, until 30 counted checkpoints of the workspace domain pass, then deletes it. A transaction holds at most 25,000 records. A larger move splits into ordered transactions, each atomic on its own.

A byte-identical policy change, epoch bundle or envelope that the server already holds returns success with its original receipt, as operations do, so a device that lost a response can tell its own commit from someone else's.

Staged parts expire after 24 hours unused and count against the workspace quota until then. Policy changes, epoch bundles, envelopes, and the transactions that carry them don't count against the quota, so a workspace over quota can still remove a device. While a workspace is over quota, the server also accepts checkpoints, purges, and operations that list no blob, cover every live operation of their `rid` and carry a payload of the minimum padded length, 256 bytes. A delete has that shape, so you can delete files to free space ([§19](#19-compaction-and-purge)). The server rejects other records with `quota_exceeded`.

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

`ciphertext_digest` is `BLAKE3(tag(noura.sync.blob.parts) || list<digest>)` over the BLAKE3 hash of each 1 MiB segment of the ciphertext, in order; the last segment may be shorter. Any server instance can then check each segment as it arrives, and the final check costs one hash per segment.

The receiver hashes each segment as it downloads, checks the segment hashes against `ciphertext_digest` before decrypting, writes the plaintext to a temporary file, checks `plaintext_length` and the file `revision`, and only then replaces the destination.

### Upload over a tus subset

Uploads use tus 1.0.0 core with the `creation`, `termination` and `expiration` extensions. The server advertises `Tus-Version: 1.0.0`, `Tus-Extension: creation,termination,expiration` and `Tus-Max-Size`.

| Request | Purpose |
|---|---|
| `POST /v2/workspaces/{w}/blobs` with `Upload-Length` and `Upload-Metadata: blob <base64>,digest <base64>,domain <base64>` (standard base64, as tus requires) | create; the device must be able to write that domain. Operations of that domain may list the blob, and so may operations of the workspace domain and every folder domain when the blob was created for one of those (else `blob_domain_mismatch`) |
| `HEAD /v2/workspaces/{w}/blobs/{id}` | read `Upload-Offset` to resume |
| `PATCH /v2/workspaces/{w}/blobs/{id}` with `Upload-Offset` and `Content-Type: application/offset+octet-stream` | append one segment: exactly 1 MiB at an offset that is a multiple of 1 MiB, or the rest of the upload |
| `DELETE /v2/workspaces/{w}/blobs/{id}` | abandon an incomplete upload |

The server stores each segment as its own object, records its hash, and flushes it to durable storage before it reports the new offset. When the offset reaches `Upload-Length`, it computes `ciphertext_digest` from the stored segment hashes, and on a mismatch discards the upload and answers `blob_digest_mismatch`. Incomplete uploads expire after 24 hours (`Upload-Expires`). A completed blob carries no timestamp: a filesystem store sets a fixed modification time on its files, and an S3-compatible store's `LastModified` dates the upload ([§29](#29-what-the-server-stores)).

`GET /v2/workspaces/{w}/blobs/{id}` with `Range` returns at most 1 MiB per response. The server serves a blob to a device whose account can read a domain holding a retained operation that lists the blob, when that operation sits above the account's history floor or is live at a checkpoint the device reads, and to its uploader before that operation exists. The workspace domain and the folder domains share one audience, so a blob created for one of them can serve operations in any of them without widening who reads it. Once no retained operation lists a blob, the server records the number of counted checkpoints of the domain it was created for at that moment, and it deletes the blob when enough further counted checkpoints have passed to exceed its backup retention.

## 22. Live collaboration

A Markdown note enters text mode when a device writes a `text_snapshot` for it with a new random text lineage ID. Text mode keeps the note's body in a yrs document whose root text is named `content` (UTF-16 offsets, update format v1). For a managed note it keeps the frontmatter block in `properties`, the exact frontmatter bytes including delimiters and their line ending. An unmanaged note keeps its whole file, frontmatter included, in `content` and leaves `properties` empty. The file's bytes are `properties` followed by the text of `content`.

- The editor batches local edits into at most one `text_update` every 500 ms while you type.
- A frontmatter edit to a managed note sends a `properties` patch with the expected and new value of each changed key; the workspace-format serializer rewrites the block.
- A rename sends a `move` patch with the expected and new path.
- An outside edit to the file becomes a `text_update` computed from the difference, as the editor treats any outside edit. An outside edit that leaves a managed note's frontmatter unparseable becomes a `file` operation instead, which ends text mode.
- A `file` operation that covers every live operation takes the note out of text mode. Clients do this for files that turn binary or grow past 8 MiB of text.

**State.** The note's text is the merge of every live `text_snapshot` and every live `text_update` of its text lineage, applied in any order, because yrs updates commute. Two live snapshots of one lineage merge without a conflict prompt. Properties and move patches apply on that state in sequence order ([§16](#16-conflicts)).

**Snapshots.** A device writes a new snapshot, the encoded state of the yrs document with garbage collection on, when the note's live patches exceed 256 records or the larger of 1 MiB and a quarter of the last snapshot's encoded size, at most once a minute per note. The device with the lowest device ID among the note's recent patch writers writes it; another device writes it only when the live patches pass twice that threshold. The snapshot covers each contributing device's latest counter, so patches it didn't include stay live and still apply. A reader that already holds every covered patch, and whose text matches the snapshot's `revision`, skips downloading the snapshot's blob.

**Rebase.** When the encoded document reaches 32 MiB, a device writes a snapshot with a new text lineage, encoded from the current text alone. Text updates of the old lineage that arrive afterward go to review as text.

**The document on a device.** Each device holds one yrs document per open note, inside the collaboration sandbox. The editor sends local edits into it as transactions and receives encoded updates and text changes back; it never inserts remote text again as a local edit. Relative positions for presence encode and decode inside the sandbox, and undo tracks only transactions of local origin. A device draws a fresh random 53-bit yrs client ID each time it loads a document. The journal keeps each text-mode note's live snapshot and patches, and the device rebuilds the document from them after a restart.

**Sandbox limits.** Remote updates pass through the collaboration sandbox before they reach the editor or the file. The sandbox meters decoding with Wasmtime fuel, 10,000,000,000 units per update at the pinned Wasmtime version, and 512 MiB of linear memory, so every device reaches the same verdict on the same update, however fast it runs. No clock takes part in the verdict or in applying the record. A device counts a `text_snapshot` or `text_update` as applied for its chain head, cursor, live set and manifest once the record passes the checks in [§13](#client-checks-on-an-operation) and reaches the journal, and decodes it in the background. Until decoding ends, the note shows as pending and opens read-only, and its file keeps its last decoded state. An update over the fuel or memory limit is unreadable ([§13](#client-checks-on-an-operation)). Continuous integration measures how long the fuel budget lasts under Pulley on the slowest supported machine, and the budget drops when that passes 60 seconds. A snapshot or update larger than the inline limit travels as a blob. Text is limited to 8 MiB and an encoded yrs document to 32 MiB.

Edits made offline merge on reconnect: every live patch of the lineage applies whatever snapshot is current, and a snapshot never discards an update it didn't cover.

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
| 6 records | server to client | `id domain`, `u64 previous_seq`, `list<entry>`: records that fit one frame, where `previous_seq` is the sequence of the domain's record before the first entry |
| 7 push | client to server | `u32 request`, `list<record_ref>`: a push ([§15](#push)) that fits one frame |
| 8 push_result | server to client | `u32 request`, `opt<error>`, `u64 policy_revision`, `list<receipt>` |
| 9 subscribe | client to server | `list<id>`: the presence channels of the notes this device has open |
| 10 restored | server to client | `u64 restore_generation`, sent when the channel opens and whenever the generation changes ([§14](#after-a-server-restore)) |

The server sends notify and records frames only to devices that can read the domain. It sends a records frame in place of notify when the new records fit one, and at most one notify per domain every 250 ms on a connection. It sends these frames after the push commits, never from inside the database transaction. A client applies a records frame when its cursor lies between `previous_seq` and the first entry's sequence, through the same verifier as a pull, and pulls otherwise.

Notifications and records frames only feed verified pulls, so a relay that forges or drops them can delay sync but can't change what the client accepts. The app sends a WebSocket ping every 20 seconds and reconnects with jittered backoff between 1 and 30 seconds. While the channel is up, the app still makes one workspace pull ([§15](#pull)) a minute; while it's down, the app long-polls that pull.

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
| selections | `list<selection>`, at most 8, each `bytes anchor`, `bytes head` (yrs relative positions, at most 256 bytes each) |

The server checks the signer's device, its read access to the domain and the current epoch, then forwards the record to the other connected devices that can read the domain and subscribed to its channel. It never stores presence. A device sends at most 2 presence records a second, on the 500 ms editing tick, and only when its selection changed. A device draws a new session ID for each connection. Receivers reject a record whose epoch isn't current, whose `minute` lies more than 2 minutes from their clock, or whose sequence is at or below the last one seen for that device and session during the epoch. They drop a session's cursors 30 seconds after its last record or when the server sends `presence_gone`.

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

`share_box`, which lets owner and admin devices list and update links:

| Field | Type |
|---|---|
| tag | `tag(noura.sync.share.box)` |
| workspace | `id` |
| domain | `id` |
| epoch | `u64` |
| token_hash | `digest` |
| *AEAD associated data ends here; the epoch record's digest follows ([§4](#records-signatures-and-digests))* | |
| nonce | `nonce` |
| ciphertext | `bytes` |

The box uses the share box subkey of the shared note's domain at `epoch`. Its padded plaintext is `tag(noura.sync.share.box.payload) | id token | key link_key | key link_seed | id rid`, so only members who can read the note recover the link. Those members could sign a snapshot with the seed, and the server accepts an update only from an owner or admin session. The server stores the token hash, workspace, domain, expiry (1 to 30 days, 30 by default), the snapshot and the box. A workspace holds at most 1,000 active share links.

Update link replaces the snapshot under the same token and keys while the domain's current epoch equals the box's `epoch`. After a rotation, members who lost access may still hold the old link key, so Update link revokes the old link and creates a new one with a new token and keys, and the app shows the new URL. Revoking or expiring a link deletes its snapshot and box, and the viewer then shows a not-found page.

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

The ciphertext uses the profile subkey of the workspace domain's `epoch` key. Its padded plaintext is `tag(noura.sync.profile.payload) | str name`. Clients accept a profile only when its signer is an owner or admin device in their current verified policy and `policy_head` matches their change at `policy_revision`, and report `sync_profile_rollback` for a `profile_revision` below the last one they accepted.

## 26. HTTP API

Request and response bodies use the encoding in [§4](#4-encoding) with media type `application/vnd.noura.sync.v2`, except the tus requests in [§21](#21-attachments). Every route requires `Authorization: Bearer <session token>` except `GET /v2/limits`, `POST /v2/device/authorize`, `POST /v2/device/token`, `GET /invite/{token}`, `GET /v2/invitations/{token}`, `GET /s/{token}` and `GET /v2/shares/{token}`. Errors return an HTTP status and the body `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. Messages never include database or operating system details, which stay in the server's local log.

Every route that creates something under a client-chosen ID (a workspace, transaction, blob, invitation, share link or approval offer) returns the original result when a client retries it with identical bytes, and `id_conflict` when the bytes differ.

| Route | Purpose |
|---|---|
| `GET /v2/limits` | protocol version (2), restore generation, and the limits in [§27](#27-limits) |
| `POST /v2/device/authorize`, `POST /v2/device/token` | device authorization grant |
| `POST /v2/devices/challenge`, `POST /v2/devices` | enrollment challenge; enroll with an `enroll` record |
| `GET /v2/devices`, `DELETE /v2/devices/{device}` | list the account's devices and sessions; end a device's sessions |
| `POST /v2/approval-requests`, `GET /v2/approval-requests` | post a candidate's request; list requests of the account's own new devices |
| `GET /v2/workspaces/{w}/pending-devices` | requests of invited accounts awaiting approval in this workspace |
| `POST /v2/approvals`, `GET /v2/approvals` | send an `offer`; offers addressed to the calling device's card |
| `POST`, `GET /v2/approvals/{offer_digest}/reveal` | reveal the candidate's nonce; read it |
| `POST /v2/recovery` | prove possession of a recovery identity ([§12](#12-recovery-kit)) |
| `GET /v2/workspaces` | workspaces the account belongs to, with role and latest profile record |
| `POST /v2/workspaces` | create a workspace: genesis, the workspace domain's first epoch, envelopes and profile in one request |
| `GET /v2/workspaces/{w}/policy?after={revision}` | policy changes after a revision, with the head revision |
| `POST /v2/workspaces/{w}/freeze` | freeze devices before a change that removes their write access, from a device that may sign that change ([§8](#policy-change-record)) |
| `GET /v2/workspaces/{w}/keys?after={key_seq}&recipient={device}` | epoch bundles of readable domains and envelopes addressed to this device, or to a recovery identity this session proved, ordered by the server's key sequence |
| `GET /v2/workspaces/{w}/domains/{d}/epochs/{epoch}` | one epoch bundle, for walking lineage |
| `GET /v2/workspaces/{w}/keys/missing` | for the device holding that duty, eligible devices lacking an envelope for the current epoch |
| `POST /v2/workspaces/{w}/envelopes` | upload envelopes outside a transaction |
| `POST /v2/workspaces/{w}/ops` | push ([§15](#push)) |
| `POST /v2/workspaces/{w}/pull` | pull every listed domain in one request ([§15](#pull)) |
| `GET /v2/workspaces/{w}/domains/{d}/ops?after={seq}&wait={seconds}` | pull one domain ([§15](#pull)) |
| `GET /v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}` | one record by chain position, as an `entry` |
| `GET /v2/workspaces/{w}/domains/{d}/heads/{device}` | the server's chain head for a device |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoint` | latest checkpoint bundle, leased to the caller for 24 hours |
| `GET /v2/workspaces/{w}/domains/{d}/checkpoints/{digest}` | one checkpoint by digest, as an `entry` ([§17](#checkpoint-chain)) |
| `POST /v2/workspaces/{w}/domains/{d}/checkpoints` | submit a checkpoint bundle |
| `GET /v2/workspaces/{w}/domains/{d}/live?at={seq}&after={cursor}` or `?at={seq}&rid={rid}` | bootstrap page: operations live at `at`, ordered by `rid`, device and counter, or those of one `rid` |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&level={0..3}&index={j}&after={cursor}` | 16 child hashes of a node, or a bucket's leaf refs at level 3 |
| `GET /v2/workspaces/{w}/domains/{d}/manifest?at={seq}&buckets=all` | all 4,096 bucket hashes |
| `POST /v2/workspaces/{w}/txns` and the routes in [§20](#20-chunked-staging) | staged transactions |
| `POST /v2/workspaces/{w}/repair` | resubmit records for 14 days after a server restore ([§14](#after-a-server-restore)) |
| tus routes in [§21](#21-attachments), `GET /v2/workspaces/{w}/blobs/{id}` | attachments |
| `POST`, `GET /v2/workspaces/{w}/invitations`, `DELETE .../invitations/{id}` | create, list and revoke invitations |
| `GET /invite/{token}` | invitation page (HTML), which sends you to the desktop app |
| `GET /v2/invitations/{token}` | the invitation's context, for the desktop app |
| `POST /v2/invitations/{token}/accept` | bind the invitation to the signed-in account |
| `POST`, `GET /v2/workspaces/{w}/shares`, `PUT`, `DELETE .../shares/{token_hash}` | create, list, update and revoke share links |
| `GET /s/{token}`, `GET /v2/shares/{token}` | share viewer page (HTML) and its snapshot record |
| `PUT /v2/workspaces/{w}/profile` | replace the profile record |
| `GET /v2/workspaces/{w}/realtime` | realtime channel |

`live` and `manifest` accept `at` only when it equals the `covered_seq` of the domain's latest checkpoint or of a checkpoint the caller leased ([§18](#18-bootstrap)), at or above the caller's history floor; otherwise they return `before_history_floor` or `checkpoint_expired`.

### Message bodies

Each body below uses the types in [§4](#4-encoding) and carries no tag. A route not listed takes an empty request body and returns an empty response body on success. Timestamps in these bodies (`expires_at`) are Unix seconds as `u64`; they describe invitations and links, never objects. Every list response ends with `opt<bytes> next`, an opaque cursor the client passes back as `after` to read the next page; the table leaves it out.

| Route | Request | Response |
|---|---|---|
| `GET /v2/limits` | | `u16 protocol`, `u64 restore_generation`, `u32 max_body`, `u32 max_inline`, `u64 max_blob`, `u32 max_txn_parts`, `u32 max_txn_records`, `u32 max_covers`, `u32 presence_per_second` |
| `POST /v2/device/authorize` | | `str device_code`, `str user_code`, `str verification_uri`, `u32 expires_in`, `u32 interval` |
| `POST /v2/device/token` | `str device_code` | `str session_token`; errors use the RFC 8628 codes `authorization_pending`, `slow_down`, `expired_token`, `access_denied` |
| `POST /v2/devices/challenge` | | `key challenge`, `u32 expires_in` |
| `POST /v2/devices` | `bytes enroll` | |
| `GET /v2/devices` | | `list<device_info>`: `bytes card`, `u8 state` (1 active, 2 removed) |
| `POST /v2/approval-requests` | `digest commit`, `opt<id> invitation`, `opt<digest> invite_proof` | |
| `GET /v2/approval-requests`, `GET .../pending-devices` | | `list<pending>`: `bytes card`, `digest commit`, `opt<id> invitation`, `opt<digest> invite_proof` |
| `POST /v2/approvals` | `bytes offer` | |
| `GET /v2/approvals` | | `list<bytes>` of offer records |
| `POST /v2/approvals/{offer_digest}/reveal` | `key nc` | |
| `GET /v2/approvals/{offer_digest}/reveal` | | `key nc` |
| `POST /v2/recovery` | `bytes recovery_proof` | |
| `GET /v2/workspaces` | | `list<workspace_info>`: `id workspace`, `u8 role`, `opt<bytes> profile` |
| `POST /v2/workspaces` | `list<record_ref>`: genesis, the workspace domain's first epoch bundle, envelopes, profile, and `add_device` changes, with their envelopes, for the devices and recovery identity of your account that the creating device pinned ([§7](#pinned-cards)), never for a card only `GET /v2/devices` lists | `u64 policy_head` |
| `GET .../policy` | | `u64 head_revision`, `list<bytes>` of policy records |
| `POST .../freeze` | `list<pair>`: `id device`, `id domain`, sorted | `list<cut>`: each pair's current head |
| `GET .../keys` | | `u64 next_key_seq`, `list<key_entry>`: `u64 key_seq`, `u8 record_type` (3 epoch bundle or 4 envelope), `bytes record` |
| `GET .../epochs/{epoch}` | | `bytes epoch_bundle`, whose recipient list is empty once the server dropped it |
| `GET .../keys/missing` | | `list<missing>`: `id domain`, `u64 epoch`, `id device` |
| `POST .../envelopes` | `list<bytes>` of envelope records | |
| `POST .../ops` | `list<record_ref>` | `u64 policy_revision`, `list<receipt>` |
| `POST .../pull` | `list<pull_cursor>`, `u32 wait_seconds` | `list<page>` of pull pages ([§15](#pull)) |
| `GET .../ops` | | pull page ([§15](#pull)) |
| `GET .../chain/{device}/{counter}` | | `entry` |
| `GET .../heads/{device}` | | `u64 counter`, `digest record_digest`, `u64 seq` |
| `GET .../checkpoint` | | `checkpoint_bundle` |
| `GET .../checkpoints/{digest}` | | `entry` |
| `POST .../checkpoints` | `checkpoint_bundle` | `u64 seq` |
| `GET .../live` | | `u64 at`, `opt<live_cursor> next`, `list<entry>`; `live_cursor` is `id rid`, `id device`, `u64 counter`, the last entry returned, and pages exclude it |
| `GET .../manifest`, levels 0 to 2 | | `list<digest>` of the node's 16 children |
| `GET .../manifest`, level 3 | | `list<leaf_ref>`: `id rid`, `ref`, sorted by `rid`, device and counter, with `opt<live_cursor> next` |
| `GET .../manifest`, `buckets=all` | | `list<digest>` of the 4,096 bucket hashes |
| `POST .../txns` | `id txn`, `u32 part_count` | |
| `PUT .../txns/{t}/parts/{i}` | the part bytes | `digest part_digest` |
| `POST .../txns/{t}/commit` | `bytes txn` | `202 Accepted`, empty |
| `GET .../txns/{t}` | | `u8 state` (1 staged, 2 committing, 3 committed, 4 failed), `opt<error>`, `list<receipt>` for its operations, purges and checkpoints, `u64 policy_head` |
| `POST .../repair` | `list<record_ref>` | `list<receipt>` |
| `POST .../invitations` | `id invitation`, `digest token_hash`, `u8 role`, `bytes invite_context` | `u64 expires_at` |
| `GET /v2/invitations/{token}` | | `id workspace`, `id invitation`, `u8 role`, `u8 status`, `bytes invite_context` |
| `GET .../invitations` | | `list<invitation_info>`: `id invitation`, `u8 role`, `u8 status` (1 pending, 2 accepted), `opt<id> accepted_by`, `u64 expires_at`, `bytes invite_context` |
| `POST .../shares` | `digest token_hash`, `id domain`, `u8 expires_in_days`, `bytes share`, `bytes share_box` | `u64 expires_at` |
| `GET .../shares` | | `list<share_info>`: `digest token_hash`, `id domain`, `u64 expires_at`, `bytes share_box` |
| `PUT .../shares/{token_hash}` | `bytes share` | |
| `GET /v2/shares/{token}` | | `bytes share` |
| `PUT .../profile` | `bytes profile` | |

`error` is `str code | str message | opt<u64> retry_after_seconds | opt<bytes> detail`. The `detail` of `chain_conflict` is `id domain | u64 counter | digest record_digest`, the server's head; of `policy_cuts_stale`, `list<cut>`; of `device_frozen`, `id freezing_device`; of `cursor_compacted`, `u64 retention_floor | u64 checkpoint_covered_seq` ([§15](#pull)); of a failed push, transaction or repair, `u32 record_index | str code | opt<bytes> record_detail`, where `record_detail` holds the failing record's own detail, such as a `chain_conflict` head.

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
| `epoch_stale`, `epoch_conflict`, `rotation_pending` | 409 | the operation's epoch isn't current; another rotation took this epoch; the domain waits for a rotation |
| `policy_stale`, `policy_conflict`, `policy_cuts_stale` | 409 | a record names a revision above the head; another change took this revision, or `policy_head` differs from it; cuts differ from the server's heads, which `detail` holds |
| `policy_regressed` | 422 | a record's `policy_revision` is below the previous record's in its chain |
| `device_frozen` | 409 | the device is frozen in this domain while a change removes its write access |
| `recipients_invalid` | 422 | an epoch's recipients or an envelope's recipient don't match the authorized devices |
| `manifest_mismatch` | 422 | a checkpoint's root differs from the server's |
| `cursor_compacted`, `pruned` | 410 | the cursor is below the retention floor; the chain position was pruned |
| `checkpoint_expired` | 410 | the requested checkpoint is neither the latest nor leased to the caller |
| `blob_incomplete`, `blob_digest_mismatch`, `blob_domain_mismatch` | 409, 422, 422 | a referenced blob isn't complete; an upload's digest doesn't match; the blob was created for another domain |
| `txn_incomplete`, `txn_expired` | 409, 410 | parts are missing or differ; the transaction expired |
| `quota_exceeded` | 507 | the workspace is over its storage quota, and the record isn't one that frees space ([§20](#20-chunked-staging)) |
| `id_conflict` | 409 | a new random ID already exists with other bytes |
| `rid_domain_mismatch` | 409 | the `rid` belongs to another domain |
| `beyond_cut` | 409 | the record lies beyond its device's cut and names no later re-grant |
| `checkpoint_not_due` | 409 | a standalone checkpoint arrived while the checkpoint duty was clear, or its `counted` doesn't match the duty |
| `checkpoint_stale`, `checkpoint_unknown` | 409 | a checkpoint's `previous` isn't the domain's latest checkpoint; a record names a checkpoint this domain doesn't hold at that `covered_seq` |
| `rotation_not_due` | 409 | an epoch record arrived with no new domain, pending rotation or authorized lineage break behind it |
| `repair_closed` | 409 | no restore opened repair, or its 14 days have passed |
| `purge_invalid` | 409 | a purged `rid` is outside the domain or has a live operation above the horizon |
| `snapshot_required`, `conflict_limit` | 409 | the `rid` holds too many live patches, so the client writes a snapshot first; the `rid` holds 64 live state operations, so the client resolves first |
| `duty_leased` | 409 | another device holds this duty; `retry_after_seconds` says when its lease ends |
| `domain_limit` | 409 | the workspace already holds 256 domains |
| `before_history_floor` | 403 | the request reaches below the account's history floor |
| `challenge_invalid`, `origin_mismatch` | 400 | the enrollment challenge is unknown, used or expired; the proof names another origin |
| `offer_expired`, `invitation_limit`, `invitation_invalid`, `share_limit` | 410, 409, 404, 409 | the approval offer is older than 10 minutes; the workspace already has 100 open invitations; the invitation is revoked, expired, used or unknown; the workspace already has 1,000 active share links |

Client error codes: `sync_chain_gap`, `sync_chain_reordered`, `sync_chain_replayed`, `sync_chain_forked`, `sync_policy_forked`, `sync_policy_rollback`, `sync_policy_unauthorized`, `sync_policy_cut_invalid`, `sync_policy_regressed`, `sync_writer_not_authorized`, `sync_writer_stale`, `sync_epoch_stale`, `sync_key_mismatch`, `sync_record_unreadable`, `sync_manifest_mismatch`, `sync_manifest_rollback`, `sync_journal_rollback`, `sync_purge_live_object`, `sync_approval_mismatch`, `sync_recovery_kit_invalid`, `sync_workspace_unverified`, `sync_profile_rollback`, `sync_reseal_limit`. Chain and record errors name the device and domain involved. A client that gets `before_history_floor` or `cursor_compacted` bootstraps ([§18](#18-bootstrap)).

### Duties

Some work falls to any of several devices: signing a due checkpoint, purging tombstones, filling missing envelopes, and rotating a pending domain.

The server leases the first three to one capable device at a time, chosen among devices with an open realtime channel or a pull in the last minute, through the `duties` bits of its pull pages and the `keys/missing` route. A lease lasts 30 seconds and renews each time its holder submits work toward it; when the holder submits nothing for 30 seconds, the server moves the lease to another device. The server answers other devices that attempt leased work with `duty_leased`.

Rotation takes no lease, because `epoch_conflict` already settles races ([§9](#rotation)). The device that committed the rotation-requiring change rotates each domain it can read at once, and a writer that gets `rotation_pending` and holds the domain's key rotates it at once. Once a rotation has waited 30 seconds, the server sets bit 4 on pages for every device that can rotate that domain. Such a device waits a random 0 to 30 seconds, checks that the rotation is still pending, and rotates.

Text snapshots follow the client rule in [§22](#22-live-collaboration).

### Rate limits

The server limits sync traffic per device and in bytes, with a separate allowance for bootstrap pages and range reads. Records that reach a device through the realtime channel don't count.

## 27. Limits

| Limit | Value |
|---|---|
| Request or response body, tus PATCH, range response | 1,048,576 bytes |
| Inline payload before padding | 262,144 bytes |
| Minimum padded length | 256 bytes |
| Blob file content | 2,147,483,648 bytes |
| Blob chunk; blob segment | 65,536 bytes; 1,048,576 bytes |
| Supersedes entries per operation | 256 |
| Blobs per operation | 4 |
| Live patches per `rid` (server) | 2,048 records or 8 MiB of ciphertext |
| Live state operations per `rid` | 64 |
| Domains per workspace | 256 |
| Staged transaction | 64 parts, 25,000 records, 24 hours; outcome kept for 30 counted checkpoints |
| Purge | 10,000 `rid`s |
| Envelope ciphertext; X-Wing encapsulated key; X-Wing public key | 48 bytes; 1,120 bytes; 1,216 bytes |
| Epoch recipients | 4,096 |
| Presence | 2 a second per device, 2-minute clock window, 30-second expiry, 8 KiB padded plaintext |
| Realtime frame | 65,536 bytes |
| Text updates | one per note every 500 ms |
| Invitations | 7 days, 100 open per workspace |
| Share links | 1 to 30 days, 1,000 active per workspace |
| Device authorization codes, enrollment challenges, approval offers | 10 minutes |
| Freezes | 10 minutes, at most 3 renewals by the freezing device |
| Duty leases | 30 seconds, renewed by each submission |
| Bootstrap checkpoint lease | 24 hours |
| Re-seals per chain position that no newer epoch or policy change forces | 3 |
| Text in text mode, yrs document | 8 MiB, 32 MiB |
| Collaboration sandbox, per update | 10,000,000,000 fuel units, 512 MiB of memory |
| Share snapshot, padded | 983,040 bytes |
| Recovery kit Argon2id | at least m = 65,536 KiB, t = 3, p = 4; at most m = 1,048,576 KiB, t = 10, p = 16 |
| Retention | 30 counted checkpoints, at most one a day per domain; devices whose cursor hasn't advanced for 90 stop holding the floor, and the floor never sits below the 90th; tombstones purge after 90 |
| Records a client keeps for a server restore | 14 counted checkpoints, at most 64 MiB per workspace |
| Repair after a server restore | 14 days |

The targets of 100,000 objects and 200 devices are tested sizes, not caps. No structure in this document stops at 1,000 objects.

## 28. Worked example: 100,000 objects

The workspace holds 95,000 notes averaging 3 KiB and 5,000 attachments averaging 2 MiB, across 200 devices. It has 12 domains: the workspace domain, 9 folder domains, and 2 restricted domains of 20 devices each.

**Record sizes** from the layouts above: an envelope is 1,366 bytes; an epoch record is 351 bytes, and its bundle with 199 recipients 3,543 bytes; an operation carrying a 3 KiB note and one cover is 3,716 bytes (388 bytes of fields, AEAD tag and signature, and a 3,328-byte padded payload); a checkpoint bundle with 200 chain heads is 13,160 bytes.

### Removing one device

The removed device could read all 12 domains. Its owner's other device freezes it, then signs and pushes the removal by itself. The rotations follow, one push per domain: about 276 KB for the workspace domain, which also covers the 9 folder domains, and about 27 KB for each restricted domain.

| Record | Count | Bytes |
|---|---|---|
| `remove_device` with 12 cuts | 1 | 1,041 |
| Epoch bundle, workspace domain | 1 | 3,543 |
| Envelopes, workspace domain | 199 | 271,834 |
| Epoch bundles, restricted domains | 2 | 1,326 |
| Envelopes, restricted domains | 2 × 19 = 38 | 51,908 |
| Record framing, 5 bytes per record | 241 | 1,205 |
| Total | | 330,857 bytes |

The removal re-encrypts no object; the 100,000 objects add nothing. Adding folder domains adds nothing either, because folder keys derive from the workspace key. The worst case is a workspace whose 256 domains are all restricted to all 200 devices: 256 rotations of about 276 KB each, about 71 MB in all, each in its own push.

### Joining

A new member's device joins with access to the workspace and folder domains.

1. **Approval transaction**, about 135 KB in one request: `add_member` (194 bytes), `add_device` (1,507 bytes), one envelope for the workspace domain (1,366 bytes), from which the folder keys derive, and 10 fresh checkpoint bundles of 13,160 bytes.
2. **Policy and keys**: a policy chain of 3,000 changes is about 3.8 MB (4 pages) if half are `add_device` changes and half `remove_device` changes with 12 cuts. The device fetches the current epoch bundle of the workspace domain; walking lineage back across 300 past epochs, when live operations name them, reads about 106 KB of epoch records.
3. **Live operations**: 95,000 notes at 3,729 bytes per page entry, 281 per page, take 339 pages. The 5,000 attachment operations take 4 more.
4. **Attachments**: each 2 MiB attachment pads to a 2,163,269-byte blob, so the 5,000 blobs take 3 range requests each, 15,000 in all.
5. **Verification**: the device computes 100,000 leaf hashes and 4,369 bucket and node hashes per domain (43,690 for 10 domains) and compares each root. That takes milliseconds.

No request or response exceeds 1 MiB.

### Moving a folder of attachments

Moving a folder that holds the 5,000 attachments from the workspace domain into a folder domain writes 5,000 new state operations and 5,000 deletes, about 7 MB, and re-uploads nothing: the new operations list the same blobs ([§21](#21-attachments)), so other devices download no attachment again. Moving the same folder into a restricted domain re-encrypts and uploads all 10 GB, and each grantee device downloads it again.

### Download pacing

A sync rate limit counted only in requests would hold the join download back: at 120 requests a minute, the attachment ranges alone would take 125 minutes. The byte budget and bootstrap allowance in [§26](#rate-limits) avoid that.

## 29. What the server stores

| Data | Fields | What it reveals |
|---|---|---|
| Accounts | account ID, email, passkey credentials, email confirmation state | who you are |
| Sessions, magic links, device grants, enrollment challenges | token hashes, account, expiry | that you signed in |
| Devices | card (account, device ID, public keys), active or removed, which sessions belong to which device, freezes in progress with the freezing device | how many devices you enrolled; a card that never enrolls a session suggests a recovery identity; who froze which device |
| Pending approvals | approval requests with their commitments, offers and reveals until they expire | that a device is waiting for approval |
| Workspaces | workspace ID, owner account, quota usage, restore generation and repair window | that the workspace exists |
| Policy changes | full signed records | members, roles, device events, domain kinds and restricted grants, in order |
| Domains | domain ID, kind, marker `rid`, current epoch, rotation state, history floors, the highest published retention floor, duty leases | how many domains exist and who reads each |
| Device progress | per device and domain: the highest acknowledged cursor, the index of the counted checkpoint when it last advanced, and the highest policy revision served to the device | how far each device has synced, in counted checkpoints |
| Chain heads | per device and domain: counter, digest, sequence | the last position each device wrote |
| Epoch records and envelopes | full signed records, also as log entries in the domains they cover; recipient lists while an epoch is current or named by a retained counted checkpoint; envelopes of current epochs; the key sequence; which authorized devices still lack an envelope | when each domain rotated and which devices hold its key |
| Operations | domain, sequence, device, counter, previous digest, epoch, policy revision and head, restore generation, the checkpoint the writer had reached, `rid`, supersedes, blob IDs, nonce, padded ciphertext, signature, digest | which device changed which random ID and how often; concurrent edits and incremental updates; padded payload size; how far each writer lagged behind the latest checkpoint |
| Purges | full signed records | which tombstone deleted each listed object, so which device deleted it and, through counted checkpoints, about which day, at least 90 counted checkpoints after the delete |
| Checkpoints | full signed records, with the previous checkpoint's digest, the counted index and the heads floor, and chain heads, as log entries; bucket hashes of the latest and leased checkpoints | the number of live objects per domain, and, through the daily counted checkpoints, about which day each sequence was written |
| Staged transactions | parts and descriptor while staged; state and receipts after commit, for 30 counted checkpoints | size of large pending changes |
| Blobs | blob ID, the domain it was created for, uploading device, padded ciphertext length, ciphertext digest and segment hashes, upload state, referencing operations, the counted checkpoint index when it became unreferenced; expiry only while incomplete | padded attachment size; that an operation refers to a large file; which operations of the workspace and folder domains share an attachment |
| Share links | token hash, workspace, domain, expiry, signed padded snapshot, box | that a domain has a shared item, and from its expiry roughly when someone shared it |
| Invitations | while open: ID, token hash, workspace, role, inviter, accepting account, status, expiry, padded context | who is inviting whom, with which role, until the invitation ends |
| Workspace profile | signed encrypted record | when the name changed |
| Rate counters | key and counter | request volume |
| Presence | nothing stored | |

No stored table holds an object type, a per-record timestamp or an unpadded size.

- **Object types.** `rid`s are random, payload kinds are encrypted, and paths, file names and frontmatter stay inside ciphertext. Two patterns show how an object is edited, not what it is: operations that cover nothing mark a note in text mode, which any Markdown type (note, task, project, chat) can enter, and a blob ID marks content above 256 KiB, a size class the padded length would show anyway.
- **Creation times.** Operations, `rid`s and completed blobs carry no timestamp, and retention counts checkpoints, so the server keeps no table that turns a sequence into a date. Counted checkpoints come at most once a day per domain, though, so someone holding the stored data can count them back and date a sequence to about a day. An S3-compatible blob store records `LastModified` for each segment, which dates an attachment's upload to the second; a filesystem store sets a fixed modification time. A series of backups dates each record to the backup interval. The server sees when an operation arrives while it handles the request, and it doesn't store that time. A `rid` first appears when an object first syncs: for files that existed before you turned sync on, that's the day you turned it on. Share expiries date the share, never an object, and the server deletes an invitation's row when the invitation ends.
- **Sizes.** Every ciphertext the server stores is Padmé-padded, and the server rejects unpadded lengths. Policy, epoch and envelope sizes depend on member, device and domain counts, never on content.
- **Moves between domains.** A cross-domain move commits the old `rid`'s delete and the new `rid`'s create in one push, so the server can link the two `rid`s and carry the object's edit history across domains. An attachment moved among the workspace domain and folder domains keeps its blob ID, which links the two `rid`s once more. One moved into or out of a restricted domain gets a new blob ID, which removes only the exact match.

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
| Lists an extra card in `GET /v2/devices` | yes | a zero-offer `add_device` takes cards only from the signer's pinned cards |
| Swaps a card during approval | yes, except with probability 10^-8 per attempt | the candidate's committed nonce keeps the two codes independent, and the candidate uses each nonce for one offer |
| Substitutes its own card in a missing-envelope list | yes | wrapping devices take cards only from their verified policy |
| Returns an older or different policy chain | yes, after first contact | `sync_policy_rollback`, `sync_policy_forked` |
| Shows two devices different changes at one policy revision | when a record crosses | `policy_head` in every record, `sync_policy_forked` |
| Gives recipients of one epoch different keys (with a malicious member) | yes | `key_check` |
| Lists an unauthorized recipient in an epoch | yes | recipients checked against the verified policy |
| Serves a manifest that omits an object | yes, with an honest signer | the recomputed root differs from the signed checkpoint |
| Rolls a device back to an older checkpoint | yes | `sync_manifest_rollback`, against the latest accepted checkpoint and the anchors in approvals and kits |
| Claims compaction or lowers the retention floor to force a bootstrap from an old checkpoint | yes | the published floor never drops within a generation, and the bootstrap checkpoint must cover the client's cursor |
| Applies patches in different orders for different clients | when a checkpoint arrives | sequences in leaf refs make the roots differ |
| Shows two recipients two different epoch records for one epoch (with the rotating member) | when either sees the other | the epoch record chain, checkpoints naming the epoch record, records that stop decrypting |
| Accepts writes from a device that lost write access | yes | cuts |
| Keeps writers from seeing a removal, so a removed device keeps reading their new records | when a writer's old-epoch records after the new epoch's log entry form a second run or pass 1 MiB | `sync_writer_stale` and an alert, with no checkpoint needed |
| Claims a restore so clients accept lower heads and checkpoints | shown to you | every generation change raises an alert and lists the records the server lost; clients still refuse a fork of the policy, a chain or an epoch |
| Forwards an old presence record | yes | `minute`, the current epoch, the per-session sequence |
| Forges or drops realtime notifications | harmless | notifications and records frames only feed verified pulls |

### What the client can't prevent

- **Withheld service.** The server can refuse requests, freeze a domain at an old state, or delete ciphertext. Your files stay on your devices.
- **Split views.** The server can show different clients different subsets of records. Clients detect the split once any evidence crosses it: a record, which names its writer's policy head and latest checkpoint, a checkpoint, a receipt or a policy head. A server that partitions clients completely and forever goes unnoticed. A writer that never sees a removal while no other record reaches you between its pushes keeps leaking to the removed device until its old-epoch records pass 1 MiB.
- **Timing and traffic.** The server sees when you sync, how many operations you send, padded sizes, which random IDs change together, your IP address and your account email. Text updates show which `rid` each writer edits, to the 500 ms tick, and presence channels show which devices have the same note open.
- **Membership.** Every member and the server see the policy: members, roles, devices, domain kinds and restricted grants. Members outside a restricted domain learn that it exists and who holds it, never its folder name or contents.
- **History for new members.** A new member's domain keys open older ciphertext in those domains through lineage. The history floor keeps the server from serving it, and a malicious server could serve it anyway. A text-mode note also reaches a new member with its live patches, up to the snapshot threshold, and those can hold text deleted since.
- **Access granted again.** A member or device that loses access and later gets it back can read what was written while it was away, unless the re-grant breaks the lineage ([§9](#rotation)).
- **Pending rotations.** Until a grantee device rotates a restricted domain, the server alone keeps its ciphertext from a removed device. One push per device, signed or sent before its writer saw a removal, can still use the old epoch.
- **Old share snapshots.** The viewer warns when a later visit shows a lower `snapshot_revision`, but a first visit can't tell a current snapshot from an older one the server kept after Update link.
- **Served web code.** The server serves `account-web`. A modified viewer page could read the fragment and leak a share link's keys. A modified invitation page could read the secret of an invitation link you opened in a browser, and with it the workspace and inviter names and the means to forge an invitation proof; the approval code still stands between it and your keys. The desktop app handles invitations without the browser and doesn't depend on served code.
- **Blame for forks.** A server can't fake a fork from a re-seal, because each attempt names the attempt it replaced. Two records at one position that aren't consecutive attempts come from the device's key; clients can't tell whether the device misbehaved or a stolen key did.
- **A colluding writer.** An editor who colludes with the server can sign a checkpoint that omits objects, and a new device bootstrapping from it misses them. Current devices reach that checkpoint as a log entry and detect the mismatch against their own root; returning devices hold missing files for review instead of deleting them.
- **Enrollment in another account's name.** An owner or admin can remove a member's devices and approve a new device for that member's account. The server checks that the member's own sign-in enrolled the card; clients can't check that. The member's app alerts them when another account signs an `add_device` for their account, and the device gets no restricted key, under past grants or later ones, until a manager admits it ([§9](#granting-and-wrapping)). Removing the member drops their restricted grants, so re-adding them grants nothing until a manager acts. One case escapes this: the device approved with an account's first `add_member` isn't foreign, so a manager who grants a restricted domain to a new member relies on the owner or admin who compared the approval code with that member.
- **Recovery after first contact.** A recovering device trusts the policy head stored in its kit. A server can withhold changes made after the kit was written, such as the kit's own revocation, until another device's records reveal them.
- **A restored machine.** A restore of a whole machine from backup can roll back both the journal and the credential store, and the device then can't tell. The server's heads and the checkpoints are its only check.

### Members and devices

- A member writes within their role. A malicious editor can delete objects, purge tombstones or write records nobody can read; other devices keep their local copies, report a purge of a live object, and list unreadable records for review. A member can't rotate a domain outside a required rotation or a lineage break its role allows, and can freeze only the devices whose write access its role lets it remove.
- A removed member or device keeps everything it received and can read nothing encrypted after the rotation, unless it gets access again without a lineage break.
- A device that forks its own chain halts that domain for readers until an owner or admin removes it; the removal's cuts tell readers which records to keep.
- Anyone holding a recovery kit and its secret can add one device of your account to each workspace and remove your devices, until you remove the recovery identity.

Adversarial agent reviews are this protocol's security review, and no product copy may claim an audit.

## 31. Comparison with SuperSync

SuperSync is the sync service of Super Productivity (MIT, `LICENSE` at the repository root, copyright 2018 Johannes Millan). This comparison reads [johannesjo/super-productivity](https://github.com/johannesjo/super-productivity) at commit `78139c531c808bab7c46ef96fd0fbb0a3b6f7d6d`.

### Operation log

SuperSync stores operations in PostgreSQL with a dense per-user `server_seq` (`packages/super-sync-server`). An operation carries a UUID v7 ID, a client ID, action, operation and entity types, entity IDs, a vector clock, a wall-clock timestamp and a schema version, all in cleartext; only the payload can be encrypted (`packages/shared-schema/src/supersync-http-contract.ts`). Uploads carry up to 100 operations, and the server deduplicates by operation ID and by a request ID cached for 5 minutes.

v2 adopts the server-ordered append-only log, batched uploads, idempotent retries and a cursor written after durable application. It differs in four ways: the log is per domain, an operation's identity is its chain position and digest instead of a time-ordered UUID, every type and entity ID stays inside the ciphertext, and each operation is signed and chained. A push is atomic, so the request-ID cache has no job for operation pushes; the other create routes are idempotent on their client-chosen IDs instead ([§26](#26-http-api)).

### Cursors

SuperSync clients pull with `GET /ops?sinceSeq&limit` and page with `hasMore`. The server reports `gapDetected` when the client is ahead, the range was purged, or a page has a hole, and the client restarts from zero (`operation-download.service.ts`, `getOpsSinceWithSeq`). A cursor behind compaction jumps to the newest full-state operation.

v2 adopts paging from a cursor and the snapshot-plus-tail recovery. It differs because a server-computed gap flag asks the client to trust the server: v2 clients detect gaps from signed chains, and a cursor behind compaction gets `cursor_compacted` and bootstraps from a signed checkpoint whose Merkle root the client verifies.

### Conflicts

SuperSync detects conflicts per entity with vector clocks, on the server at upload and on the client against an entity frontier (`conflict.ts`, `entity-frontier.ts`). The server rejects concurrent uploads. The client resolves by last write wins with a merge of disjoint fields, and the losing version is overwritten (`conflict-resolution.ts`, `planLwwConflictResolutions`). Clocks are pruned to 20 entries (`vector-clock.ts`, `limitVectorClockSize`), and the project tracks a case where pruning hides a real conflict.

v2 can't detect conflicts on the server, because entity IDs are encrypted. Supersession vectors name only the heads an operation covers, so they need no pruning, and both versions stay for review. v2 adopts the merge of disjoint changes, the rule that identical branches aren't a conflict (`isIdenticalConflict`), and a deterministic tie order, using operation digests where SuperSync uses client IDs.

### Compaction

SuperSync treats a full-state import or repair as a snapshot operation. A daily job deletes history before the newest snapshot once it is older than 45 days, and a 100 MB quota per user deletes old snapshots (`storage-quota.service.ts`, `deleteOldSyncedOpsForAllUsers`). Clients don't create periodic snapshots. A new device downloads the newest snapshot and the tail.

v2 adopts a retention window behind a snapshot boundary and the snapshot-plus-tail bootstrap. It differs in what a snapshot is and how the window is measured: writer devices sign periodic manifest checkpoints, the server deletes only covered operations below a retention floor that waits for every retaining device and 30 counted checkpoints, and tombstones leave through signed purges. v2 counts daily checkpoints instead of dates so the server keeps no date table. Text snapshots play the role of SuperSync's full-state operations for one note.

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
| `sync-v2-encoding.json` | field types, tag bytes, decoding rules, `opt` discriminators, record tags checked against their position, every message body, error code, HTTP status and `detail` in [§26](#26-http-api), and every realtime frame |
| `sync-v2-padding.json` | `padded_length` for boundary lengths, malformed padding |
| `sync-v2-ids.json` | remote ID format, base64url forms |
| `sync-v2-device.json` | `noura.sync.device.card` with an X-Wing key, device fingerprint, `noura.sync.device.enroll` (wrong origin, account and key) |
| `sync-v2-approval.json` | `noura.sync.approval.commit`, `noura.sync.approval.offer` with checkpoint anchors, `noura.sync.approval.code`, a reveal that doesn't match its commitment, a second offer for an already revealed commitment, ground to match the approver's code, that the candidate refuses, `noura.sync.v2 invitation proof` |
| `sync-v2-policy.json` | `noura.sync.policy`, each change kind including `amend_cut` and both device kinds, cuts, chain links, authority failures, a recovery identity signing a forbidden change, foreign devices and restricted-domain eligibility over policy chains, a member removed and added again who stays out of every restricted list until a manager grants access, a rejoining account's admin-approved device that stays foreign, a zero offer from another account, a card that only `GET /v2/devices` lists and that a creating device must not add, a chain forked at one revision and served to two clients |
| `sync-v2-keys.json` | `noura.sync.epoch`, the epoch bundle and `noura.sync.epoch.recipients`, the epoch record chain, `noura.sync.key-check` with `noura.sync.v2 key check`, lineage and a lineage break, `noura.sync.envelope`, HPKE `info` with X-Wing, an unpinned signer, epoch validity, an epoch record with no rotation due, a restricted domain's epoch 1 from a device that isn't eligible, `noura.sync.v2 folder key`, `noura.sync.v2 lineage key`, and cards a wrapping device must refuse: one absent from the policy, a pinned device ID with another HPKE key, and a device not eligible for the domain |
| `sync-v2-operation.json` | `noura.sync.op` with its `generation` and checkpoint reference, `noura.sync.payload` for each kind, AEAD associated data with the epoch record's digest, `noura.sync.v2 operation key`, path validation, property values as canonical JSON |
| `sync-v2-chain.json` | schedules for gap, reorder, replay, fork, a retried upload, consecutive re-seal attempts served in either order, sibling attempts, an attempt that arrives after a successor of the other, a replay right after bootstrap, a record beyond a cut, an unreadable record, a checkpoint that arrives after the client passed its `covered_seq`, a checkpoint entry left out of one client's log while other records still cross, a line of more than 4 re-seal attempts forced by rotations, a stale writer's second run of old-epoch records after a rotation with no checkpoint since, a missing epoch entry, a device returning after 100 counted checkpoints, a server restart that loses in-memory cursors without lowering the floor, and a removed device's head leaving at `heads_floor` |
| `sync-v2-live.json` | live sets and supersession over operation schedules (covers of covered operations, purges, pruning), the conflict outcomes in [§16](#16-conflicts), path collisions, text lineage merges, and patch order by sequence |
| `sync-v2-domains.json` | `.noura-domain` validity: planted, moved, nested and evicted markers, a marker found in another domain's log, a marker with two live states, bootstraps that meet a moved, a deleted and a conflicted marker and record held roots, a file created during bootstrap before its domain's objects land, and a marker deleted locally that the client writes again |
| `sync-v2-manifest.json` | `noura.sync.manifest.leaf` with sequences, `.bucket`, `.node`, `.heads`, `noura.sync.checkpoint`, the heads digest check, `heads_floor` and a removed device leaving the heads, `previous` links and `counted`, roots for add, remove and change, a rolled-back checkpoint, a log-entry checkpoint whose signer was removed later, and a failed log-entry checkpoint that doesn't hold the cursor |
| `sync-v2-purge.json` | `noura.sync.purge` with its checkpoint reference |
| `sync-v2-txn.json` | `noura.sync.txn`, `noura.sync.txn.parts` |
| `sync-v2-blob.json` | `noura.sync.blob` header, chunk nonces, lengths, `noura.sync.blob.parts` |
| `sync-v2-collaboration.json` | yrs updates near the sandbox's fuel and memory limits, and updates over them, with the same verdict under Cranelift and Pulley |
| `sync-v2-presence.json` | `noura.sync.presence`, `noura.sync.presence.payload`, `noura.sync.presence.channel`, `noura.sync.v2 presence key`, the clock window |
| `sync-v2-profile.json` | `noura.sync.profile`, `noura.sync.profile.payload`, `noura.sync.v2 profile key` |
| `sync-v2-invitation.json` | `noura.sync.invite.token`, `noura.sync.invite.context`, `noura.sync.invite.payload`, `noura.sync.v2 invitation key` |
| `sync-v2-share.json` | `noura.sync.share.token`, `noura.sync.share` and its link signature, the fragment, `noura.sync.share.payload`, `noura.sync.share.box`, `noura.sync.share.box.payload`, `noura.sync.v2 share key`, `noura.sync.v2 share box key` |
| `sync-v2-recovery.json` | `noura.sync.recovery-kit`, `noura.sync.recovery-kit.payload`, the secret's base32 form, `noura.sync.recovery.proof` (wrong secret, parameters below the minimum and above the caps) |

The store conformance suite runs `sync-v2-live.json` against both stores, so SQLite and PostgreSQL compute the same live sets as clients.

### Store and soak tests

Beyond the fixtures, the implementation tests these cases against both stores:

- two server instances racing pushes into one domain, with an injected commit delay, while every client receives every record;
- removing a device while it pushes a record every 500 ms;
- 200 simulated devices across 100 folder domains and 2 restricted domains, in a 24-hour soak at collaboration write volume that checks that the retention floor moves and storage stays bounded;
- a viewer that keeps pulling with an old cursor while the floor moves;
- a bootstrap during heavy checkpointing;
- a server restored from an older backup after further activity, with a removal whose cut lies above a lost head, records under since-rotated epochs, and repair from several devices;
- a freeze from a viewer, which the server rejects, and a renewal from a device other than the freezer;
- a workspace over quota that accepts deletes, purges and checkpoints and rejects other writes;
- a rotation pending while its committing device sleeps, which another device completes;
- a tampering proxy for every row of [§30](#30-threats) marked as detected;
- a stored-data check over database rows, blob-store metadata and committed transactions that finds no per-record timestamp;
- a member outside a restricted domain who receives no notify, records or presence frame for it.

### Tag bytes

Each tag is the length byte, the ASCII domain string and `0002`:

| Domain string | Length | Tag bytes (hex) |
|---|---|---|
| `noura.sync.device.card` | 22 | `166e6f7572612e73796e632e6465766963652e636172640002` |
| `noura.sync.device.enroll` | 24 | `186e6f7572612e73796e632e6465766963652e656e726f6c6c0002` |
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
| `noura.sync.purge` | 16 | `106e6f7572612e73796e632e70757267650002` |
| `noura.sync.checkpoint` | 21 | `156e6f7572612e73796e632e636865636b706f696e740002` |
| `noura.sync.manifest.leaf` | 24 | `186e6f7572612e73796e632e6d616e69666573742e6c6561660002` |
| `noura.sync.manifest.bucket` | 26 | `1a6e6f7572612e73796e632e6d616e69666573742e6275636b65740002` |
| `noura.sync.manifest.node` | 24 | `186e6f7572612e73796e632e6d616e69666573742e6e6f64650002` |
| `noura.sync.manifest.heads` | 25 | `196e6f7572612e73796e632e6d616e69666573742e68656164730002` |
| `noura.sync.txn` | 14 | `0e6e6f7572612e73796e632e74786e0002` |
| `noura.sync.txn.parts` | 20 | `146e6f7572612e73796e632e74786e2e70617274730002` |
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
| `noura.sync.share.box` | 20 | `146e6f7572612e73796e632e73686172652e626f780002` |
| `noura.sync.share.box.payload` | 28 | `1c6e6f7572612e73796e632e73686172652e626f782e7061796c6f61640002` |
| `noura.sync.recovery-kit` | 23 | `176e6f7572612e73796e632e7265636f766572792d6b69740002` |
| `noura.sync.recovery-kit.payload` | 31 | `1f6e6f7572612e73796e632e7265636f766572792d6b69742e7061796c6f61640002` |
| `noura.sync.recovery.proof` | 25 | `196e6f7572612e73796e632e7265636f766572792e70726f6f660002` |

BLAKE3 `derive_key` contexts are the exact ASCII strings in [§3](#3-primitives), with no tag and no version bytes: `noura.sync.v2 operation key`, `noura.sync.v2 lineage key`, `noura.sync.v2 key check`, `noura.sync.v2 presence key`, `noura.sync.v2 profile key`, `noura.sync.v2 share box key`, `noura.sync.v2 folder key`, `noura.sync.v2 share key`, `noura.sync.v2 invitation key` and `noura.sync.v2 invitation proof`.

### Example: a device card

With account `11` repeated 16 times, device `22` repeated 16 times, signing key `33` repeated 32 times and X-Wing public key `44` repeated 1,216 times, the card is these 1,309 bytes:

```text
16 6e6f7572612e73796e632e6465766963652e63617264 0002   tag
11111111111111111111111111111111                        account
22222222222222222222222222222222                        device
3333333333333333333333333333333333333333333333333333333333333333   signing_public
000004c0                                                hpke_public length, 1,216
44 repeated 1,216 times                                 hpke_public
```

## 33. Decisions to confirm at sign-off

These choices go beyond the list of topics the protocol had to cover. Confirm or change each one:

1. **Binary encoding.** Records use the fixed-width binary encoding in [§4](#4-encoding) instead of v1's JSON tuples, and the signed bytes are the sent bytes.
2. **New primitives.** XChaCha20-Poly1305 replaces AES-256-GCM, HPKE with the hybrid X-Wing key encapsulation replaces age and the custom web envelope, and BLAKE3 replaces SHA-256 for digests.
3. **Folder domains carry no permission and no keys of their own.** Their keys derive from the workspace domain's key, so they add nothing to a rotation, and they exist as log, checkpoint and bootstrap units. The alternative is to drop them from v2.
4. **History isolation is the server's job.** Lineage lets one envelope per domain open old epochs, so joining never re-encrypts objects; a manager can break lineage when granting access again.
5. **Recovery kits are offline devices** with limited signing rights, protected by a generated 160-bit secret behind Argon2id, and they stay current through rotations.
6. **Approval compares one 8-digit code** derived from the approver's offer and a nonce the candidate committed to first, confirmed on both devices: one offer and one comparison per `nc`.
7. **Tombstones leave through signed purges** after 90 counted checkpoints, instead of a cleartext delete flag on each operation. A purge still tells the server which operation deleted each object, but only 90 counted checkpoints or more after the delete.
8. **Retention counts daily checkpoints, not dates**, so the server stores no sequence-to-date table: 30 counted checkpoints of dead operations, 90 for idle devices and tombstones. A domain gets at most one counted checkpoint a day, which dates a sequence to about a day.
9. **Limits**: 1 MiB bodies, 256 KiB inline payloads, 2 GiB blobs, 256 covers per operation, 256 domains per workspace, 25,000 records per transaction.
10. **Edit beats delete** in a conflict until you choose, and a returning device asks before deleting files the manifest lacks.
11. **Edit mode and size class stay visible.** Patches that cover nothing show text mode, and blob IDs show content above 256 KiB. Hiding them would mean dropping server-side compaction or blob authorization.
12. **Only the inviting device approves an invitee**, and a device that another account enrolled, with every device it adds, gets no restricted key until a manager signs `admit_device`.
13. **Sync rate limits count bytes per device**, with a bootstrap allowance, and records delivered over the realtime channel don't count.
14. **Rollback protection lives in the credential store.** A high-water mark per workspace keeps a restored or copied workspace folder from rolling a device back or reusing its counters.
15. **Patches apply in server sequence order**, which checkpoints commit to through the sequence in each leaf ref.
16. **Every operation and purge names its writer's latest checkpoint and the restore generation**, 48 bytes per record, so a server can't hide a checkpoint from one client while records still cross.
17. **Attachments stay put among the workspace domain and folder domains**, which share one audience; only a move into or out of a restricted domain encrypts and uploads an attachment again.

## Sign-off

| | |
|---|---|
| Signed off by | pending (Claude, the orchestrating agent, under the owner's delegation of 2026-10-20) |
| Date | pending |
| Commit signed off | pending |
