# Noura sync service (development)

An original Bun/Hono/PostgreSQL service for signed encrypted operations. This
is the server foundation and an experimental collaboration implementation,
**not a production Noura collaboration release**. Production startup deliberately
withholds collaboration capabilities until the remaining release gates below
pass.

Once enabled, rollout admission and protocol continuity are separate. Setting
`collaborationRollout: false` while retaining `checkpointTransitions: true`
blocks new workspace capabilities while continuing to serve existing checkpoints,
transitions, live-text generations, and capability-bound object activations.

## Implemented

- Better Auth passkeys and emailed magic links, with an operator email allowlist.
- Account-authenticated, single-use device challenges and Ed25519 possession proofs.
- Seven-day opaque device sessions, stored as hashes; renewal invalidates the
  previous session, and revoked device IDs cannot enroll again.
- Workspace and object creation, device listing/revocation, and workspace listing.
- Atomic signed operation batches, durable retry IDs, increasing decimal-string
  cursors, payload quotas, per-account rate limits, and object-filtered downloads.
- Bounded notification-driven pulls using one shared PostgreSQL listener, with
  authentication and access rechecked after waiting.
- Real PostgreSQL integration tests, including account login with captured mail.
- Browser-approved native device sign-in with OS-held credentials, a desktop
  Settings account panel, and passkey/magic-link browser account pages.
- Signed access-policy revisions, member/object permissions, device-wrapped
  content keys, epoch rotation enforcement, chained policy-history verification,
  per-revision writer authorization, and permission-filtered key retrieval.
- Revocable encrypted public snapshots and a read-only browser viewer. Decryption
  keys and the pinned signer travel in the URL fragment, never the HTTP request.
- Native AES-GCM/Ed25519 file transport, age key wrapping, encrypted key-envelope
  persistence, a file-backed crash-replay journal, and conflict preservation.
- Native background capture and restart, pause/resume, owner replica joining,
  verified device approval, encrypted key backup before upload, and explicit
  local/remote resolution of same-path conflicts without overwriting later edits.
- Owner-signed workspace capabilities, automatic invitation activation, atomic
  checkpoint/key rotations, isolated fresh-recipient history, and resumable
  transition-bound checkpoint blobs.
- Writer-authorized object activation plus native create, update, move, delete,
  external-change, MCP, managed metadata/body, and plain-text collaboration paths.
- Yjs/Yrs text generations with acknowledged baselines, durable drafts, restart
  recovery, generation rebase/review, and an isolated bounded native decoder worker.
- Authenticated realtime notifications and encrypted transient presence, with
  durable HTTP pull/acknowledgment remaining authoritative.

## Run locally

From this directory, copy `.env.example` to `.env`. Set the database connection,
a random `AUTH_SECRET`, account email delivery, sender address, and
`ALLOWED_EMAILS`. Choose one delivery path: `SMTP_URL` or `RESEND_API_KEY`.
An empty allowlist admits nobody. No messages are printed to logs in lieu of a
mail transport.

```sh
bun install
bun run migrate
bun run start
```

To include the account pages and public viewer, run `bun run build:release`, then
`bun dist/main.js`. Docker packages this combined release automatically.

The browser UI is built from `apps/app`. Its `(account)` route group contains
`/account`, `/account/device`, `/invite/[token]`, and `/share/[token]`, outside
the `(workspace)` layout and its native initialization. Browser workspace routes
currently display **Workspace unavailable**; this consolidation does not add
browser file storage or workspace editing.

`build:release` creates `apps/app/build-hosted`, verifies CSP hashes and the
account routes' static import boundary, generates bundled dependency notices,
and copies the app to `dist/public`. Native builds continue to use
`apps/app/build` and Tauri's CSP. Do not substitute a native build for the hosted
build: the hosted build preserves the restrictive account/share CSP.

The server serves only explicit SPA destinations and asset prefixes. Missing API
endpoints, missing assets, unknown paths, and non-GET page requests do not fall
back to HTML. Auth callback paths and the server's `PUBLIC_ORIGIN` are unchanged.
For frontend development, `bun run --cwd ../app dev` proxies `/api`, `/public`,
and `/v1` to loopback port 1900. Configure the server's `PUBLIC_ORIGIN` to match
the frontend origin when using that proxy; test secure cookies and passkeys on
the same-origin combined release before deployment.

Use `/health` for liveness and `/ready` for database/schema readiness. Production
requires an HTTPS `PUBLIC_ORIGIN`; plain HTTP is allowed only for loopback.

`docker compose --env-file .env -f compose.yaml up --build` starts PostgreSQL,
runs migrations, then starts the service. Also set `POSTGRES_PASSWORD` for
Compose, using a URL-safe random value. The application listens only on host
loopback; use the example Caddy configuration for external TLS termination.
Neither a domain nor a managed production instance is provisioned by this code.

## API

Account routes live under `/api/auth/*` and use Better Auth cookies. Create a
device challenge with `POST /v1/device-challenges` using the authenticated cookie
and same-origin `Origin` header. The response contains `challenge`, `accountId`,
and a five-minute lifetime.

Sign the UTF-8 JSON tuple below with the device's Ed25519 private key:

```text
["noura.device.enroll", serverOrigin, accountId, deviceId, publicKeyBase64, challenge]
```

Send `{deviceId, publicKey, challenge, proof}` to `POST /v1/devices` using the
same cookie and origin. Public keys are 32 raw bytes in canonical base64; proof
signatures are 64 bytes in canonical base64. The response supplies a bearer
token. A fresh challenge/proof renews an existing non-revoked device session.
This authenticates a device; it does **not** distribute content keys.

Native clients use Better Auth's device-authorization flow and enroll an age
recipient with `{encryptionRecipient}` included. Their v2 proof signs
`["noura.device.enroll.v2",serverOrigin,accountId,deviceId,publicKeyBase64,encryptionRecipient,challenge]`.
The temporary account bearer session is exchanged for a Noura device session
inside native code and then signed out.

All remaining `/v1` routes require `Authorization: Bearer <token>`:

| Method     | Route                                   | Purpose                                    |
| ---------- | --------------------------------------- | ------------------------------------------ |
| GET        | `/v1/devices`                           | List your devices                          |
| DELETE     | `/v1/devices/:id`                       | Revoke your device and its sessions        |
| GET/POST   | `/v1/workspaces`                        | List/create (`{id}`) workspaces            |
| POST       | `/v1/workspaces/:id/objects`            | Create an opaque object (`{id}`)           |
| POST       | `/v1/workspaces/:id/operations`         | Upload `{operations: [...]}`               |
| GET        | `/v1/workspaces/:id/operations?after=0` | Fetch an authorized page                   |
| PUT/GET    | `/v1/workspaces/:id/access`             | Commit/read signed access policy revisions |
| GET        | `/v1/workspaces/:id/access-state`       | Current policy and key-distribution state  |
| PUT        | `/v1/keys/self`                         | Back up a writer's own signed key envelope |
| GET        | `/v1/workspaces/:id/keys`               | Retrieve authorized device key envelopes   |
| POST       | `/v1/workspaces/:id/links`              | Create an encrypted public snapshot        |
| PUT/DELETE | `/v1/workspaces/:id/links/:link`        | Update/revoke a public snapshot            |

`GET /public/:token` returns an active encrypted public snapshot anonymously.
`/share/:token` is its browser viewer, `/account` handles sign-in, and
`/account/device` explicitly approves or denies desktop device codes.

The native file and key formats are documented in
[`sync-v1.md`](../../docs/workspace-format/sync-v1.md).

The operation contract lives in `packages/shared/src/sync.ts`. Sign the fixed
tuple defined in `src/protocol.ts`. Operation payloads are client-encrypted
AES-256-GCM ciphertext including the tag, with a fresh 12-byte nonce. The server
validates framing and signatures; it cannot prove a malicious client actually
encrypted its submitted bytes. Clients must validate/decrypt incoming envelopes.

Limits: 100 operations per batch, 1 MiB ciphertext per operation, 2 MiB encoded
request body, 1 GiB ciphertext quota per workspace, and 120 requests per account
per minute. The first limit reached applies. No history is automatically pruned.
Matching retries return the original sequence; changed bytes under an existing
operation ID return a conflict. A failed operation rolls back the whole batch.

An idle client can add `wait=25&accessRevision=<last-seen-revision>` to its pull.
The server subscribes before reading, waits at most 25 seconds if caught up, and
then rechecks the session and permissions. Notifications carry only workspace
IDs; all returned operations still pass the normal authorization filter. At most
1000 concurrent waiters are retained per process. This long-poll path is an
authoritative fallback for the experimental native collaboration client;
WebSocket notifications only prompt durable pulls.

## Verification

```sh
bun run check
bun run build
bun test src/protocol.test.ts
# Set the dedicated test database URL and all three compiled native probe paths first.
bun run test:integration
```

For native integration coverage, first build `cargo build -p local-core --examples`
from the repository root. Set `NOURA_NATIVE_PROBE`, `NOURA_NATIVE_SIGNIN_PROBE`,
and `NOURA_NATIVE_KEYS_PROBE` to the absolute `target/debug/examples/sync_probe`,
`signin_probe`, and `keys_probe` paths.
These run real Rust clients against HTTP and PostgreSQL, including reconnection,
conflict preservation, browser approval, device revocation, signed access policies,
and native age key distribution that rejects unpinned signers. Coordinator probes
also exercise persisted pause/resume, stable moves, deletion, approved peer key
distribution, two-way edits, convergence after explicit conflict resolution, and
streaming encrypted attachment exchange through resumable uploads. They are not a
substitute for tests with three running desktop applications.

`cargo run -p local-core --example relay_authorization_probe --locked` is a
separate malicious-relay regression. It gives a synthetic viewer the real object
key and its own approved signing identity, then serves a viewer-authored update
from a loopback relay. The gate passes only when the client rejects the update as
`sync_writer_not_authorized` and leaves the canonical file unchanged.

Use a dedicated test database. The tests create uniquely named fixtures and
retain them for inspection; they never truncate existing tables. The integration
command fails if the database URL or any compiled native probe is absent. Ordinary repository tests skip
database scenarios without that variable; the server CI job requires them.

To run the attachment and native recovery cases against an S3-compatible test
server, also set `NOURA_TEST_S3_ENDPOINT`, `NOURA_TEST_S3_BUCKET`,
`NOURA_TEST_S3_ACCESS_KEY`, and `NOURA_TEST_S3_SECRET_KEY`. These are separate from
production storage variables. The tests use unique workspace/object IDs. A local
MinIO run has verified resumed uploads, S3 range reads without a local cache,
native attachment exchange, and recovery. This does not certify a production
bucket's backup or retention configuration.

## File-relay load probe

With `NOURA_TEST_DATABASE_URL` set, run `bun scripts/load-probe.ts` for 100
HTTP clients and 20 simultaneous writers. Every client must receive all 20
signed operations once, with p95 delivery below one second and no remaining
subscriptions. A local macOS/PostgreSQL run delivered all 2,000 operations with
p50 42 ms, p95 54 ms, and maximum 56 ms, measured from each HTTP upload start.
This is one synthetic burst on loopback. It does not establish WAN performance,
CRDT convergence, collaborative editor latency, or sustained-load behavior.

## Realtime collaboration load and soak probe

With `NOURA_TEST_DATABASE_URL` set to a disposable PostgreSQL database, run:

```sh
bun run test:collaboration-load
NOURA_SOAK_SECONDS=3600 bun run test:collaboration-load
```

The default run connects 100 authenticated WebSockets, submits one concurrent
version-two text operation from each of 20 writers, simulates 100 ms RTT, and
requires every client to retrieve every operation through the authoritative HTTP
pull. It fails at one-second p95, unexpected or duplicate delivery, incomplete
fanout, subscription leakage, or more than 256 MiB RSS growth after connection
warmup. A five-second authoritative recovery pull covers a notification that was
not processed and the final report exposes the recovery count. The soak form
repeats the burst once per second for one hour while keeping latency storage
fixed-size. `NOURA_LOAD_CLIENTS`, `NOURA_LOAD_WRITERS`,
`NOURA_LOAD_RTT_MS`, `NOURA_LOAD_INTERVAL_MS`, and
`NOURA_MAX_RSS_GROWTH_MIB` override the bounded defaults. Setting
`NOURA_LOAD_FORCE_RECONNECT_BATCH=1` closes one socket after the first committed
burst and verifies recovery through an authoritative pull before reconnect; the
ordinary server CI gate enables this fault. It also sets
`NOURA_LOAD_FORCE_RECOVERY_BATCH=1` to suppress one different client's first-batch
notification and prove the five-second recovery pull closes that gap.

A local smoke run completed five batches and 10,000 deliveries with 100 clients,
20 writers, 100 ms simulated RTT, p95 202 ms, and 35.6 MiB RSS growth. This
exercises the real WebSocket invalidation and signed operation/pull path, but it
does not replace multi-process native CRDT application or cross-platform desktop
acceptance.

The corrected one-hour local soak completed 3,596 batches and 7,192,000 exact
deliveries with the same 100 clients, 20 writers, and 100 ms simulated RTT. It
measured p50 172 ms, p95 204 ms, maximum 1.129 s, and 44.7 MiB RSS growth, with
zero reconnects, zero recovery pulls, and no leaked subscriptions. A separate
forced-fault run closed one socket and suppressed another client's notification;
both recovered through authoritative pulls (`reconnects: 1`, `recoveryPulls: 1`)
while p95 remained 211 ms.

## Offline restore rehearsal

`bun run test:restore` creates two uniquely named disposable databases using
`NOURA_TEST_DATABASE_URL` (which must permit `CREATE DATABASE`). It needs
PostgreSQL 16 `pg_dump` and `pg_restore` on PATH, or their directory in
`NOURA_TEST_PG_BIN`. It never restores over an existing database.

The drill waits for all synthetic requests to complete and closes the only
writer before dumping PostgreSQL and copying the local blob directory. It
restores both into fresh locations and verifies sessions, signed operations,
cursors, quotas, complete ciphertext, continuation of a partial upload, staged
and committed transitions, checkpoints, generations, and blob manifests.
The original blob directory is removed before verification. Temporary databases
and files are removed afterwards. This drill has passed locally and is included
in server CI; CI execution remains to be verified on GitHub.

To include S3, set the four `NOURA_TEST_S3_*` variables described above. The
rehearsal also backs up its completed synthetic object, deletes it from the test
bucket, restores it, and removes its completed local cache before checking the
read. This mode passed against local MinIO. It uses unique object keys and
cleans up only its own objects; it does not recreate the bucket or test provider
versioning, regional outages, bucket policies, or production recovery times.

An operator backup needs the same offline boundary: stop every server writer,
wait for requests and storage finalization to finish, then capture PostgreSQL and
the durable blob directory together. Keep server configuration and authentication
secrets in a separate protected backup. With S3 enabled, also preserve the
referenced immutable objects and bucket configuration; this local-storage drill
does not test all production S3 failure modes. Validate a restore in an isolated deployment
before replacing a live service. Never combine independently timed database and
blob backups while writes are active.

## Remaining release requirements

Existing-member permission-management UI, folder sharing, attachment conflict
resolution and storage garbage collection, snapshot compaction, desktop public
publishing, and production backup/restore rehearsal remain unfinished or outside
this collaboration milestone. Release still requires the full desktop flow on
macOS, Windows/UTM, and Linux; a 20-editor/100 ms RTT latency measurement; the
Windows worker capability confinement and crash-durability fault tests; and an
independent security review. Backend access and public-link APIs are executable,
but do not yet form a complete user-facing sharing workflow.

Workspace invitation creation, browser acceptance, desktop fingerprint approval,
signed membership activation, and pending-invitation revocation are implemented.
Acceptance does not grant membership until the owner signs the policy and wraps
the object keys for approved recipients. The native coordinator rotates every
checkpoint-enrolled object when effective recipient devices change. A user-facing
flow for arbitrary existing-member role changes/removal and folder sharing is
still absent.

## Attachment storage

`BLOB_ROOT` holds durable ciphertext uploads and defaults to `./data/blobs`.
The Docker image uses `/data/blobs`, backed by the Compose `blobs` volume.
Do not place this directory on ephemeral storage. PostgreSQL stores quota
reservations, TUS metadata, and completion records; attachment bytes remain
separate files. Backups must include both PostgreSQL and this directory.

Setting `S3_BUCKET`, `S3_ACCESS_KEY_ID`, and `S3_SECRET_ACCESS_KEY` enables
completed ciphertext storage through Bun's S3 client. Optional `S3_ENDPOINT`
must use HTTPS except on loopback; `S3_REGION` defaults to `us-east-1`.
Compose forwards these settings. Local staging remains durable and is currently
retained after completion. A deployment using S3 must back up its bucket as well
as PostgreSQL and local staging. S3 transfer and restore have passed against a
local MinIO endpoint; provider-specific versioning, outage, and retention behavior
remains an operator deployment gate.

Workspace members can join as owner, admin, editor, or viewer. The native client
verifies the signed membership policy before choosing its transfer mode. Viewer
replicas pull remote updates and retain local edits without adding them to the
upload queue. Existing queued edits survive a downgrade but are not uploaded.

`PUT /v1/keys/share` accepts a bounded array of signed recipient envelopes from
an existing object writer. Recipients must already have object access and be
active devices. This endpoint cannot grant access, change a role, rotate an epoch,
or replace an existing recipient ciphertext. It lets editors supply keys for
newly created objects without permission-management rights. Native clients still
require explicit recipient fingerprint approval before calling it. Signed access
policies remain required for membership, grants, and epoch changes.

Desktop recovery kits are workspace-bound JSON backups containing an age identity,
public trust pins, and encrypted key envelopes. Export/import use native file
dialogs; secret bytes never enter IPC. Import into the matching joined workspace
restores keys to the newly signed-in device, retrieves authorized later envelopes,
and downloads content while leaving synchronization paused. Tests recover files
and attachments with a clean credential store that lacks the original device's
signing key, and preserve external edits on retry. Re-invitation after membership
revocation and the key-rotation UI remain release work.

## Desktop login during development

Desktop debug builds use `http://localhost:1900` for Noura Sync. Run this
server with the account web assets available, apply its migrations, and configure
`ALLOWED_EMAILS` and local email delivery as described above. **Log in** opens the
system browser; **Sign up** opens account creation with the same pending device
request. After authentication, compare the device code and approve the desktop.
The website opens Noura again, where **Enable sync** authorizes synchronization
of the current workspace. Logging in alone does not upload workspace files.

Set `NOURA_SYNC_ORIGIN` when building the desktop to use another service:

```sh
NOURA_SYNC_ORIGIN=http://localhost:1909 bun run tauri build --debug --bundles app
```

Release builds have no implicit service address. Supply `NOURA_SYNC_ORIGIN` for
managed sync; custom servers remain available under **Use a self-hosted server…**.
HTTPS is required except for loopback development addresses.

The `noura://auth/complete` return link only focuses Noura and refreshes its native
sign-in flow. It carries no credentials and cannot authorize sync. If the browser
blocks automatic opening, use **Return to Noura**. On macOS, test with the generated
`.app` bundle: URL-scheme registration is part of the bundle, not plain `tauri dev`.
Closing settings does not cancel login; cancelling explicitly, expiration, or
quitting the application ends a pending attempt.
