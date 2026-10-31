# How do I run my own Noura sync server?

You run one binary, `sync-server`, with a data directory beside it. With no database URL it keeps everything in SQLite in that directory, on one instance. With a PostgreSQL URL you can run several instances on one database. The server stores only ciphertext and the metadata that [§29 of the sync protocol](workspace-format/sync-v2.md#29-what-the-server-stores) lists, and your devices keep the keys.

## Start the server

Use the container image, or run the binary yourself. Either way, a device needs an HTTPS origin to reach the server, so set `NOURA_PUBLIC_ORIGIN` on anything but a loopback address.

### Run the container image

Build the image from the repository root, then start it with a volume for `/data`:

```sh
docker build -f crates/sync-server/Dockerfile -t noura-sync-server .
docker run -d --name noura-sync \
  -v noura-data:/data -p 8080:8080 \
  -e NOURA_PUBLIC_ORIGIN=https://sync.example.com \
  -e NOURA_ALLOWED_EMAILS=you@example.com \
  -e NOURA_SMTP_URL=smtp://user:your_smtp_password_here@mail.example.com \
  noura-sync-server
```

The image holds the static binary and the account-web module that the account pages load. It runs as the user `noura` (UID 10001), listens on `0.0.0.0:8080` and keeps its data in `/data`. Its `HEALTHCHECK` runs `sync-server healthcheck`.

The entrypoint runs `migrate` before `serve`, so an empty volume starts on its own. On a platform that mounts volumes as root, start the container as root: the entrypoint hands `/data` to `noura` and drops root before the server starts.

### Run the binary

Build `sync-server` with `cargo build -p sync-server --release`, and `scripts/build-account-web.sh` for the pages' module. CI builds the binary for `x86_64-unknown-linux-musl`, which links nothing at run time.

```sh
sync-server migrate --data /var/lib/noura
NOURA_ACCOUNT_WEB_DIR=/opt/noura/account-web \
  sync-server serve --data /var/lib/noura --listen 127.0.0.1:8080
```

`serve` refuses a database that `migrate` hasn't brought up to date. Passkeys and the share viewer need `NOURA_ACCOUNT_WEB_DIR`.

## Commands

Each command reads `NOURA_DATA_DIR`, `NOURA_DATABASE_URL` and `NOURA_LISTEN`, and the flags override them:

| Command | What it does |
|---|---|
| `migrate [--data DIR] [--database-url URL]` | Creates or updates the database. In SQLite mode the first run creates the server secret. |
| `serve [--data DIR] [--database-url URL] [--listen ADDRESS] [--origin ORIGIN]` | Serves until it gets an interrupt or a terminate signal, then waits up to 30 seconds for requests in flight. |
| `healthcheck [--listen ADDRESS]` | Exits 0 when a Noura sync server answers `GET /v2/ready` within 5 seconds. |
| `backup --to FILE [--data DIR]` | Copies a running SQLite database, and writes the secret to `FILE.secret`. |
| `restore --from FILE [--secret FILE] [--data DIR]` | Puts a SQLite backup into a data directory. |
| `mark-restored [--data DIR] [--database-url URL]` | Records a restore you did with other tools. |
| `blobs migrate-to-s3 [--data DIR]` | Moves blob segments from the data directory to S3. |
| `version` | Prints the version and the libraries the binary links. |

## Settings

`serve` reads these variables:

| Variable | Default | Meaning |
|---|---|---|
| `NOURA_DATA_DIR` | `data` | The data directory |
| `NOURA_LISTEN` | `127.0.0.1:8080` | The address and port |
| `NOURA_DATABASE_URL` | none | The PostgreSQL URL; without one the server uses SQLite |
| `NOURA_SERVER_SECRET` | none | PostgreSQL mode: the secret as 64 hexadecimal digits |
| `NOURA_SERVER_SECRET_FILE` | none | PostgreSQL mode: a mounted file that holds the secret |
| `NOURA_PUBLIC_ORIGIN` | `http://localhost:{port}` on loopback | The HTTPS origin people reach the server at |
| `NOURA_ALLOWED_EMAILS` | empty | The addresses that may sign up, separated by commas; empty admits nobody |
| `NOURA_MAIL_FROM` | `Noura <noura@localhost>` | The sender of sign-in mail |
| `NOURA_SMTP_URL` | none | Send mail through this SMTP server |
| `NOURA_RESEND_API_KEY` | none | Send mail through Resend, when no SMTP URL is set |
| `NOURA_RESEND_ENDPOINT` | Resend's | Another endpoint for that API |
| `NOURA_ACCOUNT_WEB_DIR` | none | The account-web build; the image sets it |
| `NOURA_RETAINED_BYTES` | 1 GiB | The log weight each domain keeps behind a checkpoint |
| `NOURA_WORKSPACE_QUOTA_BYTES` | unlimited | The storage quota of each workspace |
| `NOURA_S3_BUCKET` | none | Keep blob segments in this bucket |
| `NOURA_S3_ENDPOINT` | AWS | The endpoint of an S3-compatible service |
| `NOURA_TRUSTED_PROXY_HEADER` | none | The header your reverse proxy sets to the client's IP |
| `NOURA_MAX_CONNECTIONS` | 1024 | How many connections stay open at once, realtime channels included; further ones wait |

Keep the process's file descriptor limit above `NOURA_MAX_CONNECTIONS`. Give a container at least 40 seconds to stop, so the 30-second wait ends before the runtime kills it. A server that gets killed leaves its lease behind for up to 90 seconds.

`NOURA_RETAINED_BYTES` bounds the history a device can catch up from without a new bootstrap. `NOURA_WORKSPACE_QUOTA_BYTES` counts live data, policy changes and epoch bundles, and history gives way to the quota while those fit.

### Choose who can sign up

Only addresses in `NOURA_ALLOWED_EMAILS` can create an account, and an empty list admits nobody. Sign-up and email sign-in also need a mail transport.

### Set the trusted proxy header

Sign-in limits count per client IP. Behind a reverse proxy, every connection comes from the proxy, so set `NOURA_TRUSTED_PROXY_HEADER` to the header it sets, such as `x-real-ip`. With a list such as `x-forwarded-for`, the server reads the last address, which your proxy added.

Set it only when a proxy overwrites or appends to that header on every request. If clients can reach the server directly, they can name any address and escape the limit. With no setting, the server ignores all such headers and uses the connection's address. It counts an IPv6 client by its /64, and it stores a keyed hash of the address in its counters.

### Change the rate limits

The defaults let a new device download a workspace of 100,000 objects and 10 GB of attachments in about 20 minutes. Each variable takes a whole number:

| Variable | Default | Counts per device |
|---|---|---|
| `NOURA_RATE_PUSHES_PER_SECOND` | 10 | Pushes |
| `NOURA_RATE_PUSH_BYTES_PER_MINUTE` | 67108864 (64 MiB) | Push bodies |
| `NOURA_RATE_UPLOAD_BYTES_PER_MINUTE` | 536870912 (512 MiB) | Blob uploads |
| `NOURA_RATE_READ_BYTES_PER_MINUTE` | 134217728 (128 MiB) | Pulls, chain and checkpoint fetches, policy and keys |
| `NOURA_RATE_BOOTSTRAP_BYTES_PER_MINUTE` | 536870912 (512 MiB) | `live`, `manifest` and blob range reads |
| `NOURA_RATE_OTHER_REQUESTS_PER_MINUTE` | 600 | Other sync requests |
| `NOURA_RATE_SIGN_IN_PER_MINUTE` | 30, per client IP | Device grants, magic links, passkeys and reset codes |

Keep each byte budget above 1 MiB, the largest single request or response. A smaller push or upload budget refuses a full-size request in every window. A session with no card yet counts against its account. A request over a limit gets HTTP 429 with `rate_limited` and `retry_after_seconds`, and the desktop app waits and tries again. In PostgreSQL mode, all instances count against one allowance.

## The server secret

Account IDs derive from a 32-byte server secret, so losing it strands every account. The database holds a key check of the secret, and `serve`, `migrate`, `restore` and `mark-restored` refuse a missing or different secret.

- **SQLite**: the first `migrate` creates `server-secret` in the data directory. Don't delete or replace it.
- **PostgreSQL**: create the secret with `openssl rand -hex 32` and give every instance the same value through `NOURA_SERVER_SECRET` or a file that `NOURA_SERVER_SECRET_FILE` names. `migrate` refuses to run without it.

Keep the secret with each backup. `backup` writes it beside the SQLite copy, and on PostgreSQL you store it yourself. A restore under another secret would change every account ID. If you lose the secret, you can only start over with a fresh `migrate`, and people then turn their folders on again as new workspaces under new accounts.

## Run PostgreSQL

Point `NOURA_DATABASE_URL` at a database and run `migrate` once with the secret, then start as many `serve` instances as you need. Each instance keeps only its log files in its data directory, and all instances need one shared blob store, so use S3. Add `sslmode=require` to the URL for a hosted database.

Three PostgreSQL settings protect what your server knows and what it can recover:

- **Keep `track_commit_timestamp` off.** It would date every record, and `serve` refuses to start with it on.
- **Keep WAL retention short and archive no WAL** if the database shouldn't date pushes. The write-ahead log and its archives hold commit times.
- **Use synchronous replication** for a failover target. An asynchronous standby can lose commits that devices already saw, and you then must run `mark-restored`.

## Store blobs in S3

By default blob segments live under `blobs` in the data directory. Set `NOURA_S3_BUCKET` to use a bucket, with `NOURA_S3_ENDPOINT` for an S3-compatible service. The standard `AWS_*` variables give the credentials and region.

To move an existing store, stop `serve`, then run the migration with the same variables set:

```sh
NOURA_S3_BUCKET=your_bucket_name_here sync-server blobs migrate-to-s3 --data /var/lib/noura
```

It checks each segment's hash before it deletes the local copy. Run it again after an interruption, and set `NOURA_S3_BUCKET` when you start `serve`.

## Back up and restore

Back up the database, the secret and the blob store together. On SQLite, `backup` copies the database while `serve` runs:

```sh
sync-server backup --to /backups/noura-2026-10-31.sqlite3 --data /var/lib/noura
```

The copy carries a stamp, and `serve` refuses a stamped database, so you can't start it by accident. On PostgreSQL, use `pg_dump` or `pg_basebackup`, and store the secret with each backup.

### Restore SQLite

Stop `serve`, then run `restore` with the old data directory or a fresh one. It finds the secret beside the backup unless you pass `--secret FILE`:

```sh
sync-server restore --from /backups/noura-2026-10-31.sqlite3 --data /var/lib/noura
```

`serve` may start as soon as the command prints the new restore generation, while the blob store check finishes.

If the old database is too damaged to read, `restore` can't see a lease in it, so make sure `serve` has stopped. The command keeps the damaged file beside the new database as `sync.sqlite3.damaged-` plus the time, and prints the name. Delete it once you no longer need it.

### Restore PostgreSQL or fail over

Restore the database with its own tools, or promote the standby, then run `mark-restored` before `serve`:

```sh
sync-server mark-restored --data /var/lib/noura --database-url "$NOURA_DATABASE_URL"
```

Run `mark-restored` after any restore or failover that can lose commits, and after you copy a SQLite file back by hand. Clients stop syncing with a database you put back without it, and stay stopped until it runs. Both commands refuse to run while an instance holds a lease on the database. A crashed instance's lease ends on its own.

### What a restore does to the blob store

The restore checks the blob store against the database. Objects the database doesn't know move under the `quarantine` prefix, and the server deletes them after 7 days. An object the database comes to know again moves back. Blobs the database lists but the store lacks wait for devices to upload them again.

### What a restore does to people

Files stay on devices, and devices write lost content again as new operations. Each person's device compares what it accepted with what the server holds:

- **Lost content**: the device writes it again from its files, and the app lists what it wrote. A change that no surviving device applied is gone.
- **Lost account link**: that account's devices stop until its person resets their devices. Other members stop syncing workspaces shared with that account until an owner or admin accepts the reset.
- **Lost policy change, epoch record or envelope**: the workspace stops on devices that relied on it, until an owner sets the workspace up again.

Tell your users after a restore that lost signed state, and point them at Reset devices and Set up sync again. Their sessions from after the backup are gone too, so they sign in again.

## Check that the server is healthy

Run `sync-server healthcheck --listen 127.0.0.1:8080`. It asks `GET /v2/ready`, which takes a real write through the store, so it fails when the server can't commit anything, and it answers 200 at most one write every 2 seconds. It takes no session and has no rate limit, so an HTTP health check can use that path. `GET /v2/limits` answers from memory and shows only that the process is up.

The server exits with an error in two cases that only a restart cures: its database moved to another restore generation, or its SQLite writer stopped. Run it under something that restarts it.

The server writes daily log files to `logs` in the data directory and keeps them for 14 days. They hold route templates, request IDs and status codes. Each file stops taking request lines at 64 MiB and goes on with warnings and errors up to 128 MiB.
