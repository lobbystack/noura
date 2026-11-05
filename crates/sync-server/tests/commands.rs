//! The binary's commands at seam 4: `migrate`, `serve` and `healthcheck`
//! on a data directory, with the secret file beside the database (§14).

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use sqlx::{ConnectOptions, Connection};
use support::{BIN, Client, Server, fill, ops_path, pull, push_body, run, seed};
use sync_protocol::codec::{Encode, List};
use sync_protocol::ids::secret_check;
use sync_protocol::messages::{
    LimitsResponse, PullCursor, PullRequest, PullResponse, PushResponse,
};
use sync_protocol::types::{Digest, SecretKey};

/// Every file under `dir`, relative to it.
fn files(dir: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).expect("readable") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path.clone());
            }
            found.insert(path.strip_prefix(dir).expect("inside").to_owned());
        }
    }
    found
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

async fn database(data: &Path) -> sqlx::SqliteConnection {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(data.join(sync_store_sqlite::DATABASE_FILE))
        .connect()
        .await
        .expect("the database opens")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_then_serve_on_an_empty_data_directory() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    let first = run(&["migrate"], &data);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(stderr(&first).contains("created the server secret"));
    let secret = std::fs::read(data.join("server-secret")).expect("the secret file");
    assert_eq!(secret.len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data.join("server-secret"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "only the owner reads the secret");
    }

    // The database holds the key check, never the secret.
    let mut connection = database(&data).await;
    let (check, generation): (Vec<u8>, i64) =
        sqlx::query_as("SELECT secret_check, restore_generation FROM server")
            .fetch_one(&mut connection)
            .await
            .expect("the server row");
    let applied: Vec<(i64,)> =
        sqlx::query_as("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .expect("migrations");
    connection.close().await.expect("closed");
    let key = SecretKey(secret.clone().try_into().expect("32 bytes"));
    assert_eq!(check, secret_check(&key).expose().to_vec());
    assert_ne!(check, secret);
    assert!(
        generation > 1_000_000_000,
        "the generation starts at the Unix time"
    );
    let dump = std::fs::read(data.join(sync_store_sqlite::DATABASE_FILE)).expect("database");
    assert!(
        !dump.windows(32).any(|w| w == secret.as_slice()),
        "the database never holds the secret"
    );

    // A second migrate changes nothing.
    let second = run(&["migrate"], &data);
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(stderr(&second).contains("ran 0 migration(s)"));
    assert_eq!(
        std::fs::read(data.join("server-secret")).expect("secret"),
        secret
    );
    let mut connection = database(&data).await;
    let again: Vec<(i64,)> =
        sqlx::query_as("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .expect("migrations");
    let (check_again,): (Vec<u8>,) = sqlx::query_as("SELECT secret_check FROM server")
        .fetch_one(&mut connection)
        .await
        .expect("the server row");
    connection.close().await.expect("closed");
    assert_eq!((again, check_again), (applied, check));

    // `serve` then answers.
    let server = Server::start(&data);
    let limits: LimitsResponse = Client::new(server.base(), None)
        .get("/v2/limits")
        .await
        .decode();
    assert_eq!(limits.protocol, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_refuses_pending_migrations() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    let unmigrated = Command::new(BIN)
        .arg("serve")
        .arg("--data")
        .arg(&data)
        .output()
        .expect("runs");
    assert!(!unmigrated.status.success());
    assert!(
        stderr(&unmigrated).contains("run `sync-server migrate` first"),
        "{}",
        stderr(&unmigrated)
    );

    assert!(run(&["migrate"], &data).status.success());
    let mut connection = database(&data).await;
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&mut connection)
        .await
        .expect("a migration forgotten");
    connection.close().await.expect("closed");
    let pending = Command::new(BIN)
        .arg("serve")
        .arg("--data")
        .arg(&data)
        .output()
        .expect("runs");
    assert!(!pending.status.success());
    assert!(
        stderr(&pending).contains("are pending"),
        "{}",
        stderr(&pending)
    );
}

/// An older schema built from the real migrations up to `version`, holding
/// every row of the database at `from` that fits it.
async fn older_database(from: &Path, data: &Path, version: i64) {
    std::fs::create_dir_all(data).expect("a data directory");
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(data.join(sync_store_sqlite::DATABASE_FILE))
        .create_if_missing(true)
        .connect()
        .await
        .expect("the database opens");
    sync_store_sqlite::MIGRATOR
        .run_to(version, &mut connection)
        .await
        .expect("the older schema");
    sqlx::query("ATTACH DATABASE ? AS seeded")
        .bind(from.to_str().expect("a UTF-8 path"))
        .execute(&mut connection)
        .await
        .expect("attached");
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlite%'",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the tables");
    for (table,) in tables {
        let columns: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info(?)")
            .bind(&table)
            .fetch_all(&mut connection)
            .await
            .expect("the columns");
        let newer: BTreeSet<String> =
            sqlx::query_as::<_, (String,)>("SELECT name FROM pragma_table_info(?, 'seeded')")
                .bind(&table)
                .fetch_all(&mut connection)
                .await
                .expect("the newer columns")
                .into_iter()
                .map(|(name,)| name)
                .collect();
        let shared: Vec<String> = columns
            .into_iter()
            .map(|(name,)| name)
            .filter(|name| newer.contains(name))
            .collect();
        if shared.is_empty() {
            continue;
        }
        let shared = shared.join(", ");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT OR REPLACE INTO main.{table} ({shared}) SELECT {shared} FROM seeded.{table}"
        )))
        .execute(&mut connection)
        .await
        .expect("the rows");
    }
    sqlx::query("DETACH DATABASE seeded")
        .execute(&mut connection)
        .await
        .expect("detached");
    connection.close().await.expect("closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_upgrades_an_older_database_and_keeps_what_it_holds() {
    // Real data, through the server.
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    assert!(run(&["migrate"], &data).status.success());
    let account = seed(&data, 1).await;
    let created = account.creation();
    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(0)));
    let held = fill(&client, &created, &account).await;
    let before = pull(&client, &created).await;
    server.kill();

    let versions: Vec<i64> = sync_store_sqlite::MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect();
    let newest = *versions.last().expect("migrations");
    for older in versions.iter().copied().filter(|v| *v < newest) {
        let root = tempfile::tempdir().expect("temporary directory");
        let upgraded = root.path().join("data");
        older_database(
            &data.join(sync_store_sqlite::DATABASE_FILE),
            &upgraded,
            older,
        )
        .await;
        std::fs::copy(data.join("server-secret"), upgraded.join("server-secret"))
            .expect("the secret");

        // `serve` waits for the upgrade, and one command runs it.
        let refused = run(&["serve"], &upgraded);
        assert!(!refused.status.success());
        assert!(
            stderr(&refused).contains("are pending"),
            "{}",
            stderr(&refused)
        );
        let migrate = run(&["migrate"], &upgraded);
        assert!(migrate.status.success(), "{}", stderr(&migrate));
        assert!(
            stderr(&migrate).contains(&format!("ran {} migration(s)", newest - older)),
            "{}",
            stderr(&migrate)
        );

        // The upgraded server serves the same log, and the log goes on.
        let server = Server::start(&upgraded);
        let client = Client::new(server.base(), Some(account.token(0)));
        assert_eq!(
            pull(&client, &created).await,
            before,
            "from version {older}"
        );
        let next = created.op(&account.keys[0], 4, held.digest(), |_| {});
        let _: PushResponse = client
            .post(&ops_path(&created), push_body(&[next]))
            .await
            .decode();
        let after = pull(&client, &created).await;
        assert_ne!(after, before, "from version {older}");
        server.kill();
    }
}

#[test]
fn every_command_refuses_a_missing_or_different_secret() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    assert!(run(&["migrate"], &data).status.success());
    let secret_path = data.join("server-secret");
    let secret = std::fs::read(&secret_path).expect("secret");

    std::fs::remove_file(&secret_path).expect("removed");
    for command in ["migrate", "serve"] {
        let output = run(&[command], &data);
        assert!(
            !output.status.success(),
            "{command} runs without the secret"
        );
        assert!(
            stderr(&output).contains("no server secret"),
            "{}",
            stderr(&output)
        );
    }
    assert!(
        !secret_path.exists(),
        "no command creates a secret for a database with a key check"
    );

    std::fs::write(&secret_path, [7u8; 32]).expect("another secret");
    for command in ["migrate", "serve"] {
        let output = run(&[command], &data);
        assert!(
            !output.status.success(),
            "{command} runs with another secret"
        );
        assert!(
            stderr(&output).contains("doesn't match"),
            "{}",
            stderr(&output)
        );
    }

    std::fs::write(&secret_path, &secret).expect("the secret back");
    assert!(run(&["migrate"], &data).status.success());
}

#[test]
fn healthcheck_follows_the_server() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    assert!(run(&["migrate"], &data).status.success());
    let server = Server::start(&data);
    let address = server.address.to_string();
    let check = || {
        Command::new(BIN)
            .args(["healthcheck", "--listen", &address])
            .status()
            .expect("runs")
            .success()
    };
    assert!(check(), "the server answers");
    server.kill();
    assert!(!check(), "nothing answers");
    let server = Server::start(&data);
    let port = server.address.port();
    assert!(
        Command::new(BIN)
            .args(["healthcheck", "--listen", &format!("0.0.0.0:{port}")])
            .status()
            .expect("runs")
            .success(),
        "an unspecified address checks the loopback address"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_writes_nothing_outside_its_data_directory() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    let cwd = root.path().join("cwd");
    let home = root.path().join("home");
    let tmp = root.path().join("tmp");
    for dir in [&cwd, &home, &tmp] {
        std::fs::create_dir(dir).expect("created");
    }
    let command = |args: &[&str]| {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .arg("--data")
            .arg(&data)
            .current_dir(&cwd)
            .env("HOME", &home)
            .env("TMPDIR", &tmp)
            .env("TMP", &tmp)
            .env("TEMP", &tmp);
        command
    };
    assert!(command(&["migrate"]).status().expect("runs").success());
    let account = seed(&data, 1).await;
    let server = Server::start_with(&mut command(&["serve"]));
    let client = Client::new(server.base(), Some(account.token(0)));
    let created = account.creation();
    let reply = client.post("/v2/workspaces", created.body.clone()).await;
    let _: PushResponse = reply.decode();
    let op = created.op(&account.keys[0], 1, Digest::ZERO, |_| {});
    let _: PushResponse = client
        .post(
            &format!("/v2/workspaces/{}/ops", created.workspace.to_base64url()),
            push_body(&[op]),
        )
        .await
        .decode();
    server.kill();

    for dir in [&cwd, &home, &tmp] {
        assert!(
            files(dir).is_empty(),
            "{} gained {:?}",
            dir.display(),
            files(dir)
        );
    }
    let listed = files(&data);
    let allowed = |path: &PathBuf| {
        let name = path.to_string_lossy().replace('\\', "/");
        name == "server-secret"
            || name.starts_with("sync.sqlite3")
            || name == "logs"
            || (name.starts_with("logs/server-") && name.ends_with(".log"))
            // The disk blob store and its temporary-file directory.
            || name == "blobs"
            || name == "blobs/.partial"
    };
    assert!(listed.iter().all(allowed), "unexpected files: {listed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acknowledged_push_survives_a_crash() {
    let root = tempfile::tempdir().expect("temporary directory");
    let data = root.path().join("data");
    assert!(run(&["migrate"], &data).status.success());
    let account = seed(&data, 1).await;
    let created = account.creation();
    let op = created.op(&account.keys[0], 1, Digest::ZERO, |_| {});

    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(0)));
    let _: PushResponse = client
        .post("/v2/workspaces", created.body.clone())
        .await
        .decode();
    let receipt: PushResponse = client
        .post(
            &format!("/v2/workspaces/{}/ops", created.workspace.to_base64url()),
            push_body(std::slice::from_ref(&op)),
        )
        .await
        .decode();
    // Stopped right after acknowledging, with no chance to flush or
    // checkpoint.
    server.kill();

    let server = Server::start(&data);
    let client = Client::new(server.base(), Some(account.token(0)));
    let pull = PullRequest {
        cursors: List(vec![PullCursor {
            domain: created.domain,
            after: 2,
        }]),
        wait_seconds: 0,
    };
    let response: PullResponse = client
        .post(
            &format!("/v2/workspaces/{}/pull", created.workspace.to_base64url()),
            pull.to_bytes(),
        )
        .await
        .decode();
    let page = &response.pages[0];
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, receipt.receipts[0].seq);
    assert!(matches!(
        &page.entries[0].record,
        sync_protocol::messages::EntryRecord::Operation(stored) if *stored == op
    ));
}
