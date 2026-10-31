//! Recording account heads and rotating keys (docs/workspace-format/
//! sync-v2.md §8, §9): the `account_heads` changes a device pushes, with a
//! current-epoch envelope for every card they bring in, the rotations those
//! changes make due, the re-wraps a device asks for and answers, and the
//! refreshes of the policy and keys that come first.
//!
//! A device records a later head of a member account, its own included,
//! after a random wait: 0 to 60 seconds for its own account, 60 to 120 for
//! another account's, so an online device of the account records it first.
//! A head whose envelopes wouldn't fit one push goes in steps, one
//! card-adding link at a time. The device that commits a change requiring
//! rotation rotates every domain it reads, one push per domain.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::Duration;

use sync_key_envelope::v2::{EnvelopeHeader, wrap};
use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::crypto::random_nonce;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{KeyRecord, MAX_BODY, RecordRef, RewrapRequest};
use sync_protocol::policy::{AccountChains, Snapshot};
use sync_protocol::records::{
    AccountHead, Envelope, EpochBundle, PolicyChange, PolicyKind, Signed,
};
use sync_protocol::types::{Id, SecretKey};

use super::{RecordedChains, RemoteWorkspace, SyncError, SyncService, random};

/// The most bytes a push of one `account_heads` change and its envelopes
/// may take: one request body (§1, §20).
pub const MAX_PUSH: usize = MAX_BODY;

/// A head the device should record in a workspace, and how long it waits
/// first (§8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingHead {
    /// The account.
    pub account: Id,
    /// Its latest head the device accepted.
    pub head: AccountHead,
    /// Whether it's the device's own account.
    pub own: bool,
    /// The random wait's range: 0 to 60 seconds for the device's own
    /// account, 60 to 120 for another account's.
    pub wait: Range<Duration>,
}

/// A random wait within `range`, from the operating system's CSPRNG.
pub fn random_wait(range: &Range<Duration>) -> Duration {
    let span = range.end.saturating_sub(range.start).as_millis() as u64;
    if span == 0 {
        return range.start;
    }
    let bytes: [u8; 8] = sync_protocol::crypto::random_array().unwrap_or_default();
    range.start + Duration::from_millis(u64::from_be_bytes(bytes) % span)
}

/// What a device does with a re-wrap request it saw (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RewrapAnswer {
    /// It wrapped the key for the requester and pushed the envelope.
    Answered(Id),
    /// The server took enough answers already, or the request closed.
    Refused(Id),
}

/// What a removal the workspace hasn't recorded means for a device's
/// writes (§9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemovalHold {
    /// The device holds its writes.
    pub held: bool,
    /// A domain is full, so no change can record the removal until room
    /// frees, and the device writes as before.
    pub over_limit: bool,
}

impl SyncService {
    /// Takes the workspace's policy again from the server and keeps it when
    /// it extends the accepted one; returns whether it moved. A shorter
    /// chain reports `sync_policy_rollback`, another change at an accepted
    /// revision `sync_policy_forked`.
    pub async fn refresh_policy(
        &mut self,
        remote: &mut RemoteWorkspace,
    ) -> Result<bool, SyncError> {
        let policy = self.verified_policy(remote.workspace).await?;
        let (old_revision, old_head) = remote.policy.head();
        let (new_revision, _) = policy.head();
        if new_revision < old_revision {
            return Err(SyncError::Check("sync_policy_rollback"));
        }
        if policy.change(old_revision).map(|c| c.digest()) != Some(old_head) {
            return Err(SyncError::Check("sync_policy_forked"));
        }
        remote.policy = policy;
        Ok(new_revision > old_revision)
    }

    /// Checks the current epoch's envelopes as a pull does at that epoch's
    /// entry, with the revision of the latest policy entry before it, and
    /// takes no key.
    #[cfg(feature = "test-hooks")]
    pub async fn check_keys_after_entry(
        &mut self,
        workspace: Id,
        domain: Id,
        policy: &sync_protocol::policy::PolicyChain,
        entry_revision: u64,
    ) -> Result<(), SyncError> {
        let recipient = self.credentials.keys.clone();
        self.load_keys_at(
            workspace,
            domain,
            policy,
            &recipient,
            true,
            Some(entry_revision),
        )
        .await
        .map(|_| ())
    }

    /// Takes the workspace domain's keys again, from `keys/current` and
    /// lineage, after a rotation or a replacement (§9). The epoch the device
    /// accepted last must still be in the chain, or a replacement must have
    /// ended it.
    pub async fn refresh_keys(&mut self, remote: &mut RemoteWorkspace) -> Result<(), SyncError> {
        self.refresh_keys_at(remote, None).await
    }

    /// [`SyncService::refresh_keys`], with the latest policy entry before
    /// the current epoch's entry once a pulled page carried it (§9). When
    /// the device can't accept the latest epoch record, or the key its
    /// envelope carries fails, after a verified check and while it holds an
    /// earlier epoch, it signs a replacement; when the server refuses that
    /// with `epoch_conflict`, it asks for a re-wrap and waits.
    pub(crate) async fn refresh_keys_at(
        &mut self,
        remote: &mut RemoteWorkspace,
        entry_revision: Option<u64>,
    ) -> Result<(), SyncError> {
        let recipient = self.credentials.keys.clone();
        let loaded = self
            .load_keys_at(
                remote.workspace,
                remote.domain,
                &remote.policy,
                &recipient,
                !remote.keys.is_empty(),
                entry_revision,
            )
            .await;
        let (epochs, keys, envelope) = match loaded {
            Ok(loaded) => loaded,
            Err(SyncError::Check("sync_key_mismatch")) if remote.epochs.latest().is_some() => {
                return self.replace_or_rewrap(remote).await;
            }
            Err(error) => return Err(error),
        };
        if let Some(old) = remote.epochs.latest() {
            let number = old.record.body.epoch;
            let kept = epochs.get(number).is_some_and(|e| e.digest == old.digest)
                || epochs.ended_at(number).is_some();
            if !kept {
                return Err(SyncError::Check("sync_key_mismatch"));
            }
        }
        let latest = epochs
            .latest()
            .ok_or(SyncError::Check("sync_key_mismatch"))?
            .record
            .body
            .epoch;
        remote.key = keys
            .get(&latest)
            .cloned()
            .ok_or(SyncError::Check("sync_key_mismatch"))?;
        remote.keys.extend(keys);
        remote.epochs = epochs;
        remote.envelope = envelope;
        Ok(())
    }

    /// The account chains a change in `remote` relies on: every chain its
    /// log records, the device's own included, judged against the heads
    /// the device keeps.
    pub(crate) async fn chains_of(
        &mut self,
        remote: &RemoteWorkspace,
    ) -> Result<AccountChains, SyncError> {
        let own = self.own_chain().await?;
        let changes: Vec<Signed<PolicyChange>> = (1..=remote.policy.head().0)
            .filter_map(|r| remote.policy.change(r).cloned())
            .collect();
        let RecordedChains { chains, .. } = self.recorded_chains(&own, &changes).await?;
        Ok(chains)
    }

    /// The heads the device should record in `remote` (§8): each member
    /// account's latest head it accepted, later than the one the workspace
    /// records, and its own account's head once when the workspace's head
    /// of it came from a change that no device of its account signed.
    pub async fn pending_heads(
        &mut self,
        remote: &RemoteWorkspace,
    ) -> Result<Vec<PendingHead>, SyncError> {
        let own_account = self.credentials.account;
        let own = self.own_chain().await?;
        let snapshot = remote.policy.head_snapshot();
        let mut pending = Vec::new();
        for (account, member) in &snapshot.members {
            let latest = if *account == own_account {
                (own.genesis() == member.head.genesis).then(|| own.head())
            } else {
                self.heads
                    .kept(account, &member.head.genesis)
                    .map(|kept| AccountHead {
                        account: *account,
                        genesis: member.head.genesis,
                        seqno: kept.seqno,
                        head: kept.head,
                    })
            };
            let Some(latest) = latest else {
                continue;
            };
            let own = *account == own_account;
            let later = latest.seqno > member.head.seqno;
            if later || (own && !member.recorded_by_own) {
                pending.push(PendingHead {
                    account: *account,
                    head: latest,
                    own,
                    wait: if own {
                        Duration::ZERO..Duration::from_secs(60)
                    } else {
                        Duration::from_secs(60)..Duration::from_secs(120)
                    },
                });
            }
        }
        Ok(pending)
    }

    /// Whether the device holds its writes in `remote`'s workspace domain
    /// after it learned of a removal the workspace hasn't recorded (§9): a
    /// head it accepted of a member account, later than the one the
    /// workspace records, takes a card out of the workspace, or the change
    /// that recorded such a head still waits for its rotation. While it
    /// holds, the device encrypts nothing there. A head the server must
    /// refuse with `recipient_limit`, which the verified policy confirms,
    /// holds nothing, and the answer says so for the app's alert.
    pub async fn removal_hold(
        &mut self,
        remote: &RemoteWorkspace,
    ) -> Result<RemovalHold, SyncError> {
        let snapshot = remote.policy.head_snapshot().clone();
        let mut over_limit = false;
        for pending in self.pending_heads(remote).await? {
            let recorded = snapshot
                .members
                .get(&pending.account)
                .map_or(0, |m| m.head.seqno);
            if pending.head.seqno <= recorded {
                continue;
            }
            let chain = if pending.own {
                self.own_chain().await?
            } else {
                let (links, _) = self
                    .client
                    .whole_chain(pending.account, Some(pending.head.genesis))
                    .await?;
                match AccountChain::verify(&links, &BTreeSet::new()) {
                    Ok(chain) => chain,
                    Err(error) => return Err(SyncError::Check(error.client_code())),
                }
            };
            // A chain cut below the head the device accepted can't show
            // the removal, so it ends no hold.
            if !chain.contains(&pending.head) {
                return Err(SyncError::Check("sync_account_forked"));
            }
            let takes_out = snapshot.devices.iter().any(|(device, wd)| {
                wd.account == pending.account
                    && chain.card(device).is_some()
                    && !chain.is_active_at(device, pending.head.seqno)
            });
            if takes_out && snapshot.head_fits(&chain, pending.head.seqno) {
                return Ok(RemovalHold {
                    held: true,
                    over_limit,
                });
            }
            over_limit |= takes_out;
        }
        Ok(RemovalHold {
            held: self.rotation_pending(remote),
            over_limit,
        })
    }

    /// Records `pending` in `remote` after its wait, unless another change
    /// recorded it meanwhile (§8). Returns whether this device pushed a
    /// change.
    pub async fn push_pending_head(
        &mut self,
        remote: &mut RemoteWorkspace,
        pending: &PendingHead,
    ) -> Result<bool, SyncError> {
        self.refresh_policy(remote).await?;
        let still = self
            .pending_heads(remote)
            .await?
            .into_iter()
            .any(|p| p.account == pending.account);
        if !still {
            return Ok(false);
        }
        let heads = if pending.own {
            Vec::new()
        } else {
            vec![pending.head.clone()]
        };
        Ok(self.record_heads(remote, &heads).await?.is_some())
    }

    /// Pushes an `account_heads` change in `remote` recording the device's
    /// own account's latest head and `others`, with a current-epoch envelope
    /// for every card the change brings into a domain the device reads, in
    /// one push when it fits and in steps of one card-adding link when it
    /// doesn't (§8). It then rotates every domain the change makes due that
    /// the device reads, one push per domain. Returns the new policy
    /// revision, or `None` when the change would record nothing new.
    pub async fn record_heads(
        &mut self,
        remote: &mut RemoteWorkspace,
        others: &[AccountHead],
    ) -> Result<Option<u64>, SyncError> {
        self.record_heads_within(remote, others, MAX_PUSH).await
    }

    /// [`SyncService::record_heads`] with pushes of at most `limit` bytes.
    pub async fn record_heads_within(
        &mut self,
        remote: &mut RemoteWorkspace,
        others: &[AccountHead],
        limit: usize,
    ) -> Result<Option<u64>, SyncError> {
        self.refresh_policy(remote).await?;
        let own = self.own_chain().await?;
        let mut targets: BTreeMap<Id, AccountHead> = BTreeMap::new();
        targets.insert(own.account(), own.head());
        for head in others {
            targets.insert(head.account, head.clone());
        }
        let mut pushed = None;
        loop {
            let chains = self.chains_of(remote).await?;
            let snapshot = remote.policy.head_snapshot().clone();
            // The heads still later than the recorded ones, and the own
            // head, which the change must hold.
            let due: BTreeMap<Id, AccountHead> = targets
                .iter()
                .filter(|(account, head)| {
                    **account == own.account()
                        || snapshot.members.get(account).is_some_and(|m| {
                            m.head.genesis == head.genesis && head.seqno > m.head.seqno
                        })
                })
                .map(|(a, h)| (*a, h.clone()))
                .collect();
            let own_recorded = snapshot.members.get(&own.account());
            let records_something = due.iter().any(|(account, head)| {
                snapshot
                    .members
                    .get(account)
                    .is_some_and(|m| m.head.genesis == head.genesis && head.seqno > m.head.seqno)
            }) || own_recorded.is_some_and(|m| !m.recorded_by_own);
            if !records_something {
                return Ok(pushed);
            }
            let mut heads = due.clone();
            let mut records = self.change_records(remote, &chains, &heads)?;
            // Too large: step the first lagging account's head back one
            // card-adding link at a time until the push fits.
            while size_of(&records) > limit {
                let stepped = step_back(&mut heads, &snapshot, &chains);
                if !stepped {
                    return Err(SyncError::Invalid(
                        "an account head's envelopes don't fit one push".into(),
                    ));
                }
                records = self.change_records(remote, &chains, &heads)?;
            }
            let (change, next) = match &records[0] {
                RecordRef::Policy(change) => {
                    let next = remote
                        .policy
                        .check(change, &chains)
                        .map_err(|e| SyncError::Check(e.client_code()))?;
                    (change.clone(), next)
                }
                _ => unreachable!("the change comes first"),
            };
            self.client.push(remote.workspace, records).await?;
            remote
                .policy
                .append(&change, &chains)
                .map_err(|e| SyncError::Check(e.client_code()))?;
            pushed = Some(change.body.revision);
            if next.rotations.contains(&remote.domain)
                && next.can_read(&self.credentials.keys.device(), &remote.domain)
            {
                self.rotate(remote).await?;
            }
            if heads == due {
                return Ok(pushed);
            }
        }
    }

    /// The records of an `account_heads` change recording `heads`, signed
    /// by this device, then a current-epoch envelope for every card it
    /// brings into the workspace domain when the device reads it.
    fn change_records(
        &self,
        remote: &RemoteWorkspace,
        chains: &AccountChains,
        heads: &BTreeMap<Id, AccountHead>,
    ) -> Result<Vec<RecordRef>, SyncError> {
        let keys = &self.credentials.keys;
        let (revision, previous) = remote.policy.head();
        let change = Signed::sign(
            PolicyChange {
                workspace: remote.workspace,
                revision: revision + 1,
                previous,
                signer: keys.device(),
                heads: Sorted::new(heads.values().cloned().collect())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                kind: PolicyKind::AccountHeads,
            },
            keys.signing(),
        );
        let before = remote.policy.head_snapshot();
        let next = remote
            .policy
            .check(&change, chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        let mut records = vec![RecordRef::Policy(change)];
        records.extend(
            self.envelopes_for(remote, before, &next)?
                .into_iter()
                .map(RecordRef::Envelope),
        );
        Ok(records)
    }

    /// A current-epoch envelope, in every domain this device holds a key
    /// of, for each card that `next` brings in, when the device could read
    /// the domain before.
    pub(crate) fn envelopes_for(
        &self,
        remote: &RemoteWorkspace,
        before: &Snapshot,
        next: &Snapshot,
    ) -> Result<Vec<Signed<Envelope>>, SyncError> {
        let device = self.credentials.keys.device();
        let mut envelopes = Vec::new();
        for (domain, added) in &next.new_readers {
            // A new domain's envelopes go with its epoch 1.
            if !before.domains.contains_key(domain) {
                continue;
            }
            // A recovering device wraps with the keys its proven recovery
            // identity reads (§12).
            let identity_reads = self
                .recovery
                .as_ref()
                .is_some_and(|identity| before.can_read(&identity.device(), domain));
            if !before.can_read(&device, domain) && !identity_reads {
                if next.rotations.contains(domain) {
                    continue;
                }
                return Err(SyncError::Invalid(
                    "only a device that reads the domain records this head, since it brings cards in without a rotation".into(),
                ));
            }
            let (epoch, key) = if *domain == remote.domain {
                (remote.epoch(), &remote.key)
            } else {
                match remote.restricted.get(domain) {
                    Some((epoch, key)) => (*epoch, key),
                    None => return Err(SyncError::KeyMissing),
                }
            };
            for reader in added {
                let envelope = wrap(
                    &EnvelopeHeader {
                        workspace: remote.workspace,
                        domain: *domain,
                        epoch,
                        wrapping_device: device,
                    },
                    key,
                    &next.devices[reader].card,
                    &self.credentials.keys,
                )
                .map_err(|e| SyncError::Invalid(e.to_string()))?;
                envelopes.push(envelope);
            }
        }
        Ok(envelopes)
    }

    /// Rotates the workspace domain (§9): a new epoch key, an epoch record
    /// one above the latest at the accepted policy head for every reader,
    /// with the previous key in its lineage, and an envelope for each
    /// reader, in one push.
    pub async fn rotate(&mut self, remote: &mut RemoteWorkspace) -> Result<(), SyncError> {
        let number = remote
            .epochs
            .latest()
            .ok_or(SyncError::Check("sync_key_mismatch"))?
            .record
            .body
            .epoch
            + 1;
        self.rotate_from(remote, number).await
    }

    /// A rotation to epoch `number` from the latest epoch the device
    /// accepted: the next epoch, or a replacement when `number` lies
    /// further on (§9).
    async fn rotate_from(
        &mut self,
        remote: &mut RemoteWorkspace,
        number: u64,
    ) -> Result<(), SyncError> {
        let keys = self.credentials.keys.clone();
        let latest = remote
            .epochs
            .latest()
            .ok_or(SyncError::Check("sync_key_mismatch"))?
            .clone();
        let snapshot = remote.policy.head_snapshot().clone();
        let (revision, head) = remote.policy.head();
        let domain = remote.domain;
        let readers: BTreeSet<Id> = snapshot.readers(&domain);
        let key = random(SecretKey::random())?;
        let epoch = Signed::sign(
            build_epoch(
                &EpochParams {
                    workspace: remote.workspace,
                    domain,
                    epoch: number,
                    previous: latest.digest,
                    policy_revision: revision,
                    policy_head: head,
                    rotating_device: keys.device(),
                    recipients: readers.clone(),
                },
                &key,
                Some(&remote.key),
                random(random_nonce())?,
            ),
            keys.signing(),
        );
        let mut records = vec![RecordRef::EpochBundle(EpochBundle {
            epoch: Nested(epoch.clone()),
            recipients: Sorted::new(readers.iter().copied().collect())
                .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
        })];
        let mut own_envelope = None;
        for reader in &readers {
            let envelope = wrap(
                &EnvelopeHeader {
                    workspace: remote.workspace,
                    domain,
                    epoch: number,
                    wrapping_device: keys.device(),
                },
                &key,
                &snapshot.devices[reader].card,
                &keys,
            )
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
            if *reader == keys.device() {
                own_envelope = Some(envelope.clone());
            }
            records.push(RecordRef::Envelope(envelope));
        }
        self.client.push(remote.workspace, records).await?;
        remote
            .epochs
            .accept(&epoch, &key, &remote.policy)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        remote.keys.insert(number, key.clone());
        remote.key = key;
        if own_envelope.is_some() {
            remote.envelope = own_envelope;
        }
        Ok(())
    }

    /// Signs a replacement of the workspace domain's latest epoch, which
    /// this device can't accept (§9, Replacing an epoch): an epoch record
    /// one above the latest, whose `previous` names the last epoch record
    /// it accepted and whose lineage holds that record's key, at the
    /// accepted policy head, with an envelope for every reader. When the
    /// server refuses it with `epoch_conflict`, because another device used
    /// a skipped epoch, the device asks for a re-wrap instead and waits.
    async fn replace_or_rewrap(&mut self, remote: &mut RemoteWorkspace) -> Result<(), SyncError> {
        let current = self
            .client
            .current_keys(remote.workspace, self.credentials.keys.device())
            .await?;
        let latest = current
            .entries
            .0
            .iter()
            .find_map(|e| match &e.record {
                KeyRecord::Epoch(epoch) if epoch.body.domain == remote.domain => {
                    Some(epoch.body.epoch)
                }
                _ => None,
            })
            .ok_or(SyncError::KeyMissing)?;
        let accepted = remote
            .epochs
            .latest()
            .ok_or(SyncError::Check("sync_key_mismatch"))?
            .clone();
        if latest <= accepted.record.body.epoch {
            return Err(SyncError::Check("sync_key_mismatch"));
        }
        match self.rotate_from(remote, latest + 1).await {
            Ok(()) => Ok(()),
            Err(SyncError::Refused { code, .. }) if code == "epoch_conflict" => {
                self.client
                    .rewrap(remote.workspace, remote.domain, latest)
                    .await?;
                Err(SyncError::KeyMissing)
            }
            Err(error) => Err(error),
        }
    }

    /// Whether the server's current epoch of `remote`'s domain is the one
    /// the device accepted.
    async fn holds_latest_epoch(&mut self, remote: &RemoteWorkspace) -> Result<bool, SyncError> {
        let Some(accepted) = remote.epochs.latest().map(|e| e.digest) else {
            return Ok(false);
        };
        let current = self
            .client
            .current_keys(remote.workspace, self.credentials.keys.device())
            .await?;
        Ok(current.entries.0.iter().any(|e| {
            matches!(&e.record, KeyRecord::Epoch(epoch)
                if epoch.body.domain == remote.domain && epoch.digest() == accepted)
        }))
    }

    /// Whether a rotation of the workspace domain is pending in the
    /// accepted policy: a change above the latest epoch record's
    /// `policy_revision` requires it (§9, Epoch authority).
    pub fn rotation_pending(&self, remote: &RemoteWorkspace) -> bool {
        let Some(latest) = remote.epochs.latest() else {
            return false;
        };
        remote
            .policy
            .rotation_required(
                &remote.domain,
                latest.record.body.policy_revision,
                remote.policy.head().0,
            )
            .is_some()
    }

    /// Rotates the workspace domain if a rotation is still pending there
    /// and the device reads it, as every key holder does after its random
    /// wait (§9); one that another device committed meanwhile leaves the
    /// device taking the new keys instead. Returns whether this device
    /// rotated.
    pub async fn rotate_if_pending(
        &mut self,
        remote: &mut RemoteWorkspace,
    ) -> Result<bool, SyncError> {
        self.refresh_policy(remote).await?;
        match self.refresh_keys(remote).await {
            // The wrapper lost its access since, so the envelope waits for
            // the epoch's entry (§9), but a device that accepted this
            // epoch holds its key and rotates from it.
            Err(SyncError::KeyMissing) if self.holds_latest_epoch(remote).await? => {}
            refreshed => refreshed?,
        }
        let reads = remote
            .policy
            .head_snapshot()
            .can_read(&self.credentials.keys.device(), &remote.domain);
        if !reads || !self.rotation_pending(remote) {
            return Ok(false);
        }
        match self.rotate(remote).await {
            Ok(()) => Ok(true),
            Err(SyncError::Refused { code, .. }) if code == "epoch_conflict" => {
                self.refresh_keys(remote).await?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Asks for a new envelope of the workspace domain's current epoch, as
    /// a device whose envelope failed and that can't sign a replacement does
    /// (§9).
    pub async fn request_rewrap(&mut self, remote: &RemoteWorkspace) -> Result<(), SyncError> {
        let epoch = remote.epoch();
        self.client
            .rewrap(remote.workspace, remote.domain, epoch)
            .await
    }

    /// Answers the open re-wrap requests of the current epochs that other
    /// devices posted (§9): this device wraps the key it verified for each
    /// requester its own verified policy lets read the domain, and pushes
    /// the envelope alone. The server takes the first few answers.
    pub async fn answer_rewraps(
        &mut self,
        remote: &mut RemoteWorkspace,
    ) -> Result<Vec<RewrapAnswer>, SyncError> {
        let mut answers = Vec::new();
        for request in self.open_rewraps(remote).await? {
            answers.push(self.answer_rewrap(remote, &request).await?);
        }
        Ok(answers)
    }

    /// The open re-wrap requests of the current epochs that this device
    /// may answer: another device's, of a domain both read under the
    /// device's verified policy, the workspace domain or a restricted one
    /// (§9).
    pub async fn open_rewraps(
        &mut self,
        remote: &RemoteWorkspace,
    ) -> Result<Vec<RewrapRequest>, SyncError> {
        let device = self.credentials.keys.device();
        let current = self.client.current_keys(remote.workspace, device).await?;
        let mut epochs = BTreeMap::from([(remote.domain, remote.epoch())]);
        for entry in &current.entries.0 {
            if let KeyRecord::Epoch(epoch) = &entry.record
                && epoch.body.domain != remote.domain
            {
                epochs.insert(epoch.body.domain, epoch.body.epoch);
            }
        }
        let snapshot = remote.policy.head_snapshot();
        Ok(current
            .entries
            .0
            .into_iter()
            .filter_map(|entry| match entry.record {
                KeyRecord::Rewrap(request)
                    if request.device != device
                        && epochs.get(&request.domain) == Some(&request.epoch)
                        && snapshot.can_read(&device, &request.domain)
                        && snapshot.can_read(&request.device, &request.domain) =>
                {
                    Some(request)
                }
                _ => None,
            })
            .collect())
    }

    /// Wraps the verified key of the domain's current epoch for the device
    /// that posted `request` and pushes the envelope alone (§9).
    pub async fn answer_rewrap(
        &mut self,
        remote: &RemoteWorkspace,
        request: &RewrapRequest,
    ) -> Result<RewrapAnswer, SyncError> {
        // The device holds the verified key of the current epoch alone; an
        // older request closed when the next epoch committed.
        let Some(key) = self.rewrap_key(remote, request).await? else {
            return Ok(RewrapAnswer::Refused(request.device));
        };
        let snapshot = remote.policy.head_snapshot();
        let card = &snapshot
            .devices
            .get(&request.device)
            .ok_or(SyncError::Check("sync_policy_unauthorized"))?
            .card;
        let envelope = wrap(
            &EnvelopeHeader {
                workspace: remote.workspace,
                domain: request.domain,
                epoch: request.epoch,
                wrapping_device: self.credentials.keys.device(),
            },
            &key,
            card,
            &self.credentials.keys,
        )
        .map_err(|e| SyncError::Invalid(e.to_string()))?;
        match self
            .client
            .push(remote.workspace, vec![RecordRef::Envelope(envelope)])
            .await
        {
            Ok(_) => Ok(RewrapAnswer::Answered(request.device)),
            Err(SyncError::Refused { code, .. }) if code == "forbidden" => {
                Ok(RewrapAnswer::Refused(request.device))
            }
            Err(error) => Err(error),
        }
    }
}

impl SyncService {
    /// The verified key `request` asks for, when it names the current epoch
    /// of a domain this device reads. A restricted domain's keys load here,
    /// since the key upkeep's view of a workspace holds the workspace
    /// domain's alone.
    async fn rewrap_key(
        &mut self,
        remote: &RemoteWorkspace,
        request: &RewrapRequest,
    ) -> Result<Option<SecretKey>, SyncError> {
        if request.domain == remote.domain {
            return Ok((request.epoch == remote.epoch()).then(|| remote.key.clone()));
        }
        let keys = self.credentials.keys.clone();
        let loaded = self
            .load_keys(remote.workspace, request.domain, &remote.policy, &keys, true)
            .await;
        let (epochs, keys, _) = match loaded {
            Ok(loaded) => loaded,
            // This device holds no key to give.
            Err(SyncError::KeyMissing | SyncError::Check("sync_key_mismatch")) => return Ok(None),
            Err(error) => return Err(error),
        };
        let current = epochs.latest().map(|e| e.record.body.epoch);
        Ok((current == Some(request.epoch))
            .then(|| keys.get(&request.epoch).cloned())
            .flatten())
    }
}

/// The bytes a push of `records` takes.
fn size_of(records: &[RecordRef]) -> usize {
    4 + records
        .iter()
        .map(|r| 5 + r.to_bytes().len())
        .sum::<usize>()
}

/// Steps one account's head in `heads` back to the latest earlier link
/// that adds a card, keeping it later than the head the workspace records;
/// returns false when no head can step back.
fn step_back(
    heads: &mut BTreeMap<Id, AccountHead>,
    snapshot: &Snapshot,
    chains: &AccountChains,
) -> bool {
    for (account, head) in heads.iter_mut() {
        let Some(member) = snapshot.members.get(account) else {
            continue;
        };
        if head.genesis != member.head.genesis || head.seqno <= member.head.seqno + 1 {
            continue;
        }
        let Some(chain) = chains.get(account, &head.genesis) else {
            continue;
        };
        // The latest link below the head that adds a card, above the
        // recorded head.
        let adds = |seqno: u64| chain.cards().any(|state| state.added_at == seqno);
        let target = (member.head.seqno + 1..head.seqno)
            .rev()
            .find(|seqno| adds(*seqno))
            .unwrap_or(member.head.seqno + 1);
        if let Some(stepped) = chain.head_at(target) {
            *head = stepped;
            return true;
        }
    }
    false
}
