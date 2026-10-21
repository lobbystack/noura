//! Seam 3: installing plugins from folders. The host compiles each
//! component on this device, caches the compiled code in its library, and
//! never accepts code compiled elsewhere.

mod support;

use plugin_host::{CodeSource, InstallErrorCode, PluginHost};
use support::plugins::{self, HELLO_MANIFEST};

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
        PluginHost::new(self.library.path()).expect("the host starts")
    }

    /// Everything in the library folder, by name.
    fn library_files(&self) -> Vec<String> {
        let mut found = Vec::new();
        for entry in walk(self.library.path()) {
            found.push(
                entry
                    .strip_prefix(self.library.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        found.sort();
        found
    }
}

fn walk(folder: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        }
        found.push(path);
    }
    found
}

#[test]
fn installing_compiles_once_and_a_second_start_runs_the_cached_code() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    let host = setup.host();

    let candidate = host.inspect(setup.folder.path()).expect("the folder reads");
    assert_eq!(candidate.manifest().id, "hello");
    host.install(&candidate).expect("the plugin installs");
    assert_eq!(host.installed_ids(), ["hello"]);
    drop(host);

    // The next start of Noura finds the compiled code the install cached.
    let host = setup.host();
    let loaded = host.load("hello").expect("the plugin loads");
    assert_eq!(loaded.source(), CodeSource::Cache);
    let mut instance = host
        .start(&loaded, &loaded.manifest().capabilities, None)
        .expect("the plugin starts");
    let pages = instance.pages().expect("the plugin lists its pages");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].title, "Hello");
}

#[test]
fn a_cache_that_is_gone_compiles_again_on_load() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    let host = setup.host();
    host.install(&host.inspect(setup.folder.path()).unwrap())
        .unwrap();
    for file in setup.library_files() {
        if file.ends_with(".cwasm") {
            std::fs::remove_file(setup.library.path().join(file)).unwrap();
        }
    }

    let loaded = setup.host().load("hello").expect("the plugin loads");

    assert_eq!(loaded.source(), CodeSource::Compiled);
    assert!(
        setup
            .library_files()
            .iter()
            .any(|file| file.ends_with(".cwasm")),
        "the load cached its code again"
    );
    assert_eq!(
        setup.host().load("hello").unwrap().source(),
        CodeSource::Cache
    );
}

#[test]
fn a_changed_component_compiles_again_and_runs_the_new_code() {
    let (Some(hello), Some(probe)) = (
        plugins::built("hello_plugin"),
        plugins::built("probe_plugin"),
    ) else {
        return;
    };
    let setup = Setup::new();
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    let host = setup.host();
    host.install(&host.inspect(setup.folder.path()).unwrap())
        .unwrap();

    // A new build lands in the folder under the same name.
    std::fs::write(setup.folder.path().join("hello_plugin.wasm"), &probe).unwrap();
    let update = host
        .inspect_update("hello", setup.folder.path())
        .expect("the new build reads");
    host.update(&update).expect("the new build installs");

    let host = setup.host();
    let loaded = host.load("hello").expect("the plugin loads");
    assert_eq!(loaded.source(), CodeSource::Cache);
    let mut instance = host
        .start(&loaded, &loaded.manifest().capabilities, None)
        .unwrap();
    assert_eq!(instance.pages().unwrap()[0].title, "Probe");
    // Only the new build's compiled code stays cached.
    let cached: Vec<_> = setup
        .library_files()
        .into_iter()
        .filter(|file| file.ends_with(".cwasm"))
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");
}

#[test]
fn a_plugin_installs_only_once() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    let host = setup.host();
    host.install(&host.inspect(setup.folder.path()).unwrap())
        .unwrap();

    let error = host
        .inspect(setup.folder.path())
        .err()
        .expect("the same ID is refused");

    assert_eq!(error.code, InstallErrorCode::AlreadyInstalled);
    assert!(error.message.contains("hello"), "{}", error.message);
}

#[test]
fn a_folder_that_offers_precompiled_code_fails_to_install() {
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
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &precompiled,
    );
    let host = setup.host();

    let error = host
        .inspect(setup.folder.path())
        .err()
        .expect("precompiled code is refused");

    assert_eq!(error.code, InstallErrorCode::PrecompiledCode);
    assert!(setup.library_files().is_empty());
}

#[test]
fn bytes_that_arent_a_component_fail_to_install() {
    let setup = Setup::new();
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        b"not WebAssembly",
    );

    let error = setup
        .host()
        .inspect(setup.folder.path())
        .err()
        .expect("refused");

    assert_eq!(error.code, InstallErrorCode::NotAComponent);
}

#[test]
fn a_missing_entry_component_fails_to_install() {
    let setup = Setup::new();
    std::fs::write(setup.folder.path().join("plugin.toml"), HELLO_MANIFEST).unwrap();

    let error = setup
        .host()
        .inspect(setup.folder.path())
        .err()
        .expect("refused");

    assert_eq!(error.code, InstallErrorCode::EntryMissing);
}

#[cfg(unix)]
#[test]
fn an_entry_that_links_out_of_the_folder_fails_to_install() {
    let setup = Setup::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("elsewhere.wasm"), b"\0asm").unwrap();
    std::fs::write(setup.folder.path().join("plugin.toml"), HELLO_MANIFEST).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("elsewhere.wasm"),
        setup.folder.path().join("hello_plugin.wasm"),
    )
    .unwrap();

    let error = setup
        .host()
        .inspect(setup.folder.path())
        .err()
        .expect("refused");

    assert_eq!(error.code, InstallErrorCode::EntryOutsideFolder);
}

#[test]
fn uninstalling_removes_the_code_and_its_compiled_cache() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let setup = Setup::new();
    plugins::write_folder(
        setup.folder.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    let host = setup.host();
    host.install(&host.inspect(setup.folder.path()).unwrap())
        .unwrap();

    host.uninstall("hello").expect("the plugin uninstalls");

    assert!(host.installed_ids().is_empty());
    assert!(
        setup.library_files().is_empty(),
        "{:?}",
        setup.library_files()
    );
    assert_eq!(
        host.load("hello").err().unwrap().code,
        InstallErrorCode::NotInstalled
    );
    // The folder it came from keeps its files.
    assert_eq!(
        std::fs::read(setup.folder.path().join("hello_plugin.wasm")).unwrap(),
        hello
    );
}
