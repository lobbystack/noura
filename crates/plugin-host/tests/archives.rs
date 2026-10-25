//! Seam 3: release archives as the TypeScript author kit's package command
//! lays them out, with the component under `dist/`, and the hostile
//! archives `releases.rs` doesn't cover: a manifest past its size limit, a
//! file larger than its headers say, and one path held twice.

mod support;

use std::io::Write;

use plugin_host::{Archive, InstallErrorCode, PluginHost, Services};
use support::plugins::{self, HELLO_MANIFEST};
use zip::write::{SimpleFileOptions, ZipWriter};

/// A zip archive holding `entries`, as (name, bytes), deflated.
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Apply `patch` to the local header and the central directory header of
/// the entry `name` in `zip`, each given as the offset of its size field
/// and of its name, as a hostile archive's author would edit them.
fn edit_headers(zip: &mut [u8], name: &str, patch: impl Fn(&mut [u8], usize, usize)) {
    let mut edited = 0;
    let mut at = 0;
    while at + 46 <= zip.len() {
        // A local header holds its uncompressed size at 22 and its name at
        // 30, and a central directory header at 24 and 46.
        let fields = match zip[at..at + 4] {
            [0x50, 0x4b, 0x03, 0x04] => Some((at + 22, at + 30)),
            [0x50, 0x4b, 0x01, 0x02] => Some((at + 24, at + 46)),
            _ => None,
        };
        if let Some((size, named)) = fields
            && zip[named..].starts_with(name.as_bytes())
        {
            patch(zip, size, named);
            edited += 1;
        }
        at += 1;
    }
    assert_eq!(edited, 2, "both headers of {name}");
}

/// The hello plugin's manifest with its component under `dist/`, as the
/// kit's package command lays an archive out.
fn manifest_in_dist() -> String {
    HELLO_MANIFEST.replace(
        "entry = \"hello_plugin.wasm\"",
        "entry = \"dist/hello_plugin.wasm\"",
    )
}

/// What the host says about the archive `bytes`, which mustn't install.
fn refusal(host: &PluginHost, bytes: &[u8]) -> plugin_host::InstallError {
    Archive::read(bytes)
        .and_then(|archive| host.inspect_archive(archive))
        .err()
        .expect("the archive is refused")
}

#[test]
fn an_archive_laid_out_as_the_kit_packages_it_installs_and_runs_like_a_folder() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let library = tempfile::tempdir().unwrap();
    let manifest = manifest_in_dist();
    let bytes = zip_bytes(&[
        ("plugin.toml", manifest.as_bytes()),
        ("dist/hello_plugin.wasm", &hello),
    ]);
    let host = plugins::host(library.path());

    let candidate = host
        .inspect_archive(Archive::read(&bytes).expect("the archive reads"))
        .expect("the component compiles");
    assert_eq!(candidate.manifest().id, "hello");
    assert_eq!(candidate.folder(), None);
    host.install(&candidate).expect("the plugin installs");

    let loaded = host.load("hello").expect("the plugin loads");
    let running = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services::default(),
    );
    let pages = running.pages().expect("the plugin lists its pages");
    assert_eq!(pages[0].title, "Hello");

    // The same archive again names a plugin that is installed already.
    let error = refusal(&host, &bytes);
    assert_eq!(error.code, InstallErrorCode::AlreadyInstalled);
}

#[test]
fn an_archive_and_a_folder_with_the_same_build_have_the_same_digest() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let library = tempfile::tempdir().unwrap();
    let manifest = manifest_in_dist();
    let bytes = zip_bytes(&[
        ("plugin.toml", manifest.as_bytes()),
        ("dist/hello_plugin.wasm", &hello),
    ]);
    let folder = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), &manifest, "dist/hello_plugin.wasm", &hello);
    let host = plugins::host(library.path());

    assert_eq!(
        host.inspect_archive(Archive::read(&bytes).unwrap())
            .unwrap()
            .digest(),
        host.inspect(folder.path()).unwrap().digest()
    );
}

#[test]
fn an_archive_whose_manifest_is_over_64_kb_installs_nothing() {
    let library = tempfile::tempdir().unwrap();
    let manifest = format!("{HELLO_MANIFEST}\n# {}\n", "x".repeat(64 << 10));
    let bytes = zip_bytes(&[
        ("plugin.toml", manifest.as_bytes()),
        ("hello_plugin.wasm", b"\0asm"),
    ]);

    let error = refusal(&plugins::host(library.path()), &bytes);

    assert_eq!(error.code, InstallErrorCode::InvalidArchive);
    assert!(error.message.contains("64 KB"), "{}", error.message);
}

#[test]
fn a_file_larger_than_its_headers_say_stops_the_read_and_installs_nothing() {
    let library = tempfile::tempdir().unwrap();
    let manifest = format!("{HELLO_MANIFEST}\n# {}\n", "x".repeat(64 << 10));
    let mut bytes = zip_bytes(&[
        ("plugin.toml", manifest.as_bytes()),
        ("hello_plugin.wasm", b"\0asm"),
    ]);
    // Both headers claim the manifest holds 100 bytes.
    edit_headers(&mut bytes, "plugin.toml", |zip, size, _| {
        zip[size..size + 4].copy_from_slice(&100u32.to_le_bytes());
    });

    let error = refusal(&plugins::host(library.path()), &bytes);

    assert_eq!(error.code, InstallErrorCode::InvalidArchive);
    assert!(error.message.contains("plugin.toml"), "{}", error.message);
}

#[test]
fn an_archive_that_holds_one_path_twice_installs_nothing() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let library = tempfile::tempdir().unwrap();
    let host = plugins::host(library.path());
    // The writer refuses a name twice, so the second entry's name is
    // edited to the first's afterwards. The archive's index keeps one of
    // the two, so only its headers show the other.
    let mut bytes = zip_bytes(&[
        ("plugin.toml", HELLO_MANIFEST.as_bytes()),
        ("hello_plugin.wasm", &hello),
        ("hello_plugin.wasX", b"\0asm"),
    ]);
    edit_headers(&mut bytes, "hello_plugin.wasX", |zip, _, name| {
        zip[name + "hello_plugin.was".len()] = b'm';
    });

    assert_eq!(
        refusal(&host, &bytes).code,
        InstallErrorCode::InvalidArchive
    );

    // A folder entry with the component's path holds it twice too.
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("plugin.toml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(HELLO_MANIFEST.as_bytes()).unwrap();
    zip.start_file("hello_plugin.wasm", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&hello).unwrap();
    zip.add_directory("hello_plugin.wasm", SimpleFileOptions::default())
        .unwrap();
    let bytes = zip.finish().unwrap().into_inner();

    let error = refusal(&host, &bytes);

    assert_eq!(error.code, InstallErrorCode::InvalidArchive);
    assert!(error.message.contains("twice"), "{}", error.message);
    assert!(host.installed_ids().is_empty());
}
