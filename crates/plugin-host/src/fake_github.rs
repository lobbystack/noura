//! A stand-in for GitHub's release servers, for tests. It answers the
//! REST API's release requests and the downloads of release assets, which
//! redirect to GitHub's asset host as the real ones do, and records every
//! address lookup and every request it gets, so a test sees what Noura
//! fetched. Releases go through `local-core`'s web access boundary on
//! their way here, as on the network. Only built with the `test-hooks`
//! feature.

use std::{
    io::{Cursor, Write},
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError},
};

use local_core::{TransportRequest, TransportResponse, WebAccessFuture, WebTransport};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

/// The address GitHub's hosts resolve to here, a public one.
const GITHUB_ADDRESS: &str = "140.82.112.3";

const HOSTS: [&str; 3] = [
    "api.github.com",
    "github.com",
    "release-assets.githubusercontent.com",
];

/// One file in an archive that [`archive`] writes.
pub enum Entry<'a> {
    /// A file with these bytes.
    File(&'a str, &'a [u8]),
    /// A symbolic link to a target.
    Link(&'a str, &'a str),
}

/// A zip archive holding `entries`, deflated.
pub fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in entries {
        match entry {
            Entry::File(name, bytes) => {
                writer.start_file(*name, options).expect("the entry starts");
                writer.write_all(bytes).expect("the entry writes");
            }
            Entry::Link(name, target) => {
                writer
                    .add_symlink(*name, *target, options)
                    .expect("the link writes");
            }
        }
    }
    writer.finish().expect("the archive closes").into_inner()
}

/// A release on the stand-in server.
struct Published {
    repository: String,
    tag: String,
    assets: Vec<(String, Vec<u8>)>,
    draft: bool,
}

#[derive(Default)]
struct State {
    /// Oldest first; the last one that isn't a draft is the newest.
    releases: Vec<Published>,
    /// The status every asset download answers with, in place of the
    /// asset.
    failing: Option<u16>,
    /// The asset downloads answer with other bytes than the ones listed.
    tampered: bool,
    /// The asset downloads redirect here in place of GitHub's asset host.
    redirect: Option<String>,
    lookups: Vec<String>,
    requests: Vec<String>,
}

/// Stands in for GitHub's API, its release download links and its asset
/// host.
#[derive(Default)]
pub struct FakeGitHub {
    state: Mutex<State>,
}

impl FakeGitHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish a release of `repository`, `owner/repo`, with the tag `tag`
    /// and these assets. It becomes the newest release.
    pub fn publish(&self, repository: &str, tag: &str, assets: Vec<(String, Vec<u8>)>) {
        self.state().releases.push(Published {
            repository: repository.to_owned(),
            tag: tag.to_owned(),
            assets,
            draft: false,
        });
    }

    /// Publish a release holding the plugin archive `archive` alone,
    /// named as a plugin package names it.
    pub fn publish_archive(&self, repository: &str, tag: &str, archive: Vec<u8>) {
        let name = repository.rsplit('/').next().unwrap_or(repository);
        self.publish(
            repository,
            tag,
            vec![(format!("{name}-{tag}{}", crate::ARCHIVE_SUFFIX), archive)],
        );
    }

    /// Publish a draft release, which only its author sees.
    pub fn publish_draft(&self, repository: &str, tag: &str, archive: Vec<u8>) {
        self.publish_archive(repository, tag, archive);
        if let Some(release) = self.state().releases.last_mut() {
            release.draft = true;
        }
    }

    /// Answer every asset download with `status`, or with the asset again
    /// when `None`.
    pub fn fail_downloads(&self, status: Option<u16>) {
        self.state().failing = status;
    }

    /// Answer asset downloads with bytes that differ from the asset's.
    pub fn tamper_with_downloads(&self) {
        self.state().tampered = true;
    }

    /// Redirect asset downloads to `location`.
    pub fn redirect_downloads(&self, location: &str) {
        self.state().redirect = Some(location.to_owned());
    }

    /// Every URL Noura requested, in order.
    pub fn requests(&self) -> Vec<String> {
        self.state().requests.clone()
    }

    /// Every host Noura looked up, in order.
    pub fn lookups(&self) -> Vec<String> {
        self.state().lookups.clone()
    }

    fn answer(&self, url: &url::Url) -> TransportResponse {
        let state = self.state();
        let path = url.path();
        match url.host_str() {
            Some("api.github.com") => {
                let Some(rest) = path.strip_prefix("/repos/") else {
                    return status(404);
                };
                let Some((repository, which)) = rest.split_once("/releases/") else {
                    return status(404);
                };
                let release =
                    match which {
                        "latest" => state
                            .releases
                            .iter()
                            .rev()
                            .find(|release| release.repository == repository && !release.draft),
                        tag => tag.strip_prefix("tags/").and_then(|tag| {
                            state.releases.iter().rev().find(|release| {
                                release.repository == repository && release.tag == tag
                            })
                        }),
                    };
                match release {
                    Some(release) => json(&details(release)),
                    None => status(404),
                }
            }
            Some("github.com") => {
                let found = state.releases.iter().find_map(|release| {
                    release.assets.iter().find(|(name, _)| {
                        path == format!(
                            "/{}/releases/download/{}/{name}",
                            release.repository, release.tag
                        )
                    })
                });
                match found {
                    Some(_) => {
                        let location = state.redirect.clone().unwrap_or_else(|| {
                            format!("https://release-assets.githubusercontent.com{path}")
                        });
                        TransportResponse {
                            status: 302,
                            headers: vec![("location".into(), location)],
                            body: Vec::new(),
                        }
                    }
                    None => status(404),
                }
            }
            Some("release-assets.githubusercontent.com") => {
                if let Some(failing) = state.failing {
                    return status(failing);
                }
                let found = state.releases.iter().find_map(|release| {
                    release.assets.iter().find(|(name, _)| {
                        path == format!(
                            "/{}/releases/download/{}/{name}",
                            release.repository, release.tag
                        )
                    })
                });
                match found {
                    Some((_, bytes)) => {
                        let mut body = bytes.clone();
                        if state.tampered
                            && let Some(last) = body.last_mut()
                        {
                            *last ^= 0xff;
                        }
                        TransportResponse {
                            status: 200,
                            headers: vec![(
                                "content-type".into(),
                                "application/octet-stream".into(),
                            )],
                            body,
                        }
                    }
                    None => status(404),
                }
            }
            _ => status(404),
        }
    }
}

impl WebTransport for FakeGitHub {
    fn resolve<'a>(&'a self, host: &'a str, _port: u16) -> WebAccessFuture<'a, Vec<IpAddr>> {
        self.state().lookups.push(host.to_owned());
        let addresses = if HOSTS.contains(&host) {
            vec![GITHUB_ADDRESS.parse().expect("the address parses")]
        } else {
            Vec::new()
        };
        Box::pin(async move { Ok(addresses) })
    }

    fn send(&self, request: TransportRequest) -> WebAccessFuture<'_, TransportResponse> {
        self.state().requests.push(request.url.to_string());
        let answer = self.answer(&request.url);
        Box::pin(async move { Ok(answer) })
    }
}

fn details(release: &Published) -> serde_json::Value {
    let assets: Vec<serde_json::Value> = release
        .assets
        .iter()
        .map(|(name, bytes)| {
            let digest: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            serde_json::json!({
                "name": name,
                "size": bytes.len(),
                "state": "uploaded",
                "browser_download_url": format!(
                    "https://github.com/{}/releases/download/{}/{name}",
                    release.repository, release.tag
                ),
                "digest": format!("sha256:{digest}"),
            })
        })
        .collect();
    serde_json::json!({
        "tag_name": release.tag,
        "name": release.tag,
        "html_url": format!(
            "https://github.com/{}/releases/tag/{}",
            release.repository, release.tag
        ),
        "draft": release.draft,
        "prerelease": false,
        "assets": assets,
    })
}

fn json(value: &serde_json::Value) -> TransportResponse {
    TransportResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::to_vec(value).expect("the JSON writes"),
    }
}

fn status(status: u16) -> TransportResponse {
    TransportResponse {
        status,
        headers: Vec::new(),
        body: Vec::new(),
    }
}
