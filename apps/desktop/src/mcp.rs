//! Settings' AI tools section: this binary is the command AI tools start,
//! in its `mcp --workspace <folder>` mode, and `noura-mcp` tests it.

use std::{
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use app_core::{App, McpProbe};

/// Starts the configured command as an AI tool would, through
/// `noura-mcp`'s MCP client.
struct Probe;

impl McpProbe for Probe {
    fn test(
        &self,
        program: &Path,
        arguments: &[OsString],
        wait: Duration,
    ) -> Result<String, String> {
        noura_mcp::test_connection(program, arguments, wait).map(|answered| {
            if answered.version.is_empty() {
                answered.name
            } else {
                format!("{} {}", answered.name, answered.version)
            }
        })
    }
}

/// Offer AI tools this binary's MCP server in Settings. Without its own
/// path, Noura can't write a configuration, so Settings leaves the
/// section out.
pub(crate) fn offer(app: &mut App) {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            tracing::warn!(%error, "Noura doesn't know its own path");
            return;
        }
    };
    let appimage = std::env::var_os("APPIMAGE");
    let appdir = std::env::var_os("APPDIR");
    let program = installed_program(
        std::env::consts::OS,
        &executable,
        appimage.as_deref(),
        appdir.as_deref(),
    );
    app.set_mcp_server(program, Arc::new(Probe));
}

/// The program AI tools start for this copy of Noura, whose executable is
/// `executable`, or why no program would keep working after Noura quits.
/// `os` is [`std::env::consts::OS`]. `appimage` and `appdir` are the
/// variables an AppImage sets for the program it runs: the AppImage's own
/// path, and the folder it mounts for this launch only.
pub(crate) fn installed_program(
    os: &str,
    executable: &Path,
    appimage: Option<&OsStr>,
    appdir: Option<&OsStr>,
) -> Result<PathBuf, String> {
    match os {
        // The mount goes away when Noura quits and has a new name on each
        // launch, so AI tools start the AppImage, which passes its
        // arguments on to Noura. A program that only inherited the
        // variables from an AppImage that started it isn't in the mount.
        "linux" => match (set(appdir), set(appimage)) {
            (Some(appdir), appimage) if executable.starts_with(appdir) => appimage
                .map(PathBuf::from)
                .ok_or_else(|| MOUNTED.to_owned()),
            _ => Ok(executable.to_owned()),
        },
        // Gatekeeper runs an app you haven't moved since you downloaded it
        // from a read-only copy whose path changes on each launch.
        "macos" if translocated(executable) => Err(TRANSLOCATED.to_owned()),
        _ => Ok(executable.to_owned()),
    }
}

const MOUNTED: &str = "You opened Noura from a temporary folder that goes away when Noura \
                       quits, so an AI tool couldn't start it later. Open Noura from its \
                       AppImage, then open Settings again.";

const TRANSLOCATED: &str = "macOS runs this copy of Noura from a temporary folder that changes \
                            each time you open it, so an AI tool couldn't start it later. Move \
                            Noura to your Applications folder, open it from there, then open \
                            Settings again.";

/// A variable's value, unless it is empty.
fn set(variable: Option<&OsStr>) -> Option<&OsStr> {
    variable.filter(|value| !value.is_empty())
}

fn translocated(executable: &Path) -> bool {
    executable
        .components()
        .any(|component| component == Component::Normal(OsStr::new("AppTranslocation")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(
        os: &str,
        executable: &str,
        appimage: Option<&str>,
        appdir: Option<&str>,
    ) -> Result<PathBuf, String> {
        installed_program(
            os,
            Path::new(executable),
            appimage.map(OsStr::new),
            appdir.map(OsStr::new),
        )
    }

    #[test]
    fn an_appimage_names_itself_rather_than_the_folder_it_mounts() {
        assert_eq!(
            program(
                "linux",
                "/tmp/.mount_NouraXk2p9q/usr/bin/noura",
                Some("/home/me/Applications/Noura-Linux.AppImage"),
                Some("/tmp/.mount_NouraXk2p9q"),
            ),
            Ok(PathBuf::from("/home/me/Applications/Noura-Linux.AppImage"))
        );
        // Mounted without the AppImage's path, nothing would last.
        assert_eq!(
            program(
                "linux",
                "/tmp/.mount_NouraXk2p9q/usr/bin/noura",
                None,
                Some("/tmp/.mount_NouraXk2p9q"),
            ),
            Err(MOUNTED.to_owned())
        );
    }

    #[test]
    fn a_package_or_an_app_bundle_names_its_executable() {
        assert_eq!(
            program("linux", "/usr/bin/noura", None, None),
            Ok(PathBuf::from("/usr/bin/noura"))
        );
        // A .deb copy started from a terminal that runs as an AppImage
        // inherits that AppImage's variables.
        assert_eq!(
            program(
                "linux",
                "/usr/bin/noura",
                Some("/home/me/Applications/Terminal.AppImage"),
                Some("/tmp/.mount_TerminQ81z"),
            ),
            Ok(PathBuf::from("/usr/bin/noura"))
        );
        assert_eq!(
            program(
                "macos",
                "/Applications/Noura.app/Contents/MacOS/noura",
                None,
                None
            ),
            Ok(PathBuf::from(
                "/Applications/Noura.app/Contents/MacOS/noura"
            ))
        );
        let windows = r"C:\Users\me\AppData\Local\Noura\noura.exe";
        assert_eq!(
            program("windows", windows, None, None),
            Ok(PathBuf::from(windows))
        );
    }

    #[test]
    fn a_copy_macos_runs_from_a_temporary_folder_says_to_move_it() {
        assert_eq!(
            program(
                "macos",
                "/private/var/folders/x1/abc/T/AppTranslocation/5B1C9F0E/d/Noura.app/Contents/MacOS/noura",
                None,
                None,
            ),
            Err(TRANSLOCATED.to_owned())
        );
    }
}
