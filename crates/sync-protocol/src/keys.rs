//! Key domains (§9): epoch keys and their subkeys, key checks, lineage,
//! epoch record acceptance with replacements, envelope checks, and the
//! write-access and epoch rules that judge each record by its position in a
//! domain's log.
//!
//! X-Wing HPKE wrapping lives in `sync-key-envelope`; the checks here take
//! the key an envelope unwrapped.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::codec::{Writer, encode_list};
use crate::crypto::{context, derive_key, hash_parts, keyed_hash, open, seal};
use crate::policy::PolicyChain;
use crate::records::{Card, Envelope, Epoch, LINEAGE_LEN, Lineage, Signed};
use crate::tag::Tag;
use crate::types::{Digest, Id, Nonce, SecretKey};

/// `derive_key("noura.sync.v2 operation key", epoch_key)`.
pub fn operation_key(epoch_key: &SecretKey) -> SecretKey {
    derive_key(context::OPERATION_KEY, epoch_key.expose())
}

/// `derive_key("noura.sync.v2 lineage key", epoch_key)`.
pub fn lineage_key(epoch_key: &SecretKey) -> SecretKey {
    derive_key(context::LINEAGE_KEY, epoch_key.expose())
}

/// `derive_key("noura.sync.v2 presence key", epoch_key)`.
pub fn presence_key(epoch_key: &SecretKey) -> SecretKey {
    derive_key(context::PRESENCE_KEY, epoch_key.expose())
}

/// `derive_key("noura.sync.v2 profile key", epoch_key)`.
pub fn profile_key(epoch_key: &SecretKey) -> SecretKey {
    derive_key(context::PROFILE_KEY, epoch_key.expose())
}

/// `key_check = BLAKE3.keyed_hash(derive_key("noura.sync.v2 key check",
/// epoch_key), tag(noura.sync.key-check) || workspace || domain || u64
/// epoch)`.
pub fn key_check(epoch_key: &SecretKey, workspace: &Id, domain: &Id, epoch: u64) -> Digest {
    let key = derive_key(context::KEY_CHECK, epoch_key.expose());
    let mut w = Writer::new();
    w.tag(Tag::KeyCheck);
    w.put(workspace);
    w.put(domain);
    w.u64(epoch);
    keyed_hash(&key, w.as_bytes())
}

/// `recipients_digest = BLAKE3(tag(noura.sync.epoch.recipients) ||
/// list<id>)` over the sorted device IDs that receive an epoch's key.
pub fn recipients_digest(recipients: &BTreeSet<Id>) -> Digest {
    let ids: Vec<Id> = recipients.iter().copied().collect();
    let mut w = Writer::new();
    w.tag(Tag::EpochRecipients);
    encode_list(&mut w, &ids);
    hash_parts(&[w.as_bytes()])
}

/// Whether `key` matches an epoch record's `key_check`.
pub fn key_matches(epoch: &Epoch, key: &SecretKey) -> bool {
    key_check(key, &epoch.workspace, &epoch.domain, epoch.epoch) == epoch.key_check
}

/// The fields of a new epoch record before its key check and lineage.
#[derive(Clone, Debug)]
pub struct EpochParams {
    /// The workspace.
    pub workspace: Id,
    /// The domain.
    pub domain: Id,
    /// The new epoch's number.
    pub epoch: u64,
    /// The digest of the record `previous` names, or zero at epoch 1.
    pub previous: Digest,
    /// The policy revision whose readers receive the key.
    pub policy_revision: u64,
    /// The change's digest at that revision.
    pub policy_head: Digest,
    /// The rotating device.
    pub rotating_device: Id,
    /// The recipients, sorted.
    pub recipients: BTreeSet<Id>,
}

/// Builds an epoch record for `key`, with the lineage of `previous_key`
/// sealed under `nonce`, or an empty lineage and zero nonce at epoch 1.
pub fn build_epoch(
    params: &EpochParams,
    key: &SecretKey,
    previous_key: Option<&SecretKey>,
    nonce: Nonce,
) -> Epoch {
    let mut epoch = Epoch {
        workspace: params.workspace,
        domain: params.domain,
        epoch: params.epoch,
        previous: params.previous,
        policy_revision: params.policy_revision,
        policy_head: params.policy_head,
        rotating_device: params.rotating_device,
        recipients_digest: recipients_digest(&params.recipients),
        key_check: key_check(key, &params.workspace, &params.domain, params.epoch),
        lineage_nonce: Nonce::ZERO,
        lineage: Lineage::default(),
    };
    if let Some(previous) = previous_key {
        epoch.lineage_nonce = nonce;
        epoch.lineage = Lineage(seal(
            &lineage_key(key),
            &nonce,
            &epoch.aad(),
            previous.expose(),
        ));
    }
    epoch
}

/// Why an epoch record, an envelope or a lineage step failed. The client
/// reports `sync_key_mismatch` for each, except a policy head that differs,
/// `sync_policy_forked`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum KeyError {
    /// The record names another workspace or domain.
    #[error("wrong workspace or domain")]
    Domain,
    /// The signature doesn't verify under the signer's card.
    #[error("bad signature")]
    Signature,
    /// The signer isn't a device that could read the domain where the rule
    /// asks, or isn't in the workspace's recorded heads.
    #[error("signer can't read the domain")]
    Signer,
    /// Epoch 1 of a domain from a device other than its creator.
    #[error("epoch 1 from a device other than the domain's creator")]
    FirstEpochSigner,
    /// `policy_revision` lies below the previous record's, or above the
    /// accepted head.
    #[error("policy revision out of order")]
    PolicyRevision,
    /// `policy_head` differs from the client's change at that revision.
    #[error("policy head differs")]
    PolicyHead,
    /// `recipients_digest` differs from the readers the client computes.
    #[error("recipients differ")]
    Recipients,
    /// The epoch number or `previous` doesn't follow an accepted record.
    #[error("epoch doesn't follow an accepted record")]
    Chain,
    /// The lineage doesn't decrypt to the key of the record `previous`
    /// names, or isn't empty at epoch 1.
    #[error("lineage mismatch")]
    Lineage,
    /// A key doesn't match its record's `key_check`.
    #[error("key doesn't match key_check")]
    KeyCheck,
    /// An envelope names another recipient, or another X-Wing key than the
    /// recipient's card.
    #[error("envelope recipient mismatch")]
    Recipient,
}

impl KeyError {
    /// The client's report.
    pub fn client_code(&self) -> &'static str {
        match self {
            KeyError::PolicyHead => "sync_policy_forked",
            _ => "sync_key_mismatch",
        }
    }
}

/// Opens an epoch record's lineage with the record's own key, returning
/// the previous epoch's key. It authenticates that key; the caller still
/// checks it against the previous record's `key_check`.
pub fn open_lineage(epoch: &Epoch, key: &SecretKey) -> Result<SecretKey, KeyError> {
    if epoch.lineage.0.len() != LINEAGE_LEN {
        return Err(KeyError::Lineage);
    }
    let bytes = open(
        &lineage_key(key),
        &epoch.lineage_nonce,
        &epoch.aad(),
        &epoch.lineage.0,
    )
    .map_err(|_| KeyError::Lineage)?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| KeyError::Lineage)?;
    Ok(SecretKey(bytes))
}

/// Walks lineage back from `key`, the key of `records[0]`, through each
/// record's lineage, checking every key it derives against its own record's
/// `key_check`. `records` runs newest first, each the record the one before
/// names in `previous`. Returns the key of each record, newest first.
pub fn walk_lineage(
    records: &[Signed<Epoch>],
    key: &SecretKey,
) -> Result<Vec<SecretKey>, KeyError> {
    let mut keys = Vec::with_capacity(records.len());
    let mut current = key.clone();
    for (i, record) in records.iter().enumerate() {
        if !key_matches(&record.body, &current) {
            return Err(KeyError::KeyCheck);
        }
        keys.push(current.clone());
        if let Some(older) = records.get(i + 1) {
            if record.body.previous != older.digest() {
                return Err(KeyError::Chain);
            }
            current = open_lineage(&record.body, &current)?;
        }
    }
    Ok(keys)
}

/// An epoch record a client accepted.
#[derive(Clone, Debug)]
pub struct AcceptedEpoch {
    /// The record.
    pub record: Signed<Epoch>,
    /// Its digest.
    pub digest: Digest,
}

/// How an epoch record entered a client's chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpochAcceptance {
    /// The next epoch after the latest.
    Next,
    /// A replacement whose `previous` names an earlier accepted record; the
    /// listed epochs end.
    Replacement {
        /// The epoch numbers it skips.
        skipped: std::ops::RangeInclusive<u64>,
    },
}

/// A client's accepted epoch records of one domain.
#[derive(Clone, Debug)]
pub struct EpochChain {
    workspace: Id,
    domain: Id,
    accepted: BTreeMap<u64, AcceptedEpoch>,
    /// Epoch numbers a replacement ended, with the replacement's policy
    /// revision.
    ended: BTreeMap<u64, u64>,
}

impl EpochChain {
    /// An empty chain for `domain`.
    pub fn new(workspace: Id, domain: Id) -> Self {
        Self {
            workspace,
            domain,
            accepted: BTreeMap::new(),
            ended: BTreeMap::new(),
        }
    }

    /// The latest accepted record.
    pub fn latest(&self) -> Option<&AcceptedEpoch> {
        self.accepted.values().next_back()
    }

    /// An accepted record by epoch number.
    pub fn get(&self, epoch: u64) -> Option<&AcceptedEpoch> {
        self.accepted.get(&epoch)
    }

    /// Whether a replacement ended `epoch`, and at which policy revision.
    pub fn ended_at(&self, epoch: u64) -> Option<u64> {
        self.ended.get(&epoch).copied()
    }

    /// The oldest accepted record.
    pub fn oldest(&self) -> Option<&AcceptedEpoch> {
        self.accepted.values().next()
    }

    /// The checks of §9 that an entry of an epoch a replacement ended can
    /// pass without its key: domain, policy position, signer and recipients.
    /// The device never accepted that record, and its log entry alone names
    /// the rotator whose records under it count as unreadable (§13).
    pub fn check_ended_entry(
        &self,
        record: &Signed<Epoch>,
        policy: &PolicyChain,
    ) -> Result<(), KeyError> {
        self.check_signed(record, policy)
    }

    /// The checks of §9 on one record that don't depend on the records
    /// before it: domain, policy position, signer, recipients and key.
    fn check_record(
        &self,
        record: &Signed<Epoch>,
        key: &SecretKey,
        policy: &PolicyChain,
    ) -> Result<(), KeyError> {
        self.check_signed(record, policy)?;
        if !key_matches(&record.body, key) {
            return Err(KeyError::KeyCheck);
        }
        Ok(())
    }

    fn check_signed(&self, record: &Signed<Epoch>, policy: &PolicyChain) -> Result<(), KeyError> {
        let body = &record.body;
        if body.workspace != self.workspace || body.domain != self.domain {
            return Err(KeyError::Domain);
        }
        let snapshot = policy
            .at(body.policy_revision)
            .ok_or(KeyError::PolicyRevision)?;
        if snapshot.digest != body.policy_head {
            return Err(KeyError::PolicyHead);
        }
        let card = snapshot
            .devices
            .get(&body.rotating_device)
            .ok_or(KeyError::Signer)?;
        record
            .verify(&card.card.signing_public)
            .map_err(|_| KeyError::Signature)?;
        if !snapshot.can_wrap(&body.rotating_device, &self.domain) {
            return Err(KeyError::Signer);
        }
        if body.recipients_digest != recipients_digest(&snapshot.readers(&self.domain)) {
            return Err(KeyError::Recipients);
        }
        Ok(())
    }

    /// Starts an empty chain at `record`, the domain's current epoch
    /// record from `keys/current`, as a new device does (§18, step 1):
    /// every check of [`EpochChain::accept`] except the link to the
    /// records before it, which [`EpochChain::accept_older`] checks once an
    /// operation names an older epoch. Epoch 1 gets the full check.
    pub fn start_at(
        &mut self,
        record: &Signed<Epoch>,
        key: &SecretKey,
        policy: &PolicyChain,
    ) -> Result<(), KeyError> {
        if !self.accepted.is_empty() {
            return Err(KeyError::Chain);
        }
        if record.body.epoch == 1 {
            return self.accept(record, key, policy).map(|_| ());
        }
        if record.body.previous.is_zero() {
            return Err(KeyError::Chain);
        }
        self.check_record(record, key, policy)?;
        self.accepted.insert(
            record.body.epoch,
            AcceptedEpoch {
                record: record.clone(),
                digest: record.digest(),
            },
        );
        Ok(())
    }

    /// Accepts `record`, the one that the oldest accepted record's
    /// `previous` names, under `key`, which the oldest record's lineage
    /// opened (§9, §18). The epochs between the two, which a replacement
    /// skipped, end at the newer record's policy revision.
    pub fn accept_older(
        &mut self,
        record: &Signed<Epoch>,
        key: &SecretKey,
        policy: &PolicyChain,
    ) -> Result<(), KeyError> {
        let newer = self.oldest().ok_or(KeyError::Chain)?.record.body.clone();
        let body = &record.body;
        if newer.previous != record.digest() || body.epoch >= newer.epoch {
            return Err(KeyError::Chain);
        }
        if newer.policy_revision < body.policy_revision {
            return Err(KeyError::PolicyRevision);
        }
        self.check_record(record, key, policy)?;
        if body.epoch == 1 {
            if !body.previous.is_zero() {
                return Err(KeyError::Chain);
            }
            if !body.lineage.0.is_empty() || !body.lineage_nonce.is_zero() {
                return Err(KeyError::Lineage);
            }
            let snapshot = policy
                .at(body.policy_revision)
                .ok_or(KeyError::PolicyRevision)?;
            let domain = snapshot.domains.get(&self.domain).ok_or(KeyError::Domain)?;
            if domain.restricted && domain.creator != body.rotating_device {
                return Err(KeyError::FirstEpochSigner);
            }
        } else if body.previous.is_zero() {
            return Err(KeyError::Chain);
        }
        for epoch in body.epoch + 1..newer.epoch {
            self.ended.insert(epoch, newer.policy_revision);
        }
        self.accepted.insert(
            body.epoch,
            AcceptedEpoch {
                record: record.clone(),
                digest: record.digest(),
            },
        );
        Ok(())
    }

    /// Accepts `record` when it passes every check of §9, given `key`, the
    /// verified key for its epoch from an envelope or a later key's lineage.
    pub fn accept(
        &mut self,
        record: &Signed<Epoch>,
        key: &SecretKey,
        policy: &PolicyChain,
    ) -> Result<EpochAcceptance, KeyError> {
        let body = &record.body;
        if body.workspace != self.workspace || body.domain != self.domain {
            return Err(KeyError::Domain);
        }
        let snapshot = policy
            .at(body.policy_revision)
            .ok_or(KeyError::PolicyRevision)?;
        if snapshot.digest != body.policy_head {
            return Err(KeyError::PolicyHead);
        }
        let card = snapshot
            .devices
            .get(&body.rotating_device)
            .ok_or(KeyError::Signer)?;
        record
            .verify(&card.card.signing_public)
            .map_err(|_| KeyError::Signature)?;
        if !snapshot.can_wrap(&body.rotating_device, &self.domain) {
            return Err(KeyError::Signer);
        }
        if body.recipients_digest != recipients_digest(&snapshot.readers(&self.domain)) {
            return Err(KeyError::Recipients);
        }
        if !key_matches(body, key) {
            return Err(KeyError::KeyCheck);
        }

        let acceptance = match self.latest() {
            None => {
                if body.epoch != 1 || !body.previous.is_zero() {
                    return Err(KeyError::Chain);
                }
                if !body.lineage.0.is_empty() || !body.lineage_nonce.is_zero() {
                    return Err(KeyError::Lineage);
                }
                // A restricted domain's epoch 1 comes from the device that
                // signed its create_domain (§9).
                let domain = snapshot.domains.get(&self.domain).ok_or(KeyError::Domain)?;
                if domain.restricted && domain.creator != body.rotating_device {
                    return Err(KeyError::FirstEpochSigner);
                }
                EpochAcceptance::Next
            }
            Some(latest) => {
                let named = self
                    .accepted
                    .values()
                    .find(|a| a.digest == body.previous)
                    .ok_or(KeyError::Chain)?;
                let latest_epoch = latest.record.body.epoch;
                if body.epoch <= latest_epoch {
                    return Err(KeyError::Chain);
                }
                if body.policy_revision < named.record.body.policy_revision {
                    return Err(KeyError::PolicyRevision);
                }
                let previous_key = open_lineage(body, key)?;
                if !key_matches(&named.record.body, &previous_key) {
                    return Err(KeyError::Lineage);
                }
                let named_epoch = named.record.body.epoch;
                if named.digest == latest.digest && body.epoch == latest_epoch + 1 {
                    EpochAcceptance::Next
                } else {
                    EpochAcceptance::Replacement {
                        skipped: (named_epoch + 1)..=(body.epoch - 1),
                    }
                }
            }
        };
        if let EpochAcceptance::Replacement { skipped } = &acceptance {
            for epoch in skipped.clone() {
                self.accepted.remove(&epoch);
                self.ended.insert(epoch, body.policy_revision);
            }
        }
        self.accepted.insert(
            body.epoch,
            AcceptedEpoch {
                record: record.clone(),
                digest: record.digest(),
            },
        );
        Ok(acceptance)
    }
}

/// What a client does with a record under an epoch a replacement skipped
/// (§9, Replacing an epoch).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkippedEpochRecord {
    /// Count it as applied and unreadable, attributed to its signer.
    Unreadable,
    /// Report `sync_key_mismatch` and hold the domain's cursor.
    Hold,
}

/// Judges a record under skipped `epoch`, signed by `signer`, on a client
/// that never accepted that epoch. It is unreadable when the signer is the
/// epoch's rotator, or lost read access to the domain through a change at or
/// below the replacement's policy revision; otherwise an honest server would
/// have refused the replacement, so the client holds.
pub fn judge_skipped_epoch_record(
    policy: &PolicyChain,
    domain: &Id,
    signer: &Id,
    rotator: Option<&Id>,
    replacement_revision: u64,
) -> SkippedEpochRecord {
    if rotator == Some(signer) || lost_read_access(policy, domain, signer, replacement_revision) {
        SkippedEpochRecord::Unreadable
    } else {
        SkippedEpochRecord::Hold
    }
}

/// Whether `device` could read `domain` at some revision up to `through`
/// and can't at `through`.
fn lost_read_access(policy: &PolicyChain, domain: &Id, device: &Id, through: u64) -> bool {
    let reads_at = |r: u64| policy.at(r).is_some_and(|s| s.can_read(device, domain));
    !reads_at(through) && (1..through).any(reads_at)
}

/// Whether an epoch entry makes a checkpoint due at its entry (§17): a
/// replacement does, since its `previous` skips the latest epoch, which
/// the domain's latest checkpoint may name. `latest` is the digest of the
/// domain's latest epoch record before this one.
pub fn makes_checkpoint_due(latest: &Digest, record: &Epoch) -> bool {
    record.previous != *latest
}

/// The server's check of a replacement epoch (§9): `previous` names a
/// record below the latest, so it skips at least one epoch, and every
/// record that names a skipped epoch was signed by that epoch's rotator or
/// by a device that lost read access through a change at or below the
/// replacement's policy revision. `users` lists, per skipped epoch, its
/// rotator and the signers of operations, checkpoints and profiles under it.
pub fn check_replacement(
    policy: &PolicyChain,
    replacement: &Epoch,
    named_epoch: u64,
    latest_epoch: u64,
    users: &BTreeMap<u64, (Id, BTreeSet<Id>)>,
) -> Result<(), &'static str> {
    if named_epoch >= latest_epoch || replacement.epoch != latest_epoch + 1 {
        return Err("epoch_conflict");
    }
    for epoch in (named_epoch + 1)..=latest_epoch {
        let Some((rotator, signers)) = users.get(&epoch) else {
            continue;
        };
        for signer in signers {
            if signer != rotator
                && !lost_read_access(
                    policy,
                    &replacement.domain,
                    signer,
                    replacement.policy_revision,
                )
            {
                return Err("epoch_conflict");
            }
        }
    }
    Ok(())
}

/// The outcome of checking an envelope (§9, Envelopes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeVerdict {
    /// Take the key.
    Accept,
    /// The wrapper fails the accepted-head test, and the client hasn't
    /// reached the epoch's entry: wait for it.
    Wait,
}

/// What a device does when no envelope it holds for a domain's current
/// epoch passes [`check_envelope`] (§9, Re-wrapping). It keeps the first
/// envelope that passes, in key order, so a failing one ahead of a good one
/// costs nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailedEnvelopes {
    /// It accepted an earlier epoch of the domain, so it signs a
    /// replacement that names that epoch.
    Replace,
    /// It accepted no earlier epoch, such as a card brought in after the
    /// current one, so it asks for a re-wrap at once.
    Rewrap,
}

/// The step after every envelope failed, for a device that did or didn't
/// accept an earlier epoch of the domain (§9).
pub fn after_failed_envelopes(accepted_earlier_epoch: bool) -> FailedEnvelopes {
    if accepted_earlier_epoch {
        FailedEnvelopes::Replace
    } else {
        FailedEnvelopes::Rewrap
    }
}

/// Checks an envelope a recipient unwrapped. `key` is the unwrapped key,
/// `epoch` the accepted record of the envelope's epoch, `recipient` the
/// recipient's own card. The wrapping device must be a device of the
/// workspace whose card verifies the signature and that could read the
/// domain at `entry_revision`, the latest policy entry before the epoch's
/// entry once the recipient reached it, or at `accepted_head`.
pub fn check_envelope(
    envelope: &Signed<Envelope>,
    epoch: &Epoch,
    key: &SecretKey,
    recipient: &Card,
    policy: &PolicyChain,
    accepted_head: u64,
    entry_revision: Option<u64>,
) -> Result<EnvelopeVerdict, KeyError> {
    let body = &envelope.body;
    if body.workspace != epoch.workspace || body.domain != epoch.domain || body.epoch != epoch.epoch
    {
        return Err(KeyError::Domain);
    }
    if body.recipient != recipient.device || body.recipient_hpke_digest != recipient.hpke_digest() {
        return Err(KeyError::Recipient);
    }
    if !key_matches(epoch, key) {
        return Err(KeyError::KeyCheck);
    }
    let authorizes = |revision: u64| {
        policy.at(revision).and_then(|s| {
            let device = s.devices.get(&body.wrapping_device)?;
            s.can_wrap(&body.wrapping_device, &body.domain)
                .then(|| envelope.verify(&device.card.signing_public).is_ok())
        })
    };
    match authorizes(accepted_head) {
        Some(true) => return Ok(EnvelopeVerdict::Accept),
        Some(false) => return Err(KeyError::Signature),
        None => {}
    }
    match entry_revision {
        None => Ok(EnvelopeVerdict::Wait),
        Some(revision) => match authorizes(revision) {
            Some(true) => Ok(EnvelopeVerdict::Accept),
            Some(false) => Err(KeyError::Signature),
            None => Err(KeyError::Signer),
        },
    }
}

/// A record that its position in a domain's log makes invalid (§8, Policy
/// entries; §9, Epoch validity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum PositionError {
    /// The signer couldn't write the domain at the latest policy entry
    /// before the record, or at the record's `policy_revision`, or that
    /// revision lies above the entry's: `sync_writer_not_authorized`.
    #[error("writer not authorized at its position")]
    WriterNotAuthorized,
    /// The record's epoch isn't the latest epoch entry's, or a policy entry
    /// before it requires a rotation past that epoch: `sync_epoch_stale`.
    #[error("epoch stale at its position")]
    EpochStale,
    /// A policy entry arrived out of order or names a revision the client
    /// hasn't accepted yet.
    #[error("policy entry out of order")]
    PolicyEntry,
}

impl PositionError {
    /// The client's report.
    pub fn client_code(&self) -> &'static str {
        match self {
            PositionError::WriterNotAuthorized => "sync_writer_not_authorized",
            PositionError::EpochStale => "sync_epoch_stale",
            PositionError::PolicyEntry => "sync_chain_gap",
        }
    }
}

/// Walks one domain's log in sequence order and judges each record by its
/// position after the policy and epoch entries before it. Server and client
/// run the same rules. It serializes, so a client's journal can keep it
/// (§15).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogJudge {
    domain: Id,
    policy_entry: Option<u64>,
    epoch: Option<(u64, u64)>,
}

impl LogJudge {
    /// A judge at the start of `domain`'s log.
    pub fn new(domain: Id) -> Self {
        Self {
            domain,
            policy_entry: None,
            epoch: None,
        }
    }

    /// A judge after a bootstrap from a checkpoint, which stands in for the
    /// entries at or below its `covered_seq`: the latest policy entry's
    /// revision, and the epoch of the latest epoch entry with that record's
    /// `policy_revision`.
    pub fn from_checkpoint(
        domain: Id,
        policy_revision: u64,
        epoch: u64,
        epoch_policy_revision: u64,
    ) -> Self {
        Self {
            domain,
            policy_entry: Some(policy_revision),
            epoch: Some((epoch, epoch_policy_revision)),
        }
    }

    /// Takes a policy entry.
    pub fn policy_entry(
        &mut self,
        revision: u64,
        policy: &PolicyChain,
    ) -> Result<(), PositionError> {
        if self.policy_entry.is_some_and(|r| revision <= r) || policy.at(revision).is_none() {
            return Err(PositionError::PolicyEntry);
        }
        self.policy_entry = Some(revision);
        Ok(())
    }

    /// Takes an epoch entry.
    pub fn epoch_entry(&mut self, epoch: &Epoch) {
        self.epoch = Some((epoch.epoch, epoch.policy_revision));
    }

    /// The revision of the latest policy entry so far.
    pub fn latest_policy_entry(&self) -> Option<u64> {
        self.policy_entry
    }

    /// Whether a rotation of the domain is pending at this position: a
    /// policy entry above the latest epoch record's `policy_revision`
    /// requires it.
    pub fn rotation_pending(&self, policy: &PolicyChain) -> bool {
        match (self.policy_entry, self.epoch) {
            (Some(entry), Some((_, epoch_revision))) => policy
                .rotation_required(&self.domain, epoch_revision, entry)
                .is_some(),
            _ => false,
        }
    }

    /// The write-access rule for any record a device signs into the log, an
    /// operation or a checkpoint.
    pub fn check_writer(
        &self,
        device: &Id,
        policy_revision: u64,
        policy: &PolicyChain,
    ) -> Result<(), PositionError> {
        let entry = self
            .policy_entry
            .ok_or(PositionError::WriterNotAuthorized)?;
        let writes = |revision: u64| {
            policy
                .at(revision)
                .is_some_and(|s| s.can_write(device, &self.domain))
        };
        if policy_revision > entry || !writes(entry) || !writes(policy_revision) {
            return Err(PositionError::WriterNotAuthorized);
        }
        Ok(())
    }

    /// Both rules for an operation: write access, then epoch validity.
    pub fn check_operation(
        &self,
        device: &Id,
        policy_revision: u64,
        epoch: u64,
        policy: &PolicyChain,
    ) -> Result<(), PositionError> {
        self.check_writer(device, policy_revision, policy)?;
        match self.epoch {
            Some((current, _)) if current == epoch && !self.rotation_pending(policy) => Ok(()),
            _ => Err(PositionError::EpochStale),
        }
    }
}
