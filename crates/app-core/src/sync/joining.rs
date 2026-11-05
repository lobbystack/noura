//! Approving an invited member's device (docs/workspace-format/sync-v2.md
//! §10, §11).
//!
//! The invitee's device commits to a fresh nonce and its account chain
//! head, and posts a request that names the invitation and carries
//! `invite_proof`. Only the inviting device approves: it lists the request
//! through `pending-devices`, checks the proof with the secret it kept, and
//! sends one offer anchoring this workspace with its latest checkpoint of
//! the workspace domain. After both people compare the code, it pushes one
//! `add_member` recording the committed head, a current-epoch envelope for
//! each card active there, and a fresh checkpoint of the workspace domain
//! (see `WorkspaceSync::approve_member`).
//!
//! The invitee's device checks that the offer comes from the card matching
//! the invitation's `inviter_fingerprint`, and, before it trusts the
//! workspace or reads anything in it, that the `add_member` names its
//! invitation and records the head its request committed to. It then
//! records its account's head in the workspace itself, so the account's
//! other devices trust the workspace too (§8).

use std::time::{Duration, Instant};

use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::messages::{ApprovalRequest, Pending};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{
    AccountHead, Anchor, Card, Offer, PolicyKind, Role, Signed, approval_code, approval_code_text,
    approval_commit, check_offer, invite_proof,
};
use sync_protocol::types::{Digest, Id, Key};

use super::approval::{CandidateStep, OutstandingOffer};
use super::invitations::{InvitationBook, InvitationLink, OpenedInvitation};
use super::{RecordedChains, RemoteWorkspace, SyncError, SyncService};

fn mismatch() -> SyncError {
    SyncError::Check("sync_approval_mismatch")
}

fn random_key() -> Result<Key, SyncError> {
    Ok(Key(sync_protocol::crypto::random_array().map_err(|e| {
        SyncError::Invalid(format!("no randomness: {e}"))
    })?))
}

/// An invitee's device waiting for the inviting device's approval.
pub struct InvitedCandidate {
    workspace: Id,
    invitation: Id,
    inviter_fingerprint: Digest,
    head: AccountHead,
    nc: Key,
    commit: Digest,
    offer: Option<(Signed<Offer>, Digest)>,
    confirmed: bool,
    /// When the request went out.
    posted: Instant,
}

/// How long a reset request stays out before the device posts a fresh one:
/// the server drops a request after 10 minutes (§27).
pub const RESET_REQUEST_LIFE: Duration = Duration::from_secs(9 * 60);
/// How long a request with an offer gets: the offer's own 10 minutes on top
/// of the request's.
pub const OFFER_LIFE: Duration = Duration::from_secs(20 * 60);
/// How long a request for an invitation or a new device stays out before
/// the device posts a fresh one. It waits for the server to drop the last,
/// so an approver never lists two requests from one device and answers the
/// one this device stopped reading.
pub const REQUEST_LIFE: Duration = Duration::from_secs(10 * 60 + 5);

impl InvitedCandidate {
    /// Whether this request lapses at `now` with nobody having finished
    /// it, so the device posts a fresh one (§7, §10).
    pub fn lapses(&self, now: Instant) -> bool {
        let life = if self.offer.is_some() {
            OFFER_LIFE
        } else if self.invitation == RESET {
            RESET_REQUEST_LIFE
        } else {
            REQUEST_LIFE
        };
        now.saturating_duration_since(self.posted) >= life
    }

    /// The workspace the invitation is for.
    pub fn workspace(&self) -> Id {
        self.workspace
    }

    /// The request's commitment, which names it in the inviter's list.
    pub fn commit(&self) -> Digest {
        self.commit
    }

    /// The account chain head the request committed to.
    pub fn head(&self) -> &AccountHead {
        &self.head
    }

    /// The approval code once an offer arrived, as 8 digits in two groups
    /// of 4.
    pub fn code(&self) -> Option<String> {
        self.offer
            .as_ref()
            .map(|(_, digest)| approval_code_text(approval_code(digest, &self.nc)))
    }

    /// You compared the codes with the inviter and confirmed.
    pub fn confirm(&mut self) {
        self.confirmed = true;
    }
}

/// An invited account's approval request, as the inviting device lists it
/// once its proof checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitedRequest {
    /// The candidate's card.
    pub card: Card,
    /// Its fingerprint, which both screens show.
    pub fingerprint: Digest,
    /// The request's commitment.
    pub commit: Digest,
    /// The candidate's account head, at which its card is active.
    pub head: AccountHead,
    /// The invitation the request names.
    pub invitation: Id,
    /// The role the invitation grants.
    pub role: Role,
}

/// The invitation ID that marks an approval as a reset's, whose commit is
/// an `accept_reset` rather than an `add_member`.
pub const RESET: Id = Id([0; 16]);

impl InvitedCandidate {
    /// Whether you confirmed the code.
    pub fn confirmed(&self) -> bool {
        self.confirmed
    }
}

impl SyncService {
    /// Posts this device's approval request for the invitation `opened`
    /// (§10, §11): a commitment to a fresh nonce and to the head of its
    /// account chain, the head itself, the invitation and `invite_proof`
    /// over its card under the link's secret. Accept the invitation first.
    pub async fn request_invited_approval(
        &mut self,
        link: &InvitationLink,
        opened: &OpenedInvitation,
    ) -> Result<InvitedCandidate, SyncError> {
        let head = self.own_chain().await?.head();
        let card = self.credentials.card();
        let nc = random_key()?;
        let commit = approval_commit(&card.fingerprint(), Some(&head), &nc);
        self.client
            .post_approval_request(&ApprovalRequest {
                commit,
                account: Some(head.clone()),
                invitation: Some(opened.invitation),
                invite_proof: Some(invite_proof(&link.secret, &card.to_bytes())),
                workspace: None,
            })
            .await?;
        Ok(InvitedCandidate {
            workspace: opened.workspace,
            invitation: opened.invitation,
            inviter_fingerprint: opened.inviter_fingerprint,
            head,
            nc,
            commit,
            offer: None,
            confirmed: false,
            posted: Instant::now(),
        })
    }

    /// Posts the approval request of a reset account's device for
    /// `workspace` (§7, §10), which carries its new account head. It
    /// answers like an invited request, with an owner or admin as the
    /// approver.
    pub async fn request_reset_approval(
        &mut self,
        workspace: Id,
    ) -> Result<InvitedCandidate, SyncError> {
        let head = self.own_chain().await?.head();
        let card = self.credentials.card();
        let nc = random_key()?;
        let commit = approval_commit(&card.fingerprint(), Some(&head), &nc);
        self.client
            .post_approval_request(&ApprovalRequest {
                commit,
                account: Some(head.clone()),
                invitation: None,
                invite_proof: None,
                workspace: Some(workspace),
            })
            .await?;
        Ok(InvitedCandidate {
            workspace,
            invitation: RESET,
            inviter_fingerprint: Digest::ZERO,
            head,
            nc,
            commit,
            offer: None,
            confirmed: false,
            posted: Instant::now(),
        })
    }

    /// The reset requests pending in `workspace`, for an owner or admin
    /// device (§7). They answer through [`Self::offer_invited`].
    pub async fn reset_requests(
        &mut self,
        workspace: Id,
    ) -> Result<Vec<InvitedRequest>, SyncError> {
        let pending = self.client.workspace_pending_devices(workspace).await?;
        Ok(pending
            .pending
            .0
            .into_iter()
            .filter_map(|entry| reset_request(entry, workspace))
            .collect())
    }

    /// Looks for offers that name `candidate`'s commitment. The first one
    /// gets the nonce once it checks out: signed by its `approver_card`,
    /// naming this card, commitment and head, from the card whose
    /// fingerprint the invitation names, and anchoring the invitation's
    /// workspace alone. Any other offer, or a second one before you
    /// confirm, ends the attempt with `sync_approval_mismatch`.
    pub async fn poll_invited(
        &mut self,
        candidate: &mut InvitedCandidate,
    ) -> Result<CandidateStep, SyncError> {
        let offers: Vec<Signed<Offer>> = self
            .client
            .offers()
            .await?
            .offers
            .0
            .into_iter()
            .map(|o| o.0)
            .filter(|o| o.body.candidate_commit == candidate.commit)
            .collect();
        match &candidate.offer {
            None => match offers.as_slice() {
                [] => Ok(CandidateStep::Waiting),
                [offer] => {
                    let body = &offer.body;
                    offer
                        .verify(&body.approver_card.0.signing_public)
                        .map_err(|_| mismatch())?;
                    check_offer(
                        body,
                        &self.credentials.card(),
                        Some(&candidate.head),
                        &candidate.nc,
                    )
                    .map_err(|_| mismatch())?;
                    // A reset takes any owner or admin; the code you
                    // compare and the commit's check decide.
                    let from_inviter = candidate.invitation == RESET
                        || body.approver_card.0.fingerprint() == candidate.inviter_fingerprint;
                    let anchors_one = body.anchors.len() == 1
                        && body
                            .anchors
                            .iter()
                            .all(|anchor| anchor.workspace == candidate.workspace);
                    if !from_inviter || !anchors_one {
                        return Err(mismatch());
                    }
                    let digest = offer.digest();
                    self.client.reveal(digest, candidate.nc).await?;
                    candidate.offer = Some((offer.clone(), digest));
                    Ok(CandidateStep::Code(candidate.code().expect("an offer")))
                }
                _ => Err(mismatch()),
            },
            Some((_, digest)) => {
                let others = offers.iter().any(|o| o.digest() != *digest);
                if others && !candidate.confirmed {
                    return Err(mismatch());
                }
                Ok(CandidateStep::Code(candidate.code().expect("an offer")))
            }
        }
    }

    /// Whether the inviting device committed the approval you confirmed
    /// (§10 step 6). Until the server serves the workspace to this device,
    /// `false`. Then the device verifies the policy from genesis, checks
    /// that the change at the offer's anchor is the one the offer names,
    /// that the approver's card was an owner or admin device there, and
    /// that the first later change recording this account is an
    /// `add_member` that the approver's card signed, naming this invitation
    /// and recording the head the request committed to; anything else
    /// reports `sync_approval_mismatch`. Only then does it keep the anchor
    /// and trust the workspace: it holds no key and no ciphertext of the
    /// workspace before.
    pub async fn join_invited(&mut self, candidate: &InvitedCandidate) -> Result<bool, SyncError> {
        if !candidate.confirmed {
            return Ok(false);
        }
        let (offer, _) = candidate.offer.as_ref().ok_or_else(mismatch)?;
        let anchor = offer
            .body
            .anchors
            .iter()
            .next()
            .ok_or_else(mismatch)?
            .clone();
        let workspace = candidate.workspace;
        let changes = match self.policy_changes(workspace).await {
            Ok(changes) => changes,
            // Not a device of the workspace yet.
            Err(SyncError::Refused { code, .. }) if code == "forbidden" || code == "not_found" => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let own = self.own_chain().await?;
        let RecordedChains { chains, .. } = self.recorded_chains(&own, &changes).await?;
        let policy = PolicyChain::verify(&changes, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        if policy.workspace() != workspace {
            return Err(SyncError::Check("sync_workspace_unverified"));
        }
        if !check_add_member(
            &policy,
            &anchor,
            &offer.body,
            &candidate.head,
            candidate.invitation,
        )? {
            return Ok(false);
        }
        self.heads
            .set_anchor(workspace, anchor.revision, anchor.policy_head)?;
        self.heads.first_trusted(workspace, anchor.revision)?;
        self.heads
            .set_checkpoint_anchor(super::account_heads::KeptCheckpointAnchor {
                workspace,
                domain: anchor.checkpoint.domain,
                covered_seq: anchor.checkpoint.covered_seq,
                checkpoint: anchor.checkpoint.checkpoint,
                generation: anchor.generation,
            })?;
        // The device now verifies and keeps the policy as any workspace's.
        self.verified_policy(workspace).await?;
        Ok(true)
    }

    /// The approval requests of invited accounts in `workspace` that name
    /// an invitation this device sent and whose `invite_proof` checks out
    /// under the secret it kept (§11). A request with another proof, such
    /// as one whose card a server swapped, isn't listed.
    pub async fn invited_requests(
        &mut self,
        book: &InvitationBook,
        workspace: Id,
    ) -> Result<Vec<InvitedRequest>, SyncError> {
        let pending = self.client.workspace_pending_devices(workspace).await?;
        let mut requests = Vec::new();
        for entry in pending.pending.0 {
            let (Some(invitation), Some(head), Some(proof)) =
                (entry.invitation, entry.account, entry.invite_proof)
            else {
                continue;
            };
            let Some(own) = book
                .invitations()
                .iter()
                .find(|i| i.invitation == invitation && i.workspace == workspace)
            else {
                continue;
            };
            let Some(secret) = self.invitation_secret(book, &invitation)? else {
                continue;
            };
            let card = entry.card.0;
            if invite_proof(&secret, &card.to_bytes()) != proof || card.account != head.account {
                continue;
            }
            requests.push(InvitedRequest {
                fingerprint: card.fingerprint(),
                card,
                commit: entry.commit,
                head,
                invitation,
                role: own.role,
            });
        }
        Ok(requests)
    }

    /// Answers the invited request you selected with one signed offer
    /// (§10): the device catches up on the workspace's policy, then offers
    /// its account chain head, the candidate's head, and one anchor: its
    /// accepted policy head here and its latest accepted checkpoint of the
    /// workspace domain. It keeps this offer outstanding and refuses
    /// another until the attempt ends.
    pub async fn offer_invited(
        &mut self,
        request: &InvitedRequest,
        remote: &mut RemoteWorkspace,
    ) -> Result<OutstandingOffer, SyncError> {
        if self.outstanding.is_some() {
            return Err(SyncError::Invalid(
                "finish or cancel the approval in progress first".into(),
            ));
        }
        let own = self.own_chain().await?;
        self.refresh_policy(remote).await?;
        let (revision, policy_head) = remote.policy.head();
        let (checkpoint, generation) = self.checkpoint_anchor(remote)?;
        let keys = &self.credentials.keys;
        let offer = Signed::sign(
            Offer {
                approver_card: Nested(keys.card(self.credentials.account)),
                candidate_card_digest: request.card.fingerprint(),
                candidate_commit: request.commit,
                candidate_head: Some(request.head.clone()),
                approver_head: own.head(),
                nonce: random_key()?,
                anchors: Sorted::new(vec![Anchor {
                    workspace: remote.workspace,
                    revision,
                    policy_head,
                    generation,
                    checkpoint,
                }])
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
            invited: Some((request.invitation, request.role)),
        })
    }
}

/// The reset request in a pending entry for `workspace`: it names the
/// workspace, carries the account's new head and no invitation, and its
/// card belongs to that account (§7). The app shows the new card's
/// fingerprint in the alert that the account's devices changed.
pub fn reset_request(entry: Pending, workspace: Id) -> Option<InvitedRequest> {
    let (None, Some(head), Some(named)) = (entry.invitation, entry.account, entry.workspace) else {
        return None;
    };
    let card = entry.card.0;
    if named != workspace || card.account != head.account {
        return None;
    }
    Some(InvitedRequest {
        fingerprint: card.fingerprint(),
        card,
        commit: entry.commit,
        head,
        invitation: RESET,
        role: sync_protocol::records::Role::Viewer,
    })
}

/// Whether `policy` holds the `accept_reset` that answers `offer` for a
/// reset request at `head`, as the reset device checks it before it trusts
/// the workspace (§7, §10). `false` while no later change records the
/// account; `sync_approval_mismatch` for any change that doesn't answer it.
pub fn reset_committed(
    policy: &PolicyChain,
    offer: &Offer,
    head: &AccountHead,
) -> Result<bool, SyncError> {
    let anchor = offer.anchors.iter().next().ok_or_else(mismatch)?;
    check_add_member(policy, anchor, offer, head, RESET)
}

/// The candidate's checks on a verified policy (§8, §10): the anchor's
/// change, the approver's authority there, and the first later change
/// that records this account. `false` while no such change exists.
fn check_add_member(
    policy: &PolicyChain,
    anchor: &Anchor,
    offer: &Offer,
    head: &AccountHead,
    invitation: Id,
) -> Result<bool, SyncError> {
    let at = policy
        .at(anchor.revision)
        .ok_or(SyncError::Check("sync_policy_rollback"))?;
    if at.digest != anchor.policy_head {
        return Err(SyncError::Check("sync_policy_forked"));
    }
    let approver = &offer.approver_card.0;
    let manages = at.devices.get(&approver.device).is_some_and(|d| {
        d.card == *approver
            && !d.recovery
            && at
                .members
                .get(&d.account)
                .is_some_and(|m| matches!(m.role, Role::Owner | Role::Admin))
    });
    if !manages {
        return Err(mismatch());
    }
    let account = head.account;
    let (head_revision, _) = policy.head();
    for revision in anchor.revision + 1..=head_revision {
        let change = policy
            .change(revision)
            .ok_or(SyncError::Check("sync_policy_rollback"))?;
        let Some(recorded) = change.body.heads.iter().find(|h| h.account == account) else {
            continue;
        };
        let answers = change.body.signer == approver.device
            && recorded == head
            && match change.body.kind {
                PolicyKind::AddMember {
                    account: added,
                    invitation: named,
                    ..
                } => added == account && named == invitation,
                PolicyKind::AcceptReset { account: accepted } => {
                    accepted == account && invitation == RESET
                }
                _ => false,
            };
        return if answers { Ok(true) } else { Err(mismatch()) };
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(invitation: Id, offered: bool) -> InvitedCandidate {
        let keys = sync_key_envelope::v2::DeviceKeys::generate().unwrap();
        let head = AccountHead {
            account: Id([1; 16]),
            genesis: Digest([2; 32]),
            seqno: 1,
            head: Digest([2; 32]),
        };
        let offer = offered.then(|| {
            let body = Offer {
                approver_card: sync_protocol::codec::Nested(keys.card(Id([1; 16]))),
                candidate_card_digest: Digest::ZERO,
                candidate_commit: Digest::ZERO,
                candidate_head: None,
                approver_head: head.clone(),
                nonce: Key([0; 32]),
                anchors: sync_protocol::codec::Sorted::default(),
            };
            (Signed::sign(body, keys.signing()), Digest::ZERO)
        });
        InvitedCandidate {
            workspace: Id([3; 16]),
            invitation,
            inviter_fingerprint: Digest::ZERO,
            head,
            nc: Key([0; 32]),
            commit: Digest::ZERO,
            offer,
            confirmed: false,
            posted: Instant::now(),
        }
    }

    #[test]
    fn a_reset_request_lapses_before_the_server_drops_it() {
        let posted = candidate(RESET, false);
        let start = posted.posted;
        assert!(!posted.lapses(start + Duration::from_secs(8 * 60)));
        assert!(posted.lapses(start + RESET_REQUEST_LIFE));
        let answered = candidate(RESET, true);
        assert!(!answered.lapses(answered.posted + RESET_REQUEST_LIFE));
        assert!(answered.lapses(answered.posted + OFFER_LIFE));
        let invited = candidate(Id([9; 16]), false);
        assert!(
            !invited.lapses(invited.posted + RESET_REQUEST_LIFE),
            "an invited request waits for the server to drop it"
        );
        assert!(invited.lapses(invited.posted + REQUEST_LIFE));
    }
}
