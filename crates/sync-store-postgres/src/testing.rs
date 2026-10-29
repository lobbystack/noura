//! Throwaway PostgreSQL for tests. The store's tests never skip when
//! PostgreSQL is missing: they fail and say how to provide it.
//!
//! [`Cluster::shared`] gives every test of a test binary one cluster, in
//! which each test makes its own database. With `NOURA_TEST_POSTGRES_URL`
//! set, such as `postgres://postgres:postgres@localhost:5432` with no
//! database path, the databases go on that server, as on CI's PostgreSQL
//! service. Otherwise the cluster is a throwaway one under the caller's
//! temporary directory, made with `initdb` and started with `pg_ctl` from
//! PostgreSQL's bin directory, listening on a free loopback port only. It
//! stops, and its directory goes, when the last test using it ends.
//! [`Cluster::start`] starts a throwaway cluster of its own, for tests that
//! need other settings.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use sqlx::{Connection, PgConnection};

/// The server that `NOURA_TEST_POSTGRES_URL` names, if set.
pub const URL_VARIABLE: &str = "NOURA_TEST_POSTGRES_URL";
/// The directory holding `initdb`, `pg_ctl` and `postgres`, if set.
pub const BIN_VARIABLE: &str = "NOURA_POSTGRES_BIN";

static SHARED: Mutex<Weak<Cluster>> = Mutex::new(Weak::new());
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A PostgreSQL server for tests.
pub struct Cluster {
    /// `postgres://user@host:port`, without a database.
    base: String,
    /// Connection options every URL carries, such as `sslmode=require`.
    options: String,
    local: Option<Local>,
}

struct Local {
    bin: PathBuf,
    root: PathBuf,
    data: PathBuf,
}

/// A fresh, empty database in a cluster.
pub struct TestDatabase {
    /// Its URL.
    pub url: String,
    /// Its name.
    pub name: String,
    /// The cluster, kept running while the database lives.
    pub cluster: Arc<Cluster>,
}

fn executable(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    if cfg!(windows) {
        path.with_extension("exe")
    } else {
        path
    }
}

fn holds_initdb(dir: &Path) -> bool {
    executable(dir, "initdb").is_file() && executable(dir, "pg_ctl").is_file()
}

/// Every directory under `parent` whose name starts with `prefix`, newest
/// version last.
fn versioned(parent: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_prefix(prefix)?;
            let digits: String = version.chars().take_while(char::is_ascii_digit).collect();
            Some((digits.parse().unwrap_or(0), entry.path().join(suffix)))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

/// PostgreSQL's bin directory: `NOURA_POSTGRES_BIN`, `PGBIN`, what
/// `pg_config --bindir` names, the `PATH`, or a usual install location.
pub fn bin_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for variable in [BIN_VARIABLE, "PGBIN"] {
        if let Some(dir) = std::env::var_os(variable) {
            candidates.push(dir.into());
        }
    }
    if let Ok(output) = Command::new("pg_config").arg("--bindir").output()
        && output.status.success()
    {
        candidates.push(String::from_utf8_lossy(&output.stdout).trim().into());
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path));
    }
    candidates.push("/opt/homebrew/bin".into());
    for root in ["/opt/homebrew/opt", "/usr/local/opt"] {
        let mut found = versioned(Path::new(root), "postgresql@", "bin");
        found.reverse();
        candidates.extend(found);
    }
    let mut debian = versioned(Path::new("/usr/lib/postgresql"), "", "bin");
    debian.reverse();
    candidates.extend(debian);
    let mut windows = versioned(Path::new(r"C:\Program Files\PostgreSQL"), "", "bin");
    windows.reverse();
    candidates.extend(windows);
    candidates.into_iter().find(|dir| holds_initdb(dir))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("a free loopback port")
}

fn pg_ctl(bin: &Path, args: &[&str], data: &Path) -> std::process::Output {
    Command::new(executable(bin, "pg_ctl"))
        .args(args)
        .arg("-D")
        .arg(data)
        // PostgreSQL on macOS refuses to start without a valid locale.
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .expect("pg_ctl runs")
}

/// Stops the clusters of test processes that died before they could.
#[cfg(unix)]
fn reap_orphans(parent: &Path) {
    let own = std::process::id().to_string();
    let Some(bin) = bin_dir() else {
        return;
    };
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.split('-').next() else {
            continue;
        };
        if pid == own || pid.parse::<u32>().is_err() {
            continue;
        }
        let alive = Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            let data = entry.path().join("data");
            if data.join("postmaster.pid").exists() {
                pg_ctl(&bin, &["stop", "-m", "immediate", "-w"], &data);
            }
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(not(unix))]
fn reap_orphans(_parent: &Path) {}

impl Cluster {
    /// The cluster this test binary's tests share, started or reached
    /// once. `tmp` is the test's temporary directory, such as
    /// `env!("CARGO_TARGET_TMPDIR")`.
    pub fn shared(tmp: &Path) -> Arc<Cluster> {
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cluster) = shared.upgrade() {
            return cluster;
        }
        let cluster = Arc::new(match std::env::var(URL_VARIABLE) {
            Ok(base) if !base.trim().is_empty() => Cluster {
                base: base.trim().trim_end_matches('/').to_owned(),
                options: String::new(),
                local: None,
            },
            _ => Cluster::start(tmp, &[]),
        });
        *shared = Arc::downgrade(&cluster);
        cluster
    }

    /// A throwaway cluster of its own, with `settings` added to its
    /// `postgresql.conf`.
    pub fn start(tmp: &Path, settings: &[(&str, &str)]) -> Cluster {
        Cluster::start_with(tmp, settings, |_| {})
    }

    /// [`Cluster::start`], with `prepare` run on the data directory after
    /// `initdb`, to write files such as certificates or `pg_hba.conf`.
    pub fn start_with(
        tmp: &Path,
        settings: &[(&str, &str)],
        prepare: impl FnOnce(&Path),
    ) -> Cluster {
        let Some(bin) = bin_dir() else {
            panic!(
                "PostgreSQL's initdb and pg_ctl weren't found. Install PostgreSQL, set {BIN_VARIABLE} \
                 to its bin directory, or set {URL_VARIABLE} to a server such as \
                 postgres://postgres:postgres@localhost:5432; the store tests never skip"
            );
        };
        let parent = tmp.join("postgres");
        std::fs::create_dir_all(&parent).expect("the clusters' directory");
        reap_orphans(&parent);
        let root = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the cluster's directory");
        let data = root.join("data");
        let init = Command::new(executable(&bin, "initdb"))
            .arg("-D")
            .arg(&data)
            .args([
                "-A",
                "trust",
                "-U",
                "postgres",
                "-E",
                "UTF8",
                "--locale=C",
                "--no-sync",
            ])
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .output()
            .expect("initdb runs");
        assert!(
            init.status.success(),
            "initdb failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        prepare(&data);
        let mut last = String::new();
        for _ in 0..3 {
            let port = free_port();
            let mut conf = format!(
                "\n# Added by sync-store-postgres's test harness.\nport = {port}\n\
                 listen_addresses = '127.0.0.1'\nunix_socket_directories = ''\n\
                 max_connections = 500\n"
            );
            for (key, value) in settings {
                conf.push_str(&format!("{key} = {value}\n"));
            }
            let path = data.join("postgresql.conf");
            let original = std::fs::read_to_string(&path).expect("postgresql.conf");
            let original = original
                .split("\n# Added by sync-store-postgres's test harness.")
                .next()
                .unwrap_or_default()
                .to_owned();
            std::fs::write(&path, original + &conf).expect("postgresql.conf written");
            let log = root.join("postgres.log");
            let started = pg_ctl(
                &bin,
                &["start", "-w", "-t", "60", "-l", &log.to_string_lossy()],
                &data,
            );
            if started.status.success() {
                return Cluster {
                    base: format!("postgres://postgres@127.0.0.1:{port}"),
                    options: String::new(),
                    local: Some(Local { bin, root, data }),
                };
            }
            last = std::fs::read_to_string(&log).unwrap_or_default();
        }
        let _ = std::fs::remove_dir_all(&root);
        panic!("the throwaway PostgreSQL cluster didn't start:\n{last}");
    }

    /// The URL of `database` in this cluster.
    pub fn url(&self, database: &str) -> String {
        if self.options.is_empty() {
            format!("{}/{database}", self.base)
        } else {
            format!("{}/{database}?{}", self.base, self.options)
        }
    }

    /// The cluster with `options`, such as `sslmode=require`, on every URL
    /// it gives, its own connections included.
    pub fn with_url_options(mut self, options: &str) -> Self {
        self.options = options.to_owned();
        self
    }

    /// A throwaway cluster's data directory; `None` on a shared server.
    pub fn data_dir(&self) -> Option<&Path> {
        self.local.as_ref().map(|local| local.data.as_path())
    }

    /// Runs `sql` on the cluster's `postgres` database.
    pub async fn execute(&self, sql: &str) {
        let mut connection = PgConnection::connect(&self.url("postgres"))
            .await
            .expect("the cluster answers");
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut connection)
            .await
            .expect("the statement runs");
        connection.close().await.expect("closed");
    }

    /// Whether the cluster dates commits (`track_commit_timestamp`).
    pub async fn dates_commits(&self) -> bool {
        crate::track_commit_timestamp(&self.url("postgres"))
            .await
            .expect("the cluster answers")
    }

    /// A fresh, empty database.
    pub async fn database(self: &Arc<Self>) -> TestDatabase {
        let suffix: [u8; 8] = sync_protocol::crypto::random_array().expect("random");
        let name = format!("noura_{}", sync_protocol::types::to_hex(&suffix));
        self.execute(&format!("CREATE DATABASE {name}")).await;
        TestDatabase {
            url: self.url(&name),
            name,
            cluster: self.clone(),
        }
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if let Some(local) = &self.local {
            pg_ctl(&local.bin, &["stop", "-m", "immediate", "-w"], &local.data);
            let _ = std::fs::remove_dir_all(&local.root);
        }
    }
}

/// One stored value, for a test that checks what the server keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// A `bytea` value.
    Bytes(Vec<u8>),
    /// A text value.
    Text(String),
    /// An integer value.
    Int(i64),
    /// A value of another type, or NULL.
    Other,
}

/// One column of one table in the `public` schema, with its type and every
/// value its rows hold.
#[derive(Clone, Debug)]
pub struct StoredColumn {
    /// The table.
    pub table: String,
    /// The column.
    pub column: String,
    /// Its type, as `information_schema.columns` names it.
    pub data_type: String,
    /// Each row's value.
    pub values: Vec<Stored>,
}

impl TestDatabase {
    /// Every column of every table the database holds, with its values.
    pub async fn columns(&self) -> Vec<StoredColumn> {
        use sqlx::Row;
        let mut connection = PgConnection::connect(&self.url)
            .await
            .expect("the database answers");
        let listed = sqlx::raw_sql(sqlx::AssertSqlSafe(
            "SELECT table_name::text, column_name::text, data_type::text \
             FROM information_schema.columns WHERE table_schema = 'public' \
             ORDER BY table_name, ordinal_position"
                .to_owned(),
        ))
        .fetch_all(&mut connection)
        .await
        .expect("the columns list");
        let mut columns = Vec::new();
        for row in listed {
            let table: String = row.get(0);
            let column: String = row.get(1);
            let data_type: String = row.get(2);
            let cast = match data_type.as_str() {
                "bytea" => format!("\"{column}\""),
                "smallint" | "integer" | "bigint" => format!("\"{column}\"::bigint"),
                "boolean" => format!("\"{column}\"::int::bigint"),
                _ => format!("\"{column}\"::text"),
            };
            let values = sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "SELECT {cast} FROM \"{table}\""
            )))
            .fetch_all(&mut connection)
            .await
            .expect("the rows read")
            .into_iter()
            .map(|row| match data_type.as_str() {
                "bytea" => row
                    .try_get::<Option<Vec<u8>>, _>(0)
                    .ok()
                    .flatten()
                    .map_or(Stored::Other, Stored::Bytes),
                "smallint" | "integer" | "bigint" | "boolean" => row
                    .try_get::<Option<i64>, _>(0)
                    .ok()
                    .flatten()
                    .map_or(Stored::Other, Stored::Int),
                _ => row
                    .try_get::<Option<String>, _>(0)
                    .ok()
                    .flatten()
                    .map_or(Stored::Other, Stored::Text),
            })
            .collect();
            columns.push(StoredColumn {
                table,
                column,
                data_type,
                values,
            });
        }
        connection.close().await.expect("closed");
        columns
    }
}
