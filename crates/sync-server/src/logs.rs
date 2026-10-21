//! The server's local log (§29): one file per UTC day in the data
//! directory's `logs` folder, kept for at most 14 days. Lines carry route
//! templates, request IDs and status codes, and the cause of a store
//! failure, never account IDs, record bodies, tokens, URL fragments, `rid`s
//! or ciphertext.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use tracing_subscriber::EnvFilter;

/// How many days of logs the server keeps, today included.
pub const RETENTION_DAYS: i64 = 14;

const PREFIX: &str = "server-";
const SUFFIX: &str = ".log";

fn today() -> Date {
    jiff::Timestamp::now().to_zoned(TimeZone::UTC).date()
}

fn file_name(date: Date) -> String {
    format!("{PREFIX}{date}{SUFFIX}")
}

/// Deletes the log files dated before the retention window that ends on
/// `today`.
pub fn prune(dir: &Path, today: Date) -> io::Result<()> {
    let oldest = today
        .checked_sub(jiff::Span::new().days(RETENTION_DAYS - 1))
        .unwrap_or(today);
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(date) = name
            .to_str()
            .and_then(|n| n.strip_prefix(PREFIX))
            .and_then(|n| n.strip_suffix(SUFFIX))
            .and_then(|d| d.parse::<Date>().ok())
        else {
            continue;
        };
        if date < oldest {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

struct Current {
    date: Date,
    file: File,
}

/// A writer that appends to the current day's file, moving to a new file
/// and pruning old ones when the day changes.
#[derive(Clone)]
pub struct DailyLog {
    dir: PathBuf,
    current: Arc<Mutex<Option<Current>>>,
}

impl DailyLog {
    /// A log in `dir`, which it creates, after pruning old files.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        prune(dir, today())?;
        Ok(Self {
            dir: dir.to_owned(),
            current: Arc::new(Mutex::new(None)),
        })
    }

    fn write_line(&self, line: &[u8]) -> io::Result<()> {
        let date = today();
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_none_or(|c| c.date != date) {
            let _ = prune(&self.dir, date);
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(file_name(date)))?;
            *current = Some(Current { date, file });
        }
        let current = current.as_mut().expect("opened above");
        current.file.write_all(line)
    }
}

/// One event's write.
pub struct LogWriter(DailyLog);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_line(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Sends this process's server events to the daily log in `dir`. Only the
/// server's own events pass: no library's, so no statement or request
/// detail reaches the file.
pub fn init(dir: &Path) -> io::Result<()> {
    let log = DailyLog::open(dir)?;
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || LogWriter(log.clone()))
        .with_ansi(false)
        .with_target(false)
        .with_env_filter(EnvFilter::new("off,sync_server=info"))
        .finish();
    tracing::subscriber::set_global_default(subscriber).map_err(|e| io::Error::other(e.to_string()))
}
