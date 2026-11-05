//! The third-party license list each package ships, and where a package
//! keeps the plugins that ship with Noura. CI generates the list with
//! cargo-about from the crates this binary links, and cargo-packager puts
//! it among the app's resources.

use std::path::{Path, PathBuf};

/// The file's name in every package.
pub const LICENSES_FILE: &str = "THIRD_PARTY_LICENSES.html";

/// Where a package keeps its resources, given the executable's path and
/// [`std::env::consts::OS`]: `Contents/Resources` in a macOS app bundle,
/// the install folder on Windows, and `usr/lib/<binary>` in a .deb or an
/// AppImage.
pub fn resources_dir(executable: &Path, os: &str) -> Option<PathBuf> {
    let folder = executable.parent()?;
    match os {
        "macos" => Some(folder.parent()?.join("Resources")),
        "windows" => Some(folder.to_owned()),
        _ => {
            let binary = executable.file_name()?;
            Some(folder.parent()?.join("lib").join(binary))
        }
    }
}

/// The license list shipped with this copy of Noura, if there is one.
/// Builds that cargo-packager didn't package have none.
pub fn bundled_licenses() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let path = resources_dir(&executable, std::env::consts::OS)?.join(LICENSES_FILE);
    path.is_file().then_some(path)
}

/// The folder of plugins that ship with this copy of Noura, `plugins`
/// among its resources, one plugin per subfolder. Builds without one ship
/// no plugins.
pub fn bundled_plugins() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let folder = resources_dir(&executable, std::env::consts::OS)?.join("plugins");
    folder.is_dir().then_some(folder)
}

/// The smoke run's line about the plugins this copy ships: the name of
/// each, or the error of a copy that lacks one or can't install it.
pub fn shipped_plugins_line() -> Result<String, String> {
    let problems = app_core::check_shipped_plugins(bundled_plugins().as_deref());
    if problems.is_empty() {
        let names = app_core::SHIPPED_PLUGINS.map(|(_, name)| name);
        return Ok(format!("Plugins: {}", names.join(", ")));
    }
    let missing = problems
        .iter()
        .map(|problem| format!("{} ({})", problem.name, problem.reason))
        .collect::<Vec<_>>();
    Err(format!("Plugins: missing {}", missing.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_package_keeps_the_list_beside_its_resources() {
        assert_eq!(
            resources_dir(
                Path::new("/Applications/Noura.app/Contents/MacOS/noura"),
                "macos"
            ),
            Some(PathBuf::from("/Applications/Noura.app/Contents/Resources"))
        );
        assert_eq!(
            resources_dir(Path::new("/usr/bin/noura"), "linux"),
            Some(PathBuf::from("/usr/lib/noura"))
        );
        assert_eq!(
            resources_dir(Path::new("/tmp/.mount_Noura/usr/bin/noura"), "linux"),
            Some(PathBuf::from("/tmp/.mount_Noura/usr/lib/noura"))
        );
        assert_eq!(
            resources_dir(Path::new("C:/Program Files/Noura/noura.exe"), "windows"),
            Some(PathBuf::from("C:/Program Files/Noura"))
        );
    }
}
