//! A tampering proxy in front of a server that claims a restore (seam 4,
//! sync-v2.md §14 and §30). The server raises its generation and the proxy
//! leaves out what the client accepted, takes over an ID or disagrees with
//! a final head. The client stops with `sync_restore_lost` before any file
//! changes and uploads nothing.

mod sync_support;

use app_core::sync::members::MemberChange;
use app_core::sync::workspace::restore::Lost;
use sync_key_envelope::v2::{DeviceKeys, genesis_link};
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{
    ChainsResponse, CheckpointResponse, Entry, EntryRecord, PolicyResponse,
};
use sync_protocol::records::{Role, Signed};
use sync_protocol::types::Id;
use sync_store_sqlite::SqliteBackend;
use sync_support::{Pair, Server, pair_of_accounts_on, reconnect, rounds};

const NEVER: std::time::Duration = std::time::Duration::from_secs(1 << 40);

type Tested = Pair<SqliteBackend>;

/// The server restores its own latest state, so the proxy alone decides
/// what went missing. The caller keeps the restored server running.
async fn claim_a_restore(pair: &mut Tested) -> Server {
    let copy = pair.server.backup("same.sqlite3").await;
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(pair, &restored.base).await;
    restored
}

/// The laptop's next round stops on `lost`. It changes no file, and with a
/// new edit waiting the laptop still uploads nothing.
async fn caught(pair: &mut Tested, lost: &Lost) {
    let files = pair.a.files();
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_ok(), "{round:?}");
    assert_eq!(pair.sync_a.restore_lost(), Some(lost));
    assert_eq!(pair.a.files(), files, "no file changed");

    pair.a.write("Stopped.md", b"# Stays here\n");
    let before = pushes(pair);
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.pushed, 0);
    assert_eq!(pushes(pair), before, "a stopped workspace uploads nothing");
}

fn pushes(pair: &Tested) -> usize {
    pair.proxy
        .requests()
        .iter()
        .filter(|request| request.method == "POST" && request.path.contains("/push"))
        .count()
}

async fn two_accounts() -> Tested {
    let mut pair = pair_of_accounts_on(Server::start().await).await;
    pair.b.write("Member.md", b"# Member\n");
    rounds(&mut pair).await;
    pair
}

/// Two accounts on a server that makes a checkpoint due every 6 entries.
/// Both devices reach one, and the desktop has records under it.
async fn two_accounts_past_a_checkpoint() -> Tested {
    let mut pair = pair_of_accounts_on(Server::start_with_interval(6).await).await;
    pair.sync_a.set_checkpoint_wait(std::time::Duration::ZERO);
    pair.sync_b.set_checkpoint_wait(std::time::Duration::ZERO);
    pair.b.write("Member.md", b"# Member\n");
    rounds(&mut pair).await;
    for i in 0..8 {
        pair.a.write(&format!("First {i}.md"), b"# First\n");
        rounds(&mut pair).await;
    }
    assert!(pair.sync_a.latest_checkpoint().is_some());
    pair
}

async fn demote_the_member(pair: &mut Tested) {
    let account = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(
            &mut pair.service,
            MemberChange::SetRole {
                account,
                role: Role::Viewer,
            },
        )
        .await
        .expect("demoted");
    rounds(pair).await;
}

/// Rewrites `account`'s chain with a new genesis on every read: the old
/// genesis stays in the answer, as a client asked for it, and `forged`
/// stands where its links were.
fn serve_chain_as(pair: &Tested, account: Id, forged: Signed<sync_protocol::records::AccountLink>) {
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/accounts/chains") {
            return body;
        }
        let mut response = ChainsResponse::from_bytes(&body).expect("a chains response");
        for page in response.pages.0.iter_mut() {
            if page.account == account {
                page.links = List(vec![Nested(forged.clone())]);
                page.head_seqno = 1;
            }
        }
        response.to_bytes()
    }));
}

/// A server that drops the policy change you accepted, under a new
/// generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_policy_change_stops_the_workspace() {
    let mut pair = two_accounts().await;
    demote_the_member(&mut pair).await;
    let _restored = claim_a_restore(&mut pair).await;
    let accepted = pair.sync_a.policy_revision();
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/policy") {
            return body;
        }
        let mut response = PolicyResponse::from_bytes(&body).expect("a policy response");
        response.changes.0.retain(|c| c.0.body.revision < accepted);
        response.head_revision = accepted - 1;
        response.to_bytes()
    }));
    caught(&mut pair, &Lost::Policy).await;
}

/// A server that drops the epoch you accepted serves the one before it,
/// with its envelope, as the current one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_epoch_record_stops_the_workspace() {
    let mut pair = two_accounts().await;
    let (workspace, laptop) = (pair.sync_a.workspace(), pair.laptop.id());
    let before = pair
        .service
        .client()
        .current_keys(workspace, laptop)
        .await
        .expect("the keys");
    let member = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(&mut pair.service, MemberChange::Remove(member))
        .await
        .expect("removed");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the rotation");
    let after = pair
        .service
        .client()
        .current_keys(workspace, laptop)
        .await
        .expect("the keys");
    assert_ne!(before, after, "the removal rotated the domain");

    let _restored = claim_a_restore(&mut pair).await;
    let older = before.to_bytes();
    pair.proxy.forge(Box::new(move |_, path| {
        path.ends_with("/keys/current")
            .then(|| (200, older.clone()))
    }));
    caught(&mut pair, &Lost::Epoch).await;
}

/// A server that drops your envelope of the current epoch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_envelope_stops_the_workspace() {
    use sync_protocol::messages::{KeyRecord, KeysResponse};

    let mut pair = two_accounts().await;
    let laptop = pair.laptop.id();
    let _restored = claim_a_restore(&mut pair).await;
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/keys/current") {
            return body;
        }
        let mut response = KeysResponse::from_bytes(&body).expect("a keys response");
        response.entries.0.retain(
            |e| !matches!(&e.record, KeyRecord::Envelope(env) if env.body.recipient == laptop),
        );
        response.to_bytes()
    }));
    caught(&mut pair, &Lost::Envelope).await;
}

/// A new chain under your own account ID, signed by a key the server
/// holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_chain_under_your_account_id_stops_you() {
    let mut pair = two_accounts().await;
    let _restored = claim_a_restore(&mut pair).await;
    let account = pair.laptop.credentials.account;
    let attacker = DeviceKeys::generate().expect("keys");
    serve_chain_as(&pair, account, genesis_link(&attacker, account));
    caught(&mut pair, &Lost::OwnAccount).await;
}

/// A new chain under a member's account ID. No `accept_reset` names it, so
/// the workspace stops and its keys stay away from the new card.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_chain_under_a_members_account_id_stops_the_workspace() {
    let mut pair = two_accounts().await;
    let member = pair.desktop.credentials.account;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(member, genesis)
        .await
        .expect("kept");
    let _restored = claim_a_restore(&mut pair).await;
    let attacker = DeviceKeys::generate().expect("keys");
    serve_chain_as(&pair, member, genesis_link(&attacker, member));
    caught(&mut pair, &Lost::MemberAccount(member)).await;
}

/// A chain that claims a member's device ID for a key the server holds.
/// A device ID derives from its signing key, so such a card doesn't even
/// decode: the comparison stays pending, nothing moves until an honest
/// answer, and that answer finds nothing lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_card_that_claims_a_device_id_never_decodes() {
    use sync_protocol::records::{AccountLink, LinkKind};

    let mut pair = two_accounts().await;
    let member = pair.desktop.credentials.account;
    let device = pair.desktop.id();
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(member, genesis)
        .await
        .expect("kept");
    let _restored = claim_a_restore(&mut pair).await;
    let attacker = DeviceKeys::generate().expect("keys");
    let mut card = attacker.card(member);
    card.device = device;
    let claimed = Signed::sign(
        AccountLink {
            account: member,
            seqno: 1,
            previous: sync_protocol::types::Digest::ZERO,
            signer: device,
            kind: LinkKind::Genesis { card: Nested(card) },
        },
        attacker.signing(),
    );
    serve_chain_as(&pair, member, claimed);

    let files = pair.a.files();
    pair.a.write("Waiting.md", b"# Waits\n");
    let before = pushes(&pair);
    let refused = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(
        matches!(&refused, Err(e) if e.to_string().contains("device_id_mismatch")),
        "{refused:?}"
    );
    assert_eq!(pushes(&pair), before, "nothing went out");
    let mut expected = files;
    expected.insert("Waiting.md".into(), b"# Waits\n".to_vec());
    assert_eq!(pair.a.files(), expected, "no file changed");

    pair.proxy.honest();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("an honest round");
    assert_eq!(pair.sync_a.restore_lost(), None);
}

/// The server lost every checkpoint the device reached, as it claims, and
/// offers one that a writer signed before it lost write access, at an
/// older revision. The device never reached it, so it isn't a start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_whose_signer_lost_write_access_stops_the_rewind() {
    let mut pair = two_accounts_past_a_checkpoint().await;
    pair.sync_a.set_checkpoint_wait(NEVER);
    pair.sync_b.set_checkpoint_wait(NEVER);
    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());
    let latest = pair
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("answered")
        .expect("a checkpoint");
    demote_the_member(&mut pair).await;

    let mut body = latest.bundle.checkpoint.0.body.clone();
    body.previous = latest.bundle.checkpoint.0.digest();
    body.number += 1;
    body.signer = pair.desktop.id();
    let mut bundle = latest.bundle.clone();
    bundle.checkpoint = Nested(Signed::sign(body, pair.desktop.credentials.keys.signing()));
    let by_digest = Entry {
        seq: latest.seq + 1,
        record: EntryRecord::Checkpoint(bundle.clone()),
    }
    .to_bytes();
    let newest = CheckpointResponse {
        seq: latest.seq + 1,
        bundle,
    }
    .to_bytes();

    let _restored = claim_a_restore(&mut pair).await;
    pair.proxy.forge(Box::new(move |_, path| {
        if path.contains("/checkpoints/") {
            Some((200, by_digest.clone()))
        } else if path.ends_with("/checkpoint") {
            Some((200, newest.clone()))
        } else {
            None
        }
    }));
    caught(&mut pair, &Lost::Checkpoints).await;
}

/// Rewrites `device`'s head in every checkpoint the rewind can start from
/// with `change`.
fn tamper_start(
    pair: &Tested,
    device: Id,
    change: impl Fn(&mut sync_protocol::records::Ref) + Send + Sync + 'static,
) {
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.contains("/checkpoints/") {
            return body;
        }
        let mut entry = Entry::from_bytes(&body).expect("a log entry");
        if let EntryRecord::Checkpoint(bundle) = &mut entry.record {
            let mut heads = bundle.heads.to_vec();
            heads
                .iter_mut()
                .filter(|head| head.device == device)
                .for_each(&change);
            bundle.heads = Sorted::new(heads).expect("sorted by device");
        }
        entry.to_bytes()
    }));
}

/// A start below the change's entry, whose head of the writer lies above
/// the final head's counter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chain_head_above_the_final_head_stops_the_rewind() {
    let mut pair = two_accounts_past_a_checkpoint().await;
    pair.sync_a.set_checkpoint_wait(NEVER);
    pair.sync_b.set_checkpoint_wait(NEVER);
    demote_the_member(&mut pair).await;
    let _restored = claim_a_restore(&mut pair).await;
    let device = pair.desktop.id();
    tamper_start(&pair, device, |head| head.counter += 5);
    caught(&mut pair, &Lost::FinalHead(Some(device))).await;
}

/// A start that covers the change's entry and lists another head than the
/// final one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_head_that_differs_from_the_final_head_stops_the_rewind() {
    let mut pair = two_accounts_past_a_checkpoint().await;
    let number = pair.sync_a.latest_checkpoint().expect("one").body.number;
    demote_the_member(&mut pair).await;
    for i in 0..8 {
        pair.a.write(&format!("After {i}.md"), b"# After\n");
        rounds(&mut pair).await;
    }
    let latest = pair.sync_a.latest_checkpoint().expect("one").clone();
    assert!(
        latest.body.number > number && latest.body.policy_revision == pair.sync_a.policy_revision(),
        "a checkpoint covers the demotion"
    );
    let _restored = claim_a_restore(&mut pair).await;
    let device = pair.desktop.id();
    tamper_start(&pair, device, |head| {
        head.op_digest = sync_protocol::crypto::hash(b"another head");
    });
    caught(&mut pair, &Lost::FinalHead(Some(device))).await;
}

/// A record the writer signed again at a position below its final head:
/// the digests fix those records, so the chain no longer reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_re_signed_record_below_the_final_head_stops_the_rewind() {
    let mut pair = two_accounts().await;
    demote_the_member(&mut pair).await;
    let _restored = claim_a_restore(&mut pair).await;
    let keys = pair.desktop.credentials.keys.clone();
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.contains("/chain/") {
            return body;
        }
        let mut entry = Entry::from_bytes(&body).expect("a log entry");
        if let EntryRecord::Operation(op) = &mut entry.record {
            let mut signed = op.body.clone();
            signed.delete = !signed.delete;
            *op = Signed::sign(signed, keys.signing());
        }
        entry.to_bytes()
    }));
    let device = pair.desktop.id();
    caught(&mut pair, &Lost::FinalHead(Some(device))).await;
}

/// With the proxy honest again a claim alone costs nothing: the same
/// restore passes every check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_restore_passes_through_an_honest_proxy() {
    let mut pair = two_accounts_past_a_checkpoint().await;
    pair.sync_a.set_checkpoint_wait(NEVER);
    pair.sync_b.set_checkpoint_wait(NEVER);
    demote_the_member(&mut pair).await;
    let _restored = claim_a_restore(&mut pair).await;
    let files = pair.a.files();
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(pair.a.files(), files);
}
