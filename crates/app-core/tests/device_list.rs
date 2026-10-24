//! The device list and its alerts (seam 4, sync-v2.md §7): the list comes
//! from the account chain, with session counts from `GET /v2/devices`, and
//! it warns about an enrolled card the chain doesn't list, a workspace whose
//! log doesn't verify, a link this device neither signed nor took part in,
//! and a policy change another device of the account signed.

mod sync_support;

use app_core::sync::device_list::{AddedBy, DeviceAlert};
use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::{Decode, Encode, Nested, Sorted};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::PolicyResponse;
use sync_protocol::records::{PolicyChange, PolicyKind, Signed};
use sync_protocol::types::{Id, Nonce, SecretKey};
use sync_support::{Desktop, Proxy, Server, approve, first_desktop};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_comes_from_the_chain_and_warns_about_what_this_device_didnt_see() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "ada@example.com").await;
    let account = laptop.credentials.account;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let mut phone = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let mut phone_remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .unwrap();

    let list = laptop
        .service
        .device_list(std::slice::from_ref(&workspace))
        .await
        .expect("the list");
    assert_eq!(
        list.account_fingerprint,
        laptop.service.own_chain().await.unwrap().genesis()
    );
    assert_eq!(list.active.len(), 2);
    assert!(list.active[0].this_device);
    assert_eq!(list.active[0].added, AddedBy::Genesis);
    let phone_row = list
        .active
        .iter()
        .find(|r| r.card.device == phone.id())
        .unwrap();
    assert_eq!(phone_row.added, AddedBy::Approval(laptop.id()));
    assert_eq!(
        phone_row.fingerprint,
        phone.credentials.card().fingerprint()
    );
    assert_eq!(phone_row.sessions, Some(1));
    assert!(list.alerts.is_empty(), "{:?}", list.alerts);
    let phone_list = phone
        .service
        .device_list(std::slice::from_ref(&phone_remote))
        .await
        .unwrap();
    assert!(phone_list.alerts.is_empty(), "{:?}", phone_list.alerts);

    // A card that signed in and enrolled with no approval request.
    let stranger = server.sign_in(account, &server.base).await;
    let keys = DeviceKeys::generate().unwrap();
    let mut client = app_core::sync::SyncClient::connect(&server.base, &stranger.session_token)
        .await
        .unwrap();
    app_core::sync::device::enroll_card(&mut client, &keys, account)
        .await
        .unwrap();
    let list = laptop.service.device_list(&[]).await.unwrap();
    assert_eq!(
        list.alerts,
        vec![DeviceAlert::UnknownCard {
            card: keys.card(account),
            sessions: 1
        }]
    );
    laptop
        .service
        .end_unknown_sessions(keys.device())
        .await
        .expect("an active device ends them");
    assert!(
        client.workspaces().await.is_err(),
        "the stranger's session ended"
    );

    // The laptop approves a tablet: the phone didn't take part.
    let tablet = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    phone
        .service
        .refresh_policy(&mut phone_remote)
        .await
        .unwrap();
    let list = phone
        .service
        .device_list(std::slice::from_ref(&phone_remote))
        .await
        .unwrap();
    assert_eq!(
        list.alerts,
        vec![DeviceAlert::Link {
            seqno: 3,
            kind: "add_device",
            signer: laptop.id(),
            card: tablet.credentials.card(),
            recovery: false,
        }]
    );

    // The phone signs a policy change; the laptop alerts you to it. The
    // server takes no such push before ticket 50, so the store gets it.
    let domain = Id::random().unwrap();
    let marker = Id::random().unwrap();
    let (revision, head) = phone_remote.policy().head();
    let chain = phone.service.own_chain().await.unwrap();
    let change = Signed::sign(
        PolicyChange {
            workspace: workspace.workspace,
            revision: revision + 1,
            previous: head,
            signer: phone.id(),
            heads: Sorted::new(vec![chain.head()]).unwrap(),
            kind: PolicyKind::CreateDomain {
                domain,
                marker_rid: marker,
                access: sync_store::conformance::manager(account),
            },
        },
        phone.credentials.keys.signing(),
    );
    let readers = phone_remote
        .policy()
        .head_snapshot()
        .readers(&workspace.domain);
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: workspace.workspace,
                domain,
                epoch: 1,
                previous: sync_protocol::types::Digest::ZERO,
                policy_revision: revision + 1,
                policy_head: change.digest(),
                rotating_device: phone.id(),
                recipients: readers,
            },
            &SecretKey::random().unwrap(),
            None,
            Nonce::ZERO,
        ),
        phone.credentials.keys.signing(),
    );
    sync_store::conformance::inject_change(
        &server.store,
        change,
        vec![workspace.domain],
        Some((domain, epoch)),
    )
    .await
    .expect("injected");
    laptop.service.refresh_policy(&mut workspace).await.unwrap();
    let list = laptop
        .service
        .device_list(std::slice::from_ref(&workspace))
        .await
        .unwrap();
    assert!(
        list.alerts.contains(&DeviceAlert::PolicyChange {
            workspace: workspace.workspace,
            revision: revision + 1,
            kind: "create_domain",
            signer: phone.id(),
        }),
        "{:?}",
        list.alerts
    );
}

/// A device that first trusts a workspace through another device's change,
/// not an approval anchor, a kit or creating it, alerts you to every change
/// in that log other than `account_heads` that another device of your
/// account signed, its genesis included; and a workspace of your account
/// whose log doesn't verify shows as unverified.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_workspace_trusted_through_a_siblings_change_alerts_on_its_earlier_changes() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "bea@example.com").await;
    let mut none = Vec::new();
    let phone = approve(&server, &mut laptop, &mut none).await;
    // The laptop creates a workspace after the phone joined.
    let later = laptop.service.create_workspace("Later").await.unwrap();
    let mut phone = Desktop::connect(
        &proxy.base,
        phone.credentials.clone(),
        phone.secrets.clone(),
        phone.state,
    )
    .await;
    let remote = phone.service.open_workspace(later.workspace).await.unwrap();
    let list = phone
        .service
        .device_list(std::slice::from_ref(&remote))
        .await
        .unwrap();
    assert_eq!(
        list.alerts,
        vec![DeviceAlert::PolicyChange {
            workspace: later.workspace,
            revision: 1,
            kind: "genesis",
            signer: laptop.id(),
        }]
    );

    // A workspace whose log the server serves with a broken signature.
    let other = laptop.service.create_workspace("Other").await.unwrap();
    let path = format!("/v2/workspaces/{}/policy", other.workspace.to_base64url());
    proxy.tamper(Box::new(move |route, body| {
        if route != path {
            return body;
        }
        let mut response = PolicyResponse::from_bytes(&body).unwrap();
        if let Some(first) = response.changes.0.first_mut() {
            let mut change: Signed<PolicyChange> = first.0.clone();
            change.sig.0[0] ^= 1;
            *first = Nested(change);
        }
        response.to_bytes()
    }));
    let list = phone
        .service
        .device_list(std::slice::from_ref(&remote))
        .await
        .unwrap();
    assert!(
        list.alerts.iter().any(|a| matches!(a,
            DeviceAlert::UnverifiedWorkspace { workspace, .. } if *workspace == other.workspace)),
        "{:?}",
        list.alerts
    );
}
