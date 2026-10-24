//! The sync service for protocol v2 (docs/workspace-format/sync-v2.md), as
//! far as the server tracer goes: it creates a remote workspace, pushes
//! encrypted operations, and on any client with the same device credentials
//! fetches the policy and keys, pulls the operations, checks each one's
//! signature, policy position, epoch and chain, and decrypts it.
//!
//! Account chains come from the server (§7). The service judges each one
//! against the heads the device keeps ([`account_heads`]), so a server that
//! forks or rolls back a chain gets caught, and it trusts a workspace only
//! through an anchor or a change its own account's active device signed
//! (§8).
//!
//! The journal, the outbox and file sync arrive with the desktop sync
//! client (ticket 48).

pub mod account_heads;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use local_core::sync::SyncCredentials;
use sync_key_envelope::v2::{DeviceKeys, EnvelopeHeader, unwrap, wrap};
use sync_protocol::account::AccountChain;
use sync_protocol::chain::{ChainRecord, ChainVerifier, NoFetch};
use sync_protocol::codec::{Bytes, Decode, Encode, List, Nested, Sorted, Text};
use sync_protocol::crypto::{hash, random_nonce};
use sync_protocol::keys::{
    EnvelopeVerdict, EpochChain, EpochParams, LogJudge, build_epoch, check_envelope,
};
use sync_protocol::live::{LiveOp, LiveSet};
use sync_protocol::messages::{
    ChainAppendRequest, ChainAppendResponse, ChainCursor, ChainsRequest, ChainsResponse,
    EntryRecord, ErrorBody, KeyRecord, KeysResponse, LimitsResponse, MEDIA_TYPE, Page, PageStatus,
    PolicyResponse, PullCursor, PullRequest, PullResponse, PushRequest, PushResponse,
    RESTORE_GENERATION_HEADER, Receipt, RecordRef,
};

pub use account_heads::{Anchor, KeptHeads};
use sync_protocol::policy::{AccountChains, PolicyChain};
use sync_protocol::records::{
    AccountHead, AccountLink, Card, Content, EpochBundle, Operation, Payload, PayloadKind,
    PolicyChange, PolicyKind, Profile, ProfilePayload, Signed, SyncPath, check_monotonic,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

/// Why a sync call failed.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The server doesn't advertise protocol 2 (§2).
    #[error("the server speaks sync protocol {0}, not 2")]
    ProtocolUnsupported(u16),
    /// The server refused the request with a §26 error code.
    #[error("the server refused the request: {code}")]
    Refused {
        /// The error code.
        code: String,
        /// The HTTP status.
        status: u16,
        /// The code's detail.
        detail: Option<Vec<u8>>,
    },
    /// The server couldn't be reached, or answered outside the protocol.
    #[error("the sync server can't be reached: {0}")]
    Network(String),
    /// A record failed a client check; the code is a client error of §26,
    /// such as `sync_chain_gap`.
    #[error("{0}")]
    Check(&'static str),
    /// A local value can't take part in sync, such as a path that breaks the
    /// rules of §13.
    #[error("{0}")]
    Invalid(String),
}

fn network(error: impl std::fmt::Display) -> SyncError {
    SyncError::Network(error.to_string())
}

fn random<T>(value: Result<T, sync_protocol::crypto::RandomError>) -> Result<T, SyncError> {
    value.map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))
}

/// What a device needs to sync: its account, its keys, its session token
/// and the head from which it verified its own account chain.
#[derive(Clone)]
pub struct DeviceCredentials {
    /// The device's account.
    pub account: Id,
    /// The device's signing and X-Wing keys.
    pub keys: DeviceKeys,
    /// The session token's 43 characters.
    pub session_token: String,
    /// The head from which the device verified its own chain: the genesis
    /// it signed, or the approver's head of the offer that approved it
    /// (§8, Verifying the chain).
    pub reference_head: AccountHead,
}

impl DeviceCredentials {
    fn card(&self) -> Card {
        self.keys.card(self.account)
    }
}

/// An HTTP client for the v2 API.
pub struct SyncClient {
    http: reqwest::Client,
    base: String,
    token: String,
    generation: u64,
}

impl SyncClient {
    /// Connects to the server at `base`, such as `https://sync.noura.app`,
    /// and refuses one that doesn't advertise protocol 2.
    pub async fn connect(base: &str, token: &str) -> Result<Self, SyncError> {
        let mut client = Self {
            http: reqwest::Client::new(),
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            generation: 0,
        };
        let limits: LimitsResponse = client
            .call(reqwest::Method::GET, "/v2/limits", None)
            .await?;
        if limits.protocol != sync_protocol::PROTOCOL_VERSION {
            return Err(SyncError::ProtocolUnsupported(limits.protocol));
        }
        client.generation = limits.restore_generation;
        Ok(client)
    }

    /// The restore generation the server reported last.
    pub fn restore_generation(&self) -> u64 {
        self.generation
    }

    async fn call<T: Decode>(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<T, SyncError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.header("content-type", MEDIA_TYPE).body(body);
        }
        let response = request.send().await.map_err(network)?;
        let status = response.status();
        if let Some(generation) = response
            .headers()
            .get(RESTORE_GENERATION_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| sync_protocol::types::from_decimal(v).ok())
        {
            self.generation = generation;
        }
        let bytes = response.bytes().await.map_err(network)?;
        if !status.is_success() {
            let error = ErrorBody::from_bytes(&bytes).map_err(network)?;
            return Err(SyncError::Refused {
                code: error.code.0,
                status: status.as_u16(),
                detail: error.detail.map(|d| d.0),
            });
        }
        T::from_bytes(&bytes).map_err(network)
    }

    /// `POST /v2/workspaces`.
    pub async fn create_workspace(
        &mut self,
        records: Vec<RecordRef>,
    ) -> Result<PushResponse, SyncError> {
        let body = PushRequest {
            records: List(records),
        }
        .to_bytes();
        self.call(reqwest::Method::POST, "/v2/workspaces", Some(body))
            .await
    }

    /// `POST /v2/workspaces/{w}/ops`.
    pub async fn push(
        &mut self,
        workspace: Id,
        records: Vec<RecordRef>,
    ) -> Result<PushResponse, SyncError> {
        let body = PushRequest {
            records: List(records),
        }
        .to_bytes();
        let path = format!("/v2/workspaces/{}/ops", workspace.to_base64url());
        self.call(reqwest::Method::POST, &path, Some(body)).await
    }

    /// `POST /v2/workspaces/{w}/pull`.
    pub async fn pull(
        &mut self,
        workspace: Id,
        cursors: Vec<PullCursor>,
        wait_seconds: u32,
    ) -> Result<PullResponse, SyncError> {
        let body = PullRequest {
            cursors: List(cursors),
            wait_seconds,
        }
        .to_bytes();
        let path = format!("/v2/workspaces/{}/pull", workspace.to_base64url());
        self.call(reqwest::Method::POST, &path, Some(body)).await
    }

    /// `GET /v2/workspaces/{w}/policy?after={revision}`.
    pub async fn policy(&mut self, workspace: Id, after: u64) -> Result<PolicyResponse, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/policy?after={after}",
            workspace.to_base64url()
        );
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `GET /v2/workspaces/{w}/keys?after={key_seq}&recipient={device}`.
    pub async fn keys(
        &mut self,
        workspace: Id,
        after: u64,
        recipient: Id,
    ) -> Result<KeysResponse, SyncError> {
        let path = format!(
            "/v2/workspaces/{}/keys?after={after}&recipient={}",
            workspace.to_base64url(),
            recipient.to_base64url()
        );
        self.call(reqwest::Method::GET, &path, None).await
    }

    /// `POST /v2/accounts/chain`: appends links to the session's account
    /// chain, in order and all or none.
    pub async fn append_chain(
        &mut self,
        links: Vec<Signed<AccountLink>>,
    ) -> Result<ChainAppendResponse, SyncError> {
        let body = ChainAppendRequest {
            links: List(links.into_iter().map(Nested).collect()),
        }
        .to_bytes();
        self.call(reqwest::Method::POST, "/v2/accounts/chain", Some(body))
            .await
    }

    /// `POST /v2/accounts/chains`: links after each cursor of the chains the
    /// session may read.
    pub async fn read_chains(
        &mut self,
        cursors: Vec<ChainCursor>,
    ) -> Result<ChainsResponse, SyncError> {
        let body = ChainsRequest {
            cursors: List(cursors),
        }
        .to_bytes();
        self.call(reqwest::Method::POST, "/v2/accounts/chains", Some(body))
            .await
    }

    /// Every link of one chain, page by page: the account's current chain
    /// when `genesis` is absent. Returns the links and the chain's genesis.
    pub async fn whole_chain(
        &mut self,
        account: Id,
        genesis: Option<Digest>,
    ) -> Result<(Vec<Signed<AccountLink>>, Digest), SyncError> {
        let mut links: Vec<Signed<AccountLink>> = Vec::new();
        let mut genesis = genesis;
        loop {
            let after = links.len() as u64;
            let response = self
                .read_chains(vec![ChainCursor {
                    account,
                    genesis,
                    after,
                }])
                .await?;
            let page = response
                .pages
                .0
                .into_iter()
                .next()
                .filter(|p| p.account == account && genesis.is_none_or(|g| g == p.genesis))
                .ok_or_else(|| SyncError::Network("the chain page names another chain".into()))?;
            genesis = Some(page.genesis);
            let more = !page.links.0.is_empty();
            links.extend(page.links.0.into_iter().map(|l| l.0));
            if links.len() as u64 >= page.head_seqno || !more {
                return Ok((links, page.genesis));
            }
        }
    }
}

/// Verifies the links the server served for `account`'s chain under
/// `genesis`, which the device doesn't keep yet: links of another chain
/// report `sync_account_unauthorized`, as a chain that breaks a rule does.
fn verify_served(
    account: Id,
    genesis: Digest,
    links: &[Signed<AccountLink>],
    listed_elsewhere: &BTreeSet<Id>,
) -> Result<AccountChain, SyncError> {
    let chain = AccountChain::verify(links, listed_elsewhere)
        .map_err(|e| SyncError::Check(e.client_code()))?;
    if chain.account() != account || chain.genesis() != genesis {
        return Err(SyncError::Check("sync_account_unauthorized"));
    }
    Ok(chain)
}

/// A remote workspace this device created or verified, with its verified
/// policy, the workspace domain's current key and the domain's log state.
pub struct RemoteWorkspace {
    /// The workspace ID.
    pub workspace: Id,
    /// The workspace domain.
    pub domain: Id,
    policy: PolicyChain,
    epochs: EpochChain,
    key: SecretKey,
    own_head: (u64, Digest),
    cursor: u64,
    judge: LogJudge,
    verifier: ChainVerifier,
    live: LiveSet,
    last: BTreeMap<Id, Operation>,
}

impl RemoteWorkspace {
    /// The accepted policy head's revision.
    pub fn policy_revision(&self) -> u64 {
        self.policy.head().0
    }
}

/// A file a pulled operation carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    /// The object's remote ID.
    pub rid: Id,
    /// The workspace-relative path.
    pub path: String,
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// The device that wrote it.
    pub device: Id,
}

/// The credential store the sync service keeps the journal key and the
/// own-account head in.
pub type Secrets = Arc<dyn SyncCredentials + Send + Sync>;

/// The sync service of one device.
pub struct SyncService {
    client: SyncClient,
    credentials: DeviceCredentials,
    heads: KeptHeads,
    secrets: Secrets,
}

impl SyncService {
    /// Connects to the server at `base` with `credentials`, judging account
    /// chains against `heads`, which keeps its own-account head and its key
    /// in `secrets`.
    pub async fn connect(
        base: &str,
        credentials: DeviceCredentials,
        heads: KeptHeads,
        secrets: Secrets,
    ) -> Result<Self, SyncError> {
        let client = SyncClient::connect(base, &credentials.session_token).await?;
        Ok(Self {
            client,
            credentials,
            heads,
            secrets,
        })
    }

    /// The heads the device keeps.
    pub fn kept_heads(&self) -> &KeptHeads {
        &self.heads
    }

    /// The HTTP client, for routes the service doesn't wrap yet.
    pub fn client(&mut self) -> &mut SyncClient {
        &mut self.client
    }

    /// Appends links this device signed to its account's chain, then takes
    /// the chain again from the server.
    pub async fn append_links(
        &mut self,
        links: Vec<Signed<AccountLink>>,
    ) -> Result<AccountChain, SyncError> {
        self.client.append_chain(links).await?;
        self.own_chain().await
    }

    /// The device's own account chain from the server: the chain its
    /// reference head lies on, judged against the head the device keeps of
    /// it, or, before it keeps one, entered from that reference head. A
    /// refusal stops every workspace.
    pub async fn own_chain(&mut self) -> Result<AccountChain, SyncError> {
        if let Some(code) = self.heads.stopped([]) {
            return Err(SyncError::Check(code));
        }
        let account = self.credentials.account;
        let genesis = self.credentials.reference_head.genesis;
        let (links, _) = self.client.whole_chain(account, Some(genesis)).await?;
        let mut served = BTreeMap::new();
        served.insert(genesis, links);
        let listed = self.listed_elsewhere(account, genesis, &served).await?;
        let links = &served[&genesis];
        let secrets = self.secrets.clone();
        if self.heads.kept(&account, &genesis).is_some() {
            return self
                .heads
                .accept(account, genesis, links, &listed, &*secrets);
        }
        let chain = verify_served(account, genesis, links, &listed)?;
        self.heads.enter(
            &chain,
            Anchor::Own(&self.credentials.reference_head),
            &*secrets,
        )?;
        Ok(chain)
    }

    /// The device IDs that the chains of `account` other than `genesis`
    /// list, among those the device keeps and those in `served`, by
    /// genesis: no link may list one of them again (§7).
    async fn listed_elsewhere(
        &mut self,
        account: Id,
        genesis: Digest,
        served: &BTreeMap<Digest, Vec<Signed<AccountLink>>>,
    ) -> Result<BTreeSet<Id>, SyncError> {
        let mut others: BTreeSet<Digest> = self
            .heads
            .kept_of(&account)
            .iter()
            .map(|h| h.genesis)
            .collect();
        others.extend(served.keys().copied());
        others.remove(&genesis);
        let mut listed = BTreeSet::new();
        for other in others {
            let links = match served.get(&other) {
                Some(links) => links.clone(),
                None => match self.client.whole_chain(account, Some(other)).await {
                    Ok((links, _)) => links,
                    // A chain the session may no longer read, such as one
                    // of an account it shares no workspace with anymore.
                    Err(SyncError::Refused { status: 404, .. }) => continue,
                    Err(error) => return Err(error),
                },
            };
            // A chain that fails its own checks gets refused where the
            // device judges it; the devices it lists still count here.
            if let Ok(chain) = AccountChain::verify(&links, &BTreeSet::new()) {
                listed.extend(chain.listed_devices());
            }
        }
        Ok(listed)
    }

    /// Verifies the chains a workspace's policy records, other than the
    /// device's own: each chain under a kept genesis must extend the kept
    /// head, and the others enter only when the verified policy names them
    /// where a new chain may enter (§7).
    async fn recorded_chains(
        &mut self,
        own: &AccountChain,
        changes: &[Signed<PolicyChange>],
    ) -> Result<(AccountChains, Vec<AccountChain>), SyncError> {
        let mut chains = AccountChains::new();
        chains.insert(own.clone());
        let recorded: BTreeSet<(Id, Digest)> = changes
            .iter()
            .flat_map(|c| c.body.heads.iter())
            .map(|h| (h.account, h.genesis))
            .filter(|(account, genesis)| !(*account == own.account() && *genesis == own.genesis()))
            .collect();
        let mut served: BTreeMap<Id, BTreeMap<Digest, Vec<Signed<AccountLink>>>> =
            BTreeMap::new();
        for (account, genesis) in &recorded {
            let (links, _) = self.client.whole_chain(*account, Some(*genesis)).await?;
            served.entry(*account).or_default().insert(*genesis, links);
        }
        let mut fresh = Vec::new();
        let secrets = self.secrets.clone();
        for (account, genesis) in recorded {
            let of_account = &served[&account];
            let listed = self.listed_elsewhere(account, genesis, of_account).await?;
            let links = &of_account[&genesis];
            let chain = if self.heads.kept(&account, &genesis).is_some() {
                self.heads
                    .accept(account, genesis, links, &listed, &*secrets)?
            } else {
                let chain = verify_served(account, genesis, links, &listed)?;
                fresh.push(chain.clone());
                chain
            };
            chains.insert(chain);
        }
        Ok((chains, fresh))
    }

    /// Keeps the heads a verified policy brings in, checks the heads it
    /// records under kept geneses, notes the chain it names for each member,
    /// and refuses a workspace that a refused chain stops.
    fn take_policy(
        &mut self,
        policy: &PolicyChain,
        chains: &AccountChains,
        fresh: Vec<AccountChain>,
    ) -> Result<(), SyncError> {
        let secrets = self.secrets.clone();
        for chain in &fresh {
            self.heads
                .enter(chain, Anchor::Workspace(policy), &*secrets)?;
        }
        let revision = policy.head().0;
        for head in policy.recorded_heads(revision) {
            if let Some(chain) = chains.get(&head.account, &head.genesis) {
                self.heads.check_recorded(chain, head)?;
            }
        }
        let snapshot = policy.head_snapshot();
        for (account, member) in &snapshot.members {
            self.heads
                .name(policy.workspace(), *account, member.head.genesis)?;
        }
        self.check_stops(policy)
    }

    fn check_stops(&self, policy: &PolicyChain) -> Result<(), SyncError> {
        match self
            .heads
            .stopped(policy.head_snapshot().members.keys().copied())
        {
            Some(code) => Err(SyncError::Check(code)),
            None => Ok(()),
        }
    }

    /// Creates a remote workspace named `name`: its genesis records the
    /// account chain's head and makes the account its owner, with the
    /// workspace domain's first epoch, an envelope for every active card of
    /// the account at that head, and the encrypted profile.
    pub async fn create_workspace(&mut self, name: &str) -> Result<RemoteWorkspace, SyncError> {
        let chain = self.own_chain().await?;
        let keys = &self.credentials.keys;
        let workspace = random(Id::random())?;
        let domain = random(Id::random())?;
        let genesis = Signed::sign(
            PolicyChange {
                workspace,
                revision: 1,
                previous: Digest::ZERO,
                signer: keys.device(),
                heads: Sorted::new(vec![chain.head()])
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                kind: PolicyKind::Genesis {
                    workspace_domain: domain,
                },
            },
            keys.signing(),
        );
        let mut chains = AccountChains::new();
        chains.insert(chain.clone());
        let policy = PolicyChain::from_genesis(&genesis, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        let readers = policy.head_snapshot().readers(&domain);
        let key = random(SecretKey::random())?;
        let epoch = Signed::sign(
            build_epoch(
                &EpochParams {
                    workspace,
                    domain,
                    epoch: 1,
                    previous: Digest::ZERO,
                    policy_revision: 1,
                    policy_head: genesis.digest(),
                    rotating_device: keys.device(),
                    recipients: readers.clone(),
                },
                &key,
                None,
                Nonce::ZERO,
            ),
            keys.signing(),
        );
        let mut records = vec![
            RecordRef::Policy(genesis.clone()),
            RecordRef::EpochBundle(EpochBundle {
                epoch: Nested(epoch.clone()),
                recipients: Sorted::new(readers.iter().copied().collect())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            }),
        ];
        for device in &readers {
            let card = &policy.head_snapshot().devices[device].card;
            let envelope = wrap(
                &EnvelopeHeader {
                    workspace,
                    domain,
                    epoch: 1,
                    wrapping_device: keys.device(),
                },
                &key,
                card,
                keys,
            )
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
            records.push(RecordRef::Envelope(envelope));
        }
        let mut profile = Profile {
            workspace,
            profile_revision: 1,
            epoch: 1,
            policy_revision: 1,
            policy_head: genesis.digest(),
            signer: keys.device(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        profile.seal_payload(
            &ProfilePayload {
                name: Text::new(name).map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            },
            &key,
            &epoch.digest(),
            random(random_nonce())?,
        );
        records.push(RecordRef::Profile(Signed::sign(profile, keys.signing())));
        self.client.create_workspace(records).await?;
        self.heads
            .name(workspace, chain.account(), chain.genesis())?;

        let mut epochs = EpochChain::new(workspace, domain);
        epochs
            .accept(&epoch, &key, &policy)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        Ok(RemoteWorkspace {
            workspace,
            domain,
            policy,
            epochs,
            key,
            own_head: (0, Digest::ZERO),
            cursor: 0,
            judge: LogJudge::new(domain),
            verifier: ChainVerifier::new(),
            live: LiveSet::new(),
            last: BTreeMap::new(),
        })
    }

    /// Opens a remote workspace from the server: fetches its policy chain
    /// and every account chain the policy records, judges each chain the
    /// device keeps a head of against that head, verifies the policy, which
    /// must record this account's head in a change that a device of the
    /// account, active at the head the device keeps, signed (§8), then
    /// fetches the workspace domain's epoch records and this device's
    /// envelope, and checks the key.
    pub async fn open_workspace(&mut self, workspace: Id) -> Result<RemoteWorkspace, SyncError> {
        let own = self.own_chain().await?;
        let response = self.client.policy(workspace, 0).await?;
        let changes: Vec<Signed<PolicyChange>> =
            response.changes.0.into_iter().map(|c| c.0).collect();
        if changes.len() as u64 != response.head_revision {
            return Err(SyncError::Check("sync_policy_rollback"));
        }
        let (chains, fresh) = self.recorded_chains(&own, &changes).await?;
        let policy = PolicyChain::verify(&changes, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        if policy.workspace() != workspace {
            return Err(SyncError::Check("sync_workspace_unverified"));
        }
        policy
            .anchored_by(&own, &self.credentials.reference_head)
            .ok_or(SyncError::Check("sync_workspace_unverified"))?;
        self.take_policy(&policy, &chains, fresh)?;
        let domain = policy.head_snapshot().workspace_domain;

        let device = self.credentials.keys.device();
        let card = self.credentials.card();
        let keys = self.client.keys(workspace, 0, device).await?;
        let mut epochs = EpochChain::new(workspace, domain);
        let mut envelopes = BTreeMap::new();
        let mut records = Vec::new();
        for entry in keys.entries.0 {
            match entry.record {
                KeyRecord::Epoch(record) if record.body.domain == domain => records.push(record),
                KeyRecord::Envelope(envelope) if envelope.body.domain == domain => {
                    envelopes.insert(envelope.body.epoch, envelope);
                }
                _ => {}
            }
        }
        let mut key = None;
        for record in records {
            let envelope = envelopes
                .get(&record.body.epoch)
                .ok_or(SyncError::Check("sync_key_mismatch"))?;
            let unwrapped = unwrap(&envelope.body, &self.credentials.keys, &card)
                .map_err(|_| SyncError::Check("sync_key_mismatch"))?;
            let verdict = check_envelope(
                envelope,
                &record.body,
                &unwrapped,
                &card,
                &policy,
                policy.head().0,
                None,
            )
            .map_err(|e| SyncError::Check(e.client_code()))?;
            if verdict != EnvelopeVerdict::Accept {
                return Err(SyncError::Check("sync_key_mismatch"));
            }
            epochs
                .accept(&record, &unwrapped, &policy)
                .map_err(|e| SyncError::Check(e.client_code()))?;
            key = Some(unwrapped);
        }
        let key = key.ok_or(SyncError::Check("sync_key_mismatch"))?;
        Ok(RemoteWorkspace {
            workspace,
            domain,
            policy,
            epochs,
            key,
            own_head: (0, Digest::ZERO),
            cursor: 0,
            judge: LogJudge::new(domain),
            verifier: ChainVerifier::new(),
            live: LiveSet::new(),
            last: BTreeMap::new(),
        })
    }

    /// Pushes a file at `path` as a new object: a padded, encrypted `file`
    /// operation signed into this device's chain for the workspace domain.
    pub async fn push_file(
        &mut self,
        remote: &mut RemoteWorkspace,
        path: &str,
        bytes: &[u8],
    ) -> Result<Receipt, SyncError> {
        self.check_stops(&remote.policy)?;
        let path =
            SyncPath::new(path).map_err(|e| SyncError::Invalid(format!("path: {}", e.name())))?;
        let epoch = remote
            .epochs
            .latest()
            .ok_or(SyncError::Check("sync_key_mismatch"))?;
        let (revision, head) = remote.policy.head();
        let rid = random(Id::random())?;
        let payload = Payload {
            rid,
            kind: PayloadKind::File {
                object_id: Text(String::new()),
                path,
                revision: hash(bytes),
                content: Content::Inline {
                    bytes: Bytes(bytes.to_vec()),
                },
                origin: None,
            },
        };
        let (counter, previous) = remote.own_head;
        let mut op = Operation {
            workspace: remote.workspace,
            domain: remote.domain,
            device: self.credentials.keys.device(),
            counter: counter + 1,
            previous,
            epoch: epoch.record.body.epoch,
            policy_revision: revision,
            policy_head: head,
            previous_attempt: Digest::ZERO,
            generation: self.client.restore_generation(),
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid,
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        op.seal_payload(
            &payload,
            &remote.key,
            &epoch.digest,
            random(random_nonce())?,
        )
        .map_err(|e| SyncError::Invalid(e.to_string()))?;
        let signed = Signed::sign(op, self.credentials.keys.signing());
        let response = self
            .client
            .push(remote.workspace, vec![RecordRef::Operation(signed.clone())])
            .await?;
        let receipt = response
            .receipts
            .0
            .into_iter()
            .next()
            .filter(|r| r.record_digest == signed.digest())
            .ok_or_else(|| SyncError::Network("the push returned no receipt".into()))?;
        remote.own_head = (signed.body.counter, signed.digest());
        Ok(receipt)
    }

    /// Pulls the workspace domain's log after the remote's cursor, checks
    /// every record before it decrypts anything, and returns the files the
    /// operations carry. A failed check applies nothing from the page and
    /// keeps the cursor.
    pub async fn pull_files(
        &mut self,
        remote: &mut RemoteWorkspace,
    ) -> Result<Vec<RemoteFile>, SyncError> {
        self.check_stops(&remote.policy)?;
        let response = self
            .client
            .pull(
                remote.workspace,
                vec![PullCursor {
                    domain: remote.domain,
                    after: remote.cursor,
                }],
                0,
            )
            .await?;
        let page = response
            .pages
            .0
            .into_iter()
            .find(|p| p.domain == remote.domain)
            .ok_or(SyncError::Check("sync_chain_gap"))?;
        check_page(&page, remote.cursor)?;
        if page.status != PageStatus::Ok {
            return Err(SyncError::Refused {
                code: format!("{:?}", page.status),
                status: 200,
                detail: None,
            });
        }
        // Judge the page on copies, and keep them only when every record
        // passes.
        let mut judge = remote.judge.clone();
        let mut verifier = remote.verifier.clone();
        let mut live = remote.live.clone();
        let mut last = remote.last.clone();
        let mut files = Vec::new();
        for entry in page.entries.iter() {
            match &entry.record {
                EntryRecord::Policy(policy_entry) => {
                    let change = remote
                        .policy
                        .change(policy_entry.revision)
                        .ok_or(SyncError::Check("sync_policy_rollback"))?;
                    if change.digest() != policy_entry.head {
                        return Err(SyncError::Check("sync_policy_forked"));
                    }
                    judge
                        .policy_entry(policy_entry.revision, &remote.policy)
                        .map_err(|e| SyncError::Check(e.client_code()))?;
                }
                EntryRecord::Epoch(epoch) => {
                    let accepted = remote
                        .epochs
                        .get(epoch.body.epoch)
                        .filter(|a| a.digest == epoch.digest())
                        .ok_or(SyncError::Check("sync_key_mismatch"))?;
                    judge.epoch_entry(&accepted.record.body);
                }
                EntryRecord::Checkpoint(_) => {}
                EntryRecord::Operation(op) => {
                    let file = check_and_open(
                        remote,
                        &judge,
                        &mut verifier,
                        &mut live,
                        &mut last,
                        op,
                        entry.seq,
                    )?;
                    files.extend(file);
                }
            }
        }
        remote.judge = judge;
        remote.verifier = verifier;
        remote.live = live;
        remote.last = last;
        remote.cursor = page.complete_through;
        Ok(files)
    }
}

/// The client checks of §13 on one pulled operation, then its decryption.
fn check_and_open(
    remote: &RemoteWorkspace,
    judge: &LogJudge,
    verifier: &mut ChainVerifier,
    live: &mut LiveSet,
    last: &mut BTreeMap<Id, Operation>,
    op: &Signed<Operation>,
    seq: u64,
) -> Result<Option<RemoteFile>, SyncError> {
    let body = &op.body;
    let policy = &remote.policy;
    let device = policy
        .head_snapshot()
        .devices
        .get(&body.device)
        .ok_or(SyncError::Check("sync_writer_not_authorized"))?;
    op.verify(&device.card.signing_public)
        .map_err(|_| SyncError::Check("sync_writer_not_authorized"))?;
    let change = policy
        .change(body.policy_revision)
        .ok_or(SyncError::Check("sync_writer_not_authorized"))?;
    if change.digest() != body.policy_head {
        return Err(SyncError::Check("sync_policy_forked"));
    }
    if let Some(previous) = last.get(&body.device) {
        check_monotonic(previous, body).map_err(|_| SyncError::Check("sync_policy_regressed"))?;
    }
    judge
        .check_operation(&body.device, body.policy_revision, body.epoch, policy)
        .map_err(|e| SyncError::Check(e.client_code()))?;
    verifier
        .process_one(&ChainRecord::from_operation(op, seq), &mut NoFetch)
        .map_err(|e| SyncError::Check(e.client_code()))?;
    last.insert(body.device, body.clone());
    let class = live.apply(LiveOp::from_operation(op, seq));
    // Older epochs' keys come through lineage once rotations exist
    // (ticket 47); the tracer holds the current one.
    let epoch = remote
        .epochs
        .latest()
        .filter(|e| e.record.body.epoch == body.epoch)
        .ok_or(SyncError::Check("sync_key_mismatch"))?;
    // A record that passes every check but doesn't open counts as applied
    // and unreadable (§13); the review store arrives with ticket 48.
    let Ok(payload) = op.body.open_payload(&remote.key, &epoch.digest, class) else {
        return Ok(None);
    };
    Ok(match payload.kind {
        PayloadKind::File {
            path,
            content: Content::Inline { bytes },
            ..
        } => Some(RemoteFile {
            rid: payload.rid,
            path: path.as_str().to_owned(),
            bytes: bytes.0,
            device: body.device,
        }),
        _ => None,
    })
}

/// Checks a pull page against the client's cursor, then its shape (§15). A
/// head below the cursor means the server went back (§14): it answers such
/// a cursor with `complete_through` at the cursor, above its head, so the
/// shape check alone would only call the page malformed. The tracer keeps
/// one generation; ticket 60 handles a page from a new one.
fn check_page(page: &Page, cursor: u64) -> Result<(), SyncError> {
    if page.status == PageStatus::Ok && page.head < cursor {
        return Err(SyncError::Check("sync_server_rollback"));
    }
    page.check_shape()
        .map_err(|_| SyncError::Network("the pull page breaks §15".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(after: u64, complete_through: u64, head: u64) -> Page {
        Page {
            domain: Id([1; 16]),
            status: PageStatus::Ok,
            after,
            complete_through,
            head,
            retention_floor: 0,
            checkpoint_seq: 0,
            restore_generation: 1,
            policy_revision: 1,
            checkpoint_due: 0,
            entries: Default::default(),
        }
    }

    /// A server put back by file copy answers a cursor above its head with
    /// `complete_through` at the cursor, so the client checks the head
    /// against its cursor before the page's shape.
    #[test]
    fn a_head_below_the_cursor_is_a_server_rollback() {
        assert!(matches!(
            check_page(&page(5, 5, 3), 5),
            Err(SyncError::Check("sync_server_rollback"))
        ));
        check_page(&page(3, 5, 5), 3).expect("a page that moves the cursor");
        check_page(&page(5, 5, 5), 5).expect("a page at the head");
        assert!(matches!(
            check_page(&page(3, 6, 5), 3),
            Err(SyncError::Network(_))
        ));
    }
}
