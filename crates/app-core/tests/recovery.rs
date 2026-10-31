//! Recovery kits (seam 4 and seam 1, sync-v2.md §12): exporting a kit adds
//! a recovery identity to the chain and records it in every workspace;
//! the kit is a new file, mode 0600 on Unix, outside every workspace, with
//! a secret shown once as 8 groups of 4. After losing every device, a new
//! computer signs in, opens the kit with the secret typed any way, proves
//! the identity, verifies the chain and the workspaces from the kit, shows
//! what changed since, and on your confirmation recovers its own card,
//! removes every other card, records the heads and rotates. A replaced kit
//! no longer recovers.

mod sync_support;

use std::sync::Arc;

use app_core::sync::device::enroll_card;
use app_core::sync::recovery::{Recovery, open_kit};
use app_core::sync::{SyncClient, SyncError};
use sync_key_envelope::v2::{DeviceKeys, next_link, recovery_proof};
use sync_protocol::codec::Nested;
use sync_protocol::records::{KitPayload, LinkKind};
use sync_protocol::types::Id;
use sync_store::Backend;
use sync_support::{MemorySecrets, Server, approve, first_desktop, head_of};

fn retyped(secret: &str) -> String {
    // Lowercase, with hyphens for spaces, as you might type it back.
    secret.to_lowercase().replace(' ', "-")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kit_recovers_the_account_after_every_device_is_lost() {
    recovery(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kit_recovers_the_account_after_every_device_is_lost_on_postgres() {
    recovery(Server::start_postgres().await).await;
}

async fn recovery<B: Backend>(server: Server<B>) {
    let mut laptop = first_desktop(&server, "ada@example.com").await;
    let account = laptop.credentials.account;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    laptop
        .service
        .push_file(&mut workspace, "Kept.md", b"# Kept\n")
        .await
        .unwrap();

    // A workspace folder, which the kit may not go into.
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(folder.path().join(".noura")).unwrap();
    std::fs::write(folder.path().join(".noura/workspace.yaml"), "id: test\n").unwrap();
    let refused = laptop
        .service
        .export_kit(
            &folder.path().join("kit.noura-kit"),
            &[],
            std::slice::from_mut(&mut workspace),
        )
        .await
        .expect_err("not inside a workspace");
    assert!(refused.to_string().contains("outside every workspace"));

    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Noura recovery kit");
    let export = laptop
        .service
        .export_kit(&path, &[], std::slice::from_mut(&mut workspace))
        .await
        .expect("exported");
    let secret = export.secret.to_string();
    assert_eq!(secret.len(), 39, "32 characters in 8 groups of 4: {secret}");
    assert_eq!(secret.split(' ').count(), 8);
    assert!(secret.split(' ').all(|g| g.len() == 4));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Nothing from the kit enters the workspace folder.
    let bytes = std::fs::read(&path).unwrap();
    for file in walk(folder.path()) {
        assert_ne!(std::fs::read(&file).unwrap(), bytes);
    }
    assert!(
        workspace
            .policy()
            .head_snapshot()
            .devices
            .values()
            .any(|d| d.recovery)
    );

    // A device a thief added from the stolen laptop, after the kit, and a
    // workspace the kit doesn't list.
    let thief = approve(&server, &mut laptop, std::slice::from_mut(&mut workspace)).await;
    let later = laptop.service.create_workspace("Later").await.unwrap();

    assert!(matches!(
        open_kit(&path, "AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA"),
        Err(SyncError::Check("sync_recovery_kit_invalid"))
    ));
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    let tampered_path = outside.path().join("tampered");
    std::fs::write(&tampered_path, tampered).unwrap();
    assert!(matches!(
        open_kit(&tampered_path, &secret),
        Err(SyncError::Check("sync_recovery_kit_invalid"))
    ));

    // A new computer with an empty credential store.
    let kit = open_kit(&path, &retyped(&secret)).expect("the kit opens");
    let signed_in = server.sign_in(account, &server.base).await;
    let secrets = Arc::new(MemorySecrets::default());
    let state = tempfile::tempdir().unwrap();
    let recovery = Recovery::begin(&signed_in, kit, secrets.clone(), state.path())
        .await
        .expect("the kit's identity proves and everything verifies");
    let review = recovery.review().clone();
    assert!(review.unverified.is_empty(), "{:?}", review.unverified);
    assert!(
        review
            .links
            .iter()
            .any(|l| l.kind == "add_device" && l.signer == laptop.id()),
        "the thief's card shows for review: {:?}",
        review.links
    );
    let mut devices: Vec<_> = review.devices.iter().map(|c| c.device).collect();
    devices.sort();
    let mut expected = vec![laptop.id(), thief.id()];
    expected.sort();
    assert_eq!(devices, expected);
    assert!(
        review
            .members
            .iter()
            .any(|(w, members)| *w == later.workspace && members == &vec![account]),
        "the workspace the kit doesn't list shows with its members"
    );

    let (mut service, mut workspaces, added) =
        recovery.finish(&[], &*secrets).await.expect("recovered");
    assert!(added.is_empty(), "no member joined after the kit");
    let chain = service.own_chain().await.unwrap();
    let head = chain.head_seqno();
    let me = service.credentials().keys.device();
    assert!(chain.is_active_at(&me, head));
    assert_eq!(
        chain.active_at(head).len(),
        1,
        "every other card, the identity included, is gone"
    );
    let remote = workspaces
        .iter_mut()
        .find(|w| w.workspace == workspace.workspace)
        .expect("the kit's workspace");
    assert_eq!(
        remote.epoch(),
        2,
        "the recovery rotated once it removed the other cards"
    );
    let files = service.pull_files(remote).await.expect("pulled");
    assert!(files.iter().any(|f| f.bytes == b"# Kept\n"));
    assert!(
        workspaces.iter().any(|w| w.workspace == later.workspace),
        "the workspace the kit doesn't list, verified in step 3"
    );
    let refused = laptop
        .service
        .pull_files(&mut workspace)
        .await
        .expect_err("the lost laptop was removed");
    assert_eq!(refused.code(), Some("device_removed"));

    // The recovered device exports a new kit.
    let renewed = outside.path().join("New kit");
    service
        .export_kit(&renewed, &[], &mut workspaces)
        .await
        .expect("a new kit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_kit_no_longer_recovers() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "bea@example.com").await;
    let account = laptop.credentials.account;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let outside = tempfile::tempdir().unwrap();
    let first = outside.path().join("First kit");
    let old = laptop
        .service
        .export_kit(&first, &[], std::slice::from_mut(&mut workspace))
        .await
        .unwrap();
    assert!(laptop.service.kit_replacement().await.unwrap().is_empty());
    laptop
        .service
        .export_kit(
            &outside.path().join("Second kit"),
            &[],
            std::slice::from_mut(&mut workspace),
        )
        .await
        .expect("replaced");
    let chain = laptop.service.own_chain().await.unwrap();
    assert_eq!(
        chain
            .active_at(chain.head_seqno())
            .iter()
            .filter(|s| s.recovery)
            .count(),
        1,
        "one kit works at a time"
    );

    let kit = open_kit(&first, &old.secret).unwrap();
    let signed_in = server.sign_in(account, &server.base).await;
    let state = tempfile::tempdir().unwrap();
    let result = Recovery::begin(
        &signed_in,
        kit,
        Arc::new(MemorySecrets::default()),
        state.path(),
    )
    .await;
    assert!(matches!(result, Err(SyncError::KitRevoked)));
}

/// An export first creates the kit file, so a folder you can't write to
/// stops it before the chain changes and the old kit keeps working; one
/// that fails after the kit's links committed removes the half-written
/// file and asks you to export again (§12).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kit_that_cant_be_saved_leaves_the_old_kit_or_asks_again() {
    use std::os::unix::fs::PermissionsExt;

    use app_core::sync::recovery::KIT_NOT_SAVED;
    use sync_protocol::codec::{Encode as _, Text};
    use sync_protocol::messages::{ErrorBody, server_error_status};
    use sync_support::{Desktop, Proxy};

    let server = Server::start().await;
    let laptop = first_desktop(&server, "cy@example.com").await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = Desktop::connect(
        &proxy.base,
        laptop.credentials.clone(),
        laptop.secrets.clone(),
        laptop.state,
    )
    .await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let outside = tempfile::tempdir().unwrap();
    let first = outside.path().join("First kit");
    let old = laptop
        .service
        .export_kit(&first, &[], std::slice::from_mut(&mut workspace))
        .await
        .unwrap();
    let before = laptop.service.own_chain().await.unwrap().head();

    let locked = outside.path().join("Locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let refused = laptop
        .service
        .export_kit(
            &locked.join("Second kit"),
            &[],
            std::slice::from_mut(&mut workspace),
        )
        .await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    let Err(SyncError::Invalid(text)) = refused else {
        panic!("the export fails: {refused:?}");
    };
    assert!(text.contains("couldn't be written"), "{text}");
    assert_eq!(
        laptop.service.own_chain().await.unwrap().head(),
        before,
        "the chain didn't change"
    );
    let kit = open_kit(&first, &old.secret).unwrap();
    let chain = laptop.service.own_chain().await.unwrap();
    assert!(
        chain.is_active_at(&kit.card.0.device, chain.head_seqno()),
        "the old kit still works"
    );

    // A server that refuses to record the new head after the links
    // committed.
    let body = ErrorBody {
        code: Text::new("internal").unwrap(),
        message: Text::new("the database failed").unwrap(),
        retry_after_seconds: None,
        detail: None,
    }
    .to_bytes();
    let status = server_error_status("internal").unwrap();
    proxy.forge(Box::new(move |method, path| {
        (method == "POST" && path.ends_with("/ops")).then(|| (status, body.clone()))
    }));
    let second = outside.path().join("Second kit");
    let failed = laptop
        .service
        .export_kit(&second, &[], std::slice::from_mut(&mut workspace))
        .await;
    proxy.honest();
    let Err(SyncError::Invalid(text)) = failed else {
        panic!("the export fails: {failed:?}");
    };
    assert_eq!(text, KIT_NOT_SAVED);
    assert!(!second.exists(), "no half-written kit stays");
    assert_ne!(laptop.service.own_chain().await.unwrap().head(), before);
}

/// A device that `kit`'s recovery identity recovers on a new computer, as
/// a recovery's step 4 appends it, while the account's other devices stay.
/// A kit whose account head a restore lost: the device compares it like
/// any state it accepted, and the app offers Reset devices.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kit_whose_head_a_restore_lost_says_so() {
    let mut pair = sync_support::pair().await;
    let account = pair.laptop.credentials.account;
    let copy = pair.server.backup("before.sqlite3").await;
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("kit");
    let export = pair
        .service
        .export_kit(
            &path,
            &[],
            std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
        )
        .await
        .expect("exported");
    let kit = open_kit(&path, &export.secret).expect("it opens");

    // The restore lost the links the kit added, under a new generation.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let restored = pair.server.put_back(&copy, true).await;
    pair.proxy.retarget(&restored.base);
    let mut signed_in = restored.sign_in(account, &restored.base).await;
    signed_in.server = pair.proxy.base.clone();
    let state = tempfile::tempdir().unwrap();
    let secrets: app_core::sync::Secrets = Arc::new(MemorySecrets::default());
    let refused = Recovery::begin(&signed_in, kit, secrets, state.path()).await;
    let error = refused.err().expect("refused");
    assert_eq!(error.code(), Some("sync_restore_lost"), "{error:?}");
}

async fn recovered_by<B: Backend>(server: &Server<B>, kit: &KitPayload) -> Id {
    let account = kit.account;
    let signed_in = server.sign_in(account, &server.base).await;
    let identity = DeviceKeys::from_seeds(&kit.signing_seed, kit.hpke_secret.clone());
    let keys = DeviceKeys::generate().unwrap();
    let mut client = SyncClient::connect(&server.base, &signed_in.session_token)
        .await
        .unwrap();
    enroll_card(&mut client, &keys, account).await.unwrap();
    let challenge = client.challenge().await.unwrap().challenge;
    let origin = client.origin().unwrap();
    client
        .prove_recovery(recovery_proof(
            &identity,
            &origin,
            &keys.card(account),
            challenge,
        ))
        .await
        .unwrap();
    let (links, _) = client.whole_chain(account, None).await.unwrap();
    let link = next_link(
        &identity,
        &head_of(&links),
        LinkKind::RecoverDevice {
            card: Nested(keys.card(account)),
        },
    );
    client.append_chain(vec![link]).await.unwrap();
    keys.device()
}

/// Replacing a kit removes the old identity, which takes down the devices it
/// recovered, so the app lists them first and keeps the ones you choose
/// (§12).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacing_a_kit_lists_the_devices_it_recovered_and_keeps_your_choice() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "cy@example.com").await;
    let mut workspace = laptop.service.create_workspace("Notes").await.unwrap();
    let outside = tempfile::tempdir().unwrap();
    let first = outside.path().join("First kit");
    let export = laptop
        .service
        .export_kit(&first, &[], std::slice::from_mut(&mut workspace))
        .await
        .unwrap();
    let kept = recovered_by(&server, &open_kit(&first, &export.secret).unwrap()).await;
    let listed: Vec<Id> = laptop
        .service
        .kit_replacement()
        .await
        .unwrap()
        .iter()
        .map(|card| card.device)
        .collect();
    assert_eq!(listed, vec![kept], "the device the kit recovered");

    // You keep it: the new kit's remove link spares it.
    let second = outside.path().join("Second kit");
    let export = laptop
        .service
        .export_kit(&second, &[kept], std::slice::from_mut(&mut workspace))
        .await
        .expect("replaced, keeping the device");
    let chain = laptop.service.own_chain().await.unwrap();
    assert!(chain.is_active_at(&kept, chain.head_seqno()));
    assert!(
        laptop.service.kit_replacement().await.unwrap().is_empty(),
        "the new kit recovered nothing yet"
    );

    // Without keeping, the next replacement takes its recovered device down.
    let gone = recovered_by(&server, &open_kit(&second, &export.secret).unwrap()).await;
    let listed: Vec<Id> = laptop
        .service
        .kit_replacement()
        .await
        .unwrap()
        .iter()
        .map(|card| card.device)
        .collect();
    assert_eq!(listed, vec![gone]);
    laptop
        .service
        .export_kit(
            &outside.path().join("Third kit"),
            &[],
            std::slice::from_mut(&mut workspace),
        )
        .await
        .expect("replaced");
    let chain = laptop.service.own_chain().await.unwrap();
    let head = chain.head_seqno();
    assert!(!chain.is_active_at(&gone, head));
    assert!(chain.is_active_at(&kept, head));
    assert!(chain.is_active_at(&laptop.id(), head));
    assert_eq!(
        chain.active_at(head).iter().filter(|s| s.recovery).count(),
        1
    );
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}
