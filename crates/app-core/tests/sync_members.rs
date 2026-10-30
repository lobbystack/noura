//! Approving an invited member's device end to end (seam 4, sync-v2.md
//! §8, §10, §11): the invitee's request names the invitation with
//! `invite_proof` and commits to its account head; only the inviting
//! device, after checking the proof, offers, and its commit is one
//! `add_member` recording that head, with an envelope for each card active
//! there and a fresh checkpoint of the workspace domain. The invitee checks
//! the offer and the `add_member`, reads nothing before, and records its
//! account's head so its other devices trust the workspace. The forged
//! changes of `sync-v2-policy.json` get `forbidden` or `id_conflict` from
//! the server and `sync_policy_unauthorized` from the client.

mod sync_support;

use std::sync::Arc;

use app_core::sync::approval::CandidateStep;
use app_core::sync::invitations::{InvitationBook, InvitationLink, NewInvitation};
use app_core::sync::joining::{InvitedCandidate, InvitedRequest};
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::{DeviceCredentials, KeptHeads, SyncError, SyncService};
use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::{Decode, Encode, List, Nested, Sorted};
use sync_protocol::messages::{PendingResponse, PolicyResponse};
use sync_protocol::records::{AccountHead, PolicyChange, PolicyKind, Role, Signed};
use sync_protocol::types::{Digest, Id, SecretKey, from_hex, to_base64url};
use sync_store::{Backend, Session};
use sync_support::{Desktop, Folder, MemorySecrets, Proxy, Server, approve, first_desktop};

/// An owner's desktop with a synced workspace and an invitation to it,
/// and the link the owner sent.
struct Inviter {
    desktop: Desktop,
    sync: WorkspaceSync,
    folder: Folder,
    book: InvitationBook,
    link: String,
}

async fn inviter<B: Backend>(server: &Server<B>) -> Inviter {
    let mut desktop = first_desktop(server, "ana@example.com").await;
    let mut folder = Folder::new("Field notes");
    folder.write("Plan.md", b"# Plan\n");
    folder.write("Notes/Day one.md", b"Rain all morning.\n");
    let mut sync = WorkspaceSync::turn_on(
        &mut desktop.service,
        folder.engine.clone(),
        "Field notes",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.round(&mut desktop.service, 0).await.expect("a round");
    let mut book = InvitationBook::open(desktop.state.path(), desktop.id()).unwrap();
    let created = desktop
        .service
        .create_invitation(
            &mut book,
            NewInvitation {
                workspace: sync.workspace(),
                role: Role::Editor,
                workspace_name: "Field notes",
                inviter_name: "Ana",
            },
        )
        .await
        .expect("invited");
    Inviter {
        desktop,
        sync,
        folder,
        book,
        link: created.url.to_string(),
    }
}

/// The invitee opens and accepts the link, then posts its request.
async fn request(service: &mut SyncService, link: &str) -> InvitedCandidate {
    let link = InvitationLink::parse(link).expect("the link reads");
    let opened = service.open_invitation(&link).await.expect("it opens");
    service.accept_invitation(&link).await.expect("accepted");
    service
        .request_invited_approval(&link, &opened)
        .await
        .expect("the request")
}

/// The one listed request, which the inviting device offers on, and both
/// codes, which match.
async fn offer_and_compare(
    ana: &mut Inviter,
    service: &mut SyncService,
    candidate: &mut InvitedCandidate,
) -> app_core::sync::approval::OutstandingOffer {
    let workspace = ana.sync.workspace();
    let requests = ana
        .desktop
        .service
        .invited_requests(&ana.book, workspace)
        .await
        .expect("listed");
    let request = requests
        .iter()
        .find(|r| r.commit == candidate.commit() && r.head == *candidate.head())
        .expect("the request, with its proof checked");
    let mut offer = ana
        .desktop
        .service
        .offer_invited(request, ana.sync.remote_mut().expect("connected"))
        .await
        .expect("offered");
    let CandidateStep::Code(code) = service.poll_invited(candidate).await.expect("polled") else {
        panic!("a code");
    };
    assert_eq!(
        ana.desktop.service.approval_code(&mut offer).await.unwrap(),
        Some(code)
    );
    candidate.confirm();
    offer
}

fn code<T>(result: Result<T, SyncError>) -> String {
    match result {
        Ok(_) => panic!("expected a refusal"),
        Err(SyncError::Check(code)) => code.to_owned(),
        Err(SyncError::Refused { code, .. }) => code,
        Err(other) => panic!("unexpected error {other:?}"),
    }
}

/// Paths of the workspace's domains, keys and blobs: what carries a key or
/// ciphertext.
fn reads_content(path: &str) -> bool {
    ["/keys", "/domains/", "/ops", "/blobs", "/profile"]
        .iter()
        .any(|part| path.contains(part))
}

async fn invited_member_joins<B: Backend>(server: Server<B>) {
    let mut ana = inviter(&server).await;
    let workspace = ana.sync.workspace();

    // Ben's laptop and phone, both active at the head his request commits
    // to; the laptop talks through a proxy that records every request.
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let phone = approve(&server, &mut ben, &mut []).await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = ben.connect_through(&proxy.base).await;
    // The link names the server, so the laptop opens it there.
    let mut candidate = request(&mut ben.service, &ana.link).await;
    assert_eq!(candidate.head().seqno, 2);
    let offer = offer_and_compare(&mut ana, &mut laptop, &mut candidate).await;
    assert!(
        !laptop.join_invited(&candidate).await.expect("not yet"),
        "nothing committed yet"
    );

    let revision = ana
        .sync
        .approve_member(&mut ana.desktop.service, offer)
        .await
        .expect("the approval commits");
    let committed = ana
        .sync
        .remote_mut()
        .unwrap()
        .policy()
        .change(revision)
        .unwrap();
    assert!(matches!(
        committed.body.kind,
        PolicyKind::AddMember { account, role: Role::Editor, .. } if account == ben.credentials.account
    ));
    assert!(committed.body.heads.iter().any(|h| h == candidate.head()));
    let domain = ana.sync.remote_mut().unwrap().domain;
    let checkpoint = ana
        .desktop
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("served")
        .expect("a checkpoint");
    assert_eq!(
        checkpoint.bundle.checkpoint.0.body.policy_revision,
        revision - 1,
        "the fresh checkpoint covers what the approver applied before the add_member"
    );
    assert!(
        proxy.requests().iter().all(|r| !reads_content(&r.path)),
        "the invitee read no key or ciphertext before it checked the add_member"
    );
    assert!(laptop.join_invited(&candidate).await.expect("joined"));
    assert!(
        ana.desktop
            .service
            .workspace_invitations(&mut ana.book, workspace)
            .await
            .expect("listed")
            .is_empty(),
        "the approved invitation ended"
    );

    // Ben's phone doesn't trust the workspace until a device of his
    // account records his head there.
    let mut phone = phone;
    let mut phone_folder = Folder::new("Phone");
    assert_eq!(
        code(
            WorkspaceSync::download(
                &mut phone.service,
                phone_folder.engine.clone(),
                workspace,
                &mut phone_folder.registry,
            )
            .await
        ),
        "sync_workspace_unverified"
    );

    let mut copy = Folder::new("Copy");
    let mut sync_b = WorkspaceSync::download(
        &mut laptop,
        copy.engine.clone(),
        workspace,
        &mut copy.registry,
    )
    .await
    .expect("the download starts");
    sync_b.round(&mut laptop, 0).await.expect("a round");
    assert_eq!(copy.files(), ana.folder.files());
    let remote = sync_b.remote_mut().expect("connected");
    let own = laptop
        .pending_heads(remote)
        .await
        .expect("pending")
        .into_iter()
        .find(|p| p.own)
        .expect("its own head, recorded by another account's device");
    assert!(
        laptop
            .push_pending_head(remote, &own)
            .await
            .expect("recorded")
    );

    let mut phone_sync = WorkspaceSync::download(
        &mut phone.service,
        phone_folder.engine.clone(),
        workspace,
        &mut phone_folder.registry,
    )
    .await
    .expect("the phone trusts the workspace through the laptop's change");
    phone_sync
        .round(&mut phone.service, 0)
        .await
        .expect("a round");
    assert_eq!(phone_folder.files(), ana.folder.files());
    drop(copy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invited_member_joins_on_sqlite() {
    invited_member_joins(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invited_member_joins_on_postgres() {
    invited_member_joins(Server::start_postgres().await).await;
}

/// An offer from a device of the inviting account other than the inviting
/// device, a request whose proof or card a server changed, and an
/// `add_member` that records another head than the request committed to
/// all end the attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_inviting_device_approves_the_committed_head() {
    let server = Server::start().await;
    let mut ana = inviter(&server).await;
    let workspace = ana.sync.workspace();
    let mut ana_two = approve(
        &server,
        &mut ana.desktop,
        std::slice::from_mut(ana.sync.remote_mut().expect("connected")),
    )
    .await;
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let mut candidate = request(&mut ben.service, &ana.link).await;

    // The inviting device checks the proof under the secret it kept: a
    // request whose proof or card the server changed isn't listed.
    let proxy = Proxy::start(&server.base).await;
    let mut through = ana.desktop.connect_through(&proxy.base).await;
    for swap in [false, true] {
        proxy.tamper(Box::new(move |path, body| {
            if !path.ends_with("/pending-devices") {
                return body;
            }
            let mut pending = PendingResponse::from_bytes(&body).expect("pending");
            for entry in pending.pending.0.iter_mut() {
                if swap {
                    let keys = DeviceKeys::generate().unwrap();
                    entry.card = Nested(keys.card(entry.card.0.account));
                } else {
                    entry.invite_proof = Some(Digest([4; 32]));
                }
            }
            pending.to_bytes()
        }));
        assert!(
            through
                .invited_requests(&ana.book, workspace)
                .await
                .expect("listed")
                .is_empty()
        );
    }
    let listed = ana
        .desktop
        .service
        .invited_requests(&ana.book, workspace)
        .await
        .expect("listed");
    assert_eq!(listed.len(), 1);

    // Another owner device of Ana's account offers: the server takes it,
    // and Ben's device refuses an offer whose card isn't the inviter's.
    let request_copy: InvitedRequest = listed[0].clone();
    let mut remote = ana_two.service.open_workspace(workspace).await.unwrap();
    ana_two
        .service
        .offer_invited(&request_copy, &mut remote)
        .await
        .expect("the server takes an offer from the inviting account");
    assert_eq!(
        code(ben.service.poll_invited(&mut candidate).await),
        "sync_approval_mismatch"
    );

    // A new attempt; Ben's chain gains a card after the request, and an
    // add_member recording that later head, though valid for the server,
    // isn't the one the request committed to.
    let mut candidate = request(&mut ben.service, &ana.link).await;
    let _offer = offer_and_compare(&mut ana, &mut ben.service, &mut candidate).await;
    let _phone = approve(&server, &mut ben, &mut []).await;
    let later = ben.service.own_chain().await.unwrap().head();
    assert_eq!(later.seqno, 2);
    let remote = ana.sync.remote_mut().unwrap();
    let snapshot = remote.policy().head_snapshot().clone();
    let (revision, previous) = remote.policy().head();
    let ana_head = snapshot.members[&ana.desktop.credentials.account]
        .head
        .clone();
    let invitation = ana.book.invitations()[0].invitation;
    let keys = &ana.desktop.credentials.keys;
    let forged = Signed::sign(
        PolicyChange {
            workspace,
            revision: revision + 1,
            previous,
            signer: keys.device(),
            heads: Sorted::new(sorted(vec![ana_head, later])).unwrap(),
            kind: PolicyKind::AddMember {
                account: ben.credentials.account,
                role: Role::Editor,
                invitation,
            },
        },
        keys.signing(),
    );
    sync_store::conformance::inject_change(&server.store, forged, vec![remote.domain], None)
        .await
        .expect("injected");
    assert_eq!(
        code(ben.service.join_invited(&candidate).await),
        "sync_approval_mismatch"
    );
    assert!(
        ben.service.kept_heads().anchor(&workspace).is_none(),
        "the device keeps no anchor of the workspace"
    );
}

fn sorted(mut heads: Vec<AccountHead>) -> Vec<AccountHead> {
    heads.sort_by_key(|h| h.account);
    heads
}

/// A fixture device of `sync-v2-policy.json`, signed in to `server`.
async fn fixture_device<B: Backend>(
    server: &Server<B>,
    doc: &serde_json::Value,
    name: &str,
    base: &str,
) -> SyncService {
    let key = &doc["keys"][name];
    let hex = |field: &str| from_hex(key[field].as_str().unwrap()).unwrap();
    let seed = |field: &str| SecretKey(hex(field).try_into().unwrap());
    let keys = DeviceKeys::from_seeds(&seed("signing_seed"), seed("hpke_seed"));
    let account = Id::from_hex(key["account"].as_str().unwrap()).unwrap();
    let token: [u8; 32] = sync_protocol::crypto::random_array().unwrap();
    server
        .store
        .create_session(token, Session::new(account, keys.device()))
        .await
        .unwrap();
    let (links, genesis) = {
        let mut client = app_core::sync::SyncClient::connect(&server.base, &to_base64url(&token))
            .await
            .unwrap();
        client.whole_chain(account, None).await.unwrap()
    };
    let reference = AccountHead {
        account,
        genesis,
        seqno: 1,
        head: links[0].digest(),
    };
    let secrets = Arc::new(MemorySecrets::default());
    let state = tempfile::tempdir().unwrap();
    let heads = KeptHeads::open(state.path(), account, keys.device(), &*secrets).unwrap();
    std::mem::forget(state);
    SyncService::connect(
        base,
        DeviceCredentials {
            account,
            keys,
            session_token: to_base64url(&token),
            reference_head: reference,
        },
        heads,
        secrets,
    )
    .await
    .unwrap()
}

/// Each forged change of the fixture: the server refuses its push with the
/// code the fixture's error maps to, and a client served it by a tampering
/// proxy reports `sync_policy_unauthorized`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forged_policy_changes_are_refused_on_both_sides() {
    let path = format!(
        "{}/../../docs/workspace-format/fixtures/sync-v2-policy.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (case, server_code) in [
        (
            "another_account_outside_add_member_accept_reset_and_account_heads",
            "forbidden",
        ),
        ("add_member_for_a_member", "forbidden"),
        ("one_device_id_under_two_accounts", "id_conflict"),
    ] {
        let server = Server::start().await;
        let (workspace, forged, session) =
            sync_store::conformance::forged_policy_case(&server.store, case).await;
        // The account the forged change brings in has a request naming an
        // invitation to the workspace, so the workspace's devices read its
        // chain, as they would an invitee's.
        for head in forged
            .body
            .heads
            .iter()
            .filter(|h| h.account != session.account)
        {
            server
                .store
                .store_approval_request(
                    Session::new(head.account, Id::random().unwrap()),
                    Digest(sync_protocol::crypto::random_array().unwrap()),
                    Some((Id::random().unwrap(), workspace)),
                )
                .await
                .unwrap();
        }
        let body = sync_protocol::messages::PushRequest {
            records: List(vec![sync_protocol::messages::RecordRef::Policy(
                forged.clone(),
            )]),
        }
        .to_bytes();
        match server.store.push(session, workspace, body).await {
            Err(sync_store::Error::Refused(refusal)) => {
                assert_eq!(refusal.code, server_code, "{case}")
            }
            other => panic!("{case}: {other:?}"),
        }

        let proxy = Proxy::start(&server.base).await;
        let mut client = fixture_device(&server, &doc, "alice laptop", &proxy.base).await;
        let honest = client.client().policy(workspace, 0).await.expect("served");
        let mut served = PolicyResponse {
            head_revision: honest.head_revision + 1,
            changes: honest.changes.clone(),
            account_heads: honest.account_heads.clone(),
        };
        served.changes.0.push(Nested(forged.clone()));
        let bytes = served.to_bytes();
        proxy.forge(Box::new(move |method, path| {
            (method == "GET" && path.ends_with("/policy")).then(|| (200, bytes.clone()))
        }));
        assert_eq!(
            code(client.open_workspace(workspace).await),
            "sync_policy_unauthorized",
            "{case}"
        );
    }
}
