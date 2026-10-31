//! Conformance cases for devices (§7, §10): the device authorization
//! grant, enrollment, the sessions a removing link ends, the sessions an
//! active device ends for a card no chain lists, and approval requests,
//! offers and reveals.

use std::sync::Arc;
use std::time::Duration;

use sync_key_envelope::v2::{DeviceKeys, enroll, genesis_link, next_link};
use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::messages::{ApprovalRequest, DeviceRemovedDetail, DeviceState};
use sync_protocol::records::{AccountHead, LinkKind, Offer, Signed, approval_commit};
use sync_protocol::types::{Digest, Id, Key, to_base64url};

use super::{account, head_of, random_token, refusal};
use crate::devices::{UNBOUND, USER_CODE_ALPHABET};
use crate::{Backend, Session, Store};

const ORIGIN: &str = "https://sync.example.com";

/// A card of `account` that a session enrolled through the enrollment
/// route, with that session and its token.
async fn enrolled<B: Backend>(store: &Store<B>, account: Id) -> (DeviceKeys, Session, [u8; 32]) {
    let keys = DeviceKeys::generate().expect("random");
    let token = random_token();
    store
        .create_session(token, Session::new(account, UNBOUND))
        .await
        .expect("a session");
    let session = Session::new(account, UNBOUND);
    let challenge = store
        .challenge(session)
        .await
        .expect("a challenge")
        .challenge;
    let proof = enroll(&keys, account, ORIGIN, challenge).to_bytes();
    store
        .enroll_device(session, sync_protocol::crypto::hash(&token), ORIGIN, proof)
        .await
        .expect("enrolled");
    (keys.clone(), Session::new(account, keys.device()), token)
}

fn code_of<T: std::fmt::Debug>(result: crate::Result<T>) -> &'static str {
    refusal(result).code
}

/// The device authorization grant (RFC 8628, §7): an 8-character user code
/// from the alphabet in two groups of 4, `authorization_pending` until the
/// person decides, `slow_down` for a poll before the interval passed, which
/// adds 5 seconds to it, one session token for an approved code, which works
/// once, `access_denied` for a refused one and `expired_token` after 10
/// minutes, when the store deletes the grant.
pub async fn device_grants_follow_rfc_8628<B: Backend>(store: Store<B>) {
    let uri = "https://sync.example.com/device";
    let grant = store.authorize_device(uri).await.expect("a grant");
    assert_eq!(grant.interval, 5);
    assert_eq!(grant.expires_in, 600);
    assert_eq!(grant.verification_uri.as_str(), uri);
    let user = grant.user_code.as_str();
    assert_eq!(user.len(), 9);
    assert_eq!(&user[4..5], "-");
    assert!(
        user.bytes()
            .filter(|b| *b != b'-')
            .all(|b| USER_CODE_ALPHABET.contains(&b))
    );
    assert_eq!(grant.device_code.as_str().len(), 43);
    let code = grant.device_code.as_str().to_owned();

    assert_eq!(
        code_of(store.poll_device_token(&code).await),
        "authorization_pending"
    );
    assert_eq!(code_of(store.poll_device_token(&code).await), "slow_down");
    // The interval is now 10 seconds: a poll 5 seconds on is still early.
    store.clock().advance(Duration::from_secs(5));
    assert_eq!(code_of(store.poll_device_token(&code).await), "slow_down");
    store.clock().advance(Duration::from_secs(16));
    assert_eq!(
        code_of(store.poll_device_token(&code).await),
        "authorization_pending"
    );

    let account = Id::random().expect("random");
    assert!(store.device_grant_waits(user).await.expect("read"));
    assert!(
        store
            .device_grant_waits(&user.to_lowercase().replace('-', " "))
            .await
            .expect("read"),
        "the page ignores case, spaces and hyphens"
    );
    assert!(
        store
            .decide_device_grant(account, user, true)
            .await
            .expect("decided")
    );
    assert!(
        !store
            .decide_device_grant(account, user, false)
            .await
            .expect("read"),
        "a code takes one decision"
    );
    store.clock().advance(Duration::from_secs(16));
    let signed_in = store.poll_device_token(&code).await.expect("a token");
    assert_eq!(signed_in.account, account);
    let session = store
        .session(signed_in.session_token.as_str())
        .await
        .expect("the session");
    assert_eq!(session, Session::new(account, UNBOUND));
    assert_eq!(
        code_of(store.poll_device_token(&code).await),
        "expired_token",
        "a used code fails"
    );

    let refused = store.authorize_device(uri).await.expect("a grant");
    assert!(
        store
            .decide_device_grant(account, refused.user_code.as_str(), false)
            .await
            .expect("decided")
    );
    assert_eq!(
        code_of(store.poll_device_token(refused.device_code.as_str()).await),
        "access_denied"
    );
    assert_eq!(
        code_of(store.poll_device_token(refused.device_code.as_str()).await),
        "expired_token"
    );

    let late = store.authorize_device(uri).await.expect("a grant");
    store.clock().advance(Duration::from_secs(601));
    assert!(
        !store
            .device_grant_waits(late.user_code.as_str())
            .await
            .expect("read")
    );
    assert!(store.delete_expired().await.expect("deleted") >= 1);
    assert_eq!(
        code_of(store.poll_device_token(late.device_code.as_str()).await),
        "expired_token"
    );
    assert_eq!(
        code_of(store.poll_device_token("not a code").await),
        "bad_request"
    );
}

/// Enrollment (§7): an `enroll` record over a fresh challenge binds the
/// session to its card. The challenge works once, the proof must name this
/// origin and the session's account, a card the store holds enrolls again
/// from a new session, a session keeps the card it bound, and a card that a
/// link removed gets `id_conflict`. `GET /v2/devices` lists the enrolled
/// cards that no link removed, with their sessions and whether the chain
/// lists them.
pub async fn enrollment_binds_the_session_to_its_card<B: Backend>(store: Store<B>) {
    let account = Id::random().expect("random");
    let keys = DeviceKeys::generate().expect("random");
    let token = random_token();
    let unbound = Session::new(account, UNBOUND);
    store
        .create_session(token, unbound)
        .await
        .expect("a session");
    let hash = sync_protocol::crypto::hash(&token);
    let challenge = store
        .challenge(unbound)
        .await
        .expect("a challenge")
        .challenge;
    let proof = |origin: &str, account: Id, challenge: Key| {
        enroll(&keys, account, origin, challenge).to_bytes()
    };
    assert_eq!(
        code_of(
            store
                .enroll_device(
                    unbound,
                    hash,
                    ORIGIN,
                    proof("https://elsewhere.example", account, challenge)
                )
                .await
        ),
        "origin_mismatch"
    );
    assert_eq!(
        code_of(
            store
                .enroll_device(
                    unbound,
                    hash,
                    ORIGIN,
                    proof(ORIGIN, Id::random().expect("random"), challenge)
                )
                .await
        ),
        "forbidden"
    );
    assert_eq!(
        code_of(
            store
                .enroll_device(unbound, hash, ORIGIN, proof(ORIGIN, account, Key([3; 32])))
                .await
        ),
        "challenge_invalid"
    );
    store
        .enroll_device(unbound, hash, ORIGIN, proof(ORIGIN, account, challenge))
        .await
        .expect("enrolled");
    let bearer = to_base64url(&token);
    let session = store.session(&bearer).await.expect("the session");
    assert_eq!(session.device, keys.device());
    assert_eq!(
        code_of(
            store
                .enroll_device(unbound, hash, ORIGIN, proof(ORIGIN, account, challenge))
                .await
        ),
        "challenge_invalid",
        "a challenge works once"
    );

    let listed = store.devices(session).await.expect("the devices");
    assert_eq!(listed.devices.len(), 1);
    assert_eq!(listed.devices[0].card.0, keys.card(account));
    assert_eq!(listed.devices[0].state, DeviceState::Enrolled);
    assert_eq!(listed.devices[0].sessions, 1);

    let genesis = genesis_link(&keys, account);
    store
        .append_chain(session, vec![genesis.clone()])
        .await
        .expect("the genesis");
    let listed = store.devices(session).await.expect("the devices");
    assert_eq!(listed.devices[0].state, DeviceState::Active);

    // After a restore lost the session, the device signs in again and
    // enrolls its existing card with a fresh proof.
    let again = random_token();
    store
        .create_session(again, unbound)
        .await
        .expect("a session");
    let challenge = store
        .challenge(unbound)
        .await
        .expect("a challenge")
        .challenge;
    store
        .enroll_device(
            unbound,
            sync_protocol::crypto::hash(&again),
            ORIGIN,
            proof(ORIGIN, account, challenge),
        )
        .await
        .expect("the same card again");
    let listed = store.devices(session).await.expect("the devices");
    assert_eq!(listed.devices[0].sessions, 2);

    // A session keeps the card it bound.
    let other = DeviceKeys::generate().expect("random");
    let challenge = store
        .challenge(session)
        .await
        .expect("a challenge")
        .challenge;
    assert_eq!(
        code_of(
            store
                .enroll_device(
                    session,
                    hash,
                    ORIGIN,
                    enroll(&other, account, ORIGIN, challenge).to_bytes()
                )
                .await
        ),
        "forbidden"
    );

    // A card that a link removed never enrolls again.
    let (phone, phone_session, _) = enrolled(&store, account).await;
    let add = next_link(
        &keys,
        &head_of(std::slice::from_ref(&genesis)),
        LinkKind::AddDevice {
            card: Nested(phone.card(account)),
            offer: Digest::ZERO,
        },
    );
    store
        .append_chain(session, vec![add.clone()])
        .await
        .expect("added");
    let remove = next_link(
        &keys,
        &head_of(&[genesis.clone(), add]),
        LinkKind::Remove {
            device: phone.device(),
            keep: Sorted::default(),
        },
    );
    store
        .append_chain(session, vec![remove])
        .await
        .expect("removed");
    let listed = store.devices(session).await.expect("the devices");
    assert_eq!(listed.devices.len(), 1, "a removed card leaves the list");
    let fresh = random_token();
    store
        .create_session(fresh, unbound)
        .await
        .expect("a session");
    let challenge = store
        .challenge(unbound)
        .await
        .expect("a challenge")
        .challenge;
    assert_eq!(
        code_of(
            store
                .enroll_device(
                    unbound,
                    sync_protocol::crypto::hash(&fresh),
                    ORIGIN,
                    enroll(&phone, account, ORIGIN, challenge).to_bytes()
                )
                .await
        ),
        "id_conflict"
    );
    let _ = phone_session;
}

/// A link that removes cards ends their sessions in its transaction (§7):
/// each answers `device_removed`, naming the removing link, and keeps only
/// the right to read its account's chains. A self-removal ends the
/// signer's own sessions. Other devices keep theirs.
pub async fn removing_links_end_sessions<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(1);
    let laptop_token = random_token();
    let laptop = Session::new(account, keys[0].device());
    store
        .enroll_card(keys[0].card(account))
        .await
        .expect("enrolled");
    store
        .create_session(laptop_token, laptop)
        .await
        .expect("a session");
    store
        .append_chain(laptop, links.clone())
        .await
        .expect("the genesis");
    let (phone, _, phone_token) = enrolled(&store, account).await;
    let (tablet, tablet_session, tablet_token) = enrolled(&store, account).await;
    let mut chain = links;
    for card in [&phone, &tablet] {
        let link = next_link(
            &keys[0],
            &head_of(&chain),
            LinkKind::AddDevice {
                card: Nested(card.card(account)),
                offer: Digest::ZERO,
            },
        );
        store
            .append_chain(laptop, vec![link.clone()])
            .await
            .expect("added");
        chain.push(link);
    }
    let remove = next_link(
        &keys[0],
        &head_of(&chain),
        LinkKind::Remove {
            device: phone.device(),
            keep: Sorted::default(),
        },
    );
    store
        .append_chain(laptop, vec![remove.clone()])
        .await
        .expect("removed");
    chain.push(remove);

    let phone_bearer = to_base64url(&phone_token);
    let refused = refusal(store.session(&phone_bearer).await);
    assert_eq!(refused.code, "device_removed");
    assert_eq!(refused.status(), 401);
    let detail = <DeviceRemovedDetail as sync_protocol::codec::Decode>::from_bytes(
        refused.detail.as_deref().expect("a detail"),
    )
    .expect("the detail decodes");
    assert_eq!(detail.genesis, chain[0].digest());
    assert_eq!(detail.seqno, 4);
    // The removed session still reads its account's chain, to verify its
    // removal.
    let reader = store
        .session_reading_chains(&phone_bearer)
        .await
        .expect("the session reads chains");
    let read = super::served_seqno(&store, reader, account)
        .await
        .expect("read");
    assert_eq!(read, 4);
    // Only its own account's chains: a cursor for another account answers
    // device_removed.
    let (row, _) = store.session_row(&phone_bearer).await.expect("the row");
    let elsewhere = store
        .read_chains_as(
            row,
            sync_protocol::messages::ChainsRequest {
                cursors: sync_protocol::codec::List(vec![sync_protocol::messages::ChainCursor {
                    account: Id::random().expect("random"),
                    genesis: None,
                    after: 0,
                }]),
            },
        )
        .await;
    assert_eq!(refusal(elsewhere).code, "device_removed");
    store
        .session(&to_base64url(&laptop_token))
        .await
        .expect("the laptop keeps its session");

    // A device signs out with a link that names itself.
    let sign_out = next_link(
        &tablet,
        &head_of(&chain),
        LinkKind::Remove {
            device: tablet.device(),
            keep: Sorted::default(),
        },
    );
    store
        .append_chain(tablet_session, vec![sign_out])
        .await
        .expect("signed out");
    assert_eq!(
        refusal(store.session(&to_base64url(&tablet_token)).await).code,
        "device_removed"
    );
}

/// Ending sessions (§7): an active device of the account ends the sessions
/// of an enrolled card that no chain lists; a session of that card can't,
/// a card a chain lists leaves only through a link, and an unknown card is
/// `not_found`.
pub async fn active_devices_end_a_pending_cards_sessions<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(1);
    let laptop = Session::new(account, keys[0].device());
    store
        .enroll_card(keys[0].card(account))
        .await
        .expect("enrolled");
    store
        .create_session(random_token(), laptop)
        .await
        .expect("a session");
    store
        .append_chain(laptop, links)
        .await
        .expect("the genesis");
    let (pending, pending_session, pending_token) = enrolled(&store, account).await;
    assert_eq!(
        code_of(
            store
                .end_device_sessions(pending_session, pending.device())
                .await
        ),
        "forbidden",
        "a session alone ends nothing"
    );
    assert_eq!(
        code_of(store.end_device_sessions(laptop, keys[0].device()).await),
        "forbidden"
    );
    assert_eq!(
        code_of(
            store
                .end_device_sessions(laptop, Id::random().expect("random"))
                .await
        ),
        "not_found"
    );
    store
        .end_device_sessions(laptop, pending.device())
        .await
        .expect("ended");
    assert_eq!(
        code_of(store.session(&to_base64url(&pending_token)).await),
        "unauthorized"
    );
    let listed = store.devices(laptop).await.expect("the devices");
    let held = listed
        .devices
        .iter()
        .find(|d| d.card.0.device == pending.device())
        .expect("the card stays enrolled");
    assert_eq!(held.sessions, 0);
}

fn request(commit: Digest) -> Vec<u8> {
    ApprovalRequest {
        commit,
        account: None,
        invitation: None,
        invite_proof: None,
        workspace: None,
    }
    .to_bytes()
}

/// Approval requests, offers and reveals (§10): a candidate's request for
/// its own account names neither an invitation nor a workspace and carries
/// no head; an active device lists it and answers with an offer that names
/// the candidate's card and commitment; the candidate sees the offer and
/// reveals the nonce that opens its commitment, which only the approving
/// device reads. Requests and offers expire after 10 minutes, and an
/// account holds at most 32 requests.
pub async fn approval_requests_offers_and_reveals<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(1);
    let laptop = Session::new(account, keys[0].device());
    store
        .enroll_card(keys[0].card(account))
        .await
        .expect("enrolled");
    store
        .create_session(random_token(), laptop)
        .await
        .expect("a session");
    store
        .append_chain(laptop, links.clone())
        .await
        .expect("the genesis");
    let (phone, phone_session, _) = enrolled(&store, account).await;
    let card = phone.card(account);
    let nc = Key(sync_protocol::crypto::random_array().expect("random"));
    let commit = approval_commit(&card.fingerprint(), None, &nc);

    assert_eq!(
        code_of(
            store
                .post_approval_request(
                    phone_session,
                    ApprovalRequest {
                        commit,
                        account: None,
                        invitation: Some(Id([1; 16])),
                        invite_proof: Some(Digest([2; 32])),
                        workspace: Some(Id([3; 16])),
                    }
                    .to_bytes()
                )
                .await
        ),
        "bad_request"
    );
    assert_eq!(
        code_of(
            store
                .post_approval_request(
                    phone_session,
                    ApprovalRequest {
                        commit,
                        account: Some(head_of(&links)),
                        invitation: None,
                        invite_proof: None,
                        workspace: None,
                    }
                    .to_bytes()
                )
                .await
        ),
        "bad_request"
    );
    store
        .post_approval_request(phone_session, request(commit))
        .await
        .expect("the request");
    store
        .post_approval_request(phone_session, request(commit))
        .await
        .expect("a retry with the same bytes");
    assert_eq!(
        code_of(
            store
                .post_approval_request(laptop, request(Digest([8; 32])))
                .await
        ),
        "forbidden",
        "a card the chain lists asks for no approval"
    );
    assert_eq!(
        code_of(store.own_approval_requests(phone_session).await),
        "forbidden",
        "only an active device lists requests"
    );
    let pending = store.own_approval_requests(laptop).await.expect("listed");
    assert_eq!(pending.pending.len(), 1);
    assert_eq!(pending.pending[0].card.0, card);
    assert_eq!(pending.pending[0].commit, commit);

    let offer = |card_digest: Digest, signer: &DeviceKeys| {
        Signed::sign(
            Offer {
                approver_card: Nested(keys[0].card(account)),
                candidate_card_digest: card_digest,
                candidate_commit: commit,
                candidate_head: None,
                approver_head: head_of(&links),
                nonce: Key([7; 32]),
                anchors: Sorted::default(),
            },
            signer.signing(),
        )
    };
    assert_eq!(
        code_of(
            store
                .post_offer(laptop, offer(Digest([4; 32]), &keys[0]))
                .await
        ),
        "forbidden"
    );
    assert_eq!(
        code_of(
            store
                .post_offer(laptop, offer(card.fingerprint(), &phone))
                .await
        ),
        "signature_invalid"
    );
    assert_eq!(
        code_of(
            store
                .post_offer(phone_session, offer(card.fingerprint(), &phone))
                .await
        ),
        "forbidden",
        "only an active device offers"
    );
    let signed = offer(card.fingerprint(), &keys[0]);
    let digest = signed.digest();
    store
        .post_offer(laptop, signed.clone())
        .await
        .expect("offered");
    store
        .post_offer(laptop, signed.clone())
        .await
        .expect("a retry");
    let offers = store.offers(phone_session).await.expect("the offers");
    assert_eq!(offers.offers.len(), 1);
    assert_eq!(offers.offers[0].0, signed);
    assert!(store.offers(laptop).await.expect("none").offers.is_empty());

    assert_eq!(
        code_of(store.read_reveal(laptop, digest).await),
        "not_found",
        "nothing revealed yet"
    );
    assert_eq!(
        code_of(store.reveal(phone_session, digest, Key([5; 32])).await),
        "forbidden",
        "a nonce that doesn't open the commitment"
    );
    assert_eq!(
        code_of(store.reveal(laptop, digest, nc).await),
        "forbidden",
        "only the candidate reveals"
    );
    store
        .reveal(phone_session, digest, nc)
        .await
        .expect("revealed");
    store
        .reveal(phone_session, digest, nc)
        .await
        .expect("again changes nothing");
    assert_eq!(
        store.read_reveal(laptop, digest).await.expect("read").nc,
        nc
    );
    assert_eq!(
        code_of(store.read_reveal(phone_session, digest).await),
        "not_found"
    );

    // 32 requests at most per account.
    for _ in 1..32 {
        let nc = Key(sync_protocol::crypto::random_array().expect("random"));
        store
            .post_approval_request(
                phone_session,
                request(approval_commit(&card.fingerprint(), None, &nc)),
            )
            .await
            .expect("a request");
    }
    let over = refusal(
        store
            .post_approval_request(phone_session, request(Digest([6; 32])))
            .await,
    );
    assert_eq!(over.code, "rate_limited");
    assert!(over.retry_after_seconds.is_some_and(|s| s > 0 && s <= 600));

    store.clock().advance(Duration::from_secs(601));
    assert_eq!(
        code_of(store.read_reveal(laptop, digest).await),
        "offer_expired"
    );
    assert!(
        store
            .own_approval_requests(laptop)
            .await
            .expect("listed")
            .pending
            .is_empty()
    );
    store.delete_expired().await.expect("deleted");
    assert_eq!(
        code_of(store.read_reveal(laptop, digest).await),
        "not_found"
    );
    let _: AccountHead = head_of(&links);
}
/// A poll that read a grant before the device page approved it never wipes
/// the approval: the device signs in on a later poll (§7).
pub async fn device_polls_race_their_decision<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    let grant = first
        .authorize_device("https://sync.example.com/device")
        .await
        .expect("a grant");
    let user = grant.user_code.as_str().to_owned();
    let code = grant.device_code.as_str().to_owned();
    second.clock().advance(Duration::from_secs(6));
    let account = Id::random().expect("random");
    let decide = {
        let first = first.clone();
        let user = user.clone();
        tokio::spawn(async move { first.decide_device_grant(account, &user, true).await })
    };
    // The poll reads while the approval waits for its commit.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let polled = second.poll_device_token(&code).await;
    assert!(decide.await.expect("joined").expect("decided"));
    if polled.is_ok() {
        return;
    }
    second.clock().advance(Duration::from_secs(16));
    let signed_in = second
        .poll_device_token(&code)
        .await
        .expect("the approval stands");
    assert_eq!(signed_in.account, account);
}

/// An approved grant starts one session, however many polls race for it.
pub async fn device_polls_race_each_other<B: Backend>(first: Arc<Store<B>>, second: Arc<Store<B>>) {
    let grant = first
        .authorize_device("https://sync.example.com/device")
        .await
        .expect("a grant");
    let code = grant.device_code.as_str().to_owned();
    let account = Id::random().expect("random");
    assert!(
        first
            .decide_device_grant(account, grant.user_code.as_str(), true)
            .await
            .expect("decided")
    );
    let (one, two) = tokio::join!(
        first.poll_device_token(&code),
        second.poll_device_token(&code)
    );
    let won = [&one, &two].iter().filter(|poll| poll.is_ok()).count();
    assert_eq!(won, 1, "one poll takes the grant: {one:?} {two:?}");
    let lost = if one.is_ok() { two } else { one };
    assert_eq!(code_of(lost), "expired_token");
}
