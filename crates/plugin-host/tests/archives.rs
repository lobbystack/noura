//! Seam 3: installing plugins from release archives, the zip files the
//! TypeScript author kit's package command writes. The host reads the
//! manifest and the component from the archive, compiles the component
//! itself, and refuses an archive that holds anything else it can't trust.

mod support;

use std::io::Write;

use plugin_host::{InstallErrorCode, PluginHost, Services};
use support::plugins::{self, HELLO_MANIFEST};
use zip::write::{SimpleFileOptions, ZipWriter};

struct Setup {
    folder: tempfile::TempDir,
    library: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        Self {
            folder: tempfile::tempdir().unwrap(),
            library: tempfile::tempdir().unwrap(),
        }
    }

    fn host(&self) -> PluginHost {
        plugins::host(self.library.path())
    }

    /// Write an archive holding `entries`, as (name, bytes), and return
    /// its path.
    fn archive(&self, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = self.folder.path().join("plugin-0.1.0.zip");
        let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn library_is_empty(&self) -> bool {
        std::fs::read_dir(self.library.path())
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    }
}

/// The hello plugin's manifest with its component under `dist/`, as the
/// kit's package command lays an archive out.
fn manifest_in_dist() -> String {
    HELLO_MANIFEST.replace(
        "entry = \"hello_plugin.wasm\"",
        "entry = \"dist/hello_plugin.wasm\"",
    )
}

#[test]
fn an_archive_installs_and_runs_like_a_folder() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    let manifest = manifest_in_dist();
    let archive = setup.archive(&[
        ("plugin.toml", manifest.as_bytes()),
        ("dist/hello_plugin.wasm", &hello),
    ]);
    let host = setup.host();

    let candidate = host.inspect_archive(&archive).expect("the archive reads");
    assert_eq!(candidate.manifest().id, "hello");
    assert_eq!(candidate.folder(), archive);
    host.install(&candidate).expect("the plugin installs");

    let loaded = host.load("hello").expect("the plugin loads");
    let running = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services::default(),
    );
    let pages = running.pages().expect("the plugin lists its pages");
    assert_eq!(pages[0].title, "Hello");
}

#[test]
fn an_archive_and_a_folder_with_the_same_build_have_the_same_digest() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    let manifest = manifest_in_dist();
    let archive = setup.archive(&[
        ("plugin.toml", manifest.as_bytes()),
        ("dist/hello_plugin.wasm", &hello),
    ]);
    let folder = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), &manifest, "dist/hello_plugin.wasm", &hello);
    let host = setup.host();

    assert_eq!(
        host.inspect_archive(&archive).unwrap().digest(),
        host.inspect(folder.path()).unwrap().digest()
    );
}

#[test]
fn an_archive_without_a_manifest_installs_nothing() {
    let setup = Setup::new();
    let archive = setup.archive(&[("hello_plugin.wasm", b"\0asm")]);

    let error = setup.host().inspect_archive(&archive).err().unwrap();

    assert_eq!(error.code, InstallErrorCode::ManifestMissing);
    assert!(setup.library_is_empty());
}

#[test]
fn an_archive_without_the_entry_component_installs_nothing() {
    let setup = Setup::new();
    let archive = setup.archive(&[("plugin.toml", HELLO_MANIFEST.as_bytes())]);

    let error = setup.host().inspect_archive(&archive).err().unwrap();

    assert_eq!(error.code, InstallErrorCode::EntryMissing);
}

#[test]
fn an_archive_whose_entries_leave_the_plugin_folder_installs_nothing() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    for escaping in [
        "../outside.wasm",
        "/absolute.wasm",
        "dist/../../outside.wasm",
    ] {
        let setup = Setup::new();
        let archive = setup.archive(&[
            ("plugin.toml", HELLO_MANIFEST.as_bytes()),
            ("hello_plugin.wasm", &hello),
            (escaping, b"\0asm"),
        ]);

        let error = setup.host().inspect_archive(&archive).err().unwrap();

        assert_eq!(error.code, InstallErrorCode::InvalidArchive, "{escaping}");
        assert!(error.message.contains(escaping), "{}", error.message);
        assert!(setup.library_is_empty());
    }
}

#[test]
fn an_archive_holding_a_symbolic_link_installs_nothing() {
    let setup = Setup::new();
    let path = setup.folder.path().join("linked.zip");
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("plugin.toml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(HELLO_MANIFEST.as_bytes()).unwrap();
    zip.add_symlink(
        "hello_plugin.wasm",
        "/etc/passwd",
        SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();

    let error = setup.host().inspect_archive(&path).err().unwrap();

    assert_eq!(error.code, InstallErrorCode::InvalidArchive);
    assert!(error.message.contains("link"), "{}", error.message);
}

#[test]
fn an_archive_that_offers_precompiled_code_installs_nothing() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let precompiled = wasmtime::Engine::new(&config)
        .unwrap()
        .precompile_component(&hello)
        .unwrap();
    let setup = Setup::new();
    let archive = setup.archive(&[
        ("plugin.toml", HELLO_MANIFEST.as_bytes()),
        ("hello_plugin.wasm", &precompiled),
    ]);

    let error = setup.host().inspect_archive(&archive).err().unwrap();

    assert_eq!(error.code, InstallErrorCode::PrecompiledCode);
    assert!(setup.library_is_empty());
}

#[test]
fn a_file_that_isnt_a_zip_archive_installs_nothing() {
    let setup = Setup::new();
    let path = setup.folder.path().join("plugin.zip");
    std::fs::write(&path, b"not a zip archive").unwrap();

    let error = setup.host().inspect_archive(&path).err().unwrap();

    assert_eq!(error.code, InstallErrorCode::InvalidArchive);
}

#[test]
fn an_archive_for_an_installed_plugin_says_so() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    let archive = setup.archive(&[
        ("plugin.toml", HELLO_MANIFEST.as_bytes()),
        ("hello_plugin.wasm", &hello),
    ]);
    let host = setup.host();
    host.install(&host.inspect_archive(&archive).unwrap())
        .unwrap();

    let error = host.inspect_archive(&archive).err().unwrap();

    assert_eq!(error.code, InstallErrorCode::AlreadyInstalled);
}
