//! Plugin releases on GitHub: the address of a release you paste, and the
//! download of its plugin archive.
//!
//! A release address is `https://github.com/<owner>/<repo>/releases/tag/<tag>`
//! or `https://github.com/<owner>/<repo>/releases/latest`. [`Releases`]
//! reads the release from GitHub's REST API, picks the one asset whose name
//! ends in [`crate::ARCHIVE_SUFFIX`], downloads it, and checks its size and
//! the SHA-256 digest GitHub lists for it. Every request goes through
//! `local-core`'s web access boundary, which allows HTTPS only, reaches
//! only GitHub's hosts, checks every redirect and refuses private
//! addresses.

use std::sync::Arc;

use local_core::{HostRequest, HostRequests, WebAccessLimits, WebMethod, WebTransport};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{ARCHIVE_SUFFIX, InstallError, InstallErrorCode, archive::MAX_COMPONENT_BYTES};

/// The host of GitHub's REST API, where release details come from.
const API_HOST: &str = "api.github.com";

/// The hosts a release asset's download goes through: GitHub, then the
/// servers it redirects downloads to.
const DOWNLOAD_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The largest release archive or release details Noura downloads: the
/// most the web access boundary reads from one response.
pub const MAX_ARCHIVE_BYTES: usize = 10 << 20;

/// The address of a release on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseUrl {
    owner: String,
    repo: String,
    /// The release's tag, as the address writes it, or `None` for the
    /// repository's newest release.
    tag: Option<String>,
}

impl ReleaseUrl {
    /// Read the address `text`. Anything other than a release on GitHub
    /// over HTTPS is refused.
    pub fn parse(text: &str) -> Result<Self, InstallError> {
        let refused = || {
            InstallError::new(
                InstallErrorCode::NotAGitHubRelease,
                "That isn't the address of a release on GitHub. Paste one like \
                 https://github.com/owner/plugin/releases/tag/v1.0.0.",
            )
        };
        let url = Url::parse(text.trim()).map_err(|_| refused())?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(refused());
        }
        let mut segments: Vec<&str> = url.path_segments().ok_or_else(refused)?.collect();
        if segments.last() == Some(&"") {
            segments.pop();
        }
        let release = match segments.as_slice() {
            [owner, repo, "releases", "latest"] => Self {
                owner: (*owner).to_owned(),
                repo: (*repo).to_owned(),
                tag: None,
            },
            [owner, repo, "releases", "tag", tag @ ..] if !tag.is_empty() => Self {
                owner: (*owner).to_owned(),
                repo: (*repo).to_owned(),
                tag: Some(tag.join("/")),
            },
            _ => return Err(refused()),
        };
        let tag_valid = release
            .tag
            .as_deref()
            .is_none_or(|tag| tag.split('/').all(|part| !part.is_empty()));
        if !valid_owner(&release.owner) || !valid_repo(&release.repo) || !tag_valid {
            return Err(refused());
        }
        Ok(release)
    }

    /// The repository's newest release.
    pub fn latest(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            tag: None,
        }
    }

    /// `owner/repo`.
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// The release's page on GitHub.
    pub fn page(&self) -> String {
        match &self.tag {
            Some(tag) => format!(
                "https://github.com/{}/{}/releases/tag/{tag}",
                self.owner, self.repo
            ),
            None => format!(
                "https://github.com/{}/{}/releases/latest",
                self.owner, self.repo
            ),
        }
    }

    fn api(&self) -> String {
        let base = format!(
            "https://{API_HOST}/repos/{}/{}/releases",
            self.owner, self.repo
        );
        match &self.tag {
            Some(tag) => format!("{base}/tags/{tag}"),
            None => format!("{base}/latest"),
        }
    }
}

/// A GitHub account or organization name: letters, digits and single
/// hyphens, up to 39 characters.
fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A GitHub repository name: letters, digits, `.`, `-` and `_`, up to 100
/// characters.
fn valid_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo.len() <= 100
        && repo != "."
        && repo != ".."
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// A release's plugin archive, downloaded and checked.
pub struct Release {
    /// The release's tag.
    pub tag: String,
    /// The release's page on GitHub, naming its tag.
    pub page: String,
    /// The archive's file name.
    pub asset: String,
    pub archive: Vec<u8>,
}

/// Downloads plugin releases from GitHub.
pub struct Releases {
    requests: HostRequests,
}

#[derive(Deserialize)]
struct ReleaseDetails {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

impl Releases {
    /// Downloads over HTTPS on this device's network.
    pub fn github() -> Self {
        let requests = HostRequests::https_with(limits())
            .unwrap_or_else(|error| {
                tracing::warn!(code = %error.code, "release download limits were refused");
                HostRequests::https()
            });
        Self { requests }
    }

    /// Downloads through `transport`, which stands in for the network, as
    /// tests do. The web access boundary checks every request as it does on
    /// the network.
    pub fn with_transport(transport: Arc<dyn WebTransport>) -> Self {
        let requests = HostRequests::new(transport, limits()).expect("the limits are valid");
        Self { requests }
    }

    /// Download the plugin archive of `release`.
    pub fn download(&self, release: &ReleaseUrl) -> Result<Release, InstallError> {
        let response = self.get(
            &release.api(),
            &[API_HOST],
            "application/vnd.github+json",
        )?;
        match response.status {
            200 => {}
            404 => return Err(not_found(release)),
            403 | 429 => {
                return Err(InstallError::new(
                    InstallErrorCode::DownloadFailed,
                    "GitHub refused the request. It allows a set number of requests each \
                     hour, so try again later.",
                ));
            }
            status => return Err(failed_status(status)),
        }
        let details: ReleaseDetails = serde_json::from_slice(&response.body).map_err(|error| {
            tracing::warn!(%error, "GitHub's release details didn't read");
            InstallError::new(
                InstallErrorCode::DownloadFailed,
                "Noura couldn't read GitHub's details of the release.",
            )
        })?;
        if details.draft {
            return Err(not_found(release));
        }
        let archives: Vec<&Asset> = details
            .assets
            .iter()
            .filter(|asset| asset.name.ends_with(ARCHIVE_SUFFIX))
            .collect();
        let asset = match archives.as_slice() {
            [asset] => *asset,
            [] => {
                return Err(InstallError::new(
                    InstallErrorCode::NoPluginArchive,
                    format!(
                        "The release has no plugin archive, a file whose name ends in \
                         {ARCHIVE_SUFFIX}."
                    ),
                ));
            }
            several => {
                return Err(InstallError::new(
                    InstallErrorCode::NoPluginArchive,
                    format!(
                        "The release has {} plugin archives, files whose names end in \
                         {ARCHIVE_SUFFIX}. Noura needs one.",
                        several.len()
                    ),
                ));
            }
        };
        if asset.size > MAX_ARCHIVE_BYTES as u64 || asset.size > MAX_COMPONENT_BYTES {
            return Err(InstallError::new(
                InstallErrorCode::InvalidArchive,
                format!(
                    "The plugin's archive is larger than {} MB.",
                    MAX_ARCHIVE_BYTES >> 20
                ),
            ));
        }
        let download = self.get(
            &asset.browser_download_url,
            &DOWNLOAD_HOSTS,
            "application/octet-stream",
        )?;
        if download.status != 200 {
            return Err(failed_status(download.status));
        }
        check_download(asset, &download.body)?;
        let page = ReleaseUrl::parse(&details.html_url)
            .ok()
            .filter(|page| page.repository().eq_ignore_ascii_case(&release.repository()))
            .map_or_else(|| release.page(), |page| page.page());
        Ok(Release {
            tag: details.tag_name,
            page,
            asset: asset.name.clone(),
            archive: download.body,
        })
    }

    fn get(
        &self,
        url: &str,
        hosts: &[&str],
        accept: &str,
    ) -> Result<local_core::HostResponse, InstallError> {
        let request = HostRequest {
            method: WebMethod::Get,
            url: url.to_owned(),
            headers: vec![
                ("accept".to_owned(), accept.to_owned()),
                ("user-agent".to_owned(), "Noura".to_owned()),
                ("x-github-api-version".to_owned(), "2022-11-28".to_owned()),
            ],
            body: Vec::new(),
        };
        let hosts: Vec<String> = hosts.iter().map(|host| (*host).to_owned()).collect();
        self.requests
            .send_blocking(request, &hosts)
            .map_err(|error| {
                tracing::warn!(code = %error.code, message = %error.message, "a release download failed");
                InstallError::new(
                    InstallErrorCode::DownloadFailed,
                    "Noura couldn't download the release from GitHub. Check your connection, \
                     then try again.",
                )
            })
    }
}

/// The boundary's limits for release downloads: the largest response it
/// allows, and its longest wait.
fn limits() -> WebAccessLimits {
    WebAccessLimits {
        max_response_bytes: MAX_ARCHIVE_BYTES,
        request_timeout: std::time::Duration::from_secs(30),
        ..WebAccessLimits::default()
    }
}

/// Refuse a download whose size or SHA-256 digest differs from what GitHub
/// lists for the asset.
fn check_download(asset: &Asset, bytes: &[u8]) -> Result<(), InstallError> {
    let corrupt = || {
        InstallError::new(
            InstallErrorCode::DownloadCorrupt,
            "The downloaded archive doesn't match the one on GitHub. Try again.",
        )
    };
    if bytes.len() as u64 != asset.size {
        return Err(corrupt());
    }
    if let Some(expected) = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
    {
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(corrupt());
        }
    }
    Ok(())
}

fn not_found(release: &ReleaseUrl) -> InstallError {
    InstallError::new(
        InstallErrorCode::ReleaseNotFound,
        format!(
            "GitHub has no release at {}. Check the address, and that the repository is \
             public.",
            release.page()
        ),
    )
}

fn failed_status(status: u16) -> InstallError {
    tracing::warn!(status, "GitHub answered a release download with an error");
    InstallError::new(
        InstallErrorCode::DownloadFailed,
        format!("GitHub answered with error {status}. Try again later."),
    )
}
