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

/// Whether the license list at `path` holds Public Sans's license, the
/// SIL Open Font License, which must ship with the font.
pub fn carries_font_license(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.contains("Public Sans") && text.contains("SIL OPEN FONT LICENSE Version 1.1")
    })
}

/// The folder of plugins that ship with this copy of Noura, `plugins`
/// among its resources, one plugin per subfolder. A debug build that runs
/// from the repository's `target` folder, as `cargo run` does, falls back
/// to the folder `scripts/build-shipped-plugins.sh` writes. Other builds
/// without a `plugins` folder ship no plugins.
pub fn bundled_plugins() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let packaged = resources_dir(&executable, std::env::consts::OS)
        .map(|resources| resources.join("plugins"))
        .filter(|folder| folder.is_dir());
    packaged.or_else(|| {
        let desktop = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = desktop.join("../../target");
        let from_cargo = target
            .canonicalize()
            .is_ok_and(|target| executable.starts_with(target));
        let built = desktop.join("packaging/plugins");
        (cfg!(debug_assertions) && from_cargo && built.is_dir()).then_some(built)
    })
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

    #[test]
    fn the_license_template_carries_the_font_s_license_and_the_check_finds_it() {
        let template = include_str!("../packaging/about.hbs");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LICENSES_FILE);
        std::fs::write(&path, template).unwrap();
        assert!(carries_font_license(&path));
        std::fs::write(&path, "crates only").unwrap();
        assert!(!carries_font_license(&path));
    }
}
