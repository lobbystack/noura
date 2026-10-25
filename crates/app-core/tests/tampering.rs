//! A tampering proxy between a device and the server (seam 4, sync-v2.md
//! §30): a server that inserts its own device key into your chain, plants
//! a card in a workspace's policy or in another member's chain, reuses a
//! device ID under another card, lists an extra card in `GET /v2/devices`,
//! brings back a removed card, swaps the approver's head in an offer, hands
//! a recipient a key other than the epoch's, or shows two recipients
//! different epoch records, gets caught with the error §30 names.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::device_list::DeviceAlert;
use sync_key_envelope::v2::{DeviceKeys, next_link};
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{
    ChainsResponse, DeviceInfo, DeviceState, DevicesResponse, OffersResponse, PolicyResponse,
};
use sync_protocol::messages::{EpochsResponse, KeyRecord, KeysResponse};
use sync_protocol::records::{LinkKind, PolicyChange, PolicyKind, Role, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};
use sync_store::{Backend as _, Session};
use sync_support::{Account, Desktop, Proxy, Server, approve, first_desktop, head_of, new_desktop};

/// A device whose service talks to the server through `proxy`.
async fn through(proxy: &Proxy, desktop: Desktop) -> Desktop {
    Desktop::connect(
        &proxy.base,
        desktop.credentials.clone(),
        desktop.secrets.clone(),
        desktop.state,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_key_inserted_into_your_chain_is_refused() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let laptop = first_desktop(&server, "ada@example.com").await;
    let mut laptop = through(&proxy, laptop).await;
    let chain = laptop.service.own_chain().await.unwrap();
    // The server's own key, which signs an add_device as if a device did.
    let server_key = DeviceKeys::generate().unwrap();
    let account = laptop.credentials.account;
    let forged = next_link(
        &server_key,
        &chain.head(),
        LinkKind::AddDevice {
            card: Nested(server_key.card(account)),
            offer: Digest([1; 32]),
        },
    );
    proxy.tamper(Box::new(move |path, body| {
        if path != "/v2/accounts/chains" {
            return body;
        }
        let mut chains = ChainsResponse::from_bytes(&body).unwrap();
        for page in chains.pages.0.iter_mut() {
            if page.account == account {
                page.links.0.push(Nested(forged.clone()));
                page.head_seqno += 1;
            }
        }
        chains.to_bytes()
    }));
    let refused = laptop.service.own_chain().await.expect_err("refused");
    assert!(
        matches!(refused, SyncError::Check("sync_account_unauthorized")),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_card_planted_in_a_workspace_policy_is_refused() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "bea@example.com").await;
    let workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let mut laptop = through(&proxy, laptop).await;
    let chain = laptop.service.own_chain().await.unwrap();
    let planted = DeviceKeys::generate().unwrap();
    let (revision, head) = workspace.policy().head();
    let change = Signed::sign(
        PolicyChange {
            workspace: workspace.workspace,
            revision: revision + 1,
            previous: head,
            signer: planted.device(),
            heads: Sorted::new(vec![chain.head()]).unwrap(),
            kind: PolicyKind::AccountHeads,
        },
        planted.signing(),
    );
    let path = format!(
        "/v2/workspaces/{}/policy",
        workspace.workspace.to_base64url()
    );
    proxy.tamper(Box::new(move |route, body| {
        if route != path {
            return body;
        }
        let mut policy = PolicyResponse::from_bytes(&body).unwrap();
        policy.changes.0.push(Nested(change.clone()));
        policy.head_revision += 1;
        policy.to_bytes()
    }));
    let refused = laptop
        .service
        .open_workspace(workspace.workspace)
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(refused, SyncError::Check("sync_policy_unauthorized")),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extra_card_in_the_device_list_raises_an_alert() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let laptop = first_desktop(&server, "cai@example.com").await;
    let account = laptop.credentials.account;
    let mut laptop = through(&proxy, laptop).await;
    let extra = DeviceKeys::generate().unwrap().card(account);
    let listed = extra.clone();
    proxy.tamper(Box::new(move |path, body| {
        if path != "/v2/devices" {
            return body;
        }
        let mut devices = DevicesResponse::from_bytes(&body).unwrap();
        devices.devices.0.push(DeviceInfo {
            card: Nested(listed.clone()),
            sessions: 2,
            state: DeviceState::Active,
        });
        devices.to_bytes()
    }));
    let list = laptop.service.device_list(&[]).await.unwrap();
    assert_eq!(list.active.len(), 1, "the list comes from the chain alone");
    assert!(
        !list.active.iter().any(|row| row.card == extra),
        "a card the server lists as active stays out"
    );
    assert_eq!(
        list.alerts,
        vec![DeviceAlert::UnknownCard {
            card: extra.clone(),
            sessions: 2
        }],
        "whatever state the server reports"
    );
    // A card that reuses this device's ID under other keys alerts too.
    let mut reused = laptop.credentials.card();
    reused.hpke_public = DeviceKeys::generate().unwrap().card(account).hpke_public;
    let shown = reused.clone();
    proxy.tamper(Box::new(move |path, body| {
        if path != "/v2/devices" {
            return body;
        }
        let mut devices = DevicesResponse::from_bytes(&body).unwrap();
        devices.devices.0.push(DeviceInfo {
            card: Nested(shown.clone()),
            sessions: 0,
            state: DeviceState::Active,
        });
        devices.to_bytes()
    }));
    let list = laptop.service.device_list(&[]).await.unwrap();
    assert_eq!(
        list.alerts,
        vec![DeviceAlert::UnknownCard {
            card: reused,
            sessions: 0
        }]
    );
    proxy.tamper(Box::new({
        let listed = extra.clone();
        move |path, body| {
            if path != "/v2/devices" {
                return body;
            }
            let mut devices = DevicesResponse::from_bytes(&body).unwrap();
            devices.devices.0.push(DeviceInfo {
                card: Nested(listed.clone()),
                sessions: 2,
                state: DeviceState::Enrolled,
            });
            devices.to_bytes()
        }
    }));
    let list = laptop.service.device_list(&[]).await.unwrap();
    assert_eq!(
        list.alerts,
        vec![DeviceAlert::UnknownCard {
            card: extra,
            sessions: 2
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_swapped_approver_head_shows_another_code() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "dan@example.com").await;
    let account = laptop.credentials.account;
    let mut phone = new_desktop(&server, account, &proxy.base).await;
    let pending = laptop.service.pending_devices().await.unwrap();
    let mut none = Vec::new();
    let mut offer = laptop
        .service
        .offer_approval(&pending[0], &mut none)
        .await
        .unwrap();
    proxy.tamper(Box::new(|path, body| {
        if path != "/v2/approvals" {
            return body;
        }
        let mut offers = OffersResponse::from_bytes(&body).unwrap();
        for offer in offers.offers.0.iter_mut() {
            offer.0.body.approver_head.seqno += 1;
        }
        OffersResponse {
            offers: List(offers.offers.0),
        }
        .to_bytes()
    }));
    // The offer the candidate sees no longer carries the approver's
    // signature, so the attempt ends before any nonce leaves the
    // candidate, and the laptop never shows a code.
    let step = phone.candidate.poll().await;
    assert!(
        matches!(step, Err(SyncError::Check("sync_approval_mismatch"))),
        "{:?}",
        step.as_ref().err()
    );
    proxy.honest();
    assert_eq!(
        laptop.service.approval_code(&mut offer).await.unwrap(),
        None,
        "no nonce reached the laptop's offer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recipient_handed_another_key_than_the_epochs_is_refused() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "eve@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let mut phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();
    // A second envelope for the phone at epoch 1 that wraps another key,
    // as a server colluding with a rotator would serve one recipient.
    let envelope = sync_key_envelope::v2::wrap(
        &sync_key_envelope::v2::EnvelopeHeader {
            workspace: workspace.workspace,
            domain: workspace.domain,
            epoch: 1,
            wrapping_device: laptop.id(),
        },
        &SecretKey::random().unwrap(),
        &phone.credentials.card(),
        &laptop.credentials.keys,
    )
    .unwrap();
    let (w, d) = (workspace.workspace, workspace.domain);
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                use sync_store::WriteTxn as _;
                txn.append_key_entry(
                    w,
                    4,
                    d,
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
    let refused = phone
        .service
        .refresh_keys(&mut remote)
        .await
        .expect_err("another key than the epoch's");
    assert!(
        matches!(refused, SyncError::Check("sync_key_mismatch")),
        "{refused:?}"
    );
    let _ = (Id([0; 16]), Session::new(Id([1; 16]), Id([2; 16])));
}

/// Makes a second account a member of the laptop's workspace, as an
/// `add_member` the laptop signed would once ticket 50 lets a push carry
/// one, and has the laptop verify it.
async fn with_member(
    server: &Server,
    laptop: &mut Desktop,
    workspace: &mut app_core::sync::RemoteWorkspace,
) -> Account {
    let member = Account::new(server).await;
    let chain = laptop.service.own_chain().await.unwrap();
    let (revision, previous) = workspace.policy().head();
    let change = Signed::sign(
        PolicyChange {
            workspace: workspace.workspace,
            revision: revision + 1,
            previous,
            signer: laptop.id(),
            heads: Sorted::new(vec![chain.head(), member.head()]).unwrap(),
            kind: PolicyKind::AddMember {
                account: member.id,
                role: Role::Editor,
                invitation: Id([0x5a; 16]),
            },
        },
        laptop.credentials.keys.signing(),
    );
    sync_store::conformance::inject_change(&server.store, change, vec![workspace.domain], None)
        .await
        .unwrap();
    laptop
        .service
        .refresh_policy(workspace)
        .await
        .expect("the member's chain verifies");
    member
}

/// Appends `link` to `account`'s chain page in every chains response.
fn extra_link(
    account: Id,
    link: Signed<sync_protocol::records::AccountLink>,
) -> sync_support::Tamper {
    Box::new(move |path, body| {
        if path != "/v2/accounts/chains" {
            return body;
        }
        let mut chains = ChainsResponse::from_bytes(&body).unwrap();
        for page in chains.pages.0.iter_mut() {
            if page.account == account {
                page.links.0.push(Nested(link.clone()));
                page.head_seqno += 1;
            }
        }
        chains.to_bytes()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_card_planted_in_another_members_chain_is_refused() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "fay@example.com").await;
    let mut workspace = laptop.service.create_workspace("Shared").await.unwrap();
    let member = with_member(&server, &mut laptop, &mut workspace).await;
    let mut laptop = through(&proxy, laptop).await;
    // The server's own key signs an add_device into the member's chain.
    let planted = DeviceKeys::generate().unwrap();
    let forged = next_link(
        &planted,
        &member.head(),
        LinkKind::AddDevice {
            card: Nested(planted.card(member.id)),
            offer: Digest([3; 32]),
        },
    );
    proxy.tamper(extra_link(member.id, forged));
    let refused = laptop
        .service
        .refresh_policy(&mut workspace)
        .await
        .expect_err("refused");
    assert!(
        matches!(refused, SyncError::Check("sync_account_unauthorized")),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_id_reused_under_another_card_is_refused() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "gus@example.com").await;
    let mut workspace = laptop.service.create_workspace("Shared").await.unwrap();
    let mut member = with_member(&server, &mut laptop, &mut workspace).await;
    let mut laptop = through(&proxy, laptop).await;
    // A malicious member and the server: the member's chain adds a card
    // under the laptop's device ID, and the member records that head in
    // the workspace, which would make the ID active under two accounts.
    let mut reused = laptop.credentials.card();
    reused.account = member.id;
    let link = member.link(
        0,
        LinkKind::AddDevice {
            card: Nested(reused),
            offer: Digest([4; 32]),
        },
    );
    member.links.push(link.clone());
    let (revision, previous) = workspace.policy().head();
    let recording = Signed::sign(
        PolicyChange {
            workspace: workspace.workspace,
            revision: revision + 1,
            previous,
            signer: member.devices[0].id(),
            heads: Sorted::new(vec![head_of(&member.links)]).unwrap(),
            kind: PolicyKind::AccountHeads,
        },
        member.devices[0].keys.signing(),
    );
    let chains = extra_link(member.id, link);
    let policy_path = format!(
        "/v2/workspaces/{}/policy",
        workspace.workspace.to_base64url()
    );
    proxy.tamper(Box::new(move |path, body| {
        if path == policy_path {
            let mut policy = PolicyResponse::from_bytes(&body).unwrap();
            policy.changes.0.push(Nested(recording.clone()));
            policy.head_revision += 1;
            return policy.to_bytes();
        }
        chains(path, body)
    }));
    let refused = laptop
        .service
        .refresh_policy(&mut workspace)
        .await
        .expect_err("refused");
    assert!(
        matches!(refused, SyncError::Check("sync_policy_unauthorized")),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_card_brought_back_is_refused() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "hal@example.com").await;
    let account = laptop.credentials.account;
    let phone = approve(&server, &mut laptop, &mut []).await;
    laptop
        .service
        .remove_device(phone.id(), &[], &mut [])
        .await
        .unwrap();
    let mut laptop = through(&proxy, laptop).await;
    // The server serves the chain as it stood before the removal, and lists
    // the phone as active.
    let phone_card = phone.credentials.card();
    proxy.tamper(Box::new(move |path, body| match path {
        "/v2/accounts/chains" => {
            let mut chains = ChainsResponse::from_bytes(&body).unwrap();
            for page in chains.pages.0.iter_mut() {
                if page.account == account && page.head_seqno == 3 {
                    page.links.0.truncate(2);
                    page.head_seqno = 2;
                }
            }
            chains.to_bytes()
        }
        "/v2/devices" => {
            let mut devices = DevicesResponse::from_bytes(&body).unwrap();
            devices.devices.0.push(DeviceInfo {
                card: Nested(phone_card.clone()),
                sessions: 1,
                state: DeviceState::Active,
            });
            devices.to_bytes()
        }
        _ => body,
    }));
    let refused = laptop.service.own_chain().await.expect_err("refused");
    assert!(
        matches!(refused, SyncError::Check("sync_account_forked")),
        "{refused:?}"
    );
    let refused = laptop
        .service
        .device_list(&[])
        .await
        .expect_err("the list comes from the chain");
    assert_eq!(refused.code(), Some("sync_account_forked"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_recipients_shown_different_epoch_records_get_caught() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "ivy@example.com").await;
    let account = laptop.credentials.account;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let tablet = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let (w, d) = (workspace.workspace, workspace.domain);
    // The phone's epoch 1 record and key, which the forged epoch 2 links to.
    let current = server
        .store
        .current_keys(Session::new(account, phone.id()), w, phone.id())
        .await
        .unwrap();
    let (mut first, mut first_key) = (None, None);
    for entry in current.entries.0 {
        match entry.record {
            KeyRecord::Epoch(epoch) => first = Some(epoch),
            KeyRecord::Envelope(envelope) if envelope.body.recipient == phone.id() => {
                first_key = Some(
                    sync_key_envelope::v2::unwrap(
                        &envelope.body,
                        &phone.credentials.keys,
                        &phone.credentials.card(),
                    )
                    .unwrap(),
                );
            }
            _ => {}
        }
    }
    let (first, first_key) = (first.unwrap(), first_key.unwrap());
    let mut phone = through(&proxy, phone).await;
    let mut remote = phone.service.open_workspace(w).await.unwrap();
    // Removing the tablet makes the laptop rotate to epoch 2.
    laptop
        .service
        .remove_device(tablet.id(), &[], std::slice::from_mut(&mut workspace))
        .await
        .unwrap();
    assert_eq!(workspace.epoch(), 2);

    // The laptop, as the rotating member colluding with the server, signs a
    // second epoch 2 record with another key, which the server shows the
    // phone alone.
    let (revision, head) = workspace.policy().head();
    let other_key = SecretKey::random().unwrap();
    let forged = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: w,
                domain: d,
                epoch: 2,
                previous: first.digest(),
                policy_revision: revision,
                policy_head: head,
                rotating_device: laptop.id(),
                recipients: workspace.policy().head_snapshot().readers(&d),
            },
            &other_key,
            Some(&first_key),
            Nonce::ZERO,
        ),
        laptop.credentials.keys.signing(),
    );
    let envelope = sync_key_envelope::v2::wrap(
        &sync_key_envelope::v2::EnvelopeHeader {
            workspace: w,
            domain: d,
            epoch: 2,
            wrapping_device: laptop.id(),
        },
        &other_key,
        &phone.credentials.card(),
        &laptop.credentials.keys,
    )
    .unwrap();
    let keys_path = format!("/v2/workspaces/{}/keys/current", w.to_base64url());
    let lineage_path = format!(
        "/v2/workspaces/{}/domains/{}/epochs/2",
        w.to_base64url(),
        d.to_base64url()
    );
    let phone_id = phone.id();
    proxy.tamper(Box::new(move |path, body| {
        if path == keys_path {
            let mut keys = KeysResponse::from_bytes(&body).unwrap();
            for entry in keys.entries.0.iter_mut() {
                match &mut entry.record {
                    KeyRecord::Epoch(epoch) if epoch.body.epoch == 2 => *epoch = forged.clone(),
                    KeyRecord::Envelope(env)
                        if env.body.epoch == 2 && env.body.recipient == phone_id =>
                    {
                        *env = envelope.clone()
                    }
                    _ => {}
                }
            }
            return keys.to_bytes();
        }
        if path == lineage_path {
            let mut lineage = EpochsResponse::from_bytes(&body).unwrap();
            if let Some(newest) = lineage.epochs.0.first_mut() {
                *newest = Nested(forged.clone());
            }
            return lineage.to_bytes();
        }
        body
    }));
    phone.service.refresh_policy(&mut remote).await.unwrap();
    phone
        .service
        .refresh_keys(&mut remote)
        .await
        .expect("the phone can't tell the forged record yet");
    assert_eq!(remote.epoch(), 2);

    // The laptop writes under the real epoch 2; once the phone sees that
    // record in the log, it refuses the page.
    laptop
        .service
        .push_file(&mut workspace, "After.md", b"# After\n")
        .await
        .unwrap();
    let refused = phone
        .service
        .pull_files(&mut remote)
        .await
        .expect_err("two records for one epoch");
    assert!(
        matches!(refused, SyncError::Check("sync_key_mismatch")),
        "{refused:?}"
    );
}
