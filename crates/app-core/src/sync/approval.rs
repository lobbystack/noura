//! Approving a new device of your account (docs/workspace-format/sync-v2.md
//! §10): the candidate commits to a secret nonce, a device you hold sends
//! one signed offer naming its account chain head and an anchor per
//! workspace, the candidate reveals the nonce for that offer alone, and
//! both screens show one 8-digit code for you to compare and confirm. The
//! approving device then appends `add_device` right after its head and
//! records the new head in every workspace, with the new card's envelopes.
//!
//! A second offer for one commitment before you confirm ends the attempt
//! with `sync_approval_mismatch` and shows no code, and an approving device
//! keeps one offer outstanding.

use std::collections::BTreeSet;

use local_core::sync::SyncCredentials;
use sync_key_envelope::v2::{DeviceKeys, next_link};
use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Nested, Sorted};
use sync_protocol::messages::ApprovalRequest;
use sync_protocol::records::{
    AccountHead, Anchor, Card, CheckpointAnchor, LinkKind, Offer, Signed, approval_code,
    approval_code_text, approval_commit, check_offer,
};
use sync_protocol::types::{Digest, Id, Key};

use super::device::StoredDevice;
use super::{DeviceCredentials, KeptHeads, RemoteWorkspace, SyncClient, SyncError, SyncService};

fn mismatch() -> SyncError {
    SyncError::Check("sync_approval_mismatch")
}

/// A new device waiting for approval: its keys, its session and the
/// commitment of this attempt's nonce.
pub struct Candidate {
    client: SyncClient,
    keys: DeviceKeys,
    account: Id,
    nc: Key,
    commit: Digest,
    offer: Option<(Signed<Offer>, Digest)>,
    confirmed: bool,
}

/// Where a candidate's attempt stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateStep {
    /// No offer yet.
    Waiting,
    /// The approval code, as 8 digits in two groups of 4, for you to
    /// compare with the approving device's.
    Code(String),
}

impl Candidate {
    /// Posts an approval request for the account's own new device (§10),
    /// committing to a fresh nonce, with no account head, since the device
    /// joins your chain.
    pub async fn request(
        mut client: SyncClient,
        keys: DeviceKeys,
        account: Id,
    ) -> Result<Self, SyncError> {
        let nc = Key(sync_protocol::crypto::random_array()
            .map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))?);
        let card = keys.card(account);
        let commit = approval_commit(&card.fingerprint(), None, &nc);
        client
            .post_approval_request(&ApprovalRequest {
                commit,
                account: None,
                invitation: None,
                invite_proof: None,
                workspace: None,
            })
            .await?;
        Ok(Self {
            client,
            keys,
            account,
            nc,
            commit,
            offer: None,
            confirmed: false,
        })
    }

    /// This device's card.
    pub fn card(&self) -> Card {
        self.keys.card(self.account)
    }

    /// This device's fingerprint, which its screen shows (§7).
    pub fn fingerprint(&self) -> Digest {
        self.card().fingerprint()
    }

    /// Looks for offers that name this attempt's commitment. The first one
    /// that names this card, commitment and head gets the nonce, and both
    /// devices show the code; a second offer before you confirm ends the
    /// attempt with `sync_approval_mismatch`, as does an offer that names
    /// another card or commitment.
    pub async fn poll(&mut self) -> Result<CandidateStep, SyncError> {
        let offers: Vec<Signed<Offer>> = self
            .client
            .offers()
            .await?
            .offers
            .0
            .into_iter()
            .map(|o| o.0)
            .filter(|o| o.body.candidate_commit == self.commit)
            .collect();
        let card = self.card();
        match &self.offer {
            None => match offers.as_slice() {
                [] => Ok(CandidateStep::Waiting),
                [offer] => {
                    // An offer the approver's card didn't sign as served,
                    // such as one whose head a server swapped, gets no
                    // nonce.
                    offer
                        .verify(&offer.body.approver_card.0.signing_public)
                        .map_err(|_| mismatch())?;
                    check_offer(&offer.body, &card, None, &self.nc).map_err(|_| mismatch())?;
                    let digest = offer.digest();
                    self.client.reveal(digest, self.nc).await?;
                    self.offer = Some((offer.clone(), digest));
                    Ok(CandidateStep::Code(self.code().expect("an offer")))
                }
                _ => Err(mismatch()),
            },
            Some((_, digest)) => {
                let others = offers.iter().any(|o| o.digest() != *digest);
                if others && !self.confirmed {
                    return Err(mismatch());
                }
                Ok(CandidateStep::Code(self.code().expect("an offer")))
            }
        }
    }

    /// The approval code once an offer arrived.
    pub fn code(&self) -> Option<String> {
        self.offer
            .as_ref()
            .map(|(_, digest)| approval_code_text(approval_code(digest, &self.nc)))
    }

    /// You compared the codes and confirmed on this device.
    pub fn confirm(&mut self) {
        self.confirmed = true;
    }

    /// Whether the approving device appended `add_device` for this card
    /// right after the offer's `approver_head`, signed by the approver's
    /// card, naming the offer (§10). Once it did, the device keeps that
    /// head as the one it verified its chain from, keeps the offer's
    /// anchors, and stores its credentials.
    pub async fn joined(
        &mut self,
        heads: &mut KeptHeads,
        store: &dyn SyncCredentials,
    ) -> Result<Option<DeviceCredentials>, SyncError> {
        if !self.confirmed {
            return Ok(None);
        }
        let (offer, digest) = self.offer.clone().ok_or_else(mismatch)?;
        let approver_head = offer.body.approver_head.clone();
        let (links, genesis) = self
            .client
            .whole_chain(self.account, Some(approver_head.genesis))
            .await?;
        let chain = AccountChain::verify(&links, &BTreeSet::new())
            .map_err(|e| SyncError::Check(e.client_code()))?;
        if chain.genesis() != genesis || !chain.contains(&approver_head) {
            return Err(SyncError::Check("sync_account_forked"));
        }
        if chain.head_seqno() <= approver_head.seqno {
            return Ok(None);
        }
        let approver = &offer.body.approver_card.0;
        let added = &links[approver_head.seqno as usize];
        let approved = added.body.signer == approver.device
            && chain.card(&approver.device).is_some_and(|state| {
                state.card == *approver && state.active_at(approver_head.seqno)
            })
            && matches!(&added.body.kind, LinkKind::AddDevice { card, offer }
                if card.0 == self.card() && *offer == digest);
        if !approved {
            return Err(mismatch());
        }
        heads.enter(&chain, super::Anchor::Own(&approver_head), store)?;
        heads.witness(added.digest())?;
        for anchor in offer.body.anchors.iter() {
            heads.set_anchor(anchor.workspace, anchor.revision, anchor.policy_head)?;
            heads.first_trusted(anchor.workspace, anchor.revision)?;
            heads.set_checkpoint_anchor(super::account_heads::KeptCheckpointAnchor {
                workspace: anchor.workspace,
                domain: anchor.checkpoint.domain,
                covered_seq: anchor.checkpoint.covered_seq,
                checkpoint: anchor.checkpoint.checkpoint,
                generation: anchor.generation,
            })?;
        }
        let server = self.client.base().to_owned();
        let mut stored = StoredDevice::load(store, &server)?
            .ok_or_else(|| SyncError::Invalid("the credential store lost the device".into()))?;
        stored.reference_head = Some(approver_head);
        stored.save(store)?;
        stored.credentials()
    }
}

/// An approval request of the account's own new device, as an approving
/// device lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDevice {
    /// The candidate's card.
    pub card: Card,
    /// Its fingerprint, which both screens show.
    pub fingerprint: Digest,
    /// The request's commitment.
    pub commit: Digest,
}

/// The one offer an approving device keeps outstanding (§10).
pub struct OutstandingOffer {
    offer: Signed<Offer>,
    digest: Digest,
    card: Card,
    nc: Option<Key>,
}

impl OutstandingOffer {
    /// The approval code once the candidate revealed its nonce.
    pub fn code(&self) -> Option<String> {
        self.nc
            .as_ref()
            .map(|nc| approval_code_text(approval_code(&self.digest, nc)))
    }

    /// The candidate's fingerprint.
    pub fn fingerprint(&self) -> Digest {
        self.card.fingerprint()
    }
}

impl SyncService {
    /// The approval requests of the account's own new devices.
    pub async fn pending_devices(&mut self) -> Result<Vec<PendingDevice>, SyncError> {
        let own = self.own_chain().await?;
        Ok(self
            .client
            .approval_requests()
            .await?
            .pending
            .0
            .into_iter()
            .filter(|p| p.card.0.account == own.account() && own.card(&p.card.0.device).is_none())
            .map(|p| PendingDevice {
                fingerprint: p.card.0.fingerprint(),
                card: p.card.0,
                commit: p.commit,
            })
            .collect())
    }

    /// Answers the request you selected with one signed offer (§10): the
    /// device catches up on each workspace of the account first, then
    /// offers its account chain head and an anchor per workspace. It keeps
    /// this offer outstanding and refuses another until the attempt ends.
    pub async fn offer_approval(
        &mut self,
        request: &PendingDevice,
        workspaces: &mut [RemoteWorkspace],
    ) -> Result<OutstandingOffer, SyncError> {
        if self.outstanding.is_some() {
            return Err(SyncError::Invalid(
                "finish or cancel the approval in progress first".into(),
            ));
        }
        let own = self.own_chain().await?;
        let mut anchors = Vec::with_capacity(workspaces.len());
        for remote in workspaces.iter_mut() {
            self.refresh_policy(remote).await?;
            let (revision, policy_head) = remote.policy.head();
            // The approver's latest accepted checkpoint of the workspace
            // domain, with its restore generation; before the first, a
            // covered_seq of 0 and 32 zero bytes (§10).
            let generation = self.client.restore_generation();
            let (checkpoint, generation) = remote.checkpoint.clone().unwrap_or((
                CheckpointAnchor {
                    domain: remote.domain,
                    covered_seq: 0,
                    checkpoint: Digest::ZERO,
                },
                generation,
            ));
            anchors.push(Anchor {
                workspace: remote.workspace,
                revision,
                policy_head,
                generation,
                checkpoint,
            });
        }
        let keys = &self.credentials.keys;
        let offer = Signed::sign(
            Offer {
                approver_card: Nested(keys.card(self.credentials.account)),
                candidate_card_digest: request.card.fingerprint(),
                candidate_commit: request.commit,
                candidate_head: None,
                approver_head: own.head(),
                nonce: Key(sync_protocol::crypto::random_array()
                    .map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))?),
                anchors: Sorted::new(anchors)
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            },
            keys.signing(),
        );
        self.client.post_offer(&offer).await?;
        let digest = offer.digest();
        self.outstanding = Some(digest);
        Ok(OutstandingOffer {
            offer,
            digest,
            card: request.card.clone(),
            nc: None,
        })
    }

    /// Reads the nonce the candidate revealed for `offer`, checks it
    /// against the request's commitment, and returns the code; `None`
    /// while the candidate revealed nothing.
    pub async fn approval_code(
        &mut self,
        offer: &mut OutstandingOffer,
    ) -> Result<Option<String>, SyncError> {
        if offer.nc.is_none() {
            let revealed = match self.client.read_reveal(offer.digest).await {
                Ok(revealed) => revealed,
                // An expired or unknown offer ends the attempt, so the
                // device takes another request; a server it can't reach
                // leaves the offer for the next poll.
                Err(error @ SyncError::Refused { .. }) => {
                    self.outstanding = None;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let Some(nc) = revealed else {
                return Ok(None);
            };
            let commit = approval_commit(&offer.card.fingerprint(), None, &nc);
            if commit != offer.offer.body.candidate_commit {
                self.outstanding = None;
                return Err(mismatch());
            }
            offer.nc = Some(nc);
        }
        Ok(offer.code())
    }

    /// Ends the attempt without approving: the device discards the offer.
    pub fn cancel_approval(&mut self, offer: OutstandingOffer) {
        let _ = offer;
        self.outstanding = None;
    }

    /// Commits the approval you confirmed (§10): appends `add_device`, with
    /// the offer's digest, right after the offer's `approver_head`, then
    /// records the new head in each workspace with the new card's
    /// current-epoch envelopes. A chain that moved past that head refuses
    /// the link with `account_conflict`, and the attempt ends.
    pub async fn confirm_approval(
        &mut self,
        mut offer: OutstandingOffer,
        workspaces: &mut [RemoteWorkspace],
    ) -> Result<AccountHead, SyncError> {
        // Confirming spends the offer whatever happens next, so the device
        // takes another request.
        let code = self.approval_code(&mut offer).await;
        self.outstanding = None;
        if code?.is_none() {
            return Err(SyncError::Invalid(
                "the new device hasn't shown its code yet".into(),
            ));
        }
        let link = next_link(
            &self.credentials.keys,
            &offer.offer.body.approver_head,
            LinkKind::AddDevice {
                card: Nested(offer.card.clone()),
                offer: offer.digest,
            },
        );
        let digest = link.digest();
        let chain = self.append_links(vec![link]).await?;
        self.heads.witness(digest)?;
        for remote in workspaces.iter_mut() {
            self.record_heads(remote, &[]).await?;
        }
        Ok(chain.head())
    }
}
