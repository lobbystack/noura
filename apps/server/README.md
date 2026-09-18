# Noura sync service (development)

Develop signed, encrypted sync operations with this Bun, Hono, and PostgreSQL service. It is an experimental collaboration implementation, not a production Noura collaboration release. Production startup does not enable collaboration capabilities. See [operating procedures](OPERATIONS.md) for deployment requirements, backups, and validation procedures.

Once enabled, rollout admission and protocol continuity remain separate. Setting `collaborationRollout: false` with `checkpointTransitions: true` blocks new workspace capabilities while the service continues existing checkpoints, transitions, live-text generations, and capability-bound object activations.

## Experimental service capabilities

The experimental service supports approved-device authentication, signed encrypted operation exchange, signed access policies, device-wrapped content keys, and revocable encrypted public snapshots. Native clients retain canonical file application, durable journals, conflict preservation, and local key material. Experimental invitation, checkpoint, text-generation, notification, and presence features retain durable HTTP pull and acknowledgment as their authoritative transport.

The service also supports browser devices. It verifies `x25519:` recipient enrollment and browser key envelopes, and delivers keys by envelope construction. The browser client library, `packages/browser-sync`, handles device custody, enrollment, key delivery, and operation transport. The hosted app does not call it yet, and no test runs it end to end against this service.

## Run locally

From this directory, copy `.env.example` to `.env`. Set the database connection, a random `AUTH_SECRET`, account email delivery, sender address, and `ALLOWED_EMAILS`. Choose one delivery path: `SMTP_URL` or `RESEND_API_KEY`. An empty allowlist admits nobody. No messages are printed to logs in lieu of a mail transport.

```sh
bun install
bun run migrate
bun run start
```

To include the account pages and public viewer, run `bun run build:release`, then `bun dist/main.js`. Docker packages this combined release automatically.

`apps/app` builds the browser UI. Its `(account)` route group contains `/account`, `/account/device`, `/invite/[token]`, and `/share/[token]`, outside the `(workspace)` layout and its native initialization. Browser workspace routes run the hosted notes, tasks, and projects client over browser storage and encrypted sync. They remain experimental.

`build:release` creates `apps/app/build-hosted`, verifies CSP hashes and the account routes’ static import boundary, generates bundled dependency notices, and copies the app to `dist/public`. Native builds use `apps/app/build` and Tauri’s CSP. Don’t substitute a native build for the hosted build: only the hosted build keeps the restrictive account and share CSP.

The server serves only explicit SPA destinations and asset prefixes. Missing API endpoints, missing assets, unknown paths, and non-GET page requests don’t fall back to HTML. For frontend development, `bun run --cwd ../app dev` proxies `/api`, `/public`, and `/v1` to loopback port 1900. Set the server’s `PUBLIC_ORIGIN` to the frontend origin when you use that proxy. Test secure cookies and passkeys on the same-origin release build before deployment.

Use `/health` for liveness and `/ready` for database/schema readiness. Production requires an HTTPS `PUBLIC_ORIGIN`; plain HTTP is allowed only for loopback.

`docker compose --env-file .env -f compose.yaml up --build` starts PostgreSQL, runs migrations, then starts the service. Also set `POSTGRES_PASSWORD` for Compose, using a URL-safe random value. The application listens only on host loopback; use the example Caddy configuration for external TLS termination. Neither a domain nor a managed production instance is provisioned by this code.

## API

Account routes live under `/api/auth/*` and use Better Auth cookies. Create a device challenge with `POST /v1/device-challenges` using the authenticated cookie and same-origin `Origin` header. The response contains `challenge`, `accountId`, and a five-minute lifetime.

Sign the canonical UTF-8 bytes of this JSON tuple with the device's Ed25519 private key. Serialize the array with `JSON.stringify` and no added whitespace or fields:

```text
["noura.device.enroll",serverOrigin,accountId,deviceId,publicKeyBase64,challenge]
```

Send `{deviceId, publicKey, challenge, proof}` to `POST /v1/devices` using the same cookie and origin. Public keys are 32 raw bytes in canonical base64; proof signatures are 64 bytes in canonical base64. The response supplies a bearer token. A fresh challenge/proof renews an existing non-revoked device session. This authenticates a device; it does **not** distribute content keys.

Native clients use Better Auth's device-authorization flow and include `encryptionRecipient`. Their v2 proof signs these canonical UTF-8 bytes:

```text
["noura.device.enroll.v2",serverOrigin,accountId,deviceId,publicKeyBase64,encryptionRecipient,challenge]
```

The temporary account bearer session is exchanged for a Noura device session inside native code and then signed out.

Browser devices enroll an `x25519:` recipient: the prefix plus standard padded base64 of a 32-byte X25519 public key. Their proof signs these canonical UTF-8 bytes:

```text
["noura.device.enroll.web",1,serverOrigin,accountId,deviceId,publicKeyBase64,encryptionRecipient,challenge]
```

An unusable recipient returns `sync.invalid_recipient`, and a bad proof returns `sync.invalid_signature`. The service stores the recipient string unchanged; the string grants no content key.

Key upload accepts an optional envelope `construction`:

- **Absent or `"age"`**: the native seven-field record, signed over `["noura.sync.key",1,workspaceId,objectId,epoch,signingDevice,deviceId,wrappedKey]`
- **`"web"`**: adds `recipientPublicKey`, `ephemeralPublicKey`, `salt`, and `nonce`, signed over the `noura.sync.key.web` version 1 tuple, and must wrap to the recipient device’s enrolled `x25519:` key

`noura_key_envelopes` stores `construction` (default `'age'`) and the four nullable browser columns. Key delivery and `access-state` return them for every envelope.

All remaining `/v1` routes require `Authorization: Bearer <token>`:

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/v1/devices` | List your devices |
| DELETE | `/v1/devices/:id` | Revoke your device and its sessions |
| GET/POST | `/v1/workspaces` | List/create (`{id}`) workspaces |
| POST/GET | `/v1/workspaces/:id/invitations` | Create/list workspace invitations |
| DELETE | `/v1/workspaces/:id/invitations/:id` | Revoke a pending workspace invitation |
| POST | `/v1/workspaces/:id/objects` | Create an opaque object (`{id}`) |
| POST | `/v1/workspaces/:id/operations` | Upload `{operations: [...]}` |
| GET | `/v1/workspaces/:id/operations?after=0` | Fetch an authorized page |
| PUT/GET | `/v1/workspaces/:id/access` | Commit/read signed access policy revisions |
| GET | `/v1/workspaces/:id/access-state` | Current policy and key-distribution state |
| PUT | `/v1/keys/self` | Back up a writer's own signed key envelope |
| GET | `/v1/workspaces/:id/keys` | Retrieve authorized device key envelopes |
| POST | `/v1/workspaces/:id/links` | Create an encrypted public snapshot |
| PUT/DELETE | `/v1/workspaces/:id/links/:link` | Update/revoke a public snapshot |

`GET /public/:token` returns an active encrypted public snapshot anonymously. `/share/:token` is its browser viewer, `/account` handles sign-in, and `/account/device` explicitly approves or denies desktop device codes.

## Invitation and access semantics

Workspace owners create invitations for the `admin`, `editor`, or `viewer` role. An invitation expires after seven days, and a workspace can have at most 100 active invitations. A link can be accepted by one account and can be revoked while pending.

Accepting an invitation records the prospective account. It does not grant membership or distribute content keys. Membership begins only after the owner signs an access policy and wraps object keys for approved recipient devices. The native coordinator rotates each checkpoint-enrolled object when effective recipient devices change.

The native file and key formats are documented in [`sync-v1.md`](../../docs/workspace-format/sync-v1.md).

The operation contract lives in `packages/shared/src/sync.ts`. Sign the fixed tuple defined in `src/protocol.ts`: serialize the array with `JSON.stringify`, encode it as UTF-8, and sign those bytes. Omit `generation` and `kind` for version 1 operations.

```text
["noura.sync.operation",version,workspaceId,objectId,deviceId,operationId,epoch,policyRevision,nonceBase64,ciphertextBase64,generation,kind]
```

Operation payloads are client-encrypted AES-256-GCM ciphertext including the tag, with a fresh 12-byte nonce. The server validates framing and signatures; it cannot prove a malicious client encrypted submitted bytes. Clients must validate and decrypt incoming envelopes.

Limits: 100 operations per batch, 1 MiB ciphertext per operation, 2 MiB encoded request body, 1 GiB ciphertext quota per workspace, and 120 requests per account per minute. The first limit reached applies. No history is automatically pruned. Matching retries return the original sequence; changed bytes under an existing operation ID return a conflict. A failed operation rolls back the whole batch.

An idle client can add `wait=25&accessRevision=<last-seen-revision>` to its pull. The server subscribes before reading, waits at most 25 seconds if caught up, and then rechecks the session and permissions. Notifications carry only workspace IDs; all returned operations still pass the normal authorization filter. At most 1000 concurrent waiters are retained per process. This long-poll path is an authoritative fallback for the experimental native collaboration client; WebSocket notifications only prompt durable pulls.

## Verification

Run the focused TypeScript checks and protocol test from this directory:

```sh
bun run check
bun run build
bun test src/protocol.test.ts
```

Run native integration coverage only against a dedicated test database. The tests create unique fixtures and retain them for inspection; they never truncate existing tables. The command fails unless the database URL and every compiled native probe are present. Ordinary repository tests skip database scenarios without that URL; the server CI job requires them.

Build the native probes from the repository root with `cargo build -p local-core --examples`. Set `NOURA_TEST_DATABASE_URL` and point `NOURA_NATIVE_PROBE`, `NOURA_NATIVE_SIGNIN_PROBE`, and `NOURA_NATIVE_KEYS_PROBE` to the absolute `target/debug/examples/sync_probe`, `signin_probe`, and `keys_probe` paths before running this command:

```sh
bun run test:integration
```

These run real Rust clients against HTTP and PostgreSQL, including reconnection, conflict preservation, browser approval, device revocation, signed access policies, and native age key distribution that rejects unpinned signers. Coordinator probes also exercise persisted pause/resume, stable moves, deletion, approved peer key distribution, two-way edits, convergence after explicit conflict resolution, and streaming encrypted attachment exchange through resumable uploads. They are not a substitute for tests with three running desktop applications.

`cargo run -p local-core --example relay_authorization_probe --locked` is a separate malicious-relay regression. It gives a synthetic viewer the real object key and its own approved signing identity, then serves a viewer-authored update from a loopback relay. The gate passes only when the client rejects the update as `sync_writer_not_authorized` and leaves the canonical file unchanged.

To run the attachment and native recovery cases against an S3-compatible test server, also set `NOURA_TEST_S3_ENDPOINT`, `NOURA_TEST_S3_BUCKET`, `NOURA_TEST_S3_ACCESS_KEY`, and `NOURA_TEST_S3_SECRET_KEY`. These are separate from production storage variables. The tests use unique workspace/object IDs. Validate a production bucket's backup and retention configuration separately.

## File-relay load probe

Run the [file-relay probe procedure](OPERATIONS.md#file-relay-load-probe) against a disposable database. The runbook defines its client and writer counts, acceptance criteria, and scope limits.

## Realtime collaboration load and soak probe

Run the [collaboration probe procedure](OPERATIONS.md#realtime-collaboration-load-and-soak-probe) against a disposable database. It documents the p95 default and CI override, fault injections, soak configuration, and scope limits.

## Offline restore rehearsal

Run the [restore rehearsal](OPERATIONS.md#offline-restore-rehearsal) before changing backup or storage operations. The runbook specifies its offline boundary, test database permissions, S3 mode, and exclusions.

## Attachment storage

See [attachment storage and retention](OPERATIONS.md#attachment-storage-and-retention) for durable storage locations, S3 requirements, and backup scope.

## Member roles and replica behavior

The native client verifies the signed membership policy before selecting a transfer mode. Workspace members can join as owner, admin, editor, or viewer. Viewer replicas pull remote updates and retain local edits without adding them to the upload queue. Existing queued edits survive a downgrade but are not uploaded.

## Recipient key distribution

`PUT /v1/keys/share` accepts a bounded array of signed recipient envelopes from an existing object writer. Recipients must already have object access and be active devices. This endpoint cannot grant access, change a role, rotate an epoch, or replace an existing recipient ciphertext. It lets editors supply keys for newly created objects without permission-management rights. Native clients still require explicit recipient fingerprint approval before calling it. Signed access policies remain required for membership, grants, and epoch changes.

## Desktop recovery kits

Recovery kits let a newly signed-in device restore workspace keys without sending secret bytes through IPC.

Desktop recovery kits are workspace-bound JSON backups containing an age identity, public trust pins, and encrypted key envelopes. Export/import use native file dialogs; secret bytes never enter IPC. Import into the matching joined workspace restores keys to the newly signed-in device, retrieves authorized later envelopes, and downloads content while leaving synchronization paused. Tests recover files and attachments with a clean credential store that lacks the original device's signing key, and preserve external edits on retry. Re-invitation after membership revocation and the key-rotation UI remain release work.

## Desktop login during development

Desktop debug builds use `http://localhost:1900` for Noura Sync. Run this server with the account web assets available, apply its migrations, and configure `ALLOWED_EMAILS` and local email delivery as described above. **Log in** opens the system browser; **Sign up** opens account creation with the same pending device request. After authentication, compare the device code and approve the desktop. The website opens Noura again, where **Enable sync** authorizes synchronization of the current workspace. Logging in alone does not upload workspace files.

Set `NOURA_SYNC_ORIGIN` when building the desktop to use another service:

```sh
NOURA_SYNC_ORIGIN=http://localhost:1909 bun run tauri build --debug --bundles app
```

Release builds have no implicit service address. Supply `NOURA_SYNC_ORIGIN` for managed sync; custom servers remain available under **Use a self-hosted server…**. HTTPS is required except for loopback development addresses.

The `noura://auth/complete` return link only focuses Noura and refreshes its native sign-in flow. It carries no credentials and cannot authorize sync. If the browser blocks automatic opening, use **Return to Noura**. On macOS, test with the generated `.app` bundle: URL-scheme registration is part of the bundle, not plain `tauri dev`. Closing settings does not cancel login; cancelling explicitly, expiration, or quitting the application ends a pending attempt.
