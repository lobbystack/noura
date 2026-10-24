//! A tampering proxy between a device and the server (seam 4, sync-v2.md
//! §30): a server that inserts its own device key into your chain, plants
//! a card in a workspace's policy, lists an extra card in `GET
//! /v2/devices`, swaps the approver's head in an offer, or hands a
//! recipient a key other than the epoch's, gets caught with the error §30
//! names.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::approval::CandidateStep;
use app_core::sync::device_list::DeviceAlert;
use sync_key_envelope::v2::{DeviceKeys, next_link};
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{
    ChainsResponse, DeviceInfo, DeviceState, DevicesResponse, OffersResponse, PolicyResponse,
};
use sync_protocol::records::{LinkKind, PolicyChange, PolicyKind, Signed};
use sync_protocol::types::{Digest, Id, SecretKey};
use sync_store::{Backend as _, Session};
use sync_support::{Desktop, Proxy, Server, approve, first_desktop, new_desktop};

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
    // The candidate reveals its nonce for the offer it was shown, whose
    // digest differs from the one the laptop signed: the laptop never
    // gets a nonce for its own offer, so it shows no code, and the two
    // screens can't match.
    let step = phone.candidate.poll().await;
    assert!(!matches!(step, Ok(CandidateStep::Waiting)));
    if let Ok(CandidateStep::Code(code)) = step {
        assert_ne!(
            Some(code),
            laptop.service.approval_code(&mut offer).await.unwrap()
        );
    }
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
