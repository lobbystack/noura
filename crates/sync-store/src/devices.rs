//! Devices on the server (§7, §10): the device authorization grant that
//! signs a desktop in, enrollment, the enrolled cards and their sessions,
//! and approval requests, offers and reveals.
//!
//! A device grant follows RFC 8628: the device code is 32 random bytes in
//! base64url, the user code 8 characters from `BCDFGHJKLMNPQRSTVWXZ` shown
//! as two groups of 4, both expire after 10 minutes, and the device polls
//! every 5 seconds, 5 more after each `slow_down`. The store keeps both
//! codes only as BLAKE3 hashes, and deletes a grant when it's used or 10
//! minutes after it issued it. A grant that a signed-in person approved
//! starts a session of their account whose card is still unknown; the
//! device then enrolls, which binds the session to its card.

use std::collections::BTreeMap;

use sync_key_envelope::v2::{EnrollmentContext, verify_enrollment};
use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Decode, Encode, List, Nested, Text};
use sync_protocol::crypto::{hash, random_array};
use sync_protocol::messages::{
    ApprovalRequest, DeviceAuthorizeResponse, DeviceInfo, DeviceState, DeviceTokenResponse,
    DevicesResponse, OffersResponse, Pending, PendingResponse, RevealBody,
};
use sync_protocol::records::{Card, Enroll, Offer, Signed, approval_commit};
use sync_protocol::types::{Digest, Id, Key, from_base64url, to_base64url};

use crate::chains::SIGN_IN_TTL;
use crate::policy::{self, PolicyCache};
use crate::{
    ApprovalRequestRow, Backend, DeviceGrantRow, Error, OfferRow, ReadTxn, Refusal, Result,
    Session, Store, WriteTxn, refuse,
};

/// The user code's alphabet: 20 consonants, no vowels and no letter that
/// reads like a digit (§7).
pub const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";

/// The polling interval a grant starts with, and what each `slow_down`
/// adds, in seconds (§7).
pub const POLL_INTERVAL: u64 = 5;

/// The most approval requests an account holds at once (§27).
pub const MAX_PENDING_REQUESTS: usize = 32;

/// The zero ID, which a session holds as its device until the device
/// enrolls (§5).
pub const UNBOUND: Id = Id([0; 16]);

/// A user code as the device page takes it: 8 characters of the alphabet,
/// in any case, with spaces and hyphens ignored, or none.
pub fn normalize_user_code(text: &str) -> Option<String> {
    let code: String = text
        .chars()
        .filter(|c| *c != ' ' && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.len() == 8 && code.bytes().all(|b| USER_CODE_ALPHABET.contains(&b))).then_some(code)
}

/// A user code as both screens show it: two groups of 4.
pub fn user_code_text(code: &str) -> String {
    match code.get(..4).zip(code.get(4..)) {
        Some((first, second)) => format!("{first}-{second}"),
        None => code.to_owned(),
    }
}

fn user_code_hash(code: &str) -> Digest {
    hash(code.as_bytes())
}

/// 8 characters drawn uniformly from the alphabet.
fn new_user_code() -> Result<String> {
    let mut code = String::with_capacity(8);
    while code.len() < 8 {
        let bytes: [u8; 16] = random_array().map_err(Error::internal)?;
        for byte in bytes {
            // 240 is the largest multiple of 20 below 256, so every letter
            // is equally likely.
            if byte < 240 && code.len() < 8 {
                code.push(char::from(USER_CODE_ALPHABET[usize::from(byte % 20)]));
            }
        }
    }
    Ok(code)
}

fn grant_refusal(code: &'static str, message: &str) -> Error {
    Error::Refused(Refusal::new(code, message))
}

/// The card `session` enrolled, or `forbidden` for a session whose device
/// hasn't enrolled.
async fn session_card(txn: &mut impl ReadTxn, session: Session) -> Result<Card> {
    match txn.enrolled_card(session.device).await? {
        Some((account, bytes)) if account == session.account && session.device != UNBOUND => {
            Card::from_bytes(&bytes).map_err(Error::internal)
        }
        _ => Err(refuse("forbidden", "enroll this device first")),
    }
}

/// Whether `session`'s card is active at the head of its account's current
/// chain: a device that may approve, remove or end sessions.
async fn active_chain(txn: &mut impl ReadTxn, session: Session) -> Result<AccountChain> {
    policy::current_chain(txn, session.account)
        .await?
        .filter(|chain| chain.is_active_at(&session.device, chain.head_seqno()))
        .ok_or_else(|| {
            refuse(
                "forbidden",
                "only an active device of the account may do this",
            )
        })
}

impl<B: Backend> Store<B> {
    /// `POST /v2/device/authorize`: a new grant whose codes the person
    /// enters at `verification_uri`, the server's device page.
    pub async fn authorize_device(
        &self,
        verification_uri: &str,
    ) -> Result<DeviceAuthorizeResponse> {
        let device_code: [u8; 32] = random_array().map_err(Error::internal)?;
        let now = self.clock().now();
        let code_hash = hash(&device_code);
        let user_code = self
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    // A user code another grant holds takes another draw.
                    let mut user_code = new_user_code()?;
                    while txn
                        .device_grant_by_user_code(user_code_hash(&user_code))
                        .await?
                        .is_some()
                    {
                        user_code = new_user_code()?;
                    }
                    txn.insert_device_grant(&DeviceGrantRow {
                        device_code_hash: code_hash,
                        user_code_hash: user_code_hash(&user_code),
                        account: None,
                        denied: false,
                        interval: POLL_INTERVAL,
                        next_poll: now,
                        expires_at: now + SIGN_IN_TTL,
                    })
                    .await?;
                    Ok(user_code)
                })
            }))
            .await?;
        let text = |s: String| Text::new(s).map_err(|e| Error::internal(e.name()));
        Ok(DeviceAuthorizeResponse {
            device_code: text(to_base64url(&device_code))?,
            user_code: text(user_code_text(&user_code))?,
            verification_uri: text(verification_uri.to_owned())?,
            expires_in: SIGN_IN_TTL as u32,
            interval: POLL_INTERVAL as u32,
        })
    }

    /// Whether `user_code` names a grant that waits for a decision, for the
    /// device page.
    pub async fn device_grant_waits(&self, user_code: &str) -> Result<bool> {
        let Some(code) = normalize_user_code(user_code) else {
            return Ok(false);
        };
        let now = self.clock().now();
        let grant = self
            .backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.device_grant_by_user_code(user_code_hash(&code)).await })
            }))
            .await?;
        Ok(grant.is_some_and(|g| g.expires_at > now && g.account.is_none() && !g.denied))
    }

    /// The device page's decision on the grant whose user code is
    /// `user_code`: `approve` signs its device in to `account`, otherwise
    /// the device gets `access_denied`. Returns false for a code that names
    /// no grant waiting for a decision.
    pub async fn decide_device_grant(
        &self,
        account: Id,
        user_code: &str,
        approve: bool,
    ) -> Result<bool> {
        let Some(code) = normalize_user_code(user_code) else {
            return Ok(false);
        };
        let now = self.clock().now();
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let Some(mut grant) =
                        txn.device_grant_by_user_code(user_code_hash(&code)).await?
                    else {
                        return Ok(false);
                    };
                    if grant.expires_at <= now || grant.account.is_some() || grant.denied {
                        return Ok(false);
                    }
                    if approve {
                        grant.account = Some(account);
                    } else {
                        grant.denied = true;
                    }
                    txn.update_device_grant(&grant).await?;
                    Ok(true)
                })
            }))
            .await
    }

    /// `POST /v2/device/token`: the device's poll. It answers RFC 8628's
    /// `authorization_pending` until the person decides, `slow_down` to a
    /// poll that comes before the interval passed, which adds 5 seconds to
    /// it, `access_denied` once they refused and `expired_token` for a code
    /// that expired, was used or never existed. An approved grant is used
    /// once: the store deletes it and starts a session of the approving
    /// account, which holds the token only as its BLAKE3 hash.
    pub async fn poll_device_token(&self, device_code: &str) -> Result<DeviceTokenResponse> {
        let code: [u8; 32] = from_base64url(device_code).map_err(|_| {
            refuse(
                "bad_request",
                "the device code isn't 43 characters of base64url",
            )
        })?;
        let code_hash = hash(&code);
        let token: [u8; 32] = random_array().map_err(Error::internal)?;
        let token_hash = hash(&token);
        let now = self.clock().now();
        let account = self
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let Some(mut grant) = txn.device_grant(code_hash).await? else {
                        return Ok(Err((
                            "expired_token",
                            "the code expired or was used; sign in again",
                        )));
                    };
                    if grant.expires_at <= now {
                        txn.delete_device_grant(code_hash).await?;
                        return Ok(Err(("expired_token", "the code expired; sign in again")));
                    }
                    if grant.denied {
                        txn.delete_device_grant(code_hash).await?;
                        return Ok(Err((
                            "access_denied",
                            "the sign-in was refused on the device page",
                        )));
                    }
                    if let Some(account) = grant.account {
                        txn.delete_device_grant(code_hash).await?;
                        txn.insert_session(token_hash, Session::new(account, UNBOUND))
                            .await?;
                        return Ok(Ok(account));
                    }
                    // A poll may come up to a second early, as a clock
                    // tick rounds it.
                    if now + 1 < grant.next_poll {
                        grant.interval += POLL_INTERVAL;
                        grant.next_poll = now + grant.interval;
                        txn.update_device_grant(&grant).await?;
                        return Ok(Err((
                            "slow_down",
                            "polling too fast; wait 5 more seconds between polls",
                        )));
                    }
                    grant.next_poll = now + grant.interval;
                    txn.update_device_grant(&grant).await?;
                    Ok(Err((
                        "authorization_pending",
                        "waiting for you to approve the code in your browser",
                    )))
                })
            }))
            .await?;
        // A refusal still commits the grant's new state, so it returns here
        // rather than rolling the transaction back.
        let account = account.map_err(|(code, message)| grant_refusal(code, message))?;
        Ok(DeviceTokenResponse {
            session_token: Text::new(to_base64url(&token))
                .map_err(|e| Error::internal(e.name()))?,
            account,
        })
    }

    /// `POST /v2/devices`: enrolls the card of a signed `enroll` record,
    /// checked with `sync-key-envelope`'s verifier for this server's
    /// `origin`, the session's account and a current unused challenge, and
    /// binds the session to it. A card the store holds byte for byte and no
    /// link removed takes a new session again; one whose device ID the
    /// store holds with other bytes, or that a link removed, gets
    /// `id_conflict` (§7).
    pub async fn enroll_device(
        &self,
        session: Session,
        token_hash: Digest,
        origin: &str,
        proof: Vec<u8>,
    ) -> Result<()> {
        let signed = Signed::<Enroll>::from_bytes(&proof).map_err(|e| {
            refuse(
                e.kind.server_code(),
                format!("the proof doesn't decode: {e}"),
            )
        })?;
        let challenge = signed.body.challenge;
        let card = verify_enrollment(
            &proof,
            &EnrollmentContext {
                origin,
                account: session.account,
                challenge,
            },
        )
        .map_err(|e| refuse(e.server_code(), e.to_string()))?;
        if session.device != UNBOUND && session.device != card.device {
            return Err(refuse(
                "forbidden",
                "this session already belongs to another card",
            ));
        }
        let now = self.clock().now();
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(session.account).await?;
                    match txn
                        .take_challenge(hash(&challenge.0), session.account)
                        .await?
                    {
                        Some(expires_at) if expires_at > now => {}
                        _ => {
                            return Err(refuse(
                                "challenge_invalid",
                                "the challenge is unknown, used or expired",
                            ));
                        }
                    }
                    crate::chains::enroll(txn, &card).await?;
                    txn.bind_session(token_hash, card.device).await
                })
            }))
            .await
    }

    /// `GET /v2/devices`: the account's enrolled cards that no link
    /// removed, with their session counts and whether the account's chain
    /// lists them (§7).
    pub async fn devices(&self, session: Session) -> Result<DevicesResponse> {
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let chains = policy::chains_of(txn, session.account).await?;
                    let current = txn.current_genesis(session.account).await?;
                    let counts: BTreeMap<Id, u64> = txn
                        .session_counts(session.account)
                        .await?
                        .into_iter()
                        .collect();
                    let mut devices = Vec::new();
                    for (device, bytes) in txn.enrolled_cards(session.account).await? {
                        let card = Card::from_bytes(&bytes).map_err(Error::internal)?;
                        let listed = chains.iter().find_map(|(genesis, chain)| {
                            chain
                                .card(&device)
                                .map(|state| (*genesis, state.removed_at))
                        });
                        let state = match listed {
                            None => DeviceState::Enrolled,
                            Some((genesis, None)) if Some(genesis) == current => {
                                DeviceState::Active
                            }
                            // Removed by a link, or retired by a reset.
                            Some(_) => continue,
                        };
                        devices.push(DeviceInfo {
                            card: Nested(card),
                            sessions: u32::try_from(counts.get(&device).copied().unwrap_or(0))
                                .unwrap_or(u32::MAX),
                            state,
                        });
                    }
                    Ok(DevicesResponse {
                        devices: List(devices),
                    })
                })
            }))
            .await
    }

    /// `DELETE /v2/devices/{device}`: ends the sessions of an enrolled card
    /// of the account that no chain lists, from a session of an active
    /// device (§7). A card a chain lists leaves only through a link.
    pub async fn end_device_sessions(&self, session: Session, device: Id) -> Result<()> {
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(session.account).await?;
                    active_chain(txn, session).await?;
                    match txn.enrolled_card(device).await? {
                        Some((account, _)) if account == session.account => {}
                        _ => return Err(refuse("not_found", "no such enrolled card")),
                    }
                    let listed = policy::chains_of(txn, session.account)
                        .await?
                        .values()
                        .any(|chain| chain.card(&device).is_some());
                    if listed {
                        return Err(refuse(
                            "forbidden",
                            "a card the account chain lists leaves only through a remove link",
                        ));
                    }
                    txn.delete_device_sessions(session.account, device).await?;
                    Ok(())
                })
            }))
            .await
    }

    /// `POST /v2/approval-requests`: stores a candidate's request for 10
    /// minutes (§10). A request names at most one of an invitation and a
    /// workspace; one for the account's own new device names neither and
    /// carries no account head, from a card the account's chains don't
    /// list. An account holds at most 32 requests, and a retry with
    /// identical bytes gets the original result.
    pub async fn post_approval_request(&self, session: Session, body: Vec<u8>) -> Result<()> {
        let request = ApprovalRequest::from_bytes(&body).map_err(|e| {
            refuse(
                e.kind.server_code(),
                format!("the body doesn't decode: {e}"),
            )
        })?;
        if request.invitation.is_some() && request.workspace.is_some() {
            return Err(refuse(
                "bad_request",
                "a request names at most one of an invitation and a workspace",
            ));
        }
        if request.invite_proof.is_some() != request.invitation.is_some() {
            return Err(refuse(
                "bad_request",
                "an invitation proof comes with its invitation",
            ));
        }
        let own = request.invitation.is_none() && request.workspace.is_none();
        if own && request.account.is_some() {
            return Err(refuse(
                "bad_request",
                "a request of the account's own device carries no account head",
            ));
        }
        if !own && request.account.is_none() {
            return Err(refuse(
                "bad_request",
                "an invitation or reset request carries the candidate's account head",
            ));
        }
        let now = self.clock().now();
        let cache = self.policies.clone();
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(session.account).await?;
                    let card = session_card(txn, session).await?;
                    let chains = policy::chains_of(txn, session.account).await?;
                    if own && chains.values().any(|c| c.card(&card.device).is_some()) {
                        return Err(refuse(
                            "forbidden",
                            "the account chain already lists this card",
                        ));
                    }
                    if let Some(held) = txn.approval_request(request.commit).await? {
                        return if held.request == body && held.device == card.device {
                            Ok(())
                        } else {
                            Err(refuse(
                                "id_conflict",
                                "another request holds this commitment",
                            ))
                        };
                    }
                    if request.invitation.is_some() {
                        // Ticket 50 builds invitations; the store holds none.
                        return Err(refuse(
                            "invitation_invalid",
                            "the invitation is revoked, expired, used or unknown",
                        ));
                    }
                    if let Some(workspace) = request.workspace {
                        reset_request_allowed(txn, &cache, session, workspace).await?;
                    }
                    let pending: Vec<ApprovalRequestRow> = txn
                        .approval_requests(session.account)
                        .await?
                        .into_iter()
                        .filter(|r| r.expires_at > now)
                        .collect();
                    if pending.len() >= MAX_PENDING_REQUESTS {
                        let first = pending.iter().map(|r| r.expires_at).min().unwrap_or(now);
                        return Err(Error::Refused(
                            Refusal::new(
                                "rate_limited",
                                "the account already holds 32 pending approval requests",
                            )
                            .retry_after(first.saturating_sub(now).max(1)),
                        ));
                    }
                    txn.insert_approval_request(&ApprovalRequestRow {
                        commit: request.commit,
                        account: session.account,
                        device: card.device,
                        card: card.to_bytes(),
                        candidate_head: request.account.clone(),
                        invitation: request.invitation,
                        invite_proof: request.invite_proof,
                        workspace: request.workspace,
                        request: body,
                        expires_at: now + SIGN_IN_TTL,
                    })
                    .await
                })
            }))
            .await
    }

    /// `GET /v2/approval-requests`: the unexpired requests of the account's
    /// own new devices, for an active device of the account: those that
    /// name neither an invitation nor a workspace and carry no account head
    /// (§26).
    pub async fn own_approval_requests(&self, session: Session) -> Result<PendingResponse> {
        let now = self.clock().now();
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    active_chain(txn, session).await?;
                    let mut pending = Vec::new();
                    for row in txn.approval_requests(session.account).await? {
                        let own = row.invitation.is_none()
                            && row.workspace.is_none()
                            && row.candidate_head.is_none();
                        if !own || row.expires_at <= now {
                            continue;
                        }
                        pending.push(Pending {
                            card: Nested(Card::from_bytes(&row.card).map_err(Error::internal)?),
                            commit: row.commit,
                            account: None,
                            invitation: None,
                            invite_proof: None,
                            workspace: None,
                        });
                    }
                    Ok(PendingResponse {
                        pending: List(pending),
                    })
                })
            }))
            .await
    }

    /// `POST /v2/approvals`: stores an approving device's offer for 10
    /// minutes (§10). The session's card must be active in its account's
    /// chain and be the offer's `approver_card` byte for byte, the signature
    /// must verify under it, and the offer must answer an unexpired request
    /// of the account's own new device whose card and head it names. A
    /// retry with identical bytes gets the original result.
    pub async fn post_offer(&self, session: Session, offer: Signed<Offer>) -> Result<()> {
        let digest = offer.digest();
        let bytes = offer.to_bytes();
        let now = self.clock().now();
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let chain = active_chain(txn, session).await?;
                    let card = &chain
                        .card(&session.device)
                        .expect("an active card is listed")
                        .card;
                    if offer.body.approver_card.0 != *card {
                        return Err(refuse(
                            "forbidden",
                            "the offer names another card than the session's",
                        ));
                    }
                    if offer.verify(&card.signing_public).is_err() {
                        return Err(refuse(
                            "signature_invalid",
                            "the offer's signature fails under the approver's card",
                        ));
                    }
                    if let Some(held) = txn.approval_offer(digest).await? {
                        return if held.offer == bytes {
                            Ok(())
                        } else {
                            Err(refuse("id_conflict", "another offer holds this digest"))
                        };
                    }
                    let request = txn
                        .approval_request(offer.body.candidate_commit)
                        .await?
                        .filter(|r| r.expires_at > now)
                        .ok_or_else(|| refuse("not_found", "no such approval request"))?;
                    let own = request.invitation.is_none() && request.workspace.is_none();
                    if !own || request.account != session.account {
                        // Invitations and resets arrive with tickets 50 and
                        // 60.
                        return Err(refuse(
                            "forbidden",
                            "the request isn't one of this account's own devices",
                        ));
                    }
                    if offer.body.candidate_card_digest != hash(&request.card)
                        || offer.body.candidate_head != request.candidate_head
                    {
                        return Err(refuse(
                            "forbidden",
                            "the offer names another card or head than the request",
                        ));
                    }
                    txn.insert_approval_offer(&OfferRow {
                        offer_digest: digest,
                        commit: request.commit,
                        candidate_card_digest: offer.body.candidate_card_digest,
                        approver: session.device,
                        account: session.account,
                        offer: bytes,
                        nc: None,
                        expires_at: now + SIGN_IN_TTL,
                    })
                    .await
                })
            }))
            .await
    }

    /// `GET /v2/approvals`: the unexpired offers addressed to the session's
    /// card.
    pub async fn offers(&self, session: Session) -> Result<OffersResponse> {
        let now = self.clock().now();
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let card = session_card(txn, session).await?;
                    let mut offers = Vec::new();
                    for row in txn.approval_offers_for(card.fingerprint()).await? {
                        if row.expires_at <= now {
                            continue;
                        }
                        offers.push(Nested(
                            Signed::<Offer>::from_bytes(&row.offer).map_err(Error::internal)?,
                        ));
                    }
                    Ok(OffersResponse {
                        offers: List(offers),
                    })
                })
            }))
            .await
    }

    /// `POST /v2/approvals/{offer_digest}/reveal`: the candidate reveals its
    /// nonce for one offer addressed to its card. The nonce must open the
    /// offer's commitment; revealing it again with the same nonce changes
    /// nothing.
    pub async fn reveal(&self, session: Session, offer_digest: Digest, nc: Key) -> Result<()> {
        let now = self.clock().now();
        self.backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let card = session_card(txn, session).await?;
                    let row = live_offer(txn, offer_digest, now).await?;
                    if row.candidate_card_digest != card.fingerprint() {
                        return Err(refuse(
                            "forbidden",
                            "the offer is addressed to another card",
                        ));
                    }
                    let offer = Signed::<Offer>::from_bytes(&row.offer).map_err(Error::internal)?;
                    let commit = approval_commit(
                        &row.candidate_card_digest,
                        offer.body.candidate_head.as_ref(),
                        &nc,
                    );
                    if commit != row.commit {
                        return Err(refuse(
                            "forbidden",
                            "the nonce doesn't open the request's commitment",
                        ));
                    }
                    match row.nc {
                        Some(held) if held == nc => Ok(()),
                        Some(_) => Err(refuse("id_conflict", "another nonce was revealed")),
                        None => txn.set_reveal(offer_digest, nc).await,
                    }
                })
            }))
            .await
    }

    /// `GET /v2/approvals/{offer_digest}/reveal`: the nonce the candidate
    /// revealed, for the approving device; `not_found` until it does.
    pub async fn read_reveal(&self, session: Session, offer_digest: Digest) -> Result<RevealBody> {
        let now = self.clock().now();
        self.backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let row = live_offer(txn, offer_digest, now).await?;
                    if row.approver != session.device || row.account != session.account {
                        return Err(refuse("not_found", "no such offer for this device"));
                    }
                    let nc = row
                        .nc
                        .ok_or_else(|| refuse("not_found", "the candidate revealed nothing yet"))?;
                    Ok(RevealBody { nc })
                })
            }))
            .await
    }
}

/// An offer the store holds: `not_found` for an unknown one, and
/// `offer_expired` once 10 minutes passed.
async fn live_offer(txn: &mut impl ReadTxn, offer_digest: Digest, now: u64) -> Result<OfferRow> {
    let row = txn
        .approval_offer(offer_digest)
        .await?
        .ok_or_else(|| refuse("not_found", "no such offer"))?;
    if row.expires_at <= now {
        return Err(refuse(
            "offer_expired",
            "the approval offer is older than 10 minutes",
        ));
    }
    Ok(row)
}

/// A reset request names a workspace where the session's account is a
/// member whose current chain isn't the one the workspace records (§10);
/// anything else is `forbidden`.
async fn reset_request_allowed(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    workspace: Id,
) -> Result<()> {
    let current = txn.current_genesis(session.account).await?;
    let recorded = match policy::load(txn, cache, workspace).await? {
        Some((_, policy)) => policy
            .head_snapshot()
            .members
            .get(&session.account)
            .map(|m| m.head.genesis),
        None => None,
    };
    match (recorded, current) {
        (Some(recorded), Some(current)) if recorded != current => Ok(()),
        _ => Err(refuse(
            "forbidden",
            "a reset request names a workspace that records another chain of a member account",
        )),
    }
}
