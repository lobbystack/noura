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
use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::crypto::random_nonce;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{KeyRecord, MAX_BODY, RecordRef};
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

    /// Takes the workspace domain's keys again, from `keys/current` and
    /// lineage, after a rotation or a replacement (§9). The epoch the device
    /// accepted last must still be in the chain, or a replacement must have
    /// ended it.
    pub async fn refresh_keys(&mut self, remote: &mut RemoteWorkspace) -> Result<(), SyncError> {
        let recipient = self.credentials.keys.clone();
        let (epochs, keys) = self
            .load_keys(remote.workspace, remote.domain, &remote.policy, &recipient)
            .await?;
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
        Ok(())
    }

    /// The account chains a change in `remote` relies on: every chain its
    /// log records, the device's own included, judged against the heads
    /// the device keeps.
    async fn chains_of(&mut self, remote: &RemoteWorkspace) -> Result<AccountChains, SyncError> {
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

    /// A current-epoch envelope of the workspace domain for every card that
    /// `next` brings in, when this device could read the domain before.
    fn envelopes_for(
        &self,
        remote: &RemoteWorkspace,
        before: &Snapshot,
        next: &Snapshot,
    ) -> Result<Vec<Signed<Envelope>>, SyncError> {
        let device = self.credentials.keys.device();
        let domain = remote.domain;
        let Some(added) = next.new_readers.get(&domain) else {
            return Ok(Vec::new());
        };
        // A recovering device wraps with the keys its proven recovery
        // identity reads (§12).
        let identity_reads = self
            .recovery
            .as_ref()
            .is_some_and(|identity| before.can_read(&identity.device(), &domain));
        if !before.can_read(&device, &domain) && !identity_reads {
            return if next.rotations.contains(&domain) {
                Ok(Vec::new())
            } else {
                Err(SyncError::Invalid(
                    "a device that reads the domain records this head".into(),
                ))
            };
        }
        let epoch = remote.epoch();
        added
            .iter()
            .map(|reader| {
                let card = &next.devices[reader].card;
                wrap(
                    &EnvelopeHeader {
                        workspace: remote.workspace,
                        domain,
                        epoch,
                        wrapping_device: device,
                    },
                    &remote.key,
                    card,
                    &self.credentials.keys,
                )
                .map_err(|e| SyncError::Invalid(e.to_string()))
            })
            .collect()
    }

    /// Rotates the workspace domain (§9): a new epoch key, an epoch record
    /// one above the latest at the accepted policy head for every reader,
    /// with the previous key in its lineage, and an envelope for each
    /// reader, in one push.
    pub async fn rotate(&mut self, remote: &mut RemoteWorkspace) -> Result<(), SyncError> {
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
        let number = latest.record.body.epoch + 1;
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
            records.push(RecordRef::Envelope(envelope));
        }
        self.client.push(remote.workspace, records).await?;
        remote
            .epochs
            .accept(&epoch, &key, &remote.policy)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        remote.keys.insert(number, key.clone());
        remote.key = key;
        Ok(())
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
        self.refresh_keys(remote).await?;
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

    /// Answers the open re-wrap requests of the workspace domain's current
    /// epoch that other devices posted (§9): this device wraps the key it
    /// verified for each requester its own verified policy lets read the
    /// domain, and pushes the envelope alone. The server takes the first
    /// few answers.
    pub async fn answer_rewraps(
        &mut self,
        remote: &mut RemoteWorkspace,
    ) -> Result<Vec<RewrapAnswer>, SyncError> {
        let device = self.credentials.keys.device();
        let current = self.client.current_keys(remote.workspace, device).await?;
        let epoch = remote.epoch();
        let mut answers = Vec::new();
        for entry in current.entries.0 {
            let KeyRecord::Rewrap(request) = entry.record else {
                continue;
            };
            let snapshot = remote.policy.head_snapshot();
            let authorized = request.device != device
                && request.epoch == epoch
                && snapshot.can_read(&request.device, &remote.domain);
            if !authorized {
                continue;
            }
            let card = &snapshot.devices[&request.device].card;
            let envelope = wrap(
                &EnvelopeHeader {
                    workspace: remote.workspace,
                    domain: remote.domain,
                    epoch,
                    wrapping_device: device,
                },
                &remote.key,
                card,
                &self.credentials.keys,
            )
            .map_err(|e| SyncError::Invalid(e.to_string()))?;
            match self
                .client
                .push(remote.workspace, vec![RecordRef::Envelope(envelope)])
                .await
            {
                Ok(_) => answers.push(RewrapAnswer::Answered(request.device)),
                Err(SyncError::Refused { code, .. }) if code == "forbidden" => {
                    answers.push(RewrapAnswer::Refused(request.device))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(answers)
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
