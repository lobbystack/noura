//! The server's local log (§29): one file per UTC day in the data
//! directory's `logs` folder, kept for at most 14 days and within a size a
//! day, so requests nobody authenticated can't fill the disk. Lines carry
//! route
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
use tracing::{Level, Metadata};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

/// How many days of logs the server keeps, today included.
pub const RETENTION_DAYS: i64 = 14;

/// How many bytes of lines a day's file takes, requests among them.
pub const DAY_BYTES: u64 = 64 * 1024 * 1024;
/// How many bytes of warnings and errors it takes past that, so a flood of
/// requests can't hide a store failure.
pub const DAY_WARNING_BYTES: u64 = 2 * DAY_BYTES;

const FULL: &[u8] = b"the log reached its size for today; only warnings and errors follow\n";

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
    /// The file's size.
    written: u64,
    /// Whether the file says it stopped taking ordinary lines.
    told: bool,
}

/// A writer that appends to the current day's file, moving to a new file
/// and pruning old ones when the day changes.
#[derive(Clone)]
pub struct DailyLog {
    dir: PathBuf,
    current: Arc<Mutex<Option<Current>>>,
    day_bytes: u64,
    warning_bytes: u64,
}

impl DailyLog {
    /// A log in `dir`, which it creates, after pruning old files.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::open_within(dir, DAY_BYTES, DAY_WARNING_BYTES)
    }

    /// [`Self::open`] with other sizes a day.
    pub fn open_within(dir: &Path, day_bytes: u64, warning_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        prune(dir, today())?;
        Ok(Self {
            dir: dir.to_owned(),
            current: Arc::new(Mutex::new(None)),
            day_bytes,
            warning_bytes,
        })
    }

    /// Appends `line`, unless the day's file reached its size for a line of
    /// this kind.
    pub fn write_line(&self, line: &[u8], warning: bool) -> io::Result<()> {
        let date = today();
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_none_or(|c| c.date != date) {
            let _ = prune(&self.dir, date);
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(file_name(date)))?;
            let written = file.metadata()?.len();
            *current = Some(Current {
                date,
                file,
                written,
                told: false,
            });
        }
        let current = current.as_mut().expect("opened above");
        let limit = if warning {
            self.warning_bytes
        } else {
            self.day_bytes
        };
        if current.written + line.len() as u64 > limit {
            if !warning && !current.told {
                current.told = true;
                current.file.write_all(FULL)?;
                current.written += FULL.len() as u64;
            }
            return Ok(());
        }
        current.file.write_all(line)?;
        current.written += line.len() as u64;
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for DailyLog {
    type Writer = LogWriter;

    fn make_writer(&self) -> LogWriter {
        LogWriter {
            log: self.clone(),
            warning: false,
        }
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> LogWriter {
        LogWriter {
            log: self.clone(),
            warning: *meta.level() <= Level::WARN,
        }
    }
}

/// One event's write.
pub struct LogWriter {
    log: DailyLog,
    warning: bool,
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.log.write_line(buf, self.warning)?;
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
        .with_writer(log)
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

    /// A day's file stops taking ordinary lines at its size and says so,
    /// warnings go on to their own size, and a restart keeps the count.
    #[test]
    fn a_days_file_stays_within_its_size() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let file = dir.path().join(file_name(today()));
        let line = vec![b'x'; 99];
        let size = |log: &DailyLog, warning| {
            for _ in 0..50 {
                log.write_line(&line, warning).expect("written");
            }
            fs::metadata(&file).expect("the file").len()
        };
        let log = DailyLog::open_within(dir.path(), 1_000, 2_000).expect("opened");
        let ordinary = size(&log, false);
        assert!(ordinary <= 1_000 + FULL.len() as u64, "{ordinary}");
        let text = fs::read(&file).expect("read");
        assert!(text.ends_with(FULL), "the file says why it stops");
        let warnings = size(&log, true);
        assert!((1_900..=2_000).contains(&warnings), "{warnings}");

        let restarted = DailyLog::open_within(dir.path(), 1_000, 2_000).expect("opened");
        assert_eq!(size(&restarted, true), warnings);
    }

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
