//! Workspace creation (§26, `POST /v2/workspaces`): the genesis, the
//! workspace domain's first epoch bundle, an envelope for every active card
//! of the owner's account at the head the genesis records, and the profile,
//! in one request.

use std::collections::BTreeSet;
use std::sync::Arc;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Encode, List};
use sync_protocol::keys::recipients_digest;
use sync_protocol::messages::{PolicyEntry, PushResponse, Receipt, RecordRef};
use sync_protocol::padding::is_padded_ciphertext_length;
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Envelope, EpochBundle, PolicyChange, Profile, Role, Signed};
use sync_protocol::types::{Digest, Id};

use crate::policy::{self, PolicyCache};
use crate::push::{ENTRY_EPOCH, KEY_ENVELOPE, KEY_EPOCH, decode_records, failure};
use crate::{
    CheckpointInterval, EpochRow, LockPlan, Refusal, Result, Session, WorkspaceRow, WriteTxn,
    refuse,
};

/// The most workspaces an account owns (§27).
pub const MAX_OWNED_WORKSPACES: u64 = 100;
/// The most workspaces an account belongs to (§27).
pub const MAX_MEMBERSHIPS: usize = 1_000;

/// A decoded creation request.
pub(crate) struct Creation {
    genesis: Signed<PolicyChange>,
    epoch: EpochBundle,
    envelopes: Vec<Signed<Envelope>>,
    profile: Signed<Profile>,
}

/// Decodes a creation request and checks its layout.
pub(crate) fn decode(body: &[u8]) -> Result<Creation> {
    let records = decode_records(body)?;
    let last = records.len().saturating_sub(1);
    let layout = |index: usize| {
        failure(
            index,
            Refusal::new(
                "bad_request",
                "a creation carries the genesis, the epoch bundle, the envelopes and the profile, in that order",
            ),
        )
    };
    if records.len() < 3 {
        return Err(layout(records.len()));
    }
    let mut genesis = None;
    let mut epoch = None;
    let mut envelopes = Vec::new();
    let mut profile = None;
    for (index, record) in records.into_iter().enumerate() {
        match (index, record) {
            (0, RecordRef::Policy(change)) => genesis = Some(change),
            (1, RecordRef::EpochBundle(bundle)) => epoch = Some(bundle),
            (i, RecordRef::Profile(p)) if i == last => profile = Some(p),
            (i, RecordRef::Envelope(envelope)) if i > 1 && i < last => envelopes.push(envelope),
            (i, _) => return Err(layout(i)),
        }
    }
    Ok(Creation {
        genesis: genesis.ok_or_else(|| layout(0))?,
        epoch: epoch.ok_or_else(|| layout(1))?,
        envelopes,
        profile: profile.ok_or_else(|| layout(last))?,
    })
}

/// Checks each record's signature under its signer's card from the session
/// account's chain, and the profile's padding, before the transaction.
pub(crate) fn check_signatures(
    creation: &Creation,
    session: Session,
    chain: &AccountChain,
) -> Result<()> {
    let verify = |index: usize, signer: &Id, check: &dyn Fn(&sync_protocol::types::Key) -> bool| {
        let card = chain.card(signer).ok_or_else(|| {
            failure(
                index,
                Refusal::new("forbidden", "the signer isn't a card of this account"),
            )
        })?;
        if check(&card.card.signing_public) {
            Ok(())
        } else {
            Err(failure(
                index,
                Refusal::new(
                    "signature_invalid",
                    "a signature fails under the signer's card",
                ),
            ))
        }
    };
    if creation.genesis.body.signer != session.device {
        return Err(failure(
            0,
            Refusal::new("forbidden", "the session's device signs the genesis"),
        ));
    }
    verify(0, &creation.genesis.body.signer, &|key| {
        creation.genesis.verify(key).is_ok()
    })?;
    let epoch = &creation.epoch.epoch.0;
    verify(1, &epoch.body.rotating_device, &|key| {
        epoch.verify(key).is_ok()
    })?;
    for (i, envelope) in creation.envelopes.iter().enumerate() {
        verify(i + 2, &envelope.body.wrapping_device, &|key| {
            envelope.verify(key).is_ok()
        })?;
    }
    let last = creation.envelopes.len() + 2;
    verify(last, &creation.profile.body.signer, &|key| {
        creation.profile.verify(key).is_ok()
    })?;
    if !is_padded_ciphertext_length(creation.profile.body.ciphertext.len() as u64) {
        return Err(failure(
            last,
            Refusal::new(
                "padding_invalid",
                "the profile ciphertext isn't a padded length",
            ),
        ));
    }
    Ok(())
}

/// Validates the creation against the store and commits it. Returns the
/// encoded response, which a retry with identical bytes gets again.
pub(crate) async fn commit(
    txn: &mut impl WriteTxn,
    cache: &PolicyCache,
    interval: CheckpointInterval,
    session: Session,
    creation: Creation,
    request_digest: Digest,
) -> Result<Vec<u8>> {
    let Creation {
        genesis,
        epoch,
        envelopes,
        profile,
    } = creation;
    let workspace = genesis.body.workspace;
    let genesis_digest = genesis.digest();
    let domain = match genesis.body.kind {
        sync_protocol::records::PolicyKind::Genesis { workspace_domain } => workspace_domain,
        _ => {
            return Err(failure(
                0,
                Refusal::new("bad_request", "the first record is a genesis"),
            ));
        }
    };
    txn.lock(&LockPlan {
        workspace: Some(workspace),
        policy_exclusive: true,
        key_sequence: true,
        domains: vec![domain],
        blobs: Vec::new(),
    })
    .await?;
    if let Some(row) = txn.workspace(workspace).await? {
        return if row.creation_digest == request_digest {
            Ok(row.creation_response)
        } else {
            Err(refuse("id_conflict", "a workspace with this ID exists"))
        };
    }
    if txn.workspace_closed(workspace).await? {
        return Err(refuse("id_conflict", "a closed workspace held this ID"));
    }
    // The creator's chain row makes creations by one account count its
    // workspaces one at a time. Only a chain append, which takes no other
    // lock, also locks that row, so no lock cycle forms (§15).
    txn.lock_account(session.account).await?;
    if txn.owned_workspaces(session.account).await? >= MAX_OWNED_WORKSPACES {
        return Err(refuse(
            "workspace_limit",
            "the account already owns 100 workspaces",
        ));
    }
    if txn.memberships(session.account).await?.len() >= MAX_MEMBERSHIPS {
        return Err(refuse(
            "workspace_limit",
            "the account already belongs to 1,000 workspaces",
        ));
    }

    let chains = policy::account_chains(txn, [session.account]).await?;
    let chain = PolicyChain::from_genesis(&genesis, &chains)
        .map_err(|e| failure(0, Refusal::new(e.server_code(), e.to_string())))?;
    let snapshot = chain.head_snapshot().clone();
    let readers = snapshot.readers(&domain);

    // The epoch bundle: epoch 1 of the workspace domain at revision 1.
    let record = &epoch.epoch.0;
    let body = &record.body;
    let epoch_failure = |code: &'static str, message: &str| failure(1, Refusal::new(code, message));
    if body.workspace != workspace
        || body.domain != domain
        || body.epoch != 1
        || !body.previous.is_zero()
    {
        return Err(epoch_failure(
            "rotation_not_due",
            "a creation carries epoch 1 of the workspace domain",
        ));
    }
    if body.policy_revision != 1 {
        return Err(epoch_failure("policy_stale", "epoch 1 names revision 1"));
    }
    if body.policy_head != genesis_digest {
        return Err(epoch_failure(
            "policy_conflict",
            "epoch 1 names the genesis",
        ));
    }
    if !body.lineage.0.is_empty() || !body.lineage_nonce.is_zero() {
        return Err(epoch_failure("bad_request", "epoch 1 carries no lineage"));
    }
    if !snapshot.can_read(&body.rotating_device, &domain) {
        return Err(epoch_failure(
            "forbidden",
            "the rotating device can't read the domain",
        ));
    }
    let bundle_recipients: BTreeSet<Id> = epoch.recipients.iter().copied().collect();
    if bundle_recipients != readers || body.recipients_digest != recipients_digest(&readers) {
        return Err(epoch_failure(
            "recipients_invalid",
            "the recipients aren't the domain's readers",
        ));
    }

    // One envelope for each reader.
    let mut wrapped = BTreeSet::new();
    for (i, envelope) in envelopes.iter().enumerate() {
        let index = i + 2;
        let e = &envelope.body;
        if !snapshot.can_read(&e.wrapping_device, &domain) {
            return Err(failure(
                index,
                Refusal::new("forbidden", "the wrapping device can't read the domain"),
            ));
        }
        let card = snapshot.devices.get(&e.recipient);
        let fits = e.workspace == workspace
            && e.domain == domain
            && e.epoch == 1
            && readers.contains(&e.recipient)
            && card.is_some_and(|c| c.card.hpke_digest() == e.recipient_hpke_digest)
            && wrapped.insert(e.recipient);
        if !fits {
            return Err(failure(
                index,
                Refusal::new(
                    "recipients_invalid",
                    "an envelope names no reader of epoch 1, or one twice",
                ),
            ));
        }
    }
    if wrapped != readers {
        return Err(epoch_failure(
            "recipients_invalid",
            "a reader of epoch 1 has no envelope",
        ));
    }

    // The profile.
    let last = envelopes.len() + 2;
    let p = &profile.body;
    let profile_failure =
        |code: &'static str, message: &str| failure(last, Refusal::new(code, message));
    if p.workspace != workspace {
        return Err(profile_failure(
            "bad_request",
            "the profile names another workspace",
        ));
    }
    let signer_role = snapshot
        .account_of(&p.signer)
        .and_then(|account| snapshot.role(&account));
    if !matches!(signer_role, Some(Role::Owner | Role::Admin)) {
        return Err(profile_failure(
            "forbidden",
            "an owner or admin device signs the profile",
        ));
    }
    if p.policy_revision != 1 {
        return Err(profile_failure(
            "policy_stale",
            "the first profile names revision 1",
        ));
    }
    if p.policy_head != genesis_digest {
        return Err(profile_failure(
            "policy_conflict",
            "the first profile names the genesis",
        ));
    }
    if p.epoch != 1 {
        return Err(profile_failure(
            "epoch_stale",
            "the first profile uses epoch 1",
        ));
    }
    if p.profile_revision != 1 {
        return Err(profile_failure(
            "profile_conflict",
            "the first profile is revision 1",
        ));
    }

    // Commit: the domain's log starts with the genesis entry, then epoch 1.
    txn.insert_domain(workspace, domain).await?;
    crate::policy::store_change(txn, workspace, &genesis).await?;
    for (account, member) in &snapshot.members {
        txn.upsert_member(workspace, *account, member.role.code())
            .await?;
    }
    let entry = PolicyEntry {
        revision: 1,
        head: genesis_digest,
    };
    let genesis_seq = crate::checkpoints::append_policy_entry(
        txn, interval, workspace, domain, &entry, None, &snapshot,
    )
    .await?;
    let epoch_bytes = record.to_bytes();
    let epoch_digest = record.digest();
    let epoch_seq = txn
        .append_entry(workspace, domain, ENTRY_EPOCH, &epoch_bytes)
        .await?;
    crate::checkpoints::note_entry(
        txn,
        interval,
        workspace,
        domain,
        epoch_seq,
        epoch_bytes.len() as u64,
        false,
    )
    .await?;
    txn.insert_epoch(
        workspace,
        domain,
        &EpochRow {
            epoch: 1,
            seq: epoch_seq,
            digest: epoch_digest,
            previous: Digest::ZERO,
            record: epoch_bytes.clone(),
        },
        1,
        &epoch.to_bytes(),
    )
    .await?;
    txn.append_key_entry(workspace, KEY_EPOCH, domain, 1, None, &epoch_bytes)
        .await?;
    let usage = genesis.to_bytes().len() + epoch.to_bytes().len();
    txn.add_usage(workspace, domain, usage as i64, epoch_bytes.len() as i64)
        .await?;
    for envelope in &envelopes {
        txn.append_key_entry(
            workspace,
            KEY_ENVELOPE,
            domain,
            1,
            Some(envelope.body.recipient),
            &envelope.to_bytes(),
        )
        .await?;
    }
    txn.insert_profile(workspace, 1, &profile.to_bytes())
        .await?;
    let response = PushResponse {
        policy_revision: 1,
        receipts: List(vec![
            Receipt {
                record_digest: genesis_digest,
                domain,
                seq: genesis_seq,
            },
            Receipt {
                record_digest: epoch_digest,
                domain,
                seq: epoch_seq,
            },
        ]),
    }
    .to_bytes();
    txn.insert_workspace(
        workspace,
        &WorkspaceRow {
            owner: session.account,
            creation_digest: request_digest,
            creation_response: response.clone(),
            policy_revision: 1,
            policy_head: genesis_digest,
            quota_pressure: false,
            quota_crossings: 0,
        },
    )
    .await?;
    cache.put(Arc::new(chain));
    Ok(response)
}
