//! The server's local log (§29): one file per UTC day in the data
//! directory's `logs` folder, kept for at most 14 days. Lines carry route
//! templates, request IDs and status codes, and the cause of a store
//! failure, never account IDs, record bodies, tokens, URL fragments, `rid`s
//! or ciphertext.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use tracing_subscriber::EnvFilter;

/// How many days of logs the server keeps, today included.
pub const RETENTION_DAYS: i64 = 14;

/// How often `serve` prunes the log besides at each new day's first line,
/// so a server that answers no request still drops expired files.
pub const PRUNE_PERIOD: Duration = Duration::from_secs(60 * 60);

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
            // The day's first line and the timer may prune at once.
            match fs::remove_file(entry.path()) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Prunes `dir` every `period` in the background, first right away.
pub fn spawn_pruning(dir: PathBuf, period: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(period);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            let dir = dir.clone();
            let _ = tokio::task::spawn_blocking(move || prune(&dir, today())).await;
        }
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A server that serves no request still drops a file once it falls out
    /// of the 14 days.
    #[tokio::test]
    async fn pruning_runs_without_requests() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let pruning = spawn_pruning(dir.path().to_owned(), Duration::from_millis(20));
        let expired = today()
            .checked_sub(jiff::Span::new().days(RETENTION_DAYS))
            .expect("a date");
        let kept = today()
            .checked_sub(jiff::Span::new().days(RETENTION_DAYS - 1))
            .expect("a date");
        let expired = dir.path().join(file_name(expired));
        let kept = dir.path().join(file_name(kept));
        fs::write(&expired, "expired\n").expect("written");
        fs::write(&kept, "kept\n").expect("written");
        let deadline = Instant::now() + Duration::from_secs(5);
        while expired.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        pruning.abort();
        assert!(!expired.exists(), "older than 14 days");
        assert!(kept.exists(), "within 14 days");
    }
}
