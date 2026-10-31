//! A device that learns of a removal before the workspace records it (seam
//! 4, sync-v2.md §9, as the refused-recording cases of `sync-v2-keys.json`
//! describe): it encrypts nothing in the workspace domain until a change
//! records the head and the domain rotates past it, then uploads what its
//! files held meanwhile under the new epoch. That holds for the remover
//! whose recording the server refuses and for a member device that
//! learned the head.

mod sync_support;

use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::ChainsResponse;
use sync_support::{Pair, Server, approve, pair, pair_of_accounts_on};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_device_that_learned_the_head_holds_its_writes() {
    let mut pair = pair().await;
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the desktop takes the new member's head");
    // The laptop removes the third device and records nothing yet.
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");

    pair.b
        .write("Plan.md", b"# Plan, written during the removal\n");
    let round = pair
        .sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert!(pair.sync_b.removal_hold(), "the desktop holds its writes");
    assert_eq!(round.captured, 0);
    assert!(
        pair.sync_b.outbox().is_empty(),
        "nothing encrypted meanwhile"
    );
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    assert_eq!(pair.a.files()["Plan.md"], b"# Plan\n");

    // The laptop records the head and rotates; the hold ends.
    pair.laptop
        .service
        .record_heads(pair.sync_a.remote_mut().unwrap(), &[])
        .await
        .expect("recorded");
    let round = pair
        .sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert!(!pair.sync_b.removal_hold());
    assert_eq!(round.pushed, 1);
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    assert_eq!(
        pair.a.files()["Plan.md"],
        b"# Plan, written during the removal\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_remover_holds_while_the_server_refuses_the_recording() {
    let Pair {
        server,
        proxy,
        mut laptop,
        mut service,
        a,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
    } = pair().await;
    let third = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().unwrap()),
    )
    .await;
    sync_a.round(&mut service, 0).await.unwrap();
    // The server refuses every push, so the change that records the
    // removal doesn't commit.
    proxy.forge(Box::new(|method, path| {
        (method == "POST" && path.ends_with("/ops")).then(|| (503, Vec::new()))
    }));
    let recorded = service
        .remove_device(
            third.id(),
            &[],
            std::slice::from_mut(sync_a.remote_mut().unwrap()),
        )
        .await;
    assert!(recorded.is_err(), "the recording didn't commit");
    a.write("Plan.md", b"# Plan, by the remover\n");
    let round = sync_a.round(&mut service, 0).await.expect("a round");
    assert!(sync_a.removal_hold());
    assert_eq!(round.captured, 0);
    assert!(sync_a.outbox().is_empty());

    proxy.honest();
    service
        .record_heads(sync_a.remote_mut().unwrap(), &[])
        .await
        .expect("recorded");
    let round = sync_a.round(&mut service, 0).await.expect("a round");
    assert!(!sync_a.removal_hold());
    assert_eq!(round.pushed, 1);
    sync_b.round(&mut desktop.service, 0).await.unwrap();
    assert_eq!(b.files()["Plan.md"], b"# Plan, by the remover\n");
}

/// A server that serves the removed head everywhere but to the hold's own
/// read can't end the hold there (§9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chain_cut_below_the_accepted_head_ends_no_hold() {
    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    pair.laptop
        .service
        .record_heads(pair.sync_a.remote_mut().unwrap(), &[])
        .await
        .expect("the third device's head goes in");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    let account = pair.laptop.credentials.account;
    let genesis = pair.laptop.service.kept_heads().kept_of(&account)[0].genesis;
    pair.desktop
        .service
        .follow_account(account, genesis)
        .await
        .expect("the desktop accepts the head");
    let remote = pair.sync_b.remote_mut().unwrap();
    let hold = pair
        .desktop
        .service
        .removal_hold(remote)
        .await
        .expect("the hold");
    assert!(hold.held && !hold.over_limit);

    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/accounts/chains") {
            return body;
        }
        let mut response = ChainsResponse::from_bytes(&body).unwrap();
        for page in response.pages.0.iter_mut() {
            if page.account == account {
                page.links.0.pop();
                page.head_seqno -= 1;
            }
        }
        response.to_bytes()
    }));
    let mut tampered = pair.desktop.connect_through(&pair.proxy.base).await;
    let error = tampered
        .removal_hold(pair.sync_b.remote_mut().unwrap())
        .await
        .expect_err("a chain short of the kept head");
    assert_eq!(error.code(), Some("sync_account_forked"));
}
