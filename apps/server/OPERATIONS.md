# Operating the development server

The Docker image contains the Bun service, migrations, account pages, public viewer, and a generated dependency-notice file. Compose starts it as the unprivileged `bun` user. Railway mounts volumes as root, so on Railway set `RAILWAY_RUN_UID=0`: the image entrypoint then starts as root only long enough to take ownership of the mounted `BLOB_ROOT`, and drops to `bun` before the service process starts. The long-running server never runs as root. Put HTTPS termination in front of it and configure `PUBLIC_ORIGIN` to the exact external origin. Allowlisted email, mail delivery (`SMTP_URL` or `RESEND_API_KEY`), and passkey origin configuration must agree with that origin. Set `TRUSTED_IP_HEADER` to the header your reverse proxy overwrites with the client address; authentication rate limiting keys on it.

## Startup and health

Run `bun migrate.js` from the image before starting `bun main.js`. The supplied Compose file separates migration from the service and waits for PostgreSQL. `/health` checks process liveness; `/ready` also checks database/schema readiness. Neither endpoint establishes that mail delivery or passkey registration works.

Keep `AUTH_SECRET`, mail delivery credentials, and database credentials in the operator's secret store. The database stores session hashes and encrypted workspace operations. It still contains account and authorization metadata, so restrict database access and protect backups. The server never possesses workspace content keys; a database restore cannot recover lost users' device keys and recovery material.

## Backup and restore

The implementation stores encrypted operations, public snapshots, key envelopes, policies, blob metadata, and authentication records in PostgreSQL. Attachment ciphertext lives in `BLOB_ROOT`; completed objects may also live in configured S3-compatible storage. A backup must capture PostgreSQL and local staging at one offline boundary and preserve every referenced S3 object. Preserve the matching server image and operator configuration separately, including `AUTH_SECRET`. Do not commit backups or secrets to Git.

Use PostgreSQL service definitions and a protected password file instead of placing credentials directly in commands. With `noura` and `noura-restore` service definitions configured by the operator:

```sh
umask 077
pg_dump --dbname=service=noura --format=custom --no-owner --no-acl --file=noura.dump
pg_restore --exit-on-error --no-owner --no-acl --dbname=service=noura-restore noura.dump
```

Restore into an empty, isolated database, never over the running service. Start the matching image against it with outbound email disabled at the network boundary. Compare table counts and deterministic row hashes with the backup's recorded inventory. Then exercise readiness, a prearranged test account, an authorized encrypted pull, and a revoked token/link. Only switch production traffic after those checks succeed. Keep the old database isolated until the operator's rollback window closes.

A local PostgreSQL 16 rehearsal on 2026-09-05 restored the complete integration test database into a new temporary database and compared every public table's row count and sorted JSON row hash. It passed, and the rehearsal removed the temporary database. This verifies the current database-only backup mechanics; it is not a production disaster-recovery rehearsal, offsite retention policy, or measured recovery-time guarantee.

## Limits and failure behavior

There is no history pruning or snapshot compaction yet. Monitor database growth. The default workspace ciphertext quota is 1 GiB. The service limits requests to 2 MiB, individual ciphertext envelopes to 1 MiB, and resumable attachment ciphertext to 1 GiB. Rate limits apply per account; this is not a substitute for perimeter connection limits or a measured capacity plan.

For a repeatable local transport probe, run from the repository root:

```sh
NOURA_TEST_DATABASE_URL=postgres://user@localhost/noura_test bun apps/server/scripts/load-probe.ts
```

It creates unique test fixtures and measures delivery of one signed opaque operation to 100 simultaneous HTTP readers. A local PostgreSQL 16 run on 2026-09-05 measured 42 ms p50, 54 ms p95, and 56 ms maximum from push start through response parsing, with zero remaining subscriptions. These are local transport measurements, not WAN performance, a soak test, or the planned 20-editor collaboration acceptance test. The probe's payload is synthetic framing data; native integration tests separately exercise authenticated encryption and file application.

Native clients retain ordered encrypted outboxes through outages. Pull cursors advance after canonical file application or durable conflict preservation. Missing content keys and untrusted senders stop a pull without advancing its cursor. Local files remain usable when the service is unavailable.

Device revocation prevents future server access. Removing access requires a new object-key epoch before further updates; clients still need the corresponding rotation and approval workflows. Revocation cannot erase plaintext or keys a recipient previously obtained. Public-link revocation prevents further server fetches; a recipient can retain an already decrypted snapshot.

## Release gates still open

Invitation and permission-management UI, recovery re-invitation and rotation, collaborative text editing and presence, compaction, attachment garbage collection, and desktop public publishing remain unfinished. Sustained soak measurements, multi-platform desktop UI runs, real SMTP/passkey deployment checks, and external security review are also outstanding. This image is a development build.
