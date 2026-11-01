//! The workspace profile (seam 4, sync-v2.md §25, with the records of
//! `sync-v2-profile.json`): owners and admins rename the workspace by
//! pushing a new profile, other devices take the new name, a client
//! reports `sync_profile_rollback` for an older profile or another one at
//! the same revision, and when a change takes the profile's signer out of
//! the workspace, an owner device signs the profile again.

mod sync_support;

use app_core::sync::workspace::{WorkspaceSync, synced_workspaces};
use sync_protocol::codec::{Decode, Encode, Nested};
use sync_protocol::messages::WorkspacesResponse;
use sync_protocol::records::{Profile, Signed};
use sync_support::{Pair, approve, pair};

async fn latest_profile(pair: &mut Pair<sync_store_sqlite::SqliteBackend>) -> Signed<Profile> {
    let workspace = pair.sync_a.workspace();
    pair.desktop
        .service
        .client()
        .workspaces()
        .await
        .unwrap()
        .workspaces
        .0
        .into_iter()
        .find(|w| w.workspace == workspace)
        .and_then(|w| w.profile)
        .expect("a profile")
        .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_reaches_the_other_desktop_and_an_older_profile_is_refused() {
    let mut pair = pair().await;
    let first = latest_profile(&mut pair).await;
    pair.sync_a
        .rename(&mut pair.service, "Garden notes")
        .await
        .expect("renamed");
    assert_eq!(pair.sync_a.name(), "Garden notes");
    let listed = synced_workspaces(&mut pair.desktop.service).await.unwrap();
    assert_eq!(listed[0].1, "Garden notes");
    pair.sync_b
        .connect(&mut pair.desktop.service)
        .await
        .expect("the desktop takes the new name");
    assert_eq!(pair.sync_b.name(), "Garden notes");

    // A server that serves the first profile again rolls the name back.
    let served = Nested(first.clone());
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/v2/workspaces") {
            return body;
        }
        let mut response = WorkspacesResponse::from_bytes(&body).unwrap();
        for workspace in response.workspaces.0.iter_mut() {
            workspace.profile = Some(served.clone());
        }
        response.to_bytes()
    }));
    let error = pair
        .sync_a
        .connect(&mut pair.service)
        .await
        .expect_err("refused");
    assert_eq!(error.code(), Some("sync_profile_rollback"));

    // So does another profile at the accepted revision.
    let mut other = latest_profile(&mut pair).await;
    other.body.policy_revision = first.body.policy_revision;
    let other = Signed::sign(other.body, pair.laptop.credentials.keys.signing());
    let served = Nested(other);
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/v2/workspaces") {
            return body;
        }
        let mut response = WorkspacesResponse::from_bytes(&body).unwrap();
        for workspace in response.workspaces.0.iter_mut() {
            workspace.profile = Some(served.clone());
        }
        response.to_bytes()
    }));
    let error = pair
        .sync_a
        .connect(&mut pair.service)
        .await
        .expect_err("refused");
    assert_eq!(error.code(), Some("sync_profile_rollback"));
    pair.proxy.honest();
    pair.sync_a
        .connect(&mut pair.service)
        .await
        .expect("honest");
    assert_eq!(pair.sync_a.name(), "Garden notes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_signers_profile_is_signed_again() {
    let mut pair = pair().await;
    // A third device of the account renames the workspace, then the
    // laptop removes it.
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    let mut c = sync_support::Folder::new("Third");
    let mut sync_c = WorkspaceSync::download(
        &mut third.service,
        c.engine.clone(),
        pair.sync_a.workspace(),
        &mut c.registry,
    )
    .await
    .expect("the third device downloads");
    sync_c
        .rename(&mut third.service, "Named on the third device")
        .await
        .expect("renamed");
    assert_eq!(latest_profile(&mut pair).await.body.signer, third.id());
    pair.laptop
        .service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
        )
        .await
        .expect("removed");
    // The laptop accepts the removal and signs the profile again.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let latest = latest_profile(&mut pair).await;
    assert_eq!(latest.body.signer, pair.laptop.id());
    assert_eq!(latest.body.profile_revision, 3);
    assert_eq!(pair.sync_a.name(), "Named on the third device");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .connect(&mut pair.desktop.service)
        .await
        .unwrap();
    assert_eq!(pair.sync_b.name(), "Named on the third device");
}
