//! Your device list and its alerts (docs/workspace-format/sync-v2.md §7,
//! Device list).
//!
//! The list comes from your account chain: each active card with the
//! device that added it and how, and each removed card with the link that
//! removed it. `GET /v2/devices` adds only session counts. The device
//! alerts you to an enrolled card the chain doesn't list and that no
//! approval request names, to a workspace of your account whose log it
//! can't verify, to each link of your chain it neither signed nor took
//! part in, and to each policy change other than `account_heads` that
//! another device of your account signed after this device first trusted
//! that workspace.

use std::collections::{BTreeMap, BTreeSet};

use sync_protocol::account::CardState;
use sync_protocol::records::{Card, LinkKind, PolicyKind};
use sync_protocol::types::{Digest, Id};

use super::removal::kind_name;
use super::{RemoteWorkspace, SyncError, SyncService};

/// How a card joined the account (§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddedBy {
    /// It signed the account's genesis.
    Genesis,
    /// A device approved it (`add_device`).
    Approval(Id),
    /// A device exported a recovery kit (`add_recovery`).
    Kit(Id),
    /// A recovery identity recovered it (`recover_device`).
    Recovery(Id),
}

/// One card of your account chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    /// The card.
    pub card: Card,
    /// Its full fingerprint.
    pub fingerprint: Digest,
    /// Whether it's a recovery identity.
    pub recovery: bool,
    /// How it joined, and by which device.
    pub added: AddedBy,
    /// The link that removed it, by seqno and signer.
    pub removed: Option<(u64, Id)>,
    /// How many sessions it holds, from the server.
    pub sessions: Option<u32>,
    /// Whether it's this device.
    pub this_device: bool,
}

/// Something your device list warns you about (§7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceAlert {
    /// An enrolled card that your chain doesn't list and that no approval
    /// request names; an active device can end its sessions.
    UnknownCard {
        /// The card.
        card: Card,
        /// Its sessions.
        sessions: u32,
    },
    /// A workspace of your account whose log this device can't verify.
    UnverifiedWorkspace {
        /// The workspace.
        workspace: Id,
        /// The client error.
        code: &'static str,
    },
    /// A link of your chain that this device didn't sign and whose
    /// approval it didn't take part in.
    Link {
        /// The link's seqno.
        seqno: u64,
        /// Its kind, as §7 names it.
        kind: &'static str,
        /// The device that signed it.
        signer: Id,
        /// The card it adds or removes.
        card: Card,
        /// Whether that card is a recovery identity.
        recovery: bool,
    },
    /// A policy change other than `account_heads` that another device of
    /// your account signed after this device first trusted the workspace.
    PolicyChange {
        /// The workspace.
        workspace: Id,
        /// The change's revision.
        revision: u64,
        /// Its kind, as §8 names it.
        kind: &'static str,
        /// The device that signed it.
        signer: Id,
    },
}

/// Your device list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceList {
    /// The account's fingerprint: the digest of its chain's genesis.
    pub account_fingerprint: Digest,
    /// The active cards.
    pub active: Vec<DeviceRow>,
    /// The removed cards.
    pub removed: Vec<DeviceRow>,
    /// What the list warns you about.
    pub alerts: Vec<DeviceAlert>,
}

fn row(
    state: &CardState,
    chain_cards: &BTreeMap<Id, &CardState>,
    sessions: Option<u32>,
    this: Id,
) -> DeviceRow {
    let added = match state.parent {
        None => AddedBy::Genesis,
        Some(parent) if state.recovery => AddedBy::Kit(parent),
        Some(parent) if chain_cards.get(&parent).is_some_and(|p| p.recovery) => {
            AddedBy::Recovery(parent)
        }
        Some(parent) => AddedBy::Approval(parent),
    };
    DeviceRow {
        card: state.card.clone(),
        fingerprint: state.card.fingerprint(),
        recovery: state.recovery,
        added,
        removed: None,
        sessions,
        this_device: state.card.device == this,
    }
}

impl SyncService {
    /// Builds your device list from your account chain, with the session
    /// counts the server reports and the alerts of §7, judging the policy
    /// changes of `workspaces`.
    pub async fn device_list(
        &mut self,
        workspaces: &[RemoteWorkspace],
    ) -> Result<DeviceList, SyncError> {
        let chain = self.own_chain().await?;
        let this = self.credentials.keys.device();
        let (links, _) = self
            .client
            .whole_chain(chain.account(), Some(chain.genesis()))
            .await?;
        let enrolled = self.client.devices().await?;
        let sessions: BTreeMap<Id, u32> = enrolled
            .devices
            .iter()
            .map(|d| (d.card.0.device, d.sessions))
            .collect();
        let cards: BTreeMap<Id, &CardState> = chain.cards().map(|s| (s.card.device, s)).collect();
        let head = chain.head_seqno();
        let mut active = Vec::new();
        let mut removed = Vec::new();
        for state in chain.cards() {
            let mut entry = row(
                state,
                &cards,
                sessions.get(&state.card.device).copied(),
                this,
            );
            match state.removed_at {
                Some(seqno) if seqno <= head => {
                    let signer = links
                        .get(seqno as usize - 1)
                        .map_or(Id([0; 16]), |l| l.body.signer);
                    entry.removed = Some((seqno, signer));
                    removed.push(entry);
                }
                _ => active.push(entry),
            }
        }
        active.sort_by_key(|r| (!r.this_device, r.recovery, r.card.device));

        let mut alerts = Vec::new();
        // Enrolled cards no chain lists, that no approval request names.
        let requested: BTreeSet<Id> = match self.client.approval_requests().await {
            Ok(pending) => pending.pending.iter().map(|p| p.card.0.device).collect(),
            Err(_) => BTreeSet::new(),
        };
        // The device judges each listed card against its chain, whatever
        // state the server reports: a card the chain doesn't hold byte for
        // byte, such as one that reuses a listed device ID under other keys,
        // alerts. A card without sessions alerts too, so a server can't hide
        // a card it planted by reporting none; only ending its sessions
        // waits for one.
        for device in enrolled.devices.iter() {
            let listed = chain
                .card(&device.card.0.device)
                .is_some_and(|state| state.card == device.card.0);
            if !listed && !requested.contains(&device.card.0.device) {
                alerts.push(DeviceAlert::UnknownCard {
                    card: device.card.0.clone(),
                    sessions: device.sessions,
                });
            }
        }
        // Links this device didn't sign or take part in, after it joined.
        let reference = self.credentials.reference_head.seqno;
        for link in links.iter().filter(|l| l.body.seqno > reference) {
            let body = &link.body;
            if body.signer == this || self.heads.witnessed(&link.digest()) {
                continue;
            }
            let (kind, device) = match &body.kind {
                LinkKind::Genesis { card } => ("genesis", card.device),
                LinkKind::AddDevice { card, .. } => ("add_device", card.device),
                LinkKind::AddRecovery { card } => ("add_recovery", card.device),
                LinkKind::RecoverDevice { card } => ("recover_device", card.device),
                LinkKind::Remove { device, .. } => ("remove", *device),
            };
            if let Some(state) = cards.get(&device) {
                alerts.push(DeviceAlert::Link {
                    seqno: body.seqno,
                    kind,
                    signer: body.signer,
                    card: state.card.clone(),
                    recovery: state.recovery,
                });
            }
        }
        // Workspaces of your account whose log doesn't verify.
        let opened: BTreeSet<Id> = workspaces.iter().map(|w| w.workspace).collect();
        let listed: Vec<Id> = self
            .client
            .workspaces()
            .await?
            .workspaces
            .iter()
            .map(|w| w.workspace)
            .collect();
        for workspace in listed {
            if opened.contains(&workspace) {
                continue;
            }
            if let Err(SyncError::Check(code)) = self.verified_policy(workspace).await {
                alerts.push(DeviceAlert::UnverifiedWorkspace { workspace, code });
            }
        }
        // Policy changes another device of your account signed.
        for remote in workspaces {
            let after = self.heads.trusted_after(&remote.workspace).unwrap_or(0);
            for revision in (after + 1)..=remote.policy.head().0 {
                let Some(change) = remote.policy.change(revision) else {
                    continue;
                };
                let body = &change.body;
                let ours = cards.contains_key(&body.signer);
                if !ours || body.signer == this || matches!(body.kind, PolicyKind::AccountHeads) {
                    continue;
                }
                alerts.push(DeviceAlert::PolicyChange {
                    workspace: remote.workspace,
                    revision,
                    kind: kind_name(&body.kind),
                    signer: body.signer,
                });
            }
        }
        Ok(DeviceList {
            account_fingerprint: chain.genesis(),
            active,
            removed,
            alerts,
        })
    }

    /// Ends the sessions of an enrolled card that your chain doesn't list
    /// (§7), as the device list's alert offers.
    pub async fn end_unknown_sessions(&mut self, device: Id) -> Result<(), SyncError> {
        self.client.end_device(device).await
    }
}
