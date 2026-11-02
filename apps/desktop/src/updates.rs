//! Updates from Noura's signed release feed, through cargo-packager's
//! updater. `app-core` decides when to check and when to install; this
//! module talks to the feed, verifies downloads against Noura's updater key
//! and replaces the installed app.

use std::{
    ffi::OsStr,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use app_core::{PendingUpdate, UpdateError, UpdateErrorKind, UpdateSource};
use cargo_packager_updater::{Config, Error, Update, UpdaterBuilder, semver::Version, url::Url};

/// The feed each release publishes.
pub const RELEASE_FEED: &str =
    "https://github.com/lobbystack/noura/releases/latest/download/latest.json";

/// Noura's updater public key: a minisign key, base64-encoded as
/// cargo-packager writes it. The `release` environment holds the private
/// key; docs/releasing.md explains it.
pub const RELEASE_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDk1NkJDREI4NUNEOUE4MkYKUldRdnFObGN1TTFybFRqNEY3cFJicGJOdVpLZjR0N2Q2S2VHNlB1U0t3cnJTbkVGQ2xEejdTYSsK";

/// reqwest's blocking client gives up after 30 seconds by default, too soon
/// for a large download on a slow connection.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Whether this copy of Noura can replace itself, or why not. `os` is
/// [`std::env::consts::OS`], and `appimage` is the `APPIMAGE` variable an
/// AppImage sets.
pub fn availability(
    debug_build: bool,
    os: &str,
    executable: &Path,
    appimage: Option<&OsStr>,
) -> Result<(), &'static str> {
    if debug_build {
        return Err("Development builds don't update themselves.");
    }
    match os {
        // Only an AppImage can replace itself. A .deb belongs to the
        // package manager.
        "linux" if appimage.is_none() => Err(
            "Packages from your package manager don't update themselves. Download new versions from noura.app.",
        ),
        // Outside an app bundle the updater would replace the folder that
        // holds the executable.
        "macos" if !in_app_bundle(executable) => {
            Err("This copy doesn't update itself. Install Noura from noura.app to get updates.")
        }
        _ => Ok(()),
    }
}

fn in_app_bundle(executable: &Path) -> bool {
    let mut parents = executable.ancestors().skip(1);
    let (Some(macos), Some(contents), Some(bundle)) =
        (parents.next(), parents.next(), parents.next())
    else {
        return false;
    };
    macos.file_name() == Some(OsStr::new("MacOS"))
        && contents.file_name() == Some(OsStr::new("Contents"))
        && bundle.extension() == Some(OsStr::new("app"))
}

/// An update that installs only after Noura's window has closed and its
/// writes have reached disk. On Windows the `.msi` installer ends the
/// running app the moment it starts, so Noura stages the update here and
/// `main` installs it last.
#[derive(Clone, Default)]
pub struct InstallAtExit(Arc<Mutex<Option<Staged>>>);

/// A downloaded and verified update, waiting to install.
struct Staged {
    update: Update,
    bytes: Vec<u8>,
}

impl InstallAtExit {
    /// Install the staged update, if there is one. On Windows this starts
    /// the installer and ends the process, and the installer opens the new
    /// version when it finishes.
    pub fn run(&self) {
        let Some(Staged { update, bytes }) = self.take() else {
            return;
        };
        if let Err(error) = update.install(bytes) {
            tracing::error!(%error, "the staged update could not be installed");
        }
    }

    fn stage(&self, update: Update, bytes: Vec<u8>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Staged { update, bytes });
    }

    fn take(&self) -> Option<Staged> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Releases from a signed feed.
pub struct FeedSource {
    endpoint: Url,
    public_key: String,
    current: Version,
    /// Ends the app as Quit does, once the relaunch is arranged.
    quit: Box<dyn Fn() + Send + Sync>,
    /// The executable whose app an update replaces, when not this one.
    executable: Option<std::path::PathBuf>,
    /// Where updates wait to install as Noura quits, when they can't
    /// install while it runs.
    at_exit: Option<InstallAtExit>,
}

impl FeedSource {
    pub fn new(
        endpoint: &str,
        public_key: &str,
        current_version: &str,
        quit: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Ok(Self {
            endpoint: endpoint.parse().map_err(|error| format!("{error}"))?,
            public_key: public_key.to_owned(),
            current: current_version
                .parse()
                .map_err(|error| format!("{error}"))?,
            quit: Box::new(quit),
            executable: None,
            at_exit: None,
        })
    }

    /// Stage downloaded updates in `at_exit` instead of installing them
    /// while Noura runs.
    pub fn installing_at_exit(mut self, at_exit: InstallAtExit) -> Self {
        self.at_exit = Some(at_exit);
        self
    }
}

impl UpdateSource for FeedSource {
    fn check(&self) -> Result<Option<Box<dyn PendingUpdate>>, UpdateError> {
        let config = Config {
            endpoints: vec![self.endpoint.clone()],
            pubkey: self.public_key.clone(),
            windows: None,
        };
        let mut builder =
            UpdaterBuilder::new(self.current.clone(), config).timeout(REQUEST_TIMEOUT);
        if let Some(executable) = &self.executable {
            builder = builder.executable_path(executable);
        }
        let updater = builder.build().map_err(failure)?;
        let update = updater.check().map_err(failure)?;
        Ok(update.map(|update| {
            Box::new(FeedUpdate {
                update,
                bytes: None,
                at_exit: self.at_exit.clone(),
            }) as Box<dyn PendingUpdate>
        }))
    }

    fn restart(&self) -> Result<(), UpdateError> {
        relaunch_after_exit()
            .map_err(|error| UpdateError::new(UpdateErrorKind::Restart, error.to_string()))?;
        (self.quit)();
        Ok(())
    }
}

/// A newer release, and its bytes once downloaded and verified.
struct FeedUpdate {
    update: Update,
    bytes: Option<Vec<u8>>,
    at_exit: Option<InstallAtExit>,
}

impl PendingUpdate for FeedUpdate {
    fn version(&self) -> &str {
        &self.update.version
    }

    fn notes(&self) -> Option<&str> {
        self.update.body.as_deref()
    }

    fn download(&mut self) -> Result<(), UpdateError> {
        // The updater checks the signature before it returns the bytes.
        self.bytes = Some(self.update.download().map_err(failure)?);
        Ok(())
    }

    fn install(&mut self) -> Result<(), UpdateError> {
        let bytes = self.bytes.clone().ok_or_else(|| {
            UpdateError::new(UpdateErrorKind::Install, "the update isn't downloaded")
        })?;
        if let Some(at_exit) = &self.at_exit {
            at_exit.stage(self.update.clone(), bytes);
            return Ok(());
        }
        self.update
            .install(bytes)
            .map_err(|error| UpdateError::new(UpdateErrorKind::Install, error.to_string()))
    }
}

fn failure(error: Error) -> UpdateError {
    let kind = match error {
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) => {
            UpdateErrorKind::BadSignature
        }
        _ => UpdateErrorKind::Unreachable,
    };
    UpdateError::new(kind, error.to_string())
}

/// Start the installed app once this process has exited, so a new copy
/// never meets the old one. On Windows the installer starts Noura itself.
#[cfg(unix)]
fn relaunch_after_exit() -> std::io::Result<()> {
    use std::process::{Command, Stdio};

    let target = relaunch_target()?;
    let script = if cfg!(target_os = "macos") {
        r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open -n "$2""#
    } else {
        r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$2""#
    };
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("noura-restart")
        .arg(std::process::id().to_string())
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(not(unix))]
fn relaunch_after_exit() -> std::io::Result<()> {
    Ok(())
}

/// The app bundle on macOS, or the AppImage on Linux.
#[cfg(unix)]
fn relaunch_target() -> std::io::Result<std::path::PathBuf> {
    let missing = || std::io::Error::new(std::io::ErrorKind::NotFound, "no app to relaunch");
    if cfg!(target_os = "macos") {
        let executable = std::env::current_exe()?;
        executable
            .ancestors()
            .nth(3)
            .map(Path::to_owned)
            .ok_or_else(missing)
    } else {
        std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .ok_or_else(missing)
    }
}

/// Release checks: the real updater against a local signed feed.
#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Cursor, Write},
        net::TcpListener,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Instant,
    };

    use app_core::{App, FIRST_UPDATE_CHECK, UPDATE_CHECK_INTERVAL, UpdateStatus};
    use base64::{Engine, engine::general_purpose::STANDARD};

    use super::*;

    /// A key pair in the form cargo-packager's signer writes.
    struct Keys {
        public: String,
        secret: minisign::SecretKey,
    }

    fn keys() -> Keys {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        Keys {
            public: STANDARD.encode(pair.pk.to_box().unwrap().to_string()),
            secret: pair.sk,
        }
    }

    /// The `.sig` file cargo-packager writes next to a package.
    fn sign(keys: &Keys, bytes: &[u8]) -> String {
        let signature = minisign::sign(
            None,
            &keys.secret,
            Cursor::new(bytes),
            Some("timestamp:0\tfile:Noura.app.tar.gz"),
            Some("signature from cargo-packager secret key"),
        )
        .unwrap();
        STANDARD.encode(signature.to_string())
    }

    /// The updater's name for this platform in a feed's `platforms`.
    fn platform() -> String {
        let os = match std::env::consts::OS {
            "macos" => "macos",
            "windows" => "windows",
            _ => "linux",
        };
        format!("{os}-{}", std::env::consts::ARCH)
    }

    /// The files a test's server holds, which the test can change.
    type Files = Arc<Mutex<Vec<(&'static str, Vec<u8>)>>>;

    /// Serve the files `files` lists for the server's base URL over HTTP on
    /// a local port. Returns the base URL and a log of the paths requested.
    fn serve(
        files: impl FnOnce(&str) -> Vec<(&'static str, Vec<u8>)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let (base, requested, _) = serve_changing(files);
        (base, requested)
    }

    /// [`serve`], with the files it serves.
    fn serve_changing(
        files: impl FnOnce(&str) -> Vec<(&'static str, Vec<u8>)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, Files) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Files = Arc::new(Mutex::new(files(&base)));
        let requested = Arc::new(Mutex::new(Vec::new()));
        let log = requested.clone();
        let serving = files.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                }
                let path = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                log.lock().unwrap().push(path.clone());
                let file = serving
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(name, _)| *name == path)
                    .map(|(_, body)| body.clone());
                let response = match file {
                    Some(body) => {
                        let mut response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        response.extend_from_slice(&body);
                        response
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = stream.write_all(&response);
            }
        });
        (base, requested, files)
    }

    /// Polls `app` as of `now` until its check and download end.
    fn settle(app: &mut App, now: Instant) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            app.poll_at(now);
            // The status moves only once the check's first event arrives.
            if !app.updates().is_busy()
                && !matches!(
                    app.updates().status(),
                    UpdateStatus::Checking | UpdateStatus::Downloading { .. }
                )
            {
                return;
            }
            assert!(Instant::now() < deadline, "update work never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    const UNREACHABLE: &str = "Couldn't reach the update server. Check your connection.";

    /// A feed offering `version` from the server at `base`, with
    /// `signature` for its bundle, in the shape
    /// scripts/write-update-feed.sh writes.
    fn feed(base: &str, version: &str, signature: &str) -> Vec<u8> {
        serde_json::json!({
            "version": version,
            "notes": "Faster search.",
            "pub_date": "2026-10-20T12:00:00Z",
            "platforms": {
                platform(): {
                    "url": format!("{base}/Noura.app.tar.gz"),
                    "signature": signature,
                    "format": "app",
                },
            },
        })
        .to_string()
        .into_bytes()
    }

    /// A source for the feed at `base`, which replaces nothing on disk even
    /// if a test installs by mistake.
    fn source(base: &str, public_key: &str, install_over: &Path) -> FeedSource {
        let mut source =
            FeedSource::new(&format!("{base}/latest.json"), public_key, "0.2.0", || {}).unwrap();
        source.executable = Some(install_over.join("Noura.app/Contents/MacOS/noura"));
        source
    }

    /// Serve a feed offering `version`, whose bundle is `served` and whose
    /// signature covers `signed`.
    fn release(
        keys: &Keys,
        version: &str,
        signed: &[u8],
        served: &[u8],
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let signature = sign(keys, signed);
        let bundle = served.to_vec();
        serve(|base| {
            vec![
                ("/latest.json", feed(base, version, &signature)),
                ("/Noura.app.tar.gz", bundle),
            ]
        })
    }

    #[test]
    fn the_updater_accepts_a_release_signed_with_the_updater_key() {
        let keys = keys();
        let bundle = b"Noura 99.0.0".to_vec();
        let (base, requested) = release(&keys, "99.0.0", &bundle, &bundle);
        let install_over = tempfile::tempdir().unwrap();
        let source = source(&base, &keys.public, install_over.path());

        let mut update = source.check().unwrap().expect("a newer release");
        assert_eq!(update.version(), "99.0.0");
        assert_eq!(update.notes(), Some("Faster search."));
        update.download().unwrap();

        assert_eq!(
            *requested.lock().unwrap(),
            ["/latest.json", "/Noura.app.tar.gz"]
        );
    }

    #[test]
    fn the_updater_rejects_a_tampered_download() {
        let keys = keys();
        let (base, _) = release(
            &keys,
            "99.0.0",
            b"Noura 99.0.0",
            b"Noura 99.0.0 with a backdoor",
        );
        let install_over = tempfile::tempdir().unwrap();
        let source = source(&base, &keys.public, install_over.path());

        let mut update = source.check().unwrap().expect("a newer release");
        let error = update.download().unwrap_err();

        assert_eq!(error.kind, UpdateErrorKind::BadSignature, "{error}");
    }

    #[test]
    fn the_updater_rejects_a_release_signed_with_another_key() {
        let keys = keys();
        let stranger = self::keys();
        let bundle = b"Noura 99.0.0".to_vec();
        let (base, _) = release(&stranger, "99.0.0", &bundle, &bundle);
        let install_over = tempfile::tempdir().unwrap();
        let source = source(&base, &keys.public, install_over.path());

        let mut update = source.check().unwrap().expect("a newer release");
        let error = update.download().unwrap_err();

        assert_eq!(error.kind, UpdateErrorKind::BadSignature, "{error}");
    }

    #[test]
    fn a_feed_with_the_running_version_has_no_update() {
        let keys = keys();
        let (base, _) = release(&keys, "0.2.0", b"same", b"same");
        let install_over = tempfile::tempdir().unwrap();

        let update = source(&base, &keys.public, install_over.path())
            .check()
            .unwrap();

        assert!(update.is_none());
    }

    #[test]
    fn a_feed_nobody_answers_is_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let install_over = tempfile::tempdir().unwrap();

        let error = match source(&base, &keys().public, install_over.path()).check() {
            Err(error) => error,
            Ok(_) => panic!("a closed port answered"),
        };

        assert_eq!(error.kind, UpdateErrorKind::Unreachable, "{error}");
    }

    #[test]
    fn noura_downloads_from_a_local_feed_and_installs_nothing_until_restart() {
        let keys = keys();
        let bundle = b"Noura 99.0.0".to_vec();
        let (base, requested) = release(&keys, "99.0.0", &bundle, &bundle);
        let install_over = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let mut app = App::launch(app_data.path());
        let launched = Instant::now();
        app.enable_updates(
            "0.2.0",
            Arc::new(source(&base, &keys.public, install_over.path())),
            launched,
        );

        let due = launched + FIRST_UPDATE_CHECK;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            app.poll_at(due);
            if matches!(app.updates().status(), UpdateStatus::Ready { .. }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no update became ready: {:?}",
                app.updates().status()
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(app.updates().offers_restart());
        assert_eq!(
            *requested.lock().unwrap(),
            ["/latest.json", "/Noura.app.tar.gz"]
        );
        let untouched: Vec<PathBuf> = std::fs::read_dir(install_over.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert!(untouched.is_empty(), "nothing installed: {untouched:?}");
    }

    #[test]
    fn noura_keeps_its_schedule_against_a_failing_feed_and_recovers_from_it() {
        let keys = keys();
        let bundle = b"Noura 99.0.0".to_vec();
        let signature = sign(&keys, &bundle);
        let (base, requested, files) = serve_changing(|_| Vec::new());
        let install_over = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let mut app = App::launch(app_data.path());
        let launched = Instant::now();
        app.enable_updates(
            "0.2.0",
            Arc::new(source(&base, &keys.public, install_over.path())),
            launched,
        );
        let checks = || {
            requested
                .lock()
                .unwrap()
                .iter()
                .filter(|path| *path == "/latest.json")
                .count()
        };

        // The feed isn't there yet: nothing goes out before the first check,
        // which fails in plain words.
        settle(
            &mut app,
            launched + FIRST_UPDATE_CHECK - Duration::from_millis(100),
        );
        assert_eq!(checks(), 0);
        let first = launched + FIRST_UPDATE_CHECK;
        app.poll_at(first);
        settle(&mut app, first);
        assert_eq!(checks(), 1);
        assert_eq!(
            app.updates().status(),
            UpdateStatus::Failed {
                message: UNREACHABLE.to_owned()
            }
        );
        assert!(!app.updates().offers_restart());

        // A failed check waits out the schedule like any other.
        let next = first + UPDATE_CHECK_INTERVAL;
        settle(&mut app, next - Duration::from_secs(1));
        assert_eq!(checks(), 1);

        // The feed answers with something that isn't a feed.
        files
            .lock()
            .unwrap()
            .push(("/latest.json", b"<html>a captive portal</html>".to_vec()));
        app.poll_at(next);
        settle(&mut app, next);
        assert_eq!(checks(), 2);
        assert_eq!(
            app.updates().status(),
            UpdateStatus::Failed {
                message: UNREACHABLE.to_owned()
            }
        );

        // Once it recovers, Check for updates finds the release at once.
        *files.lock().unwrap() = vec![
            ("/latest.json", feed(&base, "99.0.0", &signature)),
            ("/Noura.app.tar.gz", bundle),
        ];
        app.check_for_updates();
        settle(&mut app, next);
        assert_eq!(checks(), 3);
        assert!(
            matches!(
                app.updates().status(),
                UpdateStatus::Ready { ref version, .. } if version == "99.0.0"
            ),
            "{:?}",
            app.updates().status()
        );
        assert!(app.updates().offers_restart());
    }

    #[test]
    fn a_tampered_download_offers_no_restart_and_a_good_one_can_follow() {
        let keys = keys();
        let bundle = b"Noura 99.0.0".to_vec();
        let signature = sign(&keys, &bundle);
        let (base, _, files) = serve_changing(|base| {
            vec![
                ("/latest.json", feed(base, "99.0.0", &signature)),
                (
                    "/Noura.app.tar.gz",
                    b"Noura 99.0.0 with a backdoor".to_vec(),
                ),
            ]
        });
        let install_over = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let mut app = App::launch(app_data.path());
        let launched = Instant::now();
        app.enable_updates(
            "0.2.0",
            Arc::new(source(&base, &keys.public, install_over.path())),
            launched,
        );

        let first = launched + FIRST_UPDATE_CHECK;
        app.poll_at(first);
        settle(&mut app, first);
        assert_eq!(
            app.updates().status(),
            UpdateStatus::Failed {
                message: "The download's signature didn't match Noura's key, so Noura deleted it."
                    .to_owned()
            }
        );
        assert!(!app.updates().offers_restart());
        app.restart_to_update();
        assert!(
            std::fs::read_dir(install_over.path())
                .unwrap()
                .next()
                .is_none()
        );

        files.lock().unwrap()[1].1 = bundle;
        app.check_for_updates();
        settle(&mut app, first);
        assert!(
            app.updates().offers_restart(),
            "{:?}",
            app.updates().status()
        );
    }

    #[test]
    fn an_update_staged_for_exit_installs_nothing_while_noura_runs() {
        let keys = keys();
        let bundle = b"Noura 99.0.0".to_vec();
        let (base, _) = release(&keys, "99.0.0", &bundle, &bundle);
        let install_over = tempfile::tempdir().unwrap();
        let at_exit = InstallAtExit::default();
        let source =
            source(&base, &keys.public, install_over.path()).installing_at_exit(at_exit.clone());

        let mut update = source.check().unwrap().expect("a newer release");
        update.download().unwrap();
        update.install().unwrap();

        let untouched: Vec<PathBuf> = std::fs::read_dir(install_over.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert!(untouched.is_empty(), "nothing installed: {untouched:?}");
        let staged = at_exit.take().expect("the update waits for Noura to quit");
        assert_eq!(staged.update.version, "99.0.0");
        assert_eq!(staged.bytes, bundle);
    }

    #[test]
    fn noura_s_updater_key_is_a_minisign_public_key() {
        let decoded = String::from_utf8(STANDARD.decode(RELEASE_PUBLIC_KEY).unwrap()).unwrap();
        minisign::PublicKeyBox::from_string(&decoded)
            .unwrap()
            .into_public_key()
            .unwrap();
    }

    #[test]
    fn only_packaged_release_builds_update_themselves() {
        let bundle = Path::new("/Applications/Noura.app/Contents/MacOS/noura");
        let loose = Path::new("/Users/me/noura/target/release/noura");
        let windows = Path::new(r"C:\Program Files\Noura\noura.exe");
        let linux = Path::new("/usr/bin/noura");
        let appimage = Some(OsStr::new("/home/me/Noura.AppImage"));

        assert!(availability(false, "macos", bundle, None).is_ok());
        assert!(availability(false, "windows", windows, None).is_ok());
        assert!(availability(false, "linux", linux, appimage).is_ok());

        assert!(availability(true, "macos", bundle, None).is_err());
        assert!(availability(false, "macos", loose, None).is_err());
        let deb = availability(false, "linux", linux, None).unwrap_err();
        assert!(deb.contains("noura.app"), "{deb}");
    }
}
