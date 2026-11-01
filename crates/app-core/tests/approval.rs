//! Approving a new device of your account (seam 4, sync-v2.md §10): the
//! new device commits to a nonce, a device you hold offers its head and an
//! anchor per workspace, both show one 8-digit code, and the approver
//! appends `add_device` and records the new head with the new card's
//! envelope, after which the new device verifies the workspace and reads
//! what the first one wrote.

mod sync_support;

use app_core::sync::SyncError;
use app_core::sync::approval::CandidateStep;
use sync_protocol::codec::{Decode, Encode, List, Nested};
use sync_protocol::messages::OffersResponse;
use sync_protocol::records::{Offer, Signed};
use sync_protocol::types::{Digest, Key};
use sync_store::Backend;
use sync_support::{Proxy, Server, first_desktop, new_desktop};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_device_joins_through_an_approval_and_reads_the_workspace() {
    approval(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_device_joins_through_an_approval_on_postgres() {
    approval(Server::start_postgres().await).await;
}

async fn approval<B: Backend>(server: Server<B>) {
    let mut laptop = first_desktop(&server, "ada@example.com").await;
    let account = laptop.credentials.account;
    let mut workspace = laptop
        .service
        .create_workspace("Field notes")
        .await
        .expect("created");
    laptop
        .service
        .push_file(&mut workspace, "First.md", b"# First\n")
        .await
        .expect("pushed");

    let mut phone = new_desktop(&server, account, &server.base).await;
    let fingerprint = phone.candidate.fingerprint();
    assert_eq!(
        phone.candidate.poll().await.expect("polled"),
        CandidateStep::Waiting,
        "no key and no code before an offer"
    );

    // You select the request on the laptop, which shows the new device's
    // fingerprint and sends one offer.
    let pending = laptop.service.pending_devices().await.expect("listed");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].fingerprint, fingerprint);
    let mut offer = laptop
        .service
        .offer_approval(&pending[0], std::slice::from_mut(&mut workspace))
        .await
        .expect("offered");
    assert!(
        laptop
            .service
            .offer_approval(&pending[0], std::slice::from_mut(&mut workspace))
            .await
            .is_err(),
        "one offer outstanding"
    );
    assert_eq!(
        laptop.service.approval_code(&mut offer).await.unwrap(),
        None
    );
    let CandidateStep::Code(phone_code) = phone.candidate.poll().await.expect("the offer") else {
        panic!("a code");
    };
    let laptop_code = laptop
        .service
        .approval_code(&mut offer)
        .await
        .expect("the reveal")
        .expect("a code");
    assert_eq!(phone_code, laptop_code, "both screens show one code");
    assert_eq!(phone_code.len(), 9);
    assert!(phone_code.chars().all(|c| c.is_ascii_digit() || c == ' '));

    // You confirm on both devices.
    phone.candidate.confirm();
    let mut heads_before = phone.heads.kept_of(&account);
    heads_before.sort_by_key(|h| h.seqno);
    assert!(
        heads_before.is_empty(),
        "the new device keeps no head before"
    );
    assert!(
        phone
            .candidate
            .joined(&mut phone.heads, &*phone.secrets)
            .await
            .expect("checked")
            .is_none(),
        "nothing until the approver commits"
    );
    let head = laptop
        .service
        .confirm_approval(offer, std::slice::from_mut(&mut workspace))
        .await
        .expect("committed");
    assert_eq!(head.seqno, 2, "add_device right after the approver's head");
    assert_eq!(
        workspace.policy_revision(),
        2,
        "account_heads recorded the head"
    );

    let mut phone = phone.joined(&server.base).await;
    assert_eq!(phone.credentials.reference_head.seqno, 1);
    let mut remote = phone
        .service
        .open_workspace(workspace.workspace)
        .await
        .expect("the new device verifies the workspace");
    let files = phone.service.pull_files(&mut remote).await.expect("pulled");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].bytes, b"# First\n");

    // What the new device writes, the first one reads.
    phone
        .service
        .push_file(&mut remote, "Second.md", b"# Second\n")
        .await
        .expect("the new device writes");
    let files = laptop
        .service
        .pull_files(&mut workspace)
        .await
        .expect("pulled");
    assert_eq!(files.len(), 2);
    assert_eq!(files[1].bytes, b"# Second\n");
}

/// A second offer for one commitment before you confirm ends the attempt
/// with `sync_approval_mismatch` and shows no code (§10).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_offer_ends_the_attempt() {
    let server = Server::start().await;
    let mut laptop = first_desktop(&server, "bea@example.com").await;
    let account = laptop.credentials.account;
    let mut phone = new_desktop(&server, account, &server.base).await;
    let pending = laptop.service.pending_devices().await.unwrap();
    let mut workspaces = Vec::new();
    let offer = laptop
        .service
        .offer_approval(&pending[0], &mut workspaces)
        .await
        .expect("offered");
    // A second offer, as a second device of yours would send.
    let mut client =
        app_core::sync::SyncClient::connect(&server.base, &laptop.credentials.session_token)
            .await
            .unwrap();
    let chain = laptop.service.own_chain().await.unwrap();
    let second = Signed::sign(
        Offer {
            approver_card: Nested(laptop.credentials.card()),
            candidate_card_digest: pending[0].fingerprint,
            candidate_commit: pending[0].commit,
            candidate_head: None,
            approver_head: chain.head(),
            nonce: Key([3; 32]),
            anchors: Default::default(),
        },
        laptop.credentials.keys.signing(),
    );
    client.post_offer(&second).await.expect("a second offer");
    let ended = phone.candidate.poll().await.expect_err("the attempt ends");
    assert!(matches!(ended, SyncError::Check("sync_approval_mismatch")));
    assert_eq!(phone.candidate.code(), None, "no code shows");
    laptop.service.cancel_approval(offer);
}

/// A server that swaps the candidate's card in the offer it serves gets no
/// reveal: the candidate checks that the offer names its own card and
/// commitment (§10).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_swapped_card_gets_no_reveal() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "cai@example.com").await;
    let account = laptop.credentials.account;
    let mut phone = new_desktop(&server, account, &proxy.base).await;
    let pending = laptop.service.pending_devices().await.unwrap();
    let mut none = Vec::new();
    let mut offer = laptop
        .service
        .offer_approval(&pending[0], &mut none)
        .await
        .expect("offered");
    proxy.tamper(Box::new(|path, body| {
        if path != "/v2/approvals" {
            return body;
        }
        let mut offers = OffersResponse::from_bytes(&body).unwrap();
        for offer in offers.offers.0.iter_mut() {
            offer.0.body.candidate_card_digest = Digest([1; 32]);
        }
        OffersResponse {
            offers: List(offers.offers.0),
        }
        .to_bytes()
    }));
    let refused = phone.candidate.poll().await.expect_err("mismatch");
    assert!(matches!(
        refused,
        SyncError::Check("sync_approval_mismatch")
    ));
    proxy.honest();
    assert_eq!(
        laptop.service.approval_code(&mut offer).await.unwrap(),
        None,
        "the candidate revealed nothing"
    );
}
