//! Account chains on the server (§7): appending links with the server's
//! authority checks, reading the chains a session may read, enrolled cards,
//! enrollment challenges and recovery proofs (§12).

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sync_key_envelope::v2::{RecoveryContext, verify_recovery_proof};
use sync_protocol::account::{AccountChain, AccountError};
use sync_protocol::codec::{Decode, Encode, List, Nested};
use sync_protocol::messages::{
    ChainCursor, ChainPage, ChainStatus, ChainsResponse, LinkBytes, MAX_BODY,
};
use sync_protocol::records::{AccountLink, Card, LinkKind, RecoveryProof, Signed};
use sync_protocol::types::{Digest, Id, Key};

use crate::policy::{self, ChainCache, PolicyCache};
use crate::{
    ApprovalRequestRow, Error, LinkRow, ReadTxn, RemovedBy, Result, Session, WriteTxn, refuse,
};

/// How long an enrollment challenge, a magic link, a passkey ceremony or a
/// sign-up waiting for its email stays usable (§27).
pub const SIGN_IN_TTL: u64 = 600;

/// The most cursors one `POST /v2/accounts/chains` lists (§27).
pub const MAX_CHAIN_CURSORS: usize = 256;

fn refusal(error: AccountError) -> Error {
    refuse(error.server_code(), error.to_string())
}

/// The devices that the account's chains other than `except` listed.
fn listed_elsewhere(
    chains: &BTreeMap<Digest, AccountChain>,
    except: Option<Digest>,
) -> BTreeSet<Id> {
    chains
        .iter()
        .filter(|(genesis, _)| Some(**genesis) != except)
        .flat_map(|(_, chain)| chain.listed_devices())
        .collect()
}

/// `POST /v2/accounts/chain`: appends `links` to the session account's
/// chain, in order and all or none, or starts its chain with a genesis.
/// Each link must come from a session of its signer, or, for
/// `recover_device`, from a session that proved that recovery identity, and
/// a genesis, `add_device` or `recover_device` must add a card that a
/// session of the account enrolled. Returns the new head's seqno.
///
/// The append takes only the account's chain row, before it reads the
/// chain (§15), so appends to one account commit one at a time. Two
/// geneses for an account without a chain race on its chain row, and the
/// later one gets `forbidden`, as it would after the first committed.
pub(crate) async fn append(
    txn: &mut impl WriteTxn,
    session: Session,
    links: &[Signed<AccountLink>],
) -> Result<u64> {
    if links.is_empty() {
        return Err(refuse("bad_request", "no links"));
    }
    let account = session.account;
    txn.lock_account(account).await?;
    // The genesis first: under READ COMMITTED, a rival genesis that commits
    // after this read is either in the chains read next or met by
    // `start_chain`, and never both missed.
    let current = txn.current_genesis(account).await?;
    let chains = policy::chains_of(txn, account).await?;
    let listed = listed_elsewhere(&chains, current);
    let mut chain: Option<AccountChain> = current.and_then(|genesis| chains.get(&genesis).cloned());
    let start = chain.as_ref().map_or(1, |c| c.head_seqno() + 1);
    for link in links {
        let body = &link.body;
        if matches!(body.kind, LinkKind::Genesis { .. }) && chain.is_some() {
            return Err(refuse(
                "forbidden",
                "the account already has a chain; a reset starts another",
            ));
        }
        if body.account != account {
            return Err(refuse("forbidden", "the link names another account"));
        }
        let (seqno, previous) = chain
            .as_ref()
            .map_or((1, Digest::ZERO), |c| (c.head_seqno() + 1, c.head_digest()));
        if body.seqno != seqno || body.previous != previous {
            return Err(refuse(
                "account_conflict",
                "the link's previous isn't the chain's head",
            ));
        }
        let signer_session = body.signer == session.device
            || (matches!(body.kind, LinkKind::RecoverDevice { .. })
                && session.recovery == Some(body.signer));
        if !signer_session {
            return Err(refuse(
                "forbidden",
                "a link comes only from a session of its signer",
            ));
        }
        if let LinkKind::Genesis { card }
        | LinkKind::AddDevice { card, .. }
        | LinkKind::RecoverDevice { card } = &body.kind
        {
            let enrolled = txn.enrolled_card(card.device).await?;
            if enrolled != Some((account, card.0.to_bytes())) {
                return Err(refuse(
                    "forbidden",
                    "the card isn't one a session of the account enrolled",
                ));
            }
        }
        match &mut chain {
            Some(chain) => chain.append(link, &listed).map_err(refusal)?,
            None => chain = Some(AccountChain::from_genesis(link, &listed).map_err(refusal)?),
        }
    }
    let chain = chain.expect("at least one link");
    if current.is_none() && !txn.start_chain(account, chain.genesis()).await? {
        return Err(refuse(
            "forbidden",
            "the account already has a chain; a reset starts another",
        ));
    }
    for link in links.iter().filter(|l| l.body.seqno >= start) {
        txn.insert_account_link(
            account,
            &LinkRow {
                genesis: chain.genesis(),
                seqno: link.body.seqno,
                link: link.to_bytes(),
            },
            link.digest(),
        )
        .await?;
    }
    end_removed_sessions(txn, &chain, start).await?;
    Ok(chain.head_seqno())
}

/// Ends, in the append's transaction, the sessions of every card that a
/// link of `chain` at or above seqno `start` removed: each keeps only the
/// right to read its account's chains and answers `device_removed` naming
/// the link (§7).
pub(crate) async fn end_removed_sessions(
    txn: &mut impl WriteTxn,
    chain: &AccountChain,
    start: u64,
) -> Result<()> {
    for seqno in start..=chain.head_seqno() {
        let removed: Vec<Id> = chain
            .cards()
            .filter(|state| state.removed_at == Some(seqno))
            .map(|state| state.card.device)
            .collect();
        if !removed.is_empty() {
            txn.remove_sessions(
                chain.account(),
                &removed,
                RemovedBy {
                    genesis: chain.genesis(),
                    seqno,
                },
            )
            .await?;
        }
    }
    Ok(())
}

/// The highest seqno of `account`'s chain under `genesis` that `session`
/// may read, or none (§7): every chain of its own account in full; the
/// current chain in full of an account whose approval request, unexpired
/// at `now`, names an invitation to a workspace the session's account
/// belongs to; and, in each workspace whose policy the session reads,
/// every chain of an account that the policy records, up to the latest
/// head recorded on it, and the chain the policy names for a current
/// member in full to the workspace's devices. A device whose account
/// `remove_member` removed reads the policy, and so these chains, up to its
/// removal.
#[allow(clippy::too_many_arguments)]
async fn readable_seqno(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    session: Session,
    account: Id,
    genesis: Digest,
    chain: &AccountChain,
    current: bool,
    now: u64,
) -> Result<Option<u64>> {
    if account == session.account {
        return Ok(Some(chain.head_seqno()));
    }
    if current {
        for (workspace, expires_at) in txn.invitation_requests(account).await? {
            if expires_at <= now {
                continue;
            }
            let Some((_, policy)) = policy::load(txn, cache, workspace).await? else {
                continue;
            };
            if policy.reads_invitees(&session.account) {
                return Ok(Some(chain.head_seqno()));
            }
        }
    }
    let own: BTreeSet<Id> = txn
        .workspaces_recording(session.account)
        .await?
        .into_iter()
        .collect();
    let mut best: Option<u64> = None;
    for workspace in txn.workspaces_recording(account).await? {
        if !own.contains(&workspace) {
            continue;
        }
        let Some((_, policy)) = policy::load(txn, cache, workspace).await? else {
            continue;
        };
        let reader: Vec<Id> = [Some(session.device), session.recovery]
            .into_iter()
            .flatten()
            .collect();
        // Until the workspace accepts a reset, its devices read the new
        // chain to show the new fingerprint (§7).
        if current && policy.reads_reset_chain(&reader, &account, &genesis) {
            return Ok(Some(chain.head_seqno()));
        }
        let Some(bound) = policy.chain_read_bound(
            &reader,
            &session.account,
            &account,
            &genesis,
            chain.head_seqno(),
        ) else {
            continue;
        };
        let bound = bound.min(chain.head_seqno());
        best = Some(best.map_or(bound, |b| b.max(bound)));
    }
    Ok(best)
}

/// `POST /v2/accounts/chains`: one page per cursor, in request order, with
/// the links after each cursor's seqno that the session may read, at most
/// 1 MiB in all. A cursor naming a chain the store doesn't hold, or one the
/// session may not read at `now`, gets `not_found`.
pub(crate) async fn read(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    verified: &ChainCache,
    session: Session,
    cursors: &[ChainCursor],
    now: u64,
) -> Result<ChainsResponse> {
    // Each page's fixed fields take 16 + 32 + 1 + 8 + 4 bytes, and every
    // cursor gets a page, so the pages' fixed fields fit 1 MiB.
    const PAGE_FIELDS: usize = 61;
    if cursors.len() > MAX_CHAIN_CURSORS {
        return Err(refuse(
            "too_large",
            "a chain read lists at most 256 cursors",
        ));
    }
    let named: BTreeSet<(Id, Option<Digest>)> =
        cursors.iter().map(|c| (c.account, c.genesis)).collect();
    if named.len() != cursors.len() {
        return Err(refuse("bad_request", "a chain read names a chain twice"));
    }
    let mut budget = MAX_BODY - (4 + PAGE_FIELDS * cursors.len());
    let mut pages = Vec::with_capacity(cursors.len());
    // Verifying a chain costs a signature per link, so a read verifies each
    // account at most once, and its current chain only when the cache
    // lacks that head.
    let mut every_chain: BTreeMap<Id, BTreeMap<Digest, Arc<AccountChain>>> = BTreeMap::new();
    for cursor in cursors {
        let current = txn.current_genesis(cursor.account).await?;
        let genesis = cursor
            .genesis
            .or(current)
            .ok_or_else(|| refuse("not_found", "no such account chain"))?;
        let is_current = current == Some(genesis);
        let chain = if is_current {
            policy::cached_current_chain(txn, verified, cursor.account).await?
        } else {
            let chains = match every_chain.entry(cursor.account) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(unread) => {
                    let chains = policy::chains_of(txn, cursor.account).await?;
                    unread.insert(chains.into_iter().map(|(g, c)| (g, Arc::new(c))).collect())
                }
            };
            chains.get(&genesis).cloned()
        };
        let chain = chain.ok_or_else(|| refuse("not_found", "no such account chain"))?;
        let bound = readable_seqno(
            txn,
            cache,
            session,
            cursor.account,
            genesis,
            &chain,
            is_current,
            now,
        )
        .await?
        .ok_or_else(|| refuse("not_found", "no such account chain for this session"))?;
        let mut links = Vec::new();
        for row in txn.account_links(cursor.account).await? {
            if row.genesis != genesis || row.seqno <= cursor.after || row.seqno > bound {
                continue;
            }
            let size = 4 + row.link.len();
            if size > budget {
                break;
            }
            budget -= size;
            let link = Signed::<AccountLink>::from_bytes(&row.link).map_err(Error::internal)?;
            links.push(Nested(link) as LinkBytes);
        }
        pages.push(ChainPage {
            account: cursor.account,
            genesis,
            status: if is_current {
                ChainStatus::Current
            } else {
                ChainStatus::Retired
            },
            head_seqno: bound,
            links: List(links),
        });
    }
    Ok(ChainsResponse { pages: List(pages) })
}

/// Stores a card that a session of its account enrolled, or takes the same
/// card again byte for byte. A device ID that a link of the account removed
/// or that a chain a reset retired lists, or that the store holds with
/// other bytes, gets `id_conflict` (§7). So does a recovery identity, which
/// holds no session of its own (§12), and any card a chain lists that the
/// store never enrolled. Ticket 47's enrollment route calls this after it
/// verifies the proof.
pub(crate) async fn enroll(txn: &mut impl WriteTxn, card: &Card) -> Result<()> {
    // The chain row orders an enrollment with the appends that read the
    // card or remove its device ID.
    txn.lock_account(card.account).await?;
    let bytes = card.to_bytes();
    let current = txn.current_genesis(card.account).await?;
    let mut listed = false;
    for (genesis, chain) in policy::chains_of(txn, card.account).await? {
        let Some(state) = chain.card(&card.device) else {
            continue;
        };
        if state.removed_at.is_some() || Some(genesis) != current {
            return Err(refuse(
                "id_conflict",
                "a link removed this device ID, or a reset retired it",
            ));
        }
        if state.recovery {
            return Err(refuse(
                "id_conflict",
                "a recovery identity holds no session of its own",
            ));
        }
        listed = true;
    }
    let same = |held: Option<(Id, Vec<u8>)>| match held {
        Some((account, held)) if account == card.account && held == bytes => Ok(()),
        _ => Err(refuse(
            "id_conflict",
            "the device ID is enrolled with other bytes",
        )),
    };
    if let Some(held) = txn.enrolled_card(card.device).await? {
        return same(Some(held));
    }
    if listed {
        return Err(refuse(
            "id_conflict",
            "a chain lists this device ID, and it was never enrolled",
        ));
    }
    // An enrollment of the same device ID under another account, or one of
    // an account without a chain row to lock, can commit meanwhile: the
    // insert then waits for it and takes nothing, and the stored card
    // decides as above.
    if txn
        .insert_enrolled_card(card.device, card.account, &bytes)
        .await?
    {
        return Ok(());
    }
    same(txn.enrolled_card(card.device).await?)
}

/// `POST /v2/devices/challenge`: a fresh 32-byte challenge for the
/// session's account, valid for 10 minutes and usable once. The store
/// keeps only its BLAKE3 hash (§29).
pub(crate) async fn challenge(txn: &mut impl WriteTxn, session: Session, now: u64) -> Result<Key> {
    let challenge = Key(sync_protocol::crypto::random_array().map_err(Error::internal)?);
    txn.insert_challenge(
        sync_protocol::crypto::hash(&challenge.0),
        session.account,
        now + SIGN_IN_TTL,
    )
    .await?;
    Ok(challenge)
}

/// Stores an approval request that a session of `session`'s account
/// posted, expiring 10 minutes after `now` (§10, §27).
pub(crate) async fn store_approval_request(
    txn: &mut impl WriteTxn,
    session: Session,
    commit: Digest,
    invitation: Option<(Id, Id)>,
    now: u64,
) -> Result<()> {
    let card = txn
        .enrolled_card(session.device)
        .await?
        .map(|(_, card)| card)
        .unwrap_or_default();
    txn.insert_approval_request(&ApprovalRequestRow {
        commit,
        account: session.account,
        device: session.device,
        card,
        candidate_head: None,
        invitation: invitation.map(|(invitation, _)| invitation),
        invite_proof: None,
        workspace: invitation.map(|(_, workspace)| workspace),
        request: Vec::new(),
        expires_at: now + SIGN_IN_TTL,
    })
    .await
}

/// `POST /v2/recovery`: checks the proof under the recovery identity's
/// card, that the card is the active recovery identity of the session
/// account's chain, that the new card is the session's enrolled card, and
/// that the challenge is current and unused, then records the identity on
/// the session.
pub(crate) async fn prove_recovery(
    txn: &mut impl WriteTxn,
    session: Session,
    token_hash: Digest,
    origin: &str,
    proof: &Signed<RecoveryProof>,
    now: u64,
) -> Result<()> {
    txn.lock_account(session.account).await?;
    let chain = policy::current_chain(txn, session.account)
        .await?
        .ok_or_else(|| refuse("forbidden", "the account has no chain"))?;
    let session_card = match txn.enrolled_card(session.device).await? {
        Some((account, bytes)) if account == session.account => {
            Card::from_bytes(&bytes).map_err(Error::internal)?
        }
        _ => return Err(refuse("forbidden", "the session's card isn't enrolled")),
    };
    let challenge = verify_recovery_proof(
        proof,
        &RecoveryContext {
            origin,
            chain: &chain,
            session_card: &session_card,
        },
    )
    .map_err(|e| refuse(e.server_code(), e.to_string()))?;
    let challenge_hash = sync_protocol::crypto::hash(&challenge.0);
    match txn.take_challenge(challenge_hash, session.account).await? {
        Some(expires_at) if expires_at > now => {}
        _ => {
            return Err(refuse(
                "challenge_invalid",
                "the challenge is unknown, used or expired",
            ));
        }
    }
    txn.set_session_recovery(token_hash, proof.body.recovery_device)
        .await
}
