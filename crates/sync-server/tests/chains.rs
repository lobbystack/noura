//! Account chains at seam 4 (§7, §12): `POST /v2/accounts/chain` with the
//! authority cases of `sync-v2-account.json` and the server's own checks,
//! `POST /v2/accounts/chains` with the chains each session may read, and
//! `POST /v2/recovery`.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;
use support::Client;
use sync_key_envelope::v2::{DeviceKeys, genesis_link, next_link, recovery_proof};
use sync_protocol::account::{AccountChain, AccountError};
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{
    ChainAppendRequest, ChainAppendResponse, ChainCursor, ChainStatus, ChainsRequest,
    ChainsResponse, ChallengeResponse, KeysResponse, RecoveryRequest,
};
use sync_protocol::records::{
    AccountHead, AccountLink, Card, LinkKind, PolicyKind, RecoveryProof, Role, Signed,
};
use sync_protocol::types::{Digest, Id, SecretKey, from_hex, to_base64url};
use sync_store::conformance::{World, account, creation};
use sync_store::{Backend, INITIAL_RESTORE_GENERATION, LinkRow, Session, Store, WriteTxn};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

type Shared = Arc<Store<SqliteBackend>>;

async fn store() -> (Store<SqliteBackend>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join(DATABASE_FILE);
    sync_store_sqlite::migrate(&path).await.expect("migrated");
    sync_store_sqlite::store_secret_check(&path, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    let store = Store::open(SqliteBackend::open(&path).await.expect("opened"))
        .await
        .expect("store")
        .with_secret(SecretKey([1; 32]));
    (store, dir)
}

async fn serve(store: Shared) -> String {
    let address = sync_server::spawn(store, "127.0.0.1:0")
        .await
        .expect("serving");
    format!("http://localhost:{}", address.port())
}

/// A session for `device` of `account`, as ticket 47's sign-in will make.
async fn session(store: &Shared, base: &str, session: Session) -> Client {
    let token: [u8; 32] = sync_protocol::crypto::random_array().expect("random");
    store
        .create_session(token, session)
        .await
        .expect("session stored");
    Client::new(base.to_owned(), Some(to_base64url(&token)))
}

fn append_body(links: &[Signed<AccountLink>]) -> Vec<u8> {
    ChainAppendRequest {
        links: List(links.iter().cloned().map(Nested).collect()),
    }
    .to_bytes()
}

async fn append(client: &Client, links: &[Signed<AccountLink>]) -> Result<u64, String> {
    let reply = client.post("/v2/accounts/chain", append_body(links)).await;
    if reply.status == StatusCode::OK {
        Ok(reply.decode::<ChainAppendResponse>().seqno)
    } else {
        Err(reply.error().code.0)
    }
}

async fn read(
    client: &Client,
    cursors: Vec<ChainCursor>,
) -> Result<ChainsResponse, (StatusCode, String)> {
    let body = ChainsRequest {
        cursors: List(cursors),
    }
    .to_bytes();
    let reply = client.post("/v2/accounts/chains", body).await;
    if reply.status == StatusCode::OK {
        Ok(reply.decode())
    } else {
        Err((reply.status, reply.error().code.0))
    }
}

fn current(account: Id, after: u64) -> ChainCursor {
    ChainCursor {
        account,
        genesis: None,
        after,
    }
}

fn head_of(links: &[Signed<AccountLink>]) -> AccountHead {
    let last = links.last().expect("links");
    AccountHead {
        account: last.body.account,
        genesis: links[0].digest(),
        seqno: last.body.seqno,
        head: last.digest(),
    }
}

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/sync-v2-account.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
}

fn fixture_keys(doc: &Value) -> BTreeMap<Id, DeviceKeys> {
    doc["keys"]
        .as_object()
        .expect("keys")
        .values()
        .map(|key| {
            let seed = |name: &str| {
                SecretKey(
                    from_hex(key[name].as_str().unwrap())
                        .unwrap()
                        .try_into()
                        .unwrap(),
                )
            };
            let keys = DeviceKeys::from_seeds(&seed("signing_seed"), seed("hpke_seed"));
            (keys.device(), keys)
        })
        .collect()
}

fn added_card(link: &Signed<AccountLink>) -> Option<&Card> {
    match &link.body.kind {
        LinkKind::Genesis { card }
        | LinkKind::AddDevice { card, .. }
        | LinkKind::RecoverDevice { card } => Some(card),
        _ => None,
    }
}

/// Every `account_chain` case of `sync-v2-account.json`, sent one link at a
/// time from a session of its signer, or for `recover_device` from a
/// session that proved the recovery identity, after a session of the
/// account enrolled each card: the server refuses exactly the failing link
/// with the code §7 gives its error, and keeps the accepted chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_checks_each_link_as_the_account_fixture_does() {
    let doc = fixture();
    let keys = fixture_keys(&doc);
    let links: BTreeMap<String, Signed<AccountLink>> = doc["links"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(digest, hex)| {
            let link = Signed::<AccountLink>::from_bytes(&from_hex(hex.as_str().unwrap()).unwrap())
                .unwrap();
            (digest.clone(), link)
        })
        .collect();
    let mut checked = 0;
    for case in doc["cases"].as_array().unwrap() {
        if case["kind"] != "account_chain" {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let chain: Vec<Signed<AccountLink>> = case["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| links[d.as_str().unwrap()].clone())
            .collect();
        let account = chain[0].body.account;
        let (store, _dir) = store().await;
        let store = Arc::new(store);
        let base = serve(store.clone()).await;
        for card in chain.iter().filter_map(added_card) {
            let _ = store.enroll_card(card.clone()).await;
        }
        // Devices that another chain of the account listed sit in a chain
        // that a reset retired.
        for listed in case["listed_elsewhere"].as_array().unwrap() {
            let device = Id::from_hex(listed.as_str().unwrap()).unwrap();
            let retired = genesis_link(&keys[&device], account);
            store
                .backend()
                .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
                    Box::pin(async move {
                        txn.insert_account_link(
                            account,
                            &LinkRow {
                                genesis: retired.digest(),
                                seqno: 1,
                                link: retired.to_bytes(),
                            },
                            retired.digest(),
                        )
                        .await
                    })
                }))
                .await
                .unwrap();
        }
        let failing: Option<AccountError> = case["expect"]
            .get("error")
            .map(|_| serde_json::from_value(case["expect"].clone()).unwrap());
        let mut sessions: BTreeMap<(Id, Option<Id>), Client> = BTreeMap::new();
        let mut refused = None;
        for link in &chain {
            let key = match (&link.body.kind, added_card(link)) {
                (LinkKind::RecoverDevice { .. }, Some(card)) => {
                    (card.device, Some(link.body.signer))
                }
                _ => (link.body.signer, None),
            };
            if let std::collections::btree_map::Entry::Vacant(slot) = sessions.entry(key) {
                let client = session(
                    &store,
                    &base,
                    Session {
                        account,
                        device: key.0,
                        recovery: key.1,
                    },
                )
                .await;
                slot.insert(client);
            }
            match append(&sessions[&key], std::slice::from_ref(link)).await {
                Ok(seqno) => assert_eq!(seqno, link.body.seqno, "{name}"),
                Err(code) => {
                    refused = Some((link.body.seqno, code));
                    break;
                }
            }
        }
        match failing {
            Some(error) => assert_eq!(
                refused,
                Some((error.seqno(), error.server_code().to_owned())),
                "{name}"
            ),
            None => {
                assert_eq!(refused, None, "{name}");
                let reader = sessions.values().next().unwrap();
                let page = read(reader, vec![current(account, 0)]).await.unwrap();
                let page = &page.pages.0[0];
                let served: Vec<_> = page.links.0.iter().map(|l| l.0.clone()).collect();
                assert_eq!(served, chain, "{name}");
                let verified = AccountChain::verify(&served, &Default::default()).unwrap();
                let active: Vec<String> = verified
                    .active_at(verified.head_seqno())
                    .iter()
                    .map(|s| s.card.device.to_hex())
                    .collect();
                assert_eq!(
                    serde_json::json!(active),
                    case["expect"]["active"],
                    "{name}"
                );
                assert_eq!(
                    page.genesis.to_hex(),
                    case["expect"]["fingerprint"].as_str().unwrap()
                );
            }
        }
        checked += 1;
    }
    assert!(checked >= 20, "{checked} cases");
}

/// An account of `count` devices whose cards their sessions enrolled, its
/// chain started by the first device's genesis through the route.
struct Family {
    account: Id,
    keys: Vec<DeviceKeys>,
    links: Vec<Signed<AccountLink>>,
    clients: Vec<Client>,
}

impl Family {
    async fn new(store: &Shared, base: &str, count: usize) -> Self {
        let account = Id::random().unwrap();
        let mut family = Self {
            account,
            keys: Vec::new(),
            links: Vec::new(),
            clients: Vec::new(),
        };
        for _ in 0..count {
            family.enroll(store, base).await;
        }
        let genesis = genesis_link(&family.keys[0], account);
        append(&family.clients[0], std::slice::from_ref(&genesis))
            .await
            .expect("the genesis");
        family.links.push(genesis);
        for i in 1..count {
            family.add(0, i).await.expect("add_device");
        }
        family
    }

    /// A new device of the account with a session and an enrolled card.
    async fn enroll(&mut self, store: &Shared, base: &str) -> usize {
        let keys = DeviceKeys::generate().unwrap();
        store.enroll_card(keys.card(self.account)).await.unwrap();
        self.clients
            .push(session(store, base, Session::new(self.account, keys.device())).await);
        self.keys.push(keys);
        self.keys.len() - 1
    }

    fn link(&self, signer: usize, kind: LinkKind) -> Signed<AccountLink> {
        next_link(&self.keys[signer], &head_of(&self.links), kind)
    }

    async fn push(&mut self, sender: usize, link: Signed<AccountLink>) -> Result<u64, String> {
        let seqno = append(&self.clients[sender], std::slice::from_ref(&link)).await?;
        self.links.push(link);
        Ok(seqno)
    }

    async fn add(&mut self, signer: usize, device: usize) -> Result<u64, String> {
        let link = self.link(
            signer,
            LinkKind::AddDevice {
                card: Nested(self.keys[device].card(self.account)),
                offer: Digest([9; 32]),
            },
        );
        self.push(signer, link).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn links_come_from_the_signers_session_with_enrolled_cards() {
    let (store, _dir) = store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    let mut family = Family::new(&store, &base, 2).await;

    // Another device's session can't send a link the first one signed.
    let phone = family.enroll(&store, &base).await;
    let link = family.link(
        0,
        LinkKind::AddDevice {
            card: Nested(family.keys[phone].card(family.account)),
            offer: Digest([9; 32]),
        },
    );
    assert_eq!(
        append(&family.clients[1], std::slice::from_ref(&link)).await,
        Err("forbidden".into())
    );
    assert_eq!(family.push(0, link).await, Ok(3));

    // A card that no session of the account enrolled, or enrolled with
    // other bytes, can't join.
    let stranger = DeviceKeys::generate().unwrap();
    let link = family.link(
        0,
        LinkKind::AddDevice {
            card: Nested(stranger.card(family.account)),
            offer: Digest([9; 32]),
        },
    );
    assert_eq!(
        append(&family.clients[0], &[link]).await,
        Err("forbidden".into())
    );
    let other = Id::random().unwrap();
    store.enroll_card(stranger.card(other)).await.unwrap();
    let link = family.link(
        0,
        LinkKind::AddDevice {
            card: Nested(stranger.card(family.account)),
            offer: Digest([9; 32]),
        },
    );
    assert_eq!(
        append(&family.clients[0], &[link]).await,
        Err("forbidden".into())
    );
    // A recovery identity holds no session and needs no enrollment.
    let kit = DeviceKeys::generate().unwrap();
    let link = family.link(
        0,
        LinkKind::AddRecovery {
            card: Nested(kit.card(family.account)),
        },
    );
    assert_eq!(family.push(0, link).await, Ok(4));

    // The account already has a chain: a genesis needs a reset (ticket 60).
    let again = genesis_link(&family.keys[1], family.account);
    assert_eq!(
        append(&family.clients[1], &[again]).await,
        Err("forbidden".into())
    );

    // A link after a stale head: account_conflict.
    let stale = next_link(
        &family.keys[0],
        &head_of(&family.links[..2]),
        LinkKind::Remove {
            device: family.keys[1].device(),
            keep: Sorted::default(),
        },
    );
    assert_eq!(
        append(&family.clients[0], &[stale]).await,
        Err("account_conflict".into())
    );

    // All or none: a valid link followed by a refused one stores neither.
    let first = family.link(
        0,
        LinkKind::Remove {
            device: family.keys[1].device(),
            keep: Sorted::default(),
        },
    );
    let mut after = family.links.clone();
    after.push(first.clone());
    let second = next_link(
        &family.keys[1],
        &head_of(&after),
        LinkKind::Remove {
            device: family.keys[0].device(),
            keep: Sorted::default(),
        },
    );
    assert_eq!(
        append(&family.clients[0], &[first, second]).await,
        Err("forbidden".into())
    );
    let page = read(&family.clients[0], vec![current(family.account, 0)])
        .await
        .unwrap();
    assert_eq!(page.pages.0[0].head_seqno, 4);

    // A link that names another account.
    let mut body = family
        .link(
            0,
            LinkKind::Remove {
                device: family.keys[1].device(),
                keep: Sorted::default(),
            },
        )
        .body;
    body.account = Id::random().unwrap();
    let foreign = Signed::sign(body, family.keys[0].signing());
    assert_eq!(
        append(&family.clients[0], &[foreign]).await,
        Err("forbidden".into())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_device_id_never_enrolls_again_and_a_257th_card_is_refused() {
    let (store, _dir) = store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    let mut family = Family::new(&store, &base, 2).await;
    let removed = family.keys[1].card(family.account);
    let link = family.link(
        0,
        LinkKind::Remove {
            device: removed.device,
            keep: Sorted::default(),
        },
    );
    family.push(0, link).await.unwrap();
    // A device ID that a link removed never enrolls again, with its own
    // bytes or any others, and never rejoins the chain.
    let refused = store.enroll_card(removed.clone()).await;
    assert_eq!(
        sync_store::conformance::refusal(refused).code,
        "id_conflict"
    );
    let refused = store
        .enroll_card(family.keys[1].card(Id::random().unwrap()))
        .await;
    assert_eq!(
        sync_store::conformance::refusal(refused).code,
        "id_conflict"
    );
    let link = family.link(
        0,
        LinkKind::AddDevice {
            card: Nested(removed.clone()),
            offer: Digest([9; 32]),
        },
    );
    assert_eq!(
        append(&family.clients[0], &[link]).await,
        Err("forbidden".into())
    );
    // A card enrolled again byte for byte binds once more while no chain
    // removed it.
    store
        .enroll_card(family.keys[0].card(family.account))
        .await
        .unwrap();

    // 256 active cards at most.
    let (store, _dir3) = self::store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    let mut big = Family::new(&store, &base, 1).await;
    for _ in 1..256 {
        let device = big.enroll(&store, &base).await;
        big.add(0, device).await.expect("room for the card");
    }
    let extra = big.enroll(&store, &base).await;
    assert_eq!(big.add(0, extra).await, Err("forbidden".into()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_session_reads_the_chains_it_may_read() {
    let (store, _dir) = store().await;
    let mut world = World::new(store, 2).await;
    let base = serve(world.store.clone()).await;
    let owner = Client::new(base.clone(), Some(world.devices[0].bearer()));

    // Its own chain, from any cursor.
    let own = read(
        &owner,
        vec![current(world.account, 0), current(world.account, 1)],
    )
    .await
    .unwrap();
    assert_eq!(own.pages.0.len(), 2);
    assert_eq!(own.pages.0[0].links.0.len(), 2);
    assert_eq!(own.pages.0[1].links.0.len(), 1);
    assert_eq!(own.pages.0[0].status, ChainStatus::Current);

    // A member account the policy records: its current chain in full.
    let (member, member_keys, mut member_links) = account(2);
    world
        .store
        .append_account_links(member_links.clone())
        .await
        .unwrap();
    let member_client = session(
        &world.store,
        &base,
        Session::new(member, member_keys[0].device()),
    )
    .await;
    let unrelated = read(&owner, vec![current(member, 0)]).await;
    assert_eq!(unrelated, Err((StatusCode::NOT_FOUND, "not_found".into())));
    let owner_head = head_of(&world.links);
    let add = world.change(
        0,
        vec![owner_head.clone(), head_of(&member_links)],
        PolicyKind::AddMember {
            account: member,
            role: Role::Editor,
            invitation: Id::random().unwrap(),
        },
    );
    world.inject_policy(add, vec![world.domain], None).await;
    // The member's chain grows past the head the workspace records.
    let third = DeviceKeys::generate().unwrap();
    let link = next_link(
        &member_keys[0],
        &head_of(&member_links),
        LinkKind::AddDevice {
            card: Nested(third.card(member)),
            offer: Digest([9; 32]),
        },
    );
    world
        .store
        .append_account_links(vec![link.clone()])
        .await
        .unwrap();
    member_links.push(link);
    let page = read(&owner, vec![current(member, 0)]).await.unwrap();
    assert_eq!(page.pages.0[0].head_seqno, 3);
    assert_eq!(page.pages.0[0].links.0.len(), 3);
    // The member reads the owner's chain.
    let page = read(&member_client, vec![current(world.account, 1)])
        .await
        .unwrap();
    assert_eq!(page.pages.0[0].head_seqno, 2);
    assert_eq!(page.pages.0[0].links.0.len(), 1);

    // Once remove_member removes it, the policy still records the former
    // member's chain up to the latest head recorded on it.
    let remove = world.change(
        0,
        vec![owner_head.clone()],
        PolicyKind::RemoveMember { account: member },
    );
    world.inject_policy(remove, vec![world.domain], None).await;
    let page = read(&owner, vec![current(member, 0)]).await.unwrap();
    assert_eq!(page.pages.0[0].head_seqno, 2);
    // The owner's chain grows, and a later change records it; the removed
    // member reads the owner's chain only up to its removal.
    let head = world
        .append_link(
            0,
            LinkKind::Remove {
                device: world.devices[1].id(),
                keep: Sorted::default(),
            },
        )
        .await;
    let heads = world.change(0, vec![head], PolicyKind::AccountHeads);
    world.inject_policy(heads, vec![world.domain], None).await;
    let page = read(&member_client, vec![current(world.account, 0)])
        .await
        .unwrap();
    assert_eq!(page.pages.0[0].head_seqno, 2);
    assert_eq!(page.pages.0[0].links.0.len(), 2);
    let page = read(&owner, vec![current(world.account, 0)]).await.unwrap();
    assert_eq!(page.pages.0[0].head_seqno, 3);

    // A chain the store doesn't hold.
    assert_eq!(
        read(&owner, vec![current(Id::random().unwrap(), 0)]).await,
        Err((StatusCode::NOT_FOUND, "not_found".into()))
    );
    // Pages follow request order, each from its cursor.
    let both = read(&owner, vec![current(member, 2), current(world.account, 0)])
        .await
        .unwrap();
    assert_eq!(both.pages.0[0].account, member);
    assert_eq!(both.pages.0[0].links.0.len(), 0);
    assert_eq!(both.pages.0[1].links.0.len(), 3);
}

/// The `chain_read` cases of `sync-v2-policy.json`, served by the store:
/// its account chains and its workspace's policy go into the store, and
/// each case's reader device reads the target chain to the bound the
/// fixture pins, or gets `not_found`. For a case with `approval_request`,
/// the target account posts an approval request naming an invitation to
/// the workspace first, which expires before the next case.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_serves_the_chain_reads_of_the_policy_fixture() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/sync-v2-policy.json"
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases: Vec<&Value> = doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "chain_read")
        .collect();
    assert!(cases.len() >= 8);
    assert!(cases.iter().any(|c| !c["approval_request"].is_null()));
    let first = cases[0];
    let (store, _dir) = store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    for chain in first["account_chains"].as_array().unwrap() {
        let links: Vec<Signed<AccountLink>> = doc["chains"][chain.as_str().unwrap()]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let hex = doc["links"][d.as_str().unwrap()].as_str().unwrap();
                Signed::<AccountLink>::from_bytes(&from_hex(hex).unwrap()).unwrap()
            })
            .collect();
        store.append_account_links(links).await.unwrap();
    }
    let changes: Vec<Signed<sync_protocol::records::PolicyChange>> = first["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Signed::from_bytes(&from_hex(c.as_str().unwrap()).unwrap()).unwrap())
        .collect();
    let workspace = changes[0].body.workspace;
    store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                let workspace = changes[0].body.workspace;
                let last = changes.last().unwrap();
                txn.insert_workspace(
                    workspace,
                    &sync_store::WorkspaceRow {
                        owner: changes[0].body.heads.iter().next().unwrap().account,
                        creation_digest: Digest::ZERO,
                        creation_response: Vec::new(),
                        policy_revision: last.body.revision,
                        policy_head: last.digest(),
                    },
                )
                .await?;
                for change in &changes {
                    txn.insert_policy_change(
                        workspace,
                        change.body.revision,
                        change.digest(),
                        &change.to_bytes(),
                    )
                    .await?;
                    let accounts: Vec<Id> = change.body.heads.iter().map(|h| h.account).collect();
                    txn.record_accounts(workspace, &accounts).await?;
                }
                Ok(())
            })
        }))
        .await
        .unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let reader = session(
            &store,
            &base,
            Session::new(
                Id::from_hex(case["reader_account"].as_str().unwrap()).unwrap(),
                Id::from_hex(case["reader"].as_str().unwrap()).unwrap(),
            ),
        )
        .await;
        let account = Id::from_hex(case["account"].as_str().unwrap()).unwrap();
        let request = &case["approval_request"];
        if !request.is_null() {
            let invitation = Id::from_hex(request["invitation"].as_str().unwrap()).unwrap();
            store
                .store_approval_request(
                    Session::new(account, Id::random().unwrap()),
                    Digest(sync_protocol::crypto::random_array().unwrap()),
                    Some((invitation, workspace)),
                )
                .await
                .unwrap();
        }
        let cursor = ChainCursor {
            account,
            genesis: Some(Digest::from_hex(case["genesis"].as_str().unwrap()).unwrap()),
            after: 0,
        };
        let served = read(&reader, vec![cursor]).await;
        if !request.is_null() {
            store
                .clock()
                .advance(std::time::Duration::from_secs(sync_store::SIGN_IN_TTL));
        }
        match case["expect"].as_u64() {
            Some(bound) => {
                let page = &served.unwrap_or_else(|e| panic!("{name}: {e:?}")).pages.0[0];
                assert_eq!(page.head_seqno, bound, "{name}");
                assert_eq!(page.links.0.len() as u64, bound, "{name}");
            }
            None => assert_eq!(
                served.map(|_| ()),
                Err((StatusCode::NOT_FOUND, "not_found".into())),
                "{name}"
            ),
        }
    }
}

/// A chain longer than one response: each page stays within 1 MiB, and
/// the reader asks again from the last seqno it got until it reaches
/// `head_seqno`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_chain_reads_in_pages_of_at_most_one_mebibyte() {
    let (store, _dir) = store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    let account = Id::random().unwrap();
    let laptop = DeviceKeys::generate().unwrap();
    let mut links = vec![genesis_link(&laptop, account)];
    for _ in 0..700 {
        let card = DeviceKeys::generate().unwrap().card(account);
        let device = card.device;
        let add = next_link(
            &laptop,
            &head_of(&links),
            LinkKind::AddDevice {
                card: Nested(card),
                offer: Digest([9; 32]),
            },
        );
        links.push(add);
        let remove = next_link(
            &laptop,
            &head_of(&links),
            LinkKind::Remove {
                device,
                keep: Sorted::default(),
            },
        );
        links.push(remove);
    }
    let total: usize = links.iter().map(|l| l.to_bytes().len()).sum();
    assert!(total > 1 << 20, "{total} bytes");
    store.append_account_links(links.clone()).await.unwrap();
    let client = session(&store, &base, Session::new(account, laptop.device())).await;
    let mut served: Vec<Signed<AccountLink>> = Vec::new();
    let mut pages = 0;
    loop {
        let body = ChainsRequest {
            cursors: List(vec![current(account, served.len() as u64)]),
        }
        .to_bytes();
        let reply = client.post("/v2/accounts/chains", body).await;
        assert!(reply.body.len() <= 1 << 20, "{} bytes", reply.body.len());
        let response: ChainsResponse = reply.decode();
        let page = &response.pages.0[0];
        assert_eq!(page.head_seqno, links.len() as u64);
        assert!(!page.links.0.is_empty());
        served.extend(page.links.0.iter().map(|l| l.0.clone()));
        pages += 1;
        if served.len() as u64 == page.head_seqno {
            break;
        }
    }
    assert!(pages >= 2);
    assert_eq!(served, links);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_bodies_decode_as_the_encoding_fixture_pins() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/sync-v2-encoding.json"
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let record = |name: &str| {
        let r = doc["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name}"));
        from_hex(r["hex"].as_str().unwrap()).unwrap()
    };
    ChainsResponse::from_bytes(&record("chains_response")).expect("the fixture's response");
    let (store, _dir) = store().await;
    let world = World::new(store, 1).await;
    let base = serve(world.store.clone()).await;
    let owner = Client::new(base, Some(world.devices[0].bearer()));
    // The fixture's request names chains this server doesn't hold.
    let reply = owner
        .post("/v2/accounts/chains", record("chains_request"))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let mut truncated = record("chains_request");
    truncated.pop();
    let reply = owner.post("/v2/accounts/chains", truncated).await;
    assert_eq!(reply.error().code.as_str(), "bad_request");
    // The fixture's append request names an account other than the
    // session's.
    let reply = owner
        .post("/v2/accounts/chain", record("chain_append_request"))
        .await;
    assert_eq!(reply.error().code.as_str(), "forbidden");
    let ok = owner
        .post(
            "/v2/accounts/chains",
            ChainsRequest {
                cursors: List(vec![current(world.account, 0)]),
            }
            .to_bytes(),
        )
        .await;
    let body = ok.decode::<ChainsResponse>().to_bytes();
    assert_eq!(ChainsResponse::from_bytes(&body).unwrap().to_bytes(), body);
}

/// An account with a laptop and a recovery identity, a workspace whose
/// genesis records both, and a new phone that enrolled and signed in.
struct Recovery {
    store: Shared,
    base: String,
    family: Family,
    kit: DeviceKeys,
    workspace: Id,
    phone: usize,
}

async fn recovery() -> (Recovery, tempfile::TempDir) {
    let (store, dir) = store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    let mut family = Family::new(&store, &base, 1).await;
    let kit = DeviceKeys::generate().unwrap();
    let link = family.link(
        0,
        LinkKind::AddRecovery {
            card: Nested(kit.card(family.account)),
        },
    );
    family.push(0, link).await.unwrap();
    let cards = vec![
        family.keys[0].card(family.account),
        kit.card(family.account),
    ];
    let (body, genesis, _) = creation(&family.keys[0], &family.links, &cards);
    store
        .create_workspace(Session::new(family.account, family.keys[0].device()), body)
        .await
        .expect("the workspace");
    let phone = family.enroll(&store, &base).await;
    (
        Recovery {
            store,
            base,
            family,
            kit,
            workspace: genesis.body.workspace,
            phone,
        },
        dir,
    )
}

impl Recovery {
    fn client(&self) -> &Client {
        &self.family.clients[self.phone]
    }

    async fn challenge(&self) -> sync_protocol::types::Key {
        let reply = self
            .client()
            .post("/v2/devices/challenge", Vec::new())
            .await;
        let challenge: ChallengeResponse = reply.decode();
        assert_eq!(challenge.expires_in, 600);
        challenge.challenge
    }

    fn origin(&self) -> &str {
        &self.base
    }

    fn phone_card(&self) -> Card {
        self.family.keys[self.phone].card(self.family.account)
    }

    async fn prove(&self, proof: Signed<RecoveryProof>) -> Result<(), String> {
        let body = RecoveryRequest {
            recovery_proof: Nested(proof),
        }
        .to_bytes();
        let reply = self.client().post("/v2/recovery", body).await;
        if reply.status == StatusCode::OK {
            Ok(())
        } else {
            Err(reply.error().code.0)
        }
    }

    async fn kit_keys(&self) -> Result<KeysResponse, String> {
        let path = format!(
            "/v2/workspaces/{}/keys?after=0&recipient={}",
            self.workspace.to_base64url(),
            self.kit.device().to_base64url()
        );
        let reply = self.client().get(&path).await;
        if reply.status == StatusCode::OK {
            Ok(reply.decode())
        } else {
            Err(reply.error().code.0)
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recovery_proof_lets_the_session_act_for_the_recovery_identity() {
    let (r, _dir) = recovery().await;
    let account = r.family.account;

    // Without a proof, the session reads nothing of the kit's and can't
    // send the kit's recover_device.
    assert_eq!(r.kit_keys().await.unwrap_err(), "forbidden");
    let recover = next_link(
        &r.kit,
        &head_of(&r.family.links),
        LinkKind::RecoverDevice {
            card: Nested(r.phone_card()),
        },
    );
    assert_eq!(
        append(r.client(), std::slice::from_ref(&recover)).await,
        Err("forbidden".into())
    );

    // Refused proofs.
    let challenge = r.challenge().await;
    let other_origin = recovery_proof(
        &r.kit,
        "https://elsewhere.example",
        &r.phone_card(),
        challenge,
    );
    assert_eq!(r.prove(other_origin).await, Err("origin_mismatch".into()));
    let not_identity = recovery_proof(&r.family.keys[0], r.origin(), &r.phone_card(), challenge);
    assert_eq!(r.prove(not_identity).await, Err("forbidden".into()));
    let mut forged = recovery_proof(&r.kit, r.origin(), &r.phone_card(), challenge);
    forged.sig.0[63] ^= 1;
    assert_eq!(r.prove(forged).await, Err("signature_invalid".into()));
    let stranger = DeviceKeys::generate().unwrap().card(account);
    let wrong_card = recovery_proof(&r.kit, r.origin(), &stranger, challenge);
    assert_eq!(r.prove(wrong_card).await, Err("forbidden".into()));
    let unknown = recovery_proof(
        &r.kit,
        r.origin(),
        &r.phone_card(),
        sync_protocol::types::Key([3; 32]),
    );
    assert_eq!(r.prove(unknown).await, Err("challenge_invalid".into()));
    let stale = r.challenge().await;
    r.store.clock().advance(Duration::from_secs(601));
    let expired = recovery_proof(&r.kit, r.origin(), &r.phone_card(), stale);
    assert_eq!(r.prove(expired).await, Err("challenge_invalid".into()));

    // A current proof: once, for one challenge.
    let challenge = r.challenge().await;
    let proof = recovery_proof(&r.kit, r.origin(), &r.phone_card(), challenge);
    assert_eq!(r.prove(proof.clone()).await, Ok(()));
    assert_eq!(r.prove(proof).await, Err("challenge_invalid".into()));
    let keys = r.kit_keys().await.expect("the kit's envelopes");
    assert!(keys.entries.0.iter().any(|e| matches!(
        &e.record,
        sync_protocol::messages::KeyRecord::Envelope(env) if env.body.recipient == r.kit.device()
    )));

    // The proven session sends the kit's recover_device.
    assert_eq!(
        append(r.client(), std::slice::from_ref(&recover)).await,
        Ok(3)
    );
}

/// Review of `0eef7d5`, with the recovery of `sync-v2-recovery.json`, in
/// step 5's order (§12): the recovered device records the head after
/// `recover_device` in the first of the account's two workspaces, which it
/// enters, then its `remove` link commits before it enters the last one,
/// and the proven session still reads the recovery identity's envelopes
/// there (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_recovery_fixtures_removal_commits_before_the_device_enters_its_last_workspace() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/sync-v2-recovery.json"
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let keys = |name: &str| {
        let entry = &doc["keys"][name];
        let seed = |field: &str| {
            SecretKey(
                from_hex(entry[field].as_str().unwrap())
                    .unwrap()
                    .try_into()
                    .unwrap(),
            )
        };
        DeviceKeys::from_seeds(&seed("signing_seed"), seed("hpke_seed"))
    };
    let (laptop, kit, phone) = (
        keys("dora laptop"),
        keys("dora recovery kit"),
        keys("dora phone"),
    );
    let case = doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "recovery_order")
        .unwrap();
    let links: Vec<Signed<AccountLink>> = case["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let hex = doc["links"][d.as_str().unwrap()].as_str().unwrap();
            Signed::<AccountLink>::from_bytes(&from_hex(hex).unwrap()).unwrap()
        })
        .collect();
    let account = links[0].body.account;

    let (store, _dir) = store().await;
    let store = Arc::new(store);
    let base = serve(store.clone()).await;
    store.enroll_card(laptop.card(account)).await.unwrap();
    store.enroll_card(phone.card(account)).await.unwrap();
    let laptop_client = session(&store, &base, Session::new(account, laptop.device())).await;
    assert_eq!(append(&laptop_client, &links[..2]).await, Ok(2));
    let cards = vec![laptop.card(account), kit.card(account)];
    let mut workspaces = Vec::new();
    for _ in 0..2 {
        let (body, genesis, _) = creation(&laptop, &links[..2], &cards);
        store
            .create_workspace(Session::new(account, laptop.device()), body)
            .await
            .unwrap();
        workspaces.push(genesis);
    }

    let (phone_token, phone_session) = {
        let token: [u8; 32] = sync_protocol::crypto::random_array().unwrap();
        store
            .create_session(token, Session::new(account, phone.device()))
            .await
            .unwrap();
        (token, Client::new(base.clone(), Some(to_base64url(&token))))
    };
    let _ = phone_token;
    let challenge: ChallengeResponse = phone_session
        .post("/v2/devices/challenge", Vec::new())
        .await
        .decode();
    let proof = recovery_proof(&kit, &base, &phone.card(account), challenge.challenge);
    let reply = phone_session
        .post(
            "/v2/recovery",
            RecoveryRequest {
                recovery_proof: Nested(proof),
            }
            .to_bytes(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(append(&phone_session, &links[2..3]).await, Ok(3));
    // The phone records the head after recover_device in the first
    // workspace, as its account_heads push will, and so enters it.
    let first = &workspaces[0];
    let PolicyKind::Genesis { workspace_domain } = first.body.kind else {
        unreachable!("a workspace genesis");
    };
    let heads = Signed::sign(
        sync_protocol::records::PolicyChange {
            workspace: first.body.workspace,
            revision: 2,
            previous: first.digest(),
            signer: phone.device(),
            heads: Sorted::new(vec![head_of(&links[..3])]).unwrap(),
            kind: PolicyKind::AccountHeads,
        },
        phone.signing(),
    );
    sync_store::conformance::inject_change(&store, heads, vec![workspace_domain], None)
        .await
        .unwrap();
    let path = |workspace: &Signed<sync_protocol::records::PolicyChange>, recipient: Id| {
        format!(
            "/v2/workspaces/{}/keys?after=0&recipient={}",
            workspace.body.workspace.to_base64url(),
            recipient.to_base64url()
        )
    };
    let policy: sync_protocol::messages::PolicyResponse = phone_session
        .get(&format!(
            "/v2/workspaces/{}/policy?after=0",
            first.body.workspace.to_base64url()
        ))
        .await
        .decode();
    let changes: Vec<_> = policy.changes.0.into_iter().map(|c| c.0).collect();
    let mut chains = sync_protocol::policy::AccountChains::new();
    chains.insert(AccountChain::verify(&links[..3], &Default::default()).unwrap());
    let verified = sync_protocol::policy::PolicyChain::verify(&changes, &chains).unwrap();
    assert!(
        verified.head_snapshot().is_device(&phone.device()),
        "the phone is active in the first workspace"
    );
    // The remove links commit before the phone enters the last workspace.
    assert_eq!(append(&phone_session, &links[3..4]).await, Ok(4));
    let last = &workspaces[1];
    let keys: KeysResponse = phone_session.get(&path(last, kit.device())).await.decode();
    assert!(
        !keys.entries.0.is_empty(),
        "the proven session reads the kit's envelopes in the last workspace"
    );
    assert_eq!(
        case["expect"]["proven_session_reads_the_identitys_envelopes_until_the_recovered_card_joins"],
        true
    );
}

/// Review of `0eef7d5`: a recovery identity's proven session lasts until
/// the recovered device is active in each workspace, so the `remove` links
/// that take the identity down can commit first (§7, §12).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_proven_session_outlasts_the_identitys_removal() {
    let (mut r, _dir) = recovery().await;
    let challenge = r.challenge().await;
    let proof = recovery_proof(&r.kit, r.origin(), &r.phone_card(), challenge);
    r.prove(proof).await.unwrap();
    let recover = next_link(
        &r.kit,
        &head_of(&r.family.links),
        LinkKind::RecoverDevice {
            card: Nested(r.phone_card()),
        },
    );
    let phone = r.phone;
    r.family.push(phone, recover).await.unwrap();
    // Step 5: the phone removes every card whose parent isn't active: the
    // laptop, and with it the kit the laptop added.
    let remove = r.family.link(
        phone,
        LinkKind::Remove {
            device: r.family.keys[0].device(),
            keep: Sorted::default(),
        },
    );
    r.family.push(phone, remove).await.unwrap();
    let chain = AccountChain::verify(&r.family.links, &Default::default()).unwrap();
    assert_eq!(chain.active_at(chain.head_seqno()).len(), 1);
    // The workspace still records the head where the kit reads, and the
    // phone hasn't entered it: the proven session still reads the kit's
    // envelopes there.
    r.kit_keys().await.expect("the proven session reads on");
    // A new proof for the removed identity fails.
    let challenge = r.challenge().await;
    let proof = recovery_proof(&r.kit, r.origin(), &r.phone_card(), challenge);
    assert_eq!(r.prove(proof).await, Err("forbidden".into()));
}
