//! One running copy of Noura per app data folder, which means per user.
//!
//! The first launch locks a file in the app data folder and listens on a
//! local socket: a Unix domain socket on macOS and Linux, a named pipe on
//! Windows. A later launch finds the lock taken, asks the running copy to
//! bring its most recent window to the front, and exits before it opens
//! a workspace or an index. The operating system drops the lock when a
//! copy exits, even when it crashes, so a stale socket never blocks the
//! next launch.

use std::{
    fs::{File, OpenOptions, TryLockError},
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, ToFsName, ToNsName,
    prelude::*,
};

/// What a later launch sends, and what the running copy answers.
const ACTIVATE: &[u8] = b"activate\n";
const DONE: &[u8] = b"ok\n";

/// How long a later launch tries to reach a running copy that is still
/// starting up, and how long it waits for the answer.
const CONNECT_PATIENCE: Duration = Duration::from_secs(5);
const ANSWER_PATIENCE: Duration = Duration::from_secs(2);

/// Unix socket paths are limited to about 104 bytes. A longer app data
/// path uses a name derived from it instead.
const MAX_SOCKET_PATH: usize = 100;

/// What [`claim`] found.
pub enum Claim {
    /// No other copy runs for this app data folder: this one goes on and
    /// hears later launches.
    First(Instance),
    /// Another copy runs. It was asked to come forward, and this one
    /// should exit.
    Running,
}

/// The running copy's claim: the lock it holds and the socket where later
/// launches reach it. Dropping it stops listening and lets the next launch
/// run.
pub struct Instance {
    lock: Option<File>,
    shared: Arc<Shared>,
    name: Name<'static>,
    listening: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    /// A later launch asked to come forward since the last check.
    asked: AtomicBool,
    /// Wakes the UI when a later launch asks.
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The instance is going away.
    stopping: AtomicBool,
}

impl Shared {
    fn ask(&self) {
        self.asked.store(true, Ordering::SeqCst);
        if let Some(wake) = &*self.wake.lock().unwrap_or_else(|error| error.into_inner()) {
            wake();
        }
    }
}

impl Instance {
    /// Call `wake` whenever a later launch asks this copy to come forward,
    /// and once now if one asked already.
    pub fn on_activate(&self, wake: impl Fn() + Send + 'static) {
        if self.shared.asked.load(Ordering::SeqCst) {
            wake();
        }
        *self
            .shared
            .wake
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Box::new(wake));
    }

    /// Whether a later launch asked this copy to come forward since the
    /// last call.
    pub fn take_activation(&self) -> bool {
        self.shared.asked.swap(false, Ordering::SeqCst)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Wake the listening thread, which then lets go of the socket.
        if Stream::connect(self.name.clone()).is_ok()
            && let Some(listening) = self.listening.take()
            && listening.join().is_err()
        {
            tracing::debug!("the single-instance thread stopped early");
        }
        // Then let the next launch take the lock.
        drop(self.lock.take());
    }
}

/// Become the running copy for `app_data`, or ask the copy that runs to
/// come forward.
pub fn claim(app_data: &Path) -> io::Result<Claim> {
    std::fs::create_dir_all(app_data)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(app_data.join("instance.lock"))?;
    match lock.try_lock() {
        Ok(()) => listen(lock, app_data).map(Claim::First),
        Err(TryLockError::WouldBlock) => {
            ask_to_activate(app_data);
            Ok(Claim::Running)
        }
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Listen for later launches on a background thread.
fn listen(lock: File, app_data: &Path) -> io::Result<Instance> {
    let name = socket_name(app_data)?;
    // A socket file left behind by a copy that crashed is stale: this copy
    // holds the lock. Other users can't write to the socket, which the
    // process's file mode mask leaves writable by its owner alone.
    let listener = ListenerOptions::new()
        .name(name.clone())
        .try_overwrite(true)
        .create_sync()?;
    let shared = Arc::new(Shared::default());
    let heard = shared.clone();
    let listening = std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if heard.stopping.load(Ordering::SeqCst) {
                    break;
                }
                match stream {
                    Ok(stream) => answer(stream, heard.clone()),
                    Err(error) => tracing::debug!(%error, "a later launch couldn't connect"),
                }
            }
        })?;
    Ok(Instance {
        lock: Some(lock),
        shared,
        name,
        listening: Some(listening),
    })
}

/// Read one request on its own thread, so a launch that never sends one
/// can't hold up the next.
fn answer(stream: Stream, shared: Arc<Shared>) {
    let spawned = std::thread::Builder::new()
        .name("single-instance-request".into())
        .spawn(move || {
            let mut request = Vec::new();
            let read = BufReader::new(&stream)
                .take(ACTIVATE.len() as u64)
                .read_until(b'\n', &mut request);
            if read.is_ok() && request == ACTIVATE {
                shared.ask();
                let _ = (&stream).write_all(DONE);
            }
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "could not answer a later launch");
    }
}

/// Ask the running copy to come forward. A copy that is still starting
/// may not listen yet, so keep trying for a while. This launch exits
/// either way.
fn ask_to_activate(app_data: &Path) {
    allow_foreground_change();
    let deadline = Instant::now() + CONNECT_PATIENCE;
    let stream = loop {
        match socket_name(app_data).and_then(Stream::connect) {
            Ok(stream) => break stream,
            Err(error) if Instant::now() >= deadline => {
                tracing::warn!(%error, "Noura is already running but didn't answer");
                return;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    if let Err(error) = (&stream).write_all(ACTIVATE) {
        tracing::warn!(%error, "couldn't ask the running copy of Noura to come forward");
        return;
    }
    // Wait for the answer, so the request is read before this launch
    // exits. Named pipes have no read timeout, so a thread waits.
    let (answered, wait) = mpsc::channel();
    let waited = std::thread::Builder::new().spawn(move || {
        let mut answer = [0; DONE.len()];
        let _ = answered.send((&stream).read_exact(&mut answer).is_ok() && answer == DONE);
    });
    if waited.is_ok() && wait.recv_timeout(ANSWER_PATIENCE) != Ok(true) {
        tracing::warn!("the running copy of Noura didn't confirm it came forward");
    }
}

/// On Windows, only the app the person launched may bring a window to the
/// front. This launch lets the running copy do it.
#[cfg(windows)]
fn allow_foreground_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    // SAFETY: the call takes a process ID by value and touches no memory.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(not(windows))]
fn allow_foreground_change() {}

/// Where the running copy for `app_data` listens: a socket in the app
/// data folder on macOS and Linux, and a named pipe whose name comes from
/// the folder on Windows, where every user has their own.
fn socket_name(app_data: &Path) -> io::Result<Name<'static>> {
    let derived = format!(
        "noura-{:016x}",
        fnv1a(app_data.as_os_str().as_encoded_bytes())
    );
    if cfg!(unix) {
        let path = app_data.join("instance.sock");
        if path.as_os_str().len() <= MAX_SOCKET_PATH {
            return path.to_fs_name::<GenericFilePath>();
        }
    }
    derived.to_ns_name::<GenericNamespaced>()
}

/// The 64-bit FNV-1a hash of `bytes`, which stays the same across Rust
/// versions, unlike the standard library's hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait until `instance` hears a later launch.
    fn wait_for_activation(instance: &Instance) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !instance.take_activation() {
            assert!(Instant::now() < deadline, "no launch asked to come forward");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_later_launch_asks_the_running_copy_to_come_forward_and_stops() {
        let app_data = tempfile::tempdir().unwrap();
        let Claim::First(running) = claim(app_data.path()).unwrap() else {
            panic!("nothing else runs on this app data folder");
        };
        let (woken, wakes) = mpsc::channel();
        running.on_activate(move || {
            let _ = woken.send(());
        });
        assert!(!running.take_activation());

        let later = claim(app_data.path()).unwrap();

        assert!(matches!(later, Claim::Running));
        wait_for_activation(&running);
        assert!(wakes.recv_timeout(Duration::from_secs(10)).is_ok());
        assert!(!running.take_activation(), "one launch asks once");
    }

    #[test]
    fn a_launch_after_the_running_copy_exits_runs() {
        let app_data = tempfile::tempdir().unwrap();
        let first = claim(app_data.path()).unwrap();
        assert!(matches!(first, Claim::First(_)));
        drop(first);

        let Claim::First(next) = claim(app_data.path()).unwrap() else {
            panic!("the earlier copy is gone");
        };

        assert!(matches!(claim(app_data.path()).unwrap(), Claim::Running));
        wait_for_activation(&next);
    }

    /// A copy that crashed leaves its socket file behind.
    #[cfg(unix)]
    #[test]
    fn a_socket_left_by_a_copy_that_crashed_gives_way_to_the_next_launch() {
        let app_data = tempfile::tempdir().unwrap();
        let socket = app_data.path().join("instance.sock");
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        assert!(socket.exists());

        let Claim::First(next) = claim(app_data.path()).unwrap() else {
            panic!("the crashed copy holds no lock");
        };

        assert!(matches!(claim(app_data.path()).unwrap(), Claim::Running));
        wait_for_activation(&next);
    }

    #[test]
    fn copies_on_different_app_data_folders_run_side_by_side() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();

        let one = claim(first.path()).unwrap();
        let other = claim(second.path()).unwrap();

        assert!(matches!(one, Claim::First(_)));
        assert!(matches!(other, Claim::First(_)));
    }

    #[test]
    fn a_long_app_data_path_still_gets_a_working_socket() {
        let parent = tempfile::tempdir().unwrap();
        let app_data = parent.path().join("a".repeat(120));
        let Claim::First(running) = claim(&app_data).unwrap() else {
            panic!("nothing else runs on this app data folder");
        };

        assert!(matches!(claim(&app_data).unwrap(), Claim::Running));
        wait_for_activation(&running);
    }
}
