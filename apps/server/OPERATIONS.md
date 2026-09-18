# Operating the development server

Deploy, back up, and validate the Noura sync service with this runbook. The service remains an experimental collaboration implementation. Do not treat a configured GitHub Actions workflow or a local probe as release approval. Developers should use the [server README](README.md) for local setup and API contracts.

## Prepare a deployment

The image runs as the unprivileged `bun` user. Run it behind HTTPS termination and set `PUBLIC_ORIGIN` to that exact external origin. Passkey configuration, allowlisted email, and mail delivery must use the same origin. The supplied Compose configuration binds the service to `127.0.0.1:1900`; run Caddy or another TLS proxy on the host.

Set these values in an operator secret store:

- `DATABASE_URL`
- `AUTH_SECRET`, with at least 32 random characters
- `MAIL_FROM`
- One mail delivery setting: `SMTP_URL` or `RESEND_API_KEY`
- `POSTGRES_PASSWORD` when using Compose

Set `ALLOWED_EMAILS` for every account that may sign in. An empty value admits no accounts. Set `TRUSTED_IP_HEADER` only to a header that the reverse proxy overwrites with the client address. Authentication rate limiting uses that header when configured.

Keep `AUTH_SECRET`, mail credentials, and database credentials out of the repository. PostgreSQL stores session hashes, encrypted workspace operations, and account and authorization metadata. Restrict database access and protect backups. The service does not hold workspace content keys, so a database restore cannot recover lost device keys or recovery material.

## Startup and health

Run migrations before starting the service. The supplied Compose file runs `bun migrate.js` after PostgreSQL passes its health check, then starts `bun main.js`.

`/health` checks process liveness. `/ready` also checks database and schema readiness. Neither endpoint checks mail delivery or passkey registration.

The server accepts a non-loopback `PUBLIC_ORIGIN` only over HTTPS. Its optional S3 endpoint also requires HTTPS except on loopback. The repository does not provision a domain or a managed production instance.

## Backup and restore

Capture the database and local blob storage at one offline boundary. Stop every server writer, wait for requests and storage finalization to finish, then capture PostgreSQL and `BLOB_ROOT` together. Do not combine independently timed database and blob backups while writes are active.

PostgreSQL contains encrypted operations, public snapshots, key envelopes, policies, blob metadata, and authentication records. `BLOB_ROOT` contains attachment ciphertext and defaults to `./data/blobs`; Compose mounts it at `/data/blobs`. Preserve referenced S3 objects and the bucket configuration when S3 storage is enabled. Keep the matching server image, operator configuration, and authentication secrets in a separate protected backup.

Use PostgreSQL service definitions and a protected password file instead of credentials in commands. With `noura` and `noura-restore` service definitions:

```sh
umask 077
pg_dump --dbname=service=noura --format=custom --no-owner --no-acl --file=noura.dump
pg_restore --exit-on-error --no-owner --no-acl --dbname=service=noura-restore noura.dump
```

Restore into an empty, isolated database, never over the running service. Start the matching image with outbound email blocked at the network boundary. Compare table counts and deterministic row hashes with the backup inventory. Then check readiness, a prearranged test account, an authorized encrypted pull, and a revoked token or public link. Switch production traffic only after these checks succeed. Keep the old database isolated until the rollback window closes.

## Attachment storage and retention

Keep `BLOB_ROOT` on durable storage. PostgreSQL holds quota reservations, TUS metadata, and completion records; attachment bytes remain separate ciphertext files. There is no storage garbage collection yet, so monitor database and blob growth.

Set `S3_BUCKET`, `S3_ACCESS_KEY_ID`, and `S3_SECRET_ACCESS_KEY` to store completed ciphertext with Bun’s S3 client. `S3_REGION` defaults to `us-east-1`. Use the optional `S3_ENDPOINT` for a custom endpoint; HTTPS is required outside loopback.

Local staging remains durable after completion, so back up the bucket and local staging with PostgreSQL. Validate provider versioning, outage behavior, retention, bucket policies, and recovery time for the selected storage service.

## Limits and failure behavior

Monitor capacity against these current limits:

- 1 GiB ciphertext quota per workspace
- 2 MiB encoded request body
- 1 MiB ciphertext per operation
- 1 GiB resumable attachment ciphertext
- 120 requests per account per minute

No history pruning or snapshot compaction runs yet. Account rate limits do not replace perimeter connection limits or a measured capacity plan.

Native clients retain ordered encrypted outboxes during outages. Pull cursors advance only after canonical file application or durable conflict preservation. Missing content keys and untrusted senders stop a pull without advancing its cursor. Local files remain usable while the service is unavailable.

Device revocation blocks future server access. Removing access requires a new object-key epoch before later updates; clients also need the matching rotation and approval workflows. Revocation cannot erase plaintext or keys that a recipient obtained earlier. Public-link revocation blocks later server fetches, but recipients can retain decrypted snapshots.

## File-relay load probe

Run this transport probe from the repository root against a disposable database:

```sh
NOURA_TEST_DATABASE_URL=postgres://user@localhost/noura_test bun apps/server/scripts/load-probe.ts
```

The probe creates unique fixtures, opens 100 simultaneous HTTP pull clients, and submits one signed opaque operation from each of 20 concurrent writers. Each client must receive all 20 operations once. The probe fails when p95 delivery is at least 1,000 ms or subscriptions remain after cleanup.

## Realtime collaboration load and soak probe

Run the realtime probe against a disposable PostgreSQL database:

```sh
NOURA_TEST_DATABASE_URL=postgres://user@localhost/noura_test bun run --cwd apps/server test:collaboration-load
NOURA_TEST_DATABASE_URL=postgres://user@localhost/noura_test NOURA_SOAK_SECONDS=3600 bun run --cwd apps/server test:collaboration-load
```

The default run connects 100 authenticated WebSockets. Twenty writers each submit one version-two text operation with 100 ms simulated round-trip time (RTT). Every client must retrieve every operation through the authoritative HTTP pull. The default p95 budget is below 1,000 ms. The regular server workflow sets `NOURA_LOAD_P95_BUDGET_MS=3000` for its shared runner; its configuration does not establish a passing CI outcome.

The probe fails for p95 at or above the configured budget, duplicate or missing delivery, incomplete fanout, subscription leakage, or RSS growth above 256 MiB after connection warmup. A recovery pull runs after five seconds for missed notifications. The soak run repeats the burst once per second for one hour while retaining a fixed-size latency histogram.

Bounded overrides include `NOURA_LOAD_CLIENTS`, `NOURA_LOAD_WRITERS`, `NOURA_LOAD_RTT_MS`, `NOURA_LOAD_INTERVAL_MS`, `NOURA_MAX_RSS_GROWTH_MIB`, and `NOURA_LOAD_P95_BUDGET_MS`. Set `NOURA_LOAD_FORCE_RECONNECT_BATCH=1` to close one socket after the first committed burst and verify an authoritative pull before reconnect. Set `NOURA_LOAD_FORCE_RECOVERY_BATCH=1` to suppress one client's first-batch notification and verify the five-second recovery pull. The regular server workflow configures both faults. The scheduled soak workflow configures 100 clients, 20 writers, 100 ms RTT, and a one-hour duration; configuration is not execution evidence.

These probes exercise WebSocket invalidation and signed operation pulls. They do not replace multi-process native CRDT application, cross-platform desktop acceptance, or the 20-editor latency measurement required for release.

## Offline restore rehearsal

Run `bun run --cwd apps/server test:restore` only with a disposable `NOURA_TEST_DATABASE_URL` that permits `CREATE DATABASE`. The rehearsal creates two unique databases and never restores over an existing database. Install PostgreSQL 16 `pg_dump` and `pg_restore` on `PATH`, or set `NOURA_TEST_PG_BIN` to their directory.

The rehearsal waits for synthetic requests to finish, closes its only writer, dumps PostgreSQL, and copies the local blob directory. It restores both to fresh locations and verifies sessions, signed operations, cursors, quotas, complete ciphertext, a partial-upload continuation, staged and committed transitions, checkpoints, generations, and blob manifests. It removes the original blob directory before verification, then removes temporary databases and files.

Set `NOURA_TEST_S3_ENDPOINT`, `NOURA_TEST_S3_BUCKET`, `NOURA_TEST_S3_ACCESS_KEY`, and `NOURA_TEST_S3_SECRET_KEY` to include an S3-compatible test server. This mode backs up a completed synthetic object, deletes it from the test bucket, restores it, removes its completed local cache, and checks the read. It uses unique object keys and removes only its own objects. It does not create a bucket or test provider versioning, regional outages, bucket policies, or production recovery time.

The repository configures this rehearsal in the regular server workflow. Verify the workflow's current GitHub result before relying on it as CI evidence.

## Public service scope and security caveats

This service stores and routes signed encrypted records. Native clients retain content keys and canonical workspace files. The service remains experimental, and standard startup disables collaboration capabilities. Do not represent it as a production Noura collaboration service.

Collaboration notifications and transient presence do not replace durable HTTP pull and acknowledgment. Follow the protocol contracts and validation procedures before enabling experimental routes.
