//! Running work on the main thread and waiting for its answer, for the
//! macOS keyboard, input source and pasteboard calls that trap anywhere
//! else, as when a plugin's worker thread pastes.
//!
//! The caller may wait on the main thread, but the main thread never
//! waits on the caller: the work goes on the main queue without blocking
//! it, and the caller waits at most [`PATIENCE`]. Work that hasn't started
//! by then never runs, so a paste you gave up on can't land later. Work
//! that already started gets to finish, since it waits on nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// How long a caller waits for the main thread to start its work.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);

const WAITING: u8 = 0;
const RUNNING: u8 = 1;
const ABANDONED: u8 = 2;

/// The main thread didn't start the work within [`PATIENCE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Busy;

/// Hand `work` to `submit`, which runs it on the main thread some time
/// later or drops it, and wait up to `patience` for it to start.
pub(crate) fn hop<T, W, S>(work: W, patience: Duration, submit: S) -> Result<T, Busy>
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    S: FnOnce(Box<dyn FnOnce() + Send>),
{
    let state = Arc::new(AtomicU8::new(WAITING));
    let (answer, answered) = mpsc::sync_channel(1);
    let started = state.clone();
    submit(Box::new(move || {
        if started
            .compare_exchange(WAITING, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = answer.send(work());
        }
    }));
    match answered.recv_timeout(patience) {
        Ok(value) => Ok(value),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Busy),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if state
                .compare_exchange(WAITING, ABANDONED, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Err(Busy);
            }
            // It started just now and waits on nothing: let it finish.
            answered.recv().map_err(|_| Busy)
        }
    }
}

/// Run `work` on the main thread: at once when this is the main thread,
/// otherwise on the main queue, waiting at most [`PATIENCE`] for it to
/// start.
#[cfg(target_os = "macos")]
pub(crate) fn run<T, W>(work: W) -> Result<T, Busy>
where
    T: Send + 'static,
    W: FnOnce(objc2::MainThreadMarker) -> T + Send + 'static,
{
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        return Ok(work(mtm));
    }
    hop(
        move || {
            // SAFETY: the main queue runs this, on the main thread.
            work(unsafe { objc2::MainThreadMarker::new_unchecked() })
        },
        PATIENCE,
        |job| dispatch2::DispatchQueue::main().exec_async(job),
    )
    .inspect_err(|_| tracing::warn!("the main thread didn't take a paste in time"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use super::*;

    type Job = Box<dyn FnOnce() + Send>;

    #[test]
    fn work_the_main_thread_runs_answers_the_caller() {
        let answer = hop(
            || 2 + 2,
            PATIENCE,
            |job: Job| {
                std::thread::spawn(job);
            },
        );
        assert_eq!(answer, Ok(4));
    }

    #[test]
    fn work_a_busy_main_thread_starts_late_never_runs() {
        let queued: Arc<Mutex<Option<Job>>> = Arc::default();
        let ran = Arc::new(AtomicBool::new(false));
        let did = ran.clone();
        let parked = queued.clone();
        let answer = hop(
            move || did.store(true, Ordering::SeqCst),
            Duration::from_millis(50),
            move |job: Job| *parked.lock().unwrap() = Some(job),
        );
        assert_eq!(answer, Err(Busy));
        // The main thread gets to it after the caller gave up.
        let job = queued.lock().unwrap().take().unwrap();
        job();
        assert!(!ran.load(Ordering::SeqCst), "a paste you gave up on landed");
    }

    #[test]
    fn work_dropped_unrun_frees_the_caller() {
        let answer = hop(|| 1, PATIENCE, drop::<Job>);
        assert_eq!(answer, Err(Busy));
    }

    #[test]
    fn work_that_started_before_the_deadline_finishes() {
        let answer = hop(
            || {
                std::thread::sleep(Duration::from_millis(150));
                7
            },
            Duration::from_millis(50),
            |job: Job| {
                std::thread::spawn(job);
            },
        );
        assert_eq!(answer, Ok(7));
    }
}
