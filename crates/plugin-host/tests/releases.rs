//! Seam 3: installing plugins from release archives on GitHub. Downloads go
//! to a stand-in for GitHub's servers through the web access boundary, and
//! no test reaches github.com. The host compiles each archive's component
//! itself, never loads compiled code from an archive, and refuses a hostile
//! or broken archive before anything lands in its library.

mod support;

use std::sync::Arc;

use plugin_host::{
    Archive, Capability, CodeSource, InstallErrorCode, ReleaseUrl, Releases, Services,
    WorkspaceReads,
    fake_github::{Entry, FakeGitHub, archive},
};
use support::plugins::{self, FakeWorkspace};

const REPOSITORY: &str = "maker/probe-plugin";
const RELEASE: &str = "https://github.com/maker/probe-plugin/releases/tag/v0.1.0";

fn probe_manifest(id: &str, version: &str, capabilities: &[&str]) -> String {
    plugins::manifest(id, "plugin.wasm", capabilities).replace(
        "version = \"0.1.0\"\ninterface",
        &format!("version = \"{version}\"\ninterface"),
    )
}

/// An archive holding `manifest` and `component` as `plugin.wasm`.
fn plugin_archive(manifest: &str, component: &[u8]) -> Vec<u8> {
    archive(&[
        Entry::File("plugin.toml", manifest.as_bytes()),
        Entry::File("plugin.wasm", component),
    ])
}

struct Setup {
    github: Arc<FakeGitHub>,
    releases: Releases,
    library: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let github = FakeGitHub::new();
        Self {
            releases: Releases::with_transport(github.clone()),
            github,
            library: tempfile::tempdir().unwrap(),
        }
    }

    fn host(&self) -> plugin_host::PluginHost {
        plugins::host(self.library.path())
    }

    /// Download the release at `url` and read its archive.
    fn archive(&self, url: &str) -> Result<Archive, InstallErrorCode> {
        let release = self
            .releases
            .download(&ReleaseUrl::parse(url).map_err(|error| error.code)?)
            .map_err(|error| error.code)?;
        Archive::read(&release.archive).map_err(|error| error.code)
    }

    /// Every file in the library, by path.
    fn library_files(&self) -> Vec<String> {
        let mut found = Vec::new();
        walk(self.library.path(), self.library.path(), &mut found);
        found.sort();
        found
    }
}

fn walk(root: &std::path::Path, folder: &std::path::Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(root, &path, found);
        } else {
            found.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
}

#[test]
fn release_addresses_on_github_are_accepted() {
    for (url, page) in [
        (RELEASE, RELEASE),
        (
            "https://github.com/maker/probe-plugin/releases/tag/v0.1.0/",
            RELEASE,
        ),
        (
            "  https://GitHub.com/maker/probe-plugin/releases/latest ",
            "https://github.com/maker/probe-plugin/releases/latest",
        ),
    ] {
        let release = ReleaseUrl::parse(url).unwrap_or_else(|error| panic!("{url}: {error}"));
        assert_eq!(release.page(), page, "{url}");
        assert_eq!(release.repository(), REPOSITORY);
    }
}

#[test]
fn any_other_address_is_refused() {
    for url in [
        "http://github.com/maker/probe-plugin/releases/tag/v0.1.0",
        "https://gitlab.com/maker/probe-plugin/releases/tag/v0.1.0",
        "https://github.com.evil.example/maker/probe-plugin/releases/tag/v0.1.0",
        "https://github.com:8443/maker/probe-plugin/releases/tag/v0.1.0",
        "https://me@github.com/maker/probe-plugin/releases/tag/v0.1.0",
        "https://github.com/maker/probe-plugin",
        "https://github.com/maker/probe-plugin/releases",
        "https://github.com/maker/probe-plugin/releases/tag/",
        "https://github.com/maker/probe-plugin/releases/download/v0.1.0/probe.zip",
        "https://github.com/maker/probe-plugin/releases/tag/v0.1.0?download=1",
        "https://github.com/maker/probe-plugin/releases/tag/v0.1.0#assets",
        "https://github.com/../probe-plugin/releases/latest",
        "file:///Users/maker/probe-plugin",
        "probe-plugin",
        "",
    ] {
        let error = ReleaseUrl::parse(url).expect_err(url);
        assert_eq!(error.code, InstallErrorCode::NotAGitHubRelease, "{url}");
        assert!(error.message.contains("GitHub"), "{}", error.message);
    }
}

#[test]
fn a_release_downloads_over_https_from_githubs_hosts_only() {
    let setup = Setup::new();
    let archive = plugin_archive(&probe_manifest("probe", "0.1.0", &[]), b"\0asm");
    setup
        .github
        .publish_archive(REPOSITORY, "v0.1.0", archive.clone());

    let release = setup
        .releases
        .download(&ReleaseUrl::parse(RELEASE).unwrap())
        .expect("the release downloads");

    assert_eq!(release.archive, archive);
    assert_eq!(release.tag, "v0.1.0");
    assert_eq!(release.page, RELEASE);
    assert_eq!(release.asset, "probe-plugin-v0.1.0.noura-plugin.zip");
    assert_eq!(
        setup.github.requests(),
        [
            "https://api.github.com/repos/maker/probe-plugin/releases/tags/v0.1.0",
            "https://github.com/maker/probe-plugin/releases/download/v0.1.0/\
             probe-plugin-v0.1.0.noura-plugin.zip",
            "https://release-assets.githubusercontent.com/maker/probe-plugin/releases/download/\
             v0.1.0/probe-plugin-v0.1.0.noura-plugin.zip",
        ]
    );
}

#[test]
fn a_redirect_off_https_or_off_github_is_refused_before_it_is_followed() {
    for location in [
        "http://release-assets.githubusercontent.com/maker/probe-plugin/archive.zip",
        "https://downloads.example.com/archive.zip",
    ] {
        let setup = Setup::new();
        setup.github.publish_archive(
            REPOSITORY,
            "v0.1.0",
            plugin_archive(&probe_manifest("probe", "0.1.0", &[]), b"\0asm"),
        );
        setup.github.redirect_downloads(location);

        let error = setup
            .releases
            .download(&ReleaseUrl::parse(RELEASE).unwrap())
            .err()
            .expect("the redirect is refused");

        assert_eq!(error.code, InstallErrorCode::DownloadFailed, "{location}");
        assert!(
            setup
                .github
                .requests()
                .iter()
                .all(|url| url.starts_with("https://") && !url.contains("example.com")),
            "{:?}",
            setup.github.requests()
        );
        assert!(
            !setup.github.lookups().iter().any(|host| host.contains("example")),
            "{:?}",
            setup.github.lookups()
        );
    }
}

#[test]
fn a_download_that_differs_from_githubs_digest_is_refused() {
    let setup = Setup::new();
    setup.github.publish_archive(
        REPOSITORY,
        "v0.1.0",
        plugin_archive(&probe_manifest("probe", "0.1.0", &[]), b"\0asm"),
    );
    setup.github.tamper_with_downloads();

    assert_eq!(
        setup.archive(RELEASE).err(),
        Some(InstallErrorCode::DownloadCorrupt)
    );
}

#[test]
fn a_release_without_exactly_one_plugin_archive_is_refused() {
    let setup = Setup::new();
    setup.github.publish(
        REPOSITORY,
        "v0.1.0",
        vec![("probe.tar.gz".to_owned(), b"not a plugin".to_vec())],
    );
    setup.github.publish(
        REPOSITORY,
        "v0.2.0",
        vec![
            ("one.noura-plugin.zip".to_owned(), b"one".to_vec()),
            ("two.noura-plugin.zip".to_owned(), b"two".to_vec()),
        ],
    );

    for tag in ["v0.1.0", "v0.2.0"] {
        let url = format!("https://github.com/{REPOSITORY}/releases/tag/{tag}");
        assert_eq!(
            setup.archive(&url).err(),
            Some(InstallErrorCode::NoPluginArchive),
            "{tag}"
        );
    }
}

#[test]
fn a_missing_or_draft_release_is_not_found_and_a_failed_download_says_so() {
    let setup = Setup::new();
    setup.github.publish_draft(
        REPOSITORY,
        "v0.3.0",
        plugin_archive(&probe_manifest("probe", "0.3.0", &[]), b"\0asm"),
    );

    for tag in ["v9.9.9", "v0.3.0"] {
        let url = format!("https://github.com/{REPOSITORY}/releases/tag/{tag}");
        assert_eq!(
            setup.archive(&url).err(),
            Some(InstallErrorCode::ReleaseNotFound),
            "{tag}"
        );
    }
    setup.github.publish_archive(
        REPOSITORY,
        "v0.1.0",
        plugin_archive(&probe_manifest("probe", "0.1.0", &[]), b"\0asm"),
    );
    setup.github.fail_downloads(Some(503));
    assert_eq!(
        setup.archive(RELEASE).err(),
        Some(InstallErrorCode::DownloadFailed)
    );
}

#[test]
fn the_host_compiles_an_archives_component_itself_and_runs_the_cached_code() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let setup = Setup::new();
    setup.github.publish_archive(
        REPOSITORY,
        "v0.1.0",
        plugin_archive(&probe_manifest("probe", "0.1.0", &["name = \"views\""]), &probe),
    );
    let host = setup.host();

    let candidate = host
        .inspect_archive(setup.archive(RELEASE).unwrap())
        .expect("the archive compiles");
    assert_eq!(candidate.folder(), None);
    host.install(&candidate).expect("the plugin installs");
    drop(host);

    let host = setup.host();
    let loaded = host.load("probe").expect("the plugin loads");
    assert_eq!(loaded.source(), CodeSource::Cache);
    let running = host.start(&loaded, &loaded.manifest().capabilities, Services::default());
    assert_eq!(running.pages().unwrap()[0].title, "Probe");
}

#[test]
fn compiled_code_planted_in_an_archive_never_loads() {
    let (Some(probe), Some(hello)) = (
        plugins::built("probe_plugin"),
        plugins::built("hello_plugin"),
    ) else {
        return;
    };
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let planted = wasmtime::Engine::new(&config)
        .unwrap()
        .precompile_component(&hello)
        .unwrap();
    let setup = Setup::new();
    let manifest = probe_manifest("probe", "0.1.0", &["name = \"views\""]);
    // Compiled code from another plugin rides along under the names the
    // library uses, and in place of the component itself.
    setup.github.publish(
        REPOSITORY,
        "v0.1.0",
        vec![(
            "probe.noura-plugin.zip".into(),
            archive(&[
                Entry::File("plugin.toml", manifest.as_bytes()),
                Entry::File("plugin.wasm", &probe),
                Entry::File("component.wasm", &planted),
                Entry::File("component.cwasm", &planted),
                Entry::File("probe/component.cwasm", &planted),
            ]),
        )],
    );
    setup.github.publish(
        REPOSITORY,
        "v0.2.0",
        vec![(
            "probe.noura-plugin.zip".into(),
            plugin_archive(&manifest, &planted),
        )],
    );
    let host = setup.host();

    // An archive whose component is compiled code installs nothing.
    let error = host
        .inspect_archive(
            setup
                .archive("https://github.com/maker/probe-plugin/releases/tag/v0.2.0")
                .unwrap(),
        )
        .err()
        .expect("the compiled component is refused");
    assert_eq!(error.code, InstallErrorCode::PrecompiledCode);
    assert!(setup.library_files().is_empty());

    host.install(&host.inspect_archive(setup.archive(RELEASE).unwrap()).unwrap())
        .unwrap();

    let files = setup.library_files();
    assert_eq!(files.len(), 3, "{files:?}");
    assert!(files.contains(&"probe/plugin.toml".to_owned()), "{files:?}");
    assert_eq!(
        std::fs::read(setup.library.path().join("probe/component.wasm")).unwrap(),
        probe
    );
    for file in files.iter().filter(|file| file.ends_with(".cwasm")) {
        assert_ne!(
            std::fs::read(setup.library.path().join(file)).unwrap(),
            planted,
            "{file}"
        );
    }
    drop(host);
    let host = setup.host();
    let loaded = host.load("probe").unwrap();
    assert_eq!(loaded.source(), CodeSource::Cache);
    let running = host.start(&loaded, &loaded.manifest().capabilities, Services::default());
    assert_eq!(running.pages().unwrap()[0].title, "Probe");
}

#[test]
fn a_plugin_from_an_archive_runs_with_exactly_the_approved_capabilities() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let setup = Setup::new();
    setup.github.publish_archive(
        REPOSITORY,
        "v0.1.0",
        plugin_archive(
            &probe_manifest(
                "probe",
                "0.1.0",
                &["name = \"workspace-read\"", "name = \"views\""],
            ),
            &probe,
        ),
    );
    let host = setup.host();
    host.install(&host.inspect_archive(setup.archive(RELEASE).unwrap()).unwrap())
        .unwrap();
    let loaded = host.load("probe").unwrap();
    let workspace = Arc::new(FakeWorkspace::default());
    let reads: Arc<dyn WorkspaceReads> = workspace.clone();

    // You approved views and not workspace reads.
    let running = host.start(
        &loaded,
        &[Capability::Views],
        Services {
            workspace: Some(reads),
            ..Services::default()
        },
    );

    assert_eq!(running.granted(), [Capability::Views]);
    assert_eq!(
        plugins::texts(&running.render("list").unwrap()),
        ["error capability-not-declared workspace-read"]
    );
    assert!(workspace.calls().is_empty(), "{:?}", workspace.calls());
}

#[test]
fn a_hostile_or_broken_archive_installs_nothing_and_says_why() {
    let manifest = probe_manifest("probe", "0.1.0", &[]);
    let manifest = manifest.as_bytes();
    let cases: Vec<(&str, Vec<u8>, InstallErrorCode)> = vec![
        (
            "an entry that climbs out",
            archive(&[
                Entry::File("plugin.toml", manifest),
                Entry::File("plugin.wasm", b"\0asm"),
                Entry::File("../outside.txt", b"x"),
            ]),
            InstallErrorCode::ArchiveEntryOutside,
        ),
        (
            "an entry that climbs out from a folder",
            archive(&[
                Entry::File("plugin.toml", manifest),
                Entry::File("plugin.wasm", b"\0asm"),
                Entry::File("docs/../../outside.txt", b"x"),
            ]),
            InstallErrorCode::ArchiveEntryOutside,
        ),
        (
            "an absolute entry",
            archive(&[
                Entry::File("plugin.toml", manifest),
                Entry::File("plugin.wasm", b"\0asm"),
                Entry::File("/etc/outside.txt", b"x"),
            ]),
            InstallErrorCode::ArchiveEntryOutside,
        ),
        (
            "a Windows path",
            archive(&[
                Entry::File("plugin.toml", manifest),
                Entry::File("plugin.wasm", b"\0asm"),
                Entry::File("..\\outside.txt", b"x"),
            ]),
            InstallErrorCode::ArchiveEntryOutside,
        ),
        (
            "a link",
            archive(&[
                Entry::File("plugin.toml", manifest),
                Entry::Link("plugin.wasm", "/usr/lib/plugin.wasm"),
            ]),
            InstallErrorCode::ArchiveLink,
        ),
        (
            "a link beside the plugin",
            archive(&[
                Entry::File("plugin.toml", manifest),
                Entry::File("plugin.wasm", b"\0asm"),
                Entry::Link("notes", "../.."),
            ]),
            InstallErrorCode::ArchiveLink,
        ),
        (
            "no manifest",
            archive(&[Entry::File("plugin.wasm", b"\0asm")]),
            InstallErrorCode::ManifestMissing,
        ),
        (
            "a manifest in a folder",
            archive(&[
                Entry::File("probe/plugin.toml", manifest),
                Entry::File("probe/plugin.wasm", b"\0asm"),
            ]),
            InstallErrorCode::ManifestMissing,
        ),
        (
            "an invalid manifest",
            archive(&[
                Entry::File("plugin.toml", b"id = \"probe\"\nname = 3\n"),
                Entry::File("plugin.wasm", b"\0asm"),
            ]),
            InstallErrorCode::InvalidManifest,
        ),
        (
            "an interface this host doesn't run",
            archive(&[
                Entry::File(
                    "plugin.toml",
                    probe_manifest("probe", "0.1.0", &[])
                        .replace("interface-version = \"0.1.0\"", "interface-version = \"9.0.0\"")
                        .as_bytes(),
                ),
                Entry::File("plugin.wasm", b"\0asm"),
            ]),
            InstallErrorCode::UnknownInterfaceVersion,
        ),
        (
            "no entry component",
            archive(&[Entry::File("plugin.toml", manifest)]),
            InstallErrorCode::EntryMissing,
        ),
        (
            "bytes that aren't a zip archive",
            b"PK\x03\x04 not really".to_vec(),
            InstallErrorCode::InvalidArchive,
        ),
    ];
    for (case, bytes, code) in cases {
        let setup = Setup::new();
        setup
            .github
            .publish(REPOSITORY, "v0.1.0", vec![("probe.noura-plugin.zip".into(), bytes)]);
        let host = setup.host();

        let error = setup
            .archive(RELEASE)
            .and_then(|archive| host.inspect_archive(archive).map_err(|error| error.code))
            .err()
            .unwrap_or_else(|| panic!("{case} installed"));

        assert_eq!(error, code, "{case}");
        assert!(setup.library_files().is_empty(), "{case}");
        assert!(host.installed_ids().is_empty(), "{case}");
    }
}

#[test]
fn a_refusal_says_why_in_words() {
    let setup = Setup::new();
    setup.github.publish(
        REPOSITORY,
        "v0.1.0",
        vec![(
            "probe.noura-plugin.zip".into(),
            archive(&[
                Entry::File("plugin.toml", probe_manifest("probe", "0.1.0", &[]).as_bytes()),
                Entry::Link("plugin.wasm", "/usr/lib/plugin.wasm"),
            ]),
        )],
    );
    let release = setup
        .releases
        .download(&ReleaseUrl::parse(RELEASE).unwrap())
        .unwrap();

    let error = Archive::read(&release.archive).err().unwrap();

    assert!(error.message.contains("plugin.wasm"), "{}", error.message);
    assert!(error.message.contains("link"), "{}", error.message);
}

#[test]
fn an_update_that_holds_another_plugin_is_refused() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let setup = Setup::new();
    setup.github.publish_archive(
        REPOSITORY,
        "v0.1.0",
        plugin_archive(&probe_manifest("probe", "0.1.0", &[]), &probe),
    );
    setup.github.publish_archive(
        REPOSITORY,
        "v0.2.0",
        plugin_archive(&probe_manifest("intruder", "0.2.0", &[]), &probe),
    );
    let host = setup.host();
    host.install(&host.inspect_archive(setup.archive(RELEASE).unwrap()).unwrap())
        .unwrap();
    let before = setup.library_files();

    let error = host
        .inspect_archive_update(
            "probe",
            setup
                .archive("https://github.com/maker/probe-plugin/releases/latest")
                .unwrap(),
        )
        .err()
        .expect("the other plugin is refused");

    assert_eq!(error.code, InstallErrorCode::DifferentPlugin);
    assert!(error.message.contains("intruder"), "{}", error.message);
    assert_eq!(setup.library_files(), before);
    assert_eq!(host.installed_ids(), ["probe"]);
    assert_eq!(host.load("probe").unwrap().manifest().version.to_string(), "0.1.0");
}
