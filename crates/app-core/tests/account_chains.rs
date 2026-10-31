//! Account chains on a device (docs/workspace-format/sync-v2.md §7, §8,
//! §15): the heads it keeps, under a keyed hash in its application data and
//! with its own-account head in the credential store; the kept-head cases
//! of `sync-v2-account.json`; rollback and rebuild; and, at seam 4 behind a
//! tampering proxy, a server that forks a chain at one seqno, cuts it back
//! below a kept head, records a head in a workspace off the kept chain,
//! brings back a removed card or reuses a device ID under another card,
//! each caught with `sync_account_forked` or `sync_account_unauthorized`.

mod sync_support;

use std::collections::BTreeSet;
use std::sync::Arc;

use app_core::sync::{Anchor, KeptHeads, SyncError};
use serde_json::Value;
use sync_key_envelope::v2::{DeviceKeys, next_link};
use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{ChainsResponse, PolicyEntry, PolicyResponse};
use sync_protocol::policy::{AccountChains, PolicyChain};
use sync_protocol::records::{
    AccountHead, AccountLink, LinkKind, PolicyChange, PolicyKind, Role, Signed,
};
use sync_protocol::types::{Digest, Id, from_hex};
use sync_store::{Backend, WriteTxn};
use sync_support::{Account, MemorySecrets, Proxy, Server, head_of};

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/sync-v2-account.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn links_of(doc: &Value, digests: &Value) -> Vec<Signed<AccountLink>> {
    digests
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let hex = doc["links"][d.as_str().unwrap()].as_str().unwrap();
            Signed::<AccountLink>::from_bytes(&from_hex(hex).unwrap()).unwrap()
        })
        .collect()
}

/// Every `kept_head` and `kept_head_extend` case of `sync-v2-account.json`,
/// through the heads a device keeps: a served chain that doesn't extend the
/// kept head reports `sync_account_forked`, a later link that breaks a rule
/// `sync_account_unauthorized`, and either stops the workspaces the device
/// shares with the account, or every workspace for its own.
#[test]
fn kept_heads_reach_each_fixture_cases_outcome() {
    let doc = fixture();
    let mut checked = 0;
    for case in doc["cases"].as_array().unwrap() {
        let kind = case["kind"].as_str().unwrap();
        if kind != "kept_head" && kind != "kept_head_extend" {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let served = links_of(&doc, &case["links"]);
        let account = served[0].body.account;
        let own_account = case["own_account"].as_bool().unwrap_or(false);
        let own = if own_account {
            account
        } else {
            Id::random().unwrap()
        };
        let kept = AccountHead {
            account,
            genesis: Digest::from_hex(case["kept"]["genesis"].as_str().unwrap()).unwrap(),
            seqno: case["kept"]["seqno"].as_u64().unwrap(),
            head: Digest::from_hex(case["kept"]["head"].as_str().unwrap()).unwrap(),
        };
        let dir = tempfile::tempdir().unwrap();
        let secrets = MemorySecrets::default();
        let device = Id::random().unwrap();
        let mut heads =
            KeptHeads::rebuild(dir.path(), own, device, [kept.clone()], [], &secrets).unwrap();
        let mut outcome = heads.accept(account, kept.genesis, &served, &BTreeSet::new(), &secrets);
        if let (Ok(chain), Some(_)) = (&outcome, case["recorded"].as_object()) {
            let recorded: AccountHead = serde_json::from_value(case["recorded"].clone()).unwrap();
            outcome = heads
                .check_recorded(chain, &recorded, &secrets)
                .map(|()| chain.clone());
        }
        let expect = match kind {
            "kept_head" => case["expect"].as_str().unwrap().to_owned(),
            _ => case["expect"]["outcome"].as_str().unwrap().to_owned(),
        };
        match outcome {
            Ok(chain) => {
                assert_eq!(expect, "ok", "{name}");
                if let Some(seqno) = case["expect"].get("head_seqno").and_then(Value::as_u64) {
                    assert_eq!(chain.head_seqno(), seqno, "{name}");
                }
                assert_eq!(
                    heads.kept(&account, &kept.genesis).unwrap().seqno,
                    chain.head_seqno()
                );
                assert_eq!(heads.stopped([account]), None);
            }
            Err(SyncError::Check(code)) => {
                assert_eq!(code, expect, "{name}");
                // The account's workspaces stop; for the device's own
                // account, every workspace does.
                assert_eq!(heads.stopped([account]), Some(code), "{name}");
                let every = heads.stopped([Id::random().unwrap()]).is_some();
                assert_eq!(every, own_account, "{name}");
                if let Some(stops) = case["expect"].get("stops").and_then(Value::as_str) {
                    assert_eq!(stops == "every_workspace", own_account, "{name}");
                }
                // The kept head never moves back.
                assert!(heads.kept(&account, &kept.genesis).unwrap().seqno >= kept.seqno);
            }
            Err(other) => panic!("{name}: {other:?}"),
        }
        checked += 1;
    }
    assert!(checked >= 11, "{checked} cases");
}

#[test]
fn a_rolled_back_heads_file_reports_a_journal_rollback_and_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::default();
    let keys = DeviceKeys::generate().unwrap();
    let account = Id::random().unwrap();
    let genesis = sync_key_envelope::v2::genesis_link(&keys, account);
    let reference = head_of(std::slice::from_ref(&genesis));
    let mut heads = KeptHeads::open(dir.path(), account, keys.device(), &secrets).unwrap();
    let one = AccountChain::verify(std::slice::from_ref(&genesis), &BTreeSet::new()).unwrap();
    heads
        .enter(&one, Anchor::Own(&reference), &secrets)
        .unwrap();
    let file = dir
        .path()
        .join(app_core::sync::account_heads::KEPT_HEADS_FILE);
    let older = std::fs::read(&file).unwrap();

    // The chain grows, and the device keeps the new head.
    let second = DeviceKeys::generate().unwrap();
    let link = next_link(
        &keys,
        &reference,
        LinkKind::AddDevice {
            card: Nested(second.card(account)),
            offer: Digest([9; 32]),
        },
    );
    let links = vec![genesis.clone(), link];
    heads
        .accept(
            account,
            genesis.digest(),
            &links,
            &BTreeSet::new(),
            &secrets,
        )
        .unwrap();
    drop(heads);
    KeptHeads::open(dir.path(), account, keys.device(), &secrets).expect("current heads open");

    // A restored older file: its own-account head lies below the one the
    // credential store keeps.
    std::fs::write(&file, &older).unwrap();
    let refused = KeptHeads::open(dir.path(), account, keys.device(), &secrets);
    assert!(matches!(
        refused,
        Err(SyncError::Check("sync_journal_rollback"))
    ));
    // A file whose keyed hash fails is refused too.
    let mut tampered = std::fs::read(&file).unwrap();
    let last = tampered.len() - 2;
    tampered[last] ^= 1;
    std::fs::write(&file, &tampered).unwrap();
    let refused = KeptHeads::open(dir.path(), account, keys.device(), &secrets);
    assert!(matches!(
        refused,
        Err(SyncError::Check("sync_journal_rollback"))
    ));

    // The device rebuilds the heads from the heads its workspaces' logs
    // record, which never fall below the credential store's own head.
    let recorded = [reference.clone()];
    let rebuilt =
        KeptHeads::rebuild(dir.path(), account, keys.device(), recorded, [], &secrets).unwrap();
    assert_eq!(rebuilt.kept(&account, &genesis.digest()).unwrap().seqno, 2);
    drop(rebuilt);
    KeptHeads::open(dir.path(), account, keys.device(), &secrets).expect("rebuilt heads open");
}

/// A chain under a genesis the device doesn't keep enters only through a
/// verified log that names it in its genesis, an `add_member` or an
/// `accept_reset`, or for its own account through its reference head; and
/// a device that keeps two current chains of one account alerts you.
#[test]
fn a_new_chain_enters_only_through_an_anchor() {
    let owner_keys = DeviceKeys::generate().unwrap();
    let owner = Id::random().unwrap();
    let owner_genesis = sync_key_envelope::v2::genesis_link(&owner_keys, owner);
    let member_keys = DeviceKeys::generate().unwrap();
    let member = Id::random().unwrap();
    let member_genesis = sync_key_envelope::v2::genesis_link(&member_keys, member);
    let owner_chain =
        AccountChain::verify(std::slice::from_ref(&owner_genesis), &BTreeSet::new()).unwrap();
    let member_chain =
        AccountChain::verify(std::slice::from_ref(&member_genesis), &BTreeSet::new()).unwrap();
    let mut chains = AccountChains::new();
    chains.insert(owner_chain.clone());
    chains.insert(member_chain.clone());
    let workspace = Id::random().unwrap();
    let genesis = Signed::sign(
        PolicyChange {
            workspace,
            revision: 1,
            previous: Digest::ZERO,
            signer: owner_keys.device(),
            heads: Sorted::new(vec![owner_chain.head()]).unwrap(),
            kind: PolicyKind::Genesis {
                workspace_domain: Id::random().unwrap(),
            },
        },
        owner_keys.signing(),
    );
    let only_genesis = PolicyChain::verify(std::slice::from_ref(&genesis), &chains).unwrap();
    let add = Signed::sign(
        PolicyChange {
            workspace,
            revision: 2,
            previous: genesis.digest(),
            signer: owner_keys.device(),
            heads: Sorted::new(vec![owner_chain.head(), member_chain.head()]).unwrap(),
            kind: PolicyKind::AddMember {
                account: member,
                role: Role::Editor,
                invitation: Id::random().unwrap(),
            },
        },
        owner_keys.signing(),
    );
    let with_member = PolicyChain::verify(&[genesis.clone(), add], &chains).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::default();
    let mut heads = KeptHeads::open(dir.path(), owner, owner_keys.device(), &secrets).unwrap();
    // A log that doesn't name the member's chain where it may enter.
    assert!(matches!(
        heads.enter(&member_chain, Anchor::Workspace(&only_genesis), &secrets),
        Err(SyncError::Check("sync_account_unauthorized"))
    ));
    heads
        .enter(&member_chain, Anchor::Workspace(&with_member), &secrets)
        .unwrap();
    assert_eq!(heads.kept_of(&member), vec![member_chain.head()]);
    // The own account's chain enters through the head it verified from,
    // and never through another account's chain.
    let stranger = head_of(std::slice::from_ref(&member_genesis));
    assert!(
        heads
            .enter(&owner_chain, Anchor::Own(&stranger), &secrets)
            .is_err()
    );
    heads
        .enter(&owner_chain, Anchor::Own(&owner_chain.head()), &secrets)
        .unwrap();

    // Two workspaces naming different chains of one account.
    assert!(heads.two_current_chains().is_empty());
    heads
        .name(workspace, member, member_chain.genesis())
        .unwrap();
    heads
        .name(Id::random().unwrap(), member, Digest([5; 32]))
        .unwrap();
    assert_eq!(heads.two_current_chains(), vec![member]);

    // Heads rebuilt from the workspaces' logs name each member's chain
    // again, so the alert survives a rollback.
    let other_keys = DeviceKeys::generate().unwrap();
    let other_genesis = sync_key_envelope::v2::genesis_link(&other_keys, member);
    let other_chain =
        AccountChain::verify(std::slice::from_ref(&other_genesis), &BTreeSet::new()).unwrap();
    chains.insert(other_chain.clone());
    let second = Id::random().unwrap();
    let second_genesis = Signed::sign(
        PolicyChange {
            workspace: second,
            revision: 1,
            previous: Digest::ZERO,
            signer: owner_keys.device(),
            heads: Sorted::new(vec![owner_chain.head()]).unwrap(),
            kind: PolicyKind::Genesis {
                workspace_domain: Id::random().unwrap(),
            },
        },
        owner_keys.signing(),
    );
    let second_add = Signed::sign(
        PolicyChange {
            workspace: second,
            revision: 2,
            previous: second_genesis.digest(),
            signer: owner_keys.device(),
            heads: Sorted::new(vec![owner_chain.head(), other_chain.head()]).unwrap(),
            kind: PolicyKind::AddMember {
                account: member,
                role: Role::Editor,
                invitation: Id::random().unwrap(),
            },
        },
        owner_keys.signing(),
    );
    let second_policy = PolicyChain::verify(&[second_genesis, second_add], &chains).unwrap();
    drop(heads);
    let rebuilt = KeptHeads::rebuild_from(
        dir.path(),
        owner,
        owner_keys.device(),
        [&with_member, &second_policy],
        &secrets,
    )
    .unwrap();
    assert_eq!(rebuilt.two_current_chains(), vec![member]);
    assert_eq!(rebuilt.kept_of(&member).len(), 2);
}

/// Links served for one chain judge only that chain: links of another
/// account or under another genesis than the one asked for don't extend
/// the kept head, and move no other account's head.
#[test]
fn served_links_of_another_chain_extend_no_kept_head() {
    let keys = DeviceKeys::generate().unwrap();
    let account = Id::random().unwrap();
    let genesis = sync_key_envelope::v2::genesis_link(&keys, account);
    let stranger_keys = DeviceKeys::generate().unwrap();
    let stranger = Id::random().unwrap();
    let stranger_genesis = sync_key_envelope::v2::genesis_link(&stranger_keys, stranger);
    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::default();
    let kept = [
        head_of(std::slice::from_ref(&genesis)),
        head_of(std::slice::from_ref(&stranger_genesis)),
    ];
    let mut heads = KeptHeads::rebuild(
        dir.path(),
        Id::random().unwrap(),
        Id::random().unwrap(),
        kept.clone(),
        [],
        &secrets,
    )
    .unwrap();
    let grown = next_link(
        &stranger_keys,
        &kept[1],
        LinkKind::AddDevice {
            card: Nested(DeviceKeys::generate().unwrap().card(stranger)),
            offer: Digest([9; 32]),
        },
    );
    let served = [stranger_genesis.clone(), grown];
    let refused = heads.accept(
        account,
        genesis.digest(),
        &served,
        &BTreeSet::new(),
        &secrets,
    );
    assert!(matches!(
        refused,
        Err(SyncError::Check("sync_account_forked"))
    ));
    assert_eq!(heads.stopped([account]), Some("sync_account_forked"));
    assert_eq!(heads.kept_of(&stranger), vec![kept[1].clone()]);
    assert_eq!(heads.stopped([stranger]), None);
}

/// Commits a policy change into the store straight, with its entry in the
/// workspace domain, as a push of that change will once tickets 47 and 50
/// add policy pushes.
async fn inject(server: &Server, domain: Id, change: Signed<PolicyChange>) {
    let workspace = change.body.workspace;
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                let revision = change.body.revision;
                let digest = change.digest();
                txn.insert_policy_change(workspace, revision, digest, &change.to_bytes())
                    .await?;
                txn.set_policy_head(workspace, revision, digest).await?;
                let accounts: Vec<Id> = change.body.heads.iter().map(|h| h.account).collect();
                txn.record_accounts(workspace, &accounts).await?;
                let entry = PolicyEntry {
                    revision,
                    head: digest,
                };
                txn.append_entry(workspace, domain, 9, &entry.to_bytes())
                    .await?;
                Ok(())
            })
        }))
        .await
        .expect("the change is stored");
}

/// Wraps the workspace domain's epoch 1 key, which the device at `from`
/// unwraps from its envelope, for the device at `to`, and stores the
/// envelope, as the push after an `account_heads` change will.
async fn grant(
    server: &Server,
    account: &Account,
    from: usize,
    to: usize,
    workspace: Id,
    domain: Id,
) {
    let wrapper = &account.devices[from];
    let mut client = app_core::sync::SyncClient::connect(&server.base, &wrapper.token)
        .await
        .unwrap();
    let keys = client.keys(workspace, 0, wrapper.id()).await.unwrap();
    let own = keys
        .entries
        .0
        .iter()
        .find_map(|e| match &e.record {
            sync_protocol::messages::KeyRecord::Envelope(env) => Some(env.clone()),
            _ => None,
        })
        .expect("the wrapper's envelope");
    let key =
        sync_key_envelope::v2::unwrap(&own.body, &wrapper.keys, &wrapper.keys.card(account.id))
            .unwrap();
    let recipient = account.devices[to].keys.card(account.id);
    let envelope = sync_key_envelope::v2::wrap(
        &sync_key_envelope::v2::EnvelopeHeader {
            workspace,
            domain,
            epoch: 1,
            wrapping_device: wrapper.id(),
        },
        &key,
        &recipient,
        &wrapper.keys,
    )
    .unwrap();
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                txn.append_key_entry(
                    workspace,
                    4,
                    domain,
                    1,
                    Some(envelope.body.recipient),
                    &envelope.to_bytes(),
                )
                .await
                .map(|_| ())
            })
        }))
        .await
        .unwrap();
}

/// The policy changes the server holds for a workspace.
async fn policy_changes(
    server: &Server,
    device: &sync_support::Device,
    workspace: Id,
) -> Vec<Signed<PolicyChange>> {
    let mut client = app_core::sync::SyncClient::connect(&server.base, &device.token)
        .await
        .unwrap();
    client
        .policy(workspace, 0)
        .await
        .unwrap()
        .changes
        .0
        .into_iter()
        .map(|c| c.0)
        .collect()
}

fn change(
    signer: &DeviceKeys,
    after: &Signed<PolicyChange>,
    heads: Vec<AccountHead>,
    kind: PolicyKind,
) -> Signed<PolicyChange> {
    Signed::sign(
        PolicyChange {
            workspace: after.body.workspace,
            revision: after.body.revision + 1,
            previous: after.digest(),
            signer: signer.device(),
            heads: Sorted::new(heads).unwrap(),
            kind,
        },
        signer.signing(),
    )
}

/// Rewrites the page of `account`'s chain in a chains response.
fn rewrite_chain(
    account: Id,
    edit: impl Fn(&mut Vec<Signed<AccountLink>>) + Send + Sync + 'static,
) -> sync_support::Tamper {
    Box::new(move |path, body| {
        if path != "/v2/accounts/chains" {
            return body;
        }
        let mut response = ChainsResponse::from_bytes(&body).unwrap();
        for page in response.pages.0.iter_mut() {
            if page.account != account {
                continue;
            }
            // The proxy serves the whole chain from the cursor's start,
            // which the client asks from 0.
            let mut links: Vec<Signed<AccountLink>> =
                page.links.0.iter().map(|l| l.0.clone()).collect();
            edit(&mut links);
            page.head_seqno = links.len() as u64;
            page.links = List(links.into_iter().map(Nested).collect());
        }
        response.to_bytes()
    })
}

/// Two accounts: the owner's device creates a workspace that records the
/// member's account, whose second device the member's first device added.
struct Shared {
    server: Server,
    proxy: Proxy,
    owner: Account,
    member: Account,
    workspace: Id,
    domain: Id,
}

async fn shared() -> Shared {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let owner = Account::new(&server).await;
    let mut member = Account::new(&server).await;
    member.add_device(&server).await;
    let mut service = owner.devices[0]
        .connect(&proxy.base, owner.reference())
        .await;
    let created = service.create_workspace("Shared").await.unwrap();
    let changes = policy_changes(&server, &owner.devices[0], created.workspace).await;
    let add = change(
        &owner.devices[0].keys,
        changes.last().unwrap(),
        vec![owner.head(), member.head()],
        PolicyKind::AddMember {
            account: member.id,
            role: Role::Editor,
            invitation: Id::random().unwrap(),
        },
    );
    inject(&server, created.domain, add).await;
    Shared {
        server,
        proxy,
        owner,
        member,
        workspace: created.workspace,
        domain: created.domain,
    }
}

impl Shared {
    /// The owner's device with fresh kept heads, which opens the workspace
    /// once through an honest proxy and so keeps the member's head.
    async fn owner_device(&self) -> app_core::sync::SyncService {
        self.proxy.honest();
        let fresh = sync_support::Device {
            account: self.owner.id,
            keys: self.owner.devices[0].keys.clone(),
            token: self.owner.devices[0].token.clone(),
            state: tempfile::tempdir().unwrap(),
            secrets: Arc::new(MemorySecrets::default()),
        };
        let mut service = fresh
            .connect(&self.proxy.base, self.owner.reference())
            .await;
        service
            .open_workspace(self.workspace)
            .await
            .expect("an honest open");
        assert_eq!(
            service.kept_heads().kept_of(&self.member.id),
            vec![self.member.head()]
        );
        // Keep the folder alive with the service.
        std::mem::forget(fresh.state);
        service
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_chain_that_extends_the_kept_head_is_taken() {
    let mut s = shared().await;
    let mut device = s.owner_device().await;
    let third = s.member.add_device(&s.server).await;
    let _ = third;
    device
        .open_workspace(s.workspace)
        .await
        .expect("the chain extends the kept head");
    assert_eq!(
        device.kept_heads().kept_of(&s.member.id),
        vec![s.member.head()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_at_one_seqno_is_caught_and_stops_the_shared_workspace() {
    let s = shared().await;
    let mut device = s.owner_device().await;
    // The proxy serves another link at seqno 2, which the member's first
    // device signed too.
    let other = DeviceKeys::generate().unwrap();
    let fork = next_link(
        &s.member.devices[0].keys,
        &head_of(&s.member.links[..1]),
        LinkKind::AddDevice {
            card: Nested(other.card(s.member.id)),
            offer: Digest([9; 32]),
        },
    );
    s.proxy.tamper(rewrite_chain(s.member.id, move |links| {
        links[1] = fork.clone();
    }));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
    // The workspace it shares with that account stays stopped, even once
    // the proxy turns honest.
    s.proxy.honest();
    let again = device.open_workspace(s.workspace).await;
    assert!(matches!(
        again,
        Err(SyncError::Check("sync_account_forked"))
    ));
    // Its own account's chain is unharmed: it still creates a workspace.
    device
        .create_workspace("Alone")
        .await
        .expect("its own workspaces go on");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_cut_back_below_the_kept_head_is_caught() {
    let s = shared().await;
    let mut device = s.owner_device().await;
    s.proxy
        .tamper(rewrite_chain(s.member.id, |links| links.truncate(1)));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_head_recorded_off_the_kept_chain_is_caught() {
    let s = shared().await;
    let mut device = s.owner_device().await;
    // The proxy adds a change that the member's device signed, recording a
    // head at seqno 2 that the kept chain doesn't hold.
    let changes = policy_changes(&s.server, &s.owner.devices[0], s.workspace).await;
    let off = AccountHead {
        head: Digest([7; 32]),
        ..s.member.head()
    };
    let extra = change(
        &s.member.devices[0].keys,
        changes.last().unwrap(),
        vec![off],
        PolicyKind::AccountHeads,
    );
    s.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/policy") {
            return body;
        }
        let mut response = PolicyResponse::from_bytes(&body).unwrap();
        response.changes.0.push(Nested(extra.clone()));
        response.head_revision += 1;
        response.to_bytes()
    }));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_card_brought_back_or_a_device_id_reused_is_caught() {
    let mut s = shared().await;
    // The member removes its second device, and the owner's device keeps
    // the head with the removal.
    let removed = s.member.devices[1].keys.clone();
    let link = s.member.link(
        0,
        LinkKind::Remove {
            device: removed.device(),
            keep: Sorted::default(),
        },
    );
    s.member.send(&s.server, 0, link).await;
    let mut device = s.owner_device().await;
    device.open_workspace(s.workspace).await.unwrap();
    assert_eq!(device.kept_heads().kept_of(&s.member.id)[0].seqno, 3);

    // The proxy serves a link that adds the removed card again.
    let head = s.member.head();
    let again = next_link(
        &s.member.devices[0].keys,
        &head,
        LinkKind::AddDevice {
            card: Nested(removed.card(s.member.id)),
            offer: Digest([9; 32]),
        },
    );
    s.proxy.tamper(rewrite_chain(s.member.id, move |links| {
        links.push(again.clone())
    }));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_unauthorized"))),
        "{:?}",
        refused.as_ref().err()
    );

    // Another device: the proxy serves the removed device ID under another
    // card, with the same signing key and another X-Wing key.
    let mut device = s.owner_device().await;
    let reused = DeviceKeys::from_seeds(
        &removed.signing().seed(),
        sync_protocol::types::SecretKey([3; 32]),
    );
    assert_eq!(reused.device(), removed.device());
    assert_ne!(reused.card(s.member.id), removed.card(s.member.id));
    let other_card = next_link(
        &s.member.devices[0].keys,
        &head,
        LinkKind::AddDevice {
            card: Nested(reused.card(s.member.id)),
            offer: Digest([9; 32]),
        },
    );
    s.proxy.tamper(rewrite_chain(s.member.id, move |links| {
        links.push(other_card.clone())
    }));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_unauthorized"))),
        "{:?}",
        refused.as_ref().err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_the_own_chain_stops_every_workspace() {
    let mut s = shared().await;
    let second = s.owner.add_device(&s.server).await;
    let _ = second;
    let mut device = s.owner_device().await;
    let other = DeviceKeys::generate().unwrap();
    let fork = next_link(
        &s.owner.devices[0].keys,
        &head_of(&s.owner.links[..1]),
        LinkKind::AddDevice {
            card: Nested(other.card(s.owner.id)),
            offer: Digest([9; 32]),
        },
    );
    s.proxy.tamper(rewrite_chain(s.owner.id, move |links| {
        links[1] = fork.clone()
    }));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
    s.proxy.honest();
    for result in [
        device.open_workspace(s.workspace).await.map(|_| ()),
        device.create_workspace("Another").await.map(|_| ()),
    ] {
        assert!(matches!(
            result,
            Err(SyncError::Check("sync_account_forked"))
        ));
    }
    let _ = s.domain;
}

/// §8: a device trusts a workspace that no anchor brought it through a
/// change that a device of its own account, active at the head it keeps,
/// signed, at or after its reference head; once a link removes that
/// device, the change confers no trust.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_cards_change_confers_no_trust() {
    let server = Server::start().await;
    let mut account = Account::new(&server).await;
    let mut first = account.devices[0]
        .connect(&server.base, account.reference())
        .await;
    let created = first.create_workspace("Notes").await.unwrap();
    // A second device approved after the workspace's genesis: its
    // reference head is the approver's head the add_device follows. The
    // first device records the new head there and wraps the key for it.
    let approver_head = account.head();
    let second = account.add_device(&server).await;
    let changes = policy_changes(&server, &account.devices[0], created.workspace).await;
    let heads = change(
        &account.devices[0].keys,
        changes.last().unwrap(),
        vec![account.head()],
        PolicyKind::AccountHeads,
    );
    inject(&server, created.domain, heads).await;
    grant(
        &server,
        &account,
        0,
        second,
        created.workspace,
        created.domain,
    )
    .await;
    let mut device = account.devices[second]
        .connect(&server.base, approver_head.clone())
        .await;
    device
        .open_workspace(created.workspace)
        .await
        .expect("the genesis that the active first device signed anchors it");

    // The second device removes the first: the genesis it signed confers no
    // trust on a device that keeps a head with the removal.
    let remove = account.link(
        second,
        LinkKind::Remove {
            device: account.devices[0].id(),
            keep: Sorted::default(),
        },
    );
    account.send(&server, second, remove).await;
    let fresh = sync_support::Device {
        account: account.id,
        keys: account.devices[second].keys.clone(),
        token: account.devices[second].token.clone(),
        state: tempfile::tempdir().unwrap(),
        secrets: Arc::new(MemorySecrets::default()),
    };
    let mut later = fresh.connect(&server.base, approver_head).await;
    let refused = later.open_workspace(created.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_workspace_unverified"))),
        "{:?}",
        refused.as_ref().err()
    );
}

/// A refused chain stops the account's workspaces until you act on its
/// alert, even after the app restarts and the server serves the honest
/// chain again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_chain_stays_refused_after_a_restart() {
    let s = shared().await;
    let owner = &s.owner.devices[0];
    s.proxy.honest();
    let mut device = owner.connect(&s.proxy.base, s.owner.reference()).await;
    device
        .open_workspace(s.workspace)
        .await
        .expect("an honest open");
    let other = DeviceKeys::generate().unwrap();
    let fork = next_link(
        &s.member.devices[0].keys,
        &head_of(&s.member.links[..1]),
        LinkKind::AddDevice {
            card: Nested(other.card(s.member.id)),
            offer: Digest([9; 32]),
        },
    );
    s.proxy.tamper(rewrite_chain(s.member.id, move |links| {
        links[1] = fork.clone()
    }));
    assert!(matches!(
        device.open_workspace(s.workspace).await,
        Err(SyncError::Check("sync_account_forked"))
    ));
    drop(device);

    s.proxy.honest();
    let mut restarted = owner.connect(&s.proxy.base, s.owner.reference()).await;
    assert!(matches!(
        restarted.open_workspace(s.workspace).await,
        Err(SyncError::Check("sync_account_forked"))
    ));
    drop(restarted);

    let mut heads = owner.heads().unwrap();
    heads.clear_refusal(&s.member.id, &*owner.secrets).unwrap();
    drop(heads);
    let mut cleared = owner.connect(&s.proxy.base, s.owner.reference()).await;
    cleared
        .open_workspace(s.workspace)
        .await
        .expect("the stop lifts once you act on it");
}

/// Rewrites the page of `account`'s chain under `genesis` in a chains
/// response.
fn rewrite_chain_under(
    account: Id,
    genesis: Digest,
    edit: impl Fn(&mut Vec<Signed<AccountLink>>) + Send + Sync + 'static,
) -> sync_support::Tamper {
    Box::new(move |path, body| {
        if path != "/v2/accounts/chains" {
            return body;
        }
        let mut response = ChainsResponse::from_bytes(&body).unwrap();
        for page in response.pages.0.iter_mut() {
            if page.account != account || page.genesis != genesis {
                continue;
            }
            let mut links: Vec<Signed<AccountLink>> =
                page.links.0.iter().map(|l| l.0.clone()).collect();
            edit(&mut links);
            page.head_seqno = links.len() as u64;
            page.links = List(links.into_iter().map(Nested).collect());
        }
        response.to_bytes()
    })
}

/// A link that lists a device ID which another chain of the account
/// listed is refused on the device too (§7): here a retired chain of its
/// own account, which a reset would leave behind and the server serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_id_another_chain_of_the_account_listed_is_refused() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let account = Account::new(&server).await;
    let device = &account.devices[0];
    let retired_keys = DeviceKeys::generate().unwrap();
    let retired = sync_key_envelope::v2::genesis_link(&retired_keys, account.id);
    let row = sync_store::LinkRow {
        genesis: retired.digest(),
        seqno: 1,
        link: retired.to_bytes(),
    };
    let (id, digest) = (account.id, retired.digest());
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move { txn.insert_account_link(id, &row, digest).await })
        }))
        .await
        .unwrap();
    // The device keeps the heads of both chains.
    let kept = [account.head(), head_of(std::slice::from_ref(&retired))];
    drop(
        KeptHeads::rebuild(
            &device.heads_dir(),
            account.id,
            device.id(),
            kept,
            [],
            &*device.secrets,
        )
        .unwrap(),
    );
    let mut service = device.connect(&proxy.base, account.reference()).await;
    service.own_chain().await.expect("an honest chain");

    let current = account.links[0].digest();
    let reused = next_link(
        &device.keys,
        &account.head(),
        LinkKind::AddDevice {
            card: Nested(retired_keys.card(account.id)),
            offer: Digest([9; 32]),
        },
    );
    proxy.tamper(rewrite_chain_under(account.id, current, move |links| {
        links.push(reused.clone())
    }));
    let refused = service.own_chain().await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_unauthorized"))),
        "{:?}",
        refused.as_ref().err()
    );
    assert_eq!(service.kept_heads().kept_of(&account.id)[1].seqno, 1);
}

/// Removes the member from the shared workspace, as an owner device's
/// `remove_member` push will.
async fn remove_member(s: &Shared) {
    let changes = policy_changes(&s.server, &s.owner.devices[0], s.workspace).await;
    let remove = change(
        &s.owner.devices[0].keys,
        changes.last().unwrap(),
        vec![s.owner.head()],
        PolicyKind::RemoveMember {
            account: s.member.id,
        },
    );
    inject(&s.server, s.domain, remove).await;
}

/// Review of the read bounds (§7): the owner's device kept the member's
/// chain past the head the workspace recorded, because a current member's
/// chain reaches the workspace's devices in full. Once `remove_member`
/// commits, the server serves that chain only up to the recorded head,
/// and the device takes it, across restarts, keeping its higher head. A
/// chain cut below the recorded head, or one whose links differ from those
/// the device kept, is still caught.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_members_chain_served_up_to_its_recorded_head_is_taken() {
    let mut s = shared().await;
    s.member.add_device(&s.server).await;
    let owner = &s.owner.devices[0];
    s.proxy.honest();
    let mut device = owner.connect(&s.proxy.base, s.owner.reference()).await;
    device
        .open_workspace(s.workspace)
        .await
        .expect("a current member's chain in full");
    assert_eq!(device.kept_heads().kept_of(&s.member.id)[0].seqno, 3);

    remove_member(&s).await;
    let mut client = app_core::sync::SyncClient::connect(&s.server.base, &owner.token)
        .await
        .unwrap();
    let (served, _) = client
        .whole_chain(s.member.id, Some(s.member.links[0].digest()))
        .await
        .unwrap();
    assert_eq!(served.len(), 2, "the server serves the recorded head");
    device
        .open_workspace(s.workspace)
        .await
        .expect("the chain up to its recorded head");
    assert_eq!(device.kept_heads().kept_of(&s.member.id)[0].seqno, 3);
    drop(device);
    let mut restarted = owner.connect(&s.proxy.base, s.owner.reference()).await;
    restarted
        .open_workspace(s.workspace)
        .await
        .expect("again after a restart");
    assert_eq!(restarted.kept_heads().stopped([s.member.id]), None);

    // Below the head the workspace records, the chain was cut back.
    s.proxy
        .tamper(rewrite_chain(s.member.id, |links| links.truncate(1)));
    let refused = restarted.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
}

/// The links of a chain served short must be the ones the device kept: a
/// removed member's chain whose recorded head the proxy replaces with a
/// fork, recorded by a change the member's device signed, is caught.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_chain_off_the_kept_links_is_caught() {
    let mut s = shared().await;
    s.member.add_device(&s.server).await;
    let owner = &s.owner.devices[0];
    s.proxy.honest();
    let mut device = owner.connect(&s.proxy.base, s.owner.reference()).await;
    device.open_workspace(s.workspace).await.unwrap();
    remove_member(&s).await;

    // A fork at seqno 2 that the member's first device signed, and a
    // policy whose add_member records it, as a server that showed other
    // devices that branch would serve.
    let other = DeviceKeys::generate().unwrap();
    let fork = next_link(
        &s.member.devices[0].keys,
        &head_of(&s.member.links[..1]),
        LinkKind::AddDevice {
            card: Nested(other.card(s.member.id)),
            offer: Digest([9; 32]),
        },
    );
    let forked_head = head_of(&[s.member.links[0].clone(), fork.clone()]);
    let real_head = head_of(&s.member.links[..2]);
    let changes = policy_changes(&s.server, owner, s.workspace).await;
    let mut forged = vec![changes[0].clone()];
    for original in &changes[1..] {
        let heads: Vec<AccountHead> = original
            .body
            .heads
            .iter()
            .map(|h| {
                if *h == real_head {
                    forked_head.clone()
                } else {
                    h.clone()
                }
            })
            .collect();
        forged.push(change(
            &s.owner.devices[0].keys,
            forged.last().unwrap(),
            heads,
            original.body.kind.clone(),
        ));
    }
    s.proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/policy") {
            let mut response = PolicyResponse::from_bytes(&body).unwrap();
            response.changes = List(forged.iter().cloned().map(Nested).collect());
            return response.to_bytes();
        }
        if path != "/v2/accounts/chains" {
            return body;
        }
        let mut response = ChainsResponse::from_bytes(&body).unwrap();
        for page in response.pages.0.iter_mut() {
            if page.account == forked_head.account {
                page.links = List(vec![
                    Nested(page.links.0[0].0.clone()),
                    Nested(fork.clone()),
                ]);
                page.head_seqno = 2;
            }
        }
        response.to_bytes()
    }));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
}

/// While the workspace names a member's chain and counts the device among
/// its devices, the server serves that chain in full, so one stopped at
/// its recorded head, below the kept head, was cut back (§7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_current_members_chain_cut_back_to_its_recorded_head_is_caught() {
    let mut s = shared().await;
    s.member.add_device(&s.server).await;
    let owner = &s.owner.devices[0];
    s.proxy.honest();
    let mut device = owner.connect(&s.proxy.base, s.owner.reference()).await;
    device.open_workspace(s.workspace).await.unwrap();
    assert_eq!(device.kept_heads().kept_of(&s.member.id)[0].seqno, 3);
    s.proxy
        .tamper(rewrite_chain(s.member.id, |links| links.truncate(2)));
    let refused = device.open_workspace(s.workspace).await;
    assert!(
        matches!(refused, Err(SyncError::Check("sync_account_forked"))),
        "{:?}",
        refused.as_ref().err()
    );
    s.proxy.honest();
    assert!(matches!(
        device.open_workspace(s.workspace).await,
        Err(SyncError::Check("sync_account_forked"))
    ));
}

/// Refusals live in the credential store, so a rebuild after
/// `sync_journal_rollback` keeps every stop until you clear it.
#[test]
fn a_rebuild_keeps_the_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::default();
    let keys = DeviceKeys::generate().unwrap();
    let account = Id::random().unwrap();
    let genesis = sync_key_envelope::v2::genesis_link(&keys, account);
    let kept = head_of(std::slice::from_ref(&genesis));
    let own = Id::random().unwrap();
    let device = Id::random().unwrap();
    let mut heads =
        KeptHeads::rebuild(dir.path(), own, device, [kept.clone()], [], &secrets).unwrap();
    let stranger = DeviceKeys::generate().unwrap();
    let other = sync_key_envelope::v2::genesis_link(&stranger, account);
    assert!(
        heads
            .accept(account, kept.genesis, &[other], &BTreeSet::new(), &secrets)
            .is_err()
    );
    assert_eq!(heads.stopped([account]), Some("sync_account_forked"));
    drop(heads);

    let file = dir
        .path()
        .join(app_core::sync::account_heads::KEPT_HEADS_FILE);
    std::fs::write(&file, b"rolled back").unwrap();
    assert!(matches!(
        KeptHeads::open(dir.path(), own, device, &secrets),
        Err(SyncError::Check("sync_journal_rollback"))
    ));
    let mut rebuilt =
        KeptHeads::rebuild(dir.path(), own, device, [kept.clone()], [], &secrets).unwrap();
    assert_eq!(rebuilt.stopped([account]), Some("sync_account_forked"));
    rebuilt.clear_refusal(&account, &secrets).unwrap();
    drop(rebuilt);
    let rebuilt = KeptHeads::rebuild(dir.path(), own, device, [kept], [], &secrets).unwrap();
    assert_eq!(rebuilt.stopped([account]), None);

    // Two recorded heads at one seqno of one chain are a fork.
    let forked = KeptHeads::rebuild(
        dir.path(),
        own,
        device,
        [
            head_of(std::slice::from_ref(&genesis)),
            AccountHead {
                head: Digest([3; 32]),
                ..head_of(std::slice::from_ref(&genesis))
            },
        ],
        [],
        &secrets,
    )
    .unwrap();
    assert_eq!(forked.stopped([account]), Some("sync_account_forked"));
}

/// A workspace's names follow its verified members, so a member it removed,
/// or a workspace the device left, raises no two-current-chains alert.
#[test]
fn a_workspaces_names_follow_its_members() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::default();
    let mut heads = KeptHeads::open(
        dir.path(),
        Id::random().unwrap(),
        Id::random().unwrap(),
        &secrets,
    )
    .unwrap();
    let member = Id::random().unwrap();
    let (first, second) = (Id::random().unwrap(), Id::random().unwrap());
    heads
        .name_members(first, [(member, Digest([1; 32]))])
        .unwrap();
    heads
        .name_members(second, [(member, Digest([2; 32]))])
        .unwrap();
    assert_eq!(heads.two_current_chains(), vec![member]);
    // The second workspace removes the member.
    heads.name_members(second, []).unwrap();
    assert!(heads.two_current_chains().is_empty());
    // The member joins the second workspace again under another chain, and
    // the device then leaves the first.
    heads
        .name_members(second, [(member, Digest([2; 32]))])
        .unwrap();
    assert_eq!(heads.two_current_chains(), vec![member]);
    heads.forget_workspace(first).unwrap();
    assert!(heads.two_current_chains().is_empty());
}

/// An `accept_reset` in one workspace ends the retired chain there alone: a
/// workspace that still names it keeps the head and the refusal, so its
/// comparison still finds what a restore lost (§14).
#[test]
fn a_reset_one_workspace_accepted_stays_open_in_another() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::default();
    let own = Id([1; 16]);
    let device = Id([2; 16]);
    let member = Id([3; 16]);
    let (first, second) = (Id([4; 16]), Id([5; 16]));
    let (retired, fresh) = (Digest([6; 32]), Digest([7; 32]));
    let head = |genesis: Digest, seqno: u64| AccountHead {
        account: member,
        genesis,
        seqno,
        head: Digest([seqno as u8; 32]),
    };
    let mut heads = KeptHeads::rebuild(
        dir.path(),
        own,
        device,
        [head(retired, 4), head(fresh, 1)],
        [(first, member, fresh), (second, member, retired)],
        &secrets,
    )
    .unwrap();
    heads.refuse_short(member, &secrets);
    assert_eq!(
        heads.stopped_in(&first, [member]),
        Some("sync_account_forked")
    );

    heads
        .drop_retired(&member, &fresh, first, &secrets)
        .unwrap();
    assert_eq!(heads.stopped_in(&first, [member]), None, "accepted here");
    assert_eq!(
        heads.stopped_in(&second, [member]),
        Some("sync_account_forked"),
        "still named there"
    );
    assert_eq!(
        heads
            .kept_in(&first, &member)
            .iter()
            .map(|h| h.genesis)
            .collect::<Vec<_>>(),
        vec![fresh]
    );
    assert_eq!(heads.kept_in(&second, &member).len(), 2);

    heads
        .drop_retired(&member, &fresh, second, &secrets)
        .unwrap();
    assert_eq!(heads.stopped_in(&second, [member]), None);
    assert_eq!(heads.kept_of(&member).len(), 1, "no workspace names it");
    let reopened = KeptHeads::open(dir.path(), own, device, &secrets).unwrap();
    assert_eq!(reopened.stopped([member]), None, "the store forgot it too");
    assert!(reopened.kept_of(&member).iter().all(|h| h.genesis == fresh));
}
