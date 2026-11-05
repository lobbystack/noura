//! Invitations end to end (seam 4, sync-v2.md §11): the inviting device
//! generates the ID, token and secret, keeps the secret until the
//! invitation ends and never sends it; the invitee's app opens the link,
//! decrypts the context, shows the workspace, inviter and role, and
//! accepts, which grants nothing. The contexts of `sync-v2-invitation.json`
//! open through the server as the fixture says.

mod sync_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use app_core::sync::SyncError;
use app_core::sync::invitations::{
    INVITATION_UNUSABLE, InvitationBook, InvitationLink, NewInvitation, open_signed_out,
};
use app_core::sync::workspace::WorkspaceSync;
use local_core::sync::SyncCredentials;
use serde_json::Value;
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::InvitationStatus;
use sync_protocol::records::{InviteContext, Role, invite_token_hash};
use sync_protocol::types::{Id, SecretKey, from_hex, to_base64url, to_hex};
use sync_store::{Backend, InvitationRow, WriteTxn};
use sync_support::{Desktop, Folder, Server, first_desktop, pair};

fn fixture() -> Value {
    let path = format!(
        "{}/../../docs/workspace-format/fixtures/sync-v2-invitation.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture")).expect("JSON")
}

fn hex(value: &Value) -> Vec<u8> {
    from_hex(value.as_str().expect("hex text")).expect("hex")
}

fn unusable(result: Result<impl std::fmt::Debug, SyncError>) {
    let error = result.expect_err("the link doesn't open");
    assert_eq!(error.to_string(), INVITATION_UNUSABLE);
}

/// Every form in which a request could carry `secret`.
fn forms(secret: &[u8]) -> Vec<Vec<u8>> {
    vec![
        secret.to_vec(),
        to_base64url(secret).into_bytes(),
        to_hex(secret).into_bytes(),
    ]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// An owner's desktop with a synced workspace named "Field notes".
async fn owner<B: Backend>(server: &Server<B>) -> (Desktop, WorkspaceSync, Folder) {
    let mut desktop = first_desktop(server, "ana@example.com").await;
    let mut folder = Folder::new("Field notes");
    folder.write("Plan.md", b"# Plan\n");
    let mut sync = WorkspaceSync::turn_on(
        &mut desktop.service,
        folder.engine.clone(),
        "Field notes",
        &mut folder.registry,
    )
    .await
    .expect("sync turns on");
    sync.round(&mut desktop.service, 0).await.expect("a round");
    (desktop, sync, folder)
}

/// Creation, the invitee's view and acceptance, the inviter's list, and
/// revocation, on any store.
async fn end_to_end<B: Backend>(server: Server<B>) {
    let (mut ana, sync, _folder) = owner(&server).await;
    let workspace = sync.workspace();
    let mut book = InvitationBook::open(ana.state.path(), ana.id()).unwrap();
    let created = ana
        .service
        .create_invitation(
            &mut book,
            NewInvitation {
                workspace,
                role: Role::Editor,
                workspace_name: "Field notes",
                inviter_name: "Ana",
            },
        )
        .await
        .expect("invited");
    let now = server.store.clock().now();
    assert!((now + 7 * 86_400 - 60..=now + 7 * 86_400).contains(&created.invitation.expires_at));
    assert_eq!(
        book.invitations(),
        std::slice::from_ref(&created.invitation)
    );

    let mut ben = first_desktop(&server, "ben@example.com").await;
    let link = InvitationLink::parse(&created.url).expect("the link reads");
    let opened = ben.service.open_invitation(&link).await.expect("it opens");
    assert_eq!(
        (
            opened.workspace,
            opened.invitation,
            opened.role,
            opened.status
        ),
        (
            workspace,
            created.invitation.invitation,
            Role::Editor,
            InvitationStatus::Pending
        )
    );
    assert_eq!(opened.workspace_name, "Field notes");
    assert_eq!(opened.inviter_name, "Ana");
    assert_eq!(
        opened.inviter_fingerprint,
        ana.credentials.card().fingerprint()
    );

    ben.service
        .accept_invitation(&link)
        .await
        .expect("accepted");
    let listed = ana
        .service
        .workspace_invitations(&mut book, workspace)
        .await
        .expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, InvitationStatus::Accepted);
    assert_eq!(listed[0].accepted_by, Some(ben.credentials.account));
    assert!(listed[0].sent_here);
    assert!(
        ben.service
            .client()
            .workspaces()
            .await
            .expect("listed")
            .workspaces
            .is_empty(),
        "acceptance grants nothing"
    );

    ana.service
        .revoke_invitation(&mut book, workspace, created.invitation.invitation)
        .await
        .expect("revoked");
    unusable(ben.service.open_invitation(&link).await);
    unusable(ben.service.accept_invitation(&link).await);
    assert!(book.invitations().is_empty());
    assert!(
        ana.service
            .invitation_secret(&book, &created.invitation.invitation)
            .unwrap()
            .is_none(),
        "the secret ends with the invitation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitations_work_end_to_end_on_sqlite() {
    end_to_end(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitations_work_end_to_end_on_postgres() {
    end_to_end(Server::start_postgres().await).await;
}

/// You open the link before you have an account: the app shows the
/// invitation from the context alone, then you sign up through the device
/// grant and accept, which grants nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_out_app_opens_the_link_then_signs_up_and_accepts() {
    let server = Server::start().await;
    let (mut ana, sync, _folder) = owner(&server).await;
    let workspace = sync.workspace();
    let mut book = InvitationBook::open(ana.state.path(), ana.id()).unwrap();
    let created = ana
        .service
        .create_invitation(
            &mut book,
            NewInvitation {
                workspace,
                role: Role::Viewer,
                workspace_name: "Field notes",
                inviter_name: "Ana",
            },
        )
        .await
        .expect("invited");
    let link = InvitationLink::parse(&created.url).expect("the link reads");

    let opened = open_signed_out(&server.base, &link)
        .await
        .expect("it opens with no account");
    assert_eq!(
        (opened.workspace, opened.role, opened.status),
        (workspace, Role::Viewer, InvitationStatus::Pending)
    );
    assert_eq!(opened.workspace_name, "Field notes");
    assert_eq!(opened.inviter_name, "Ana");
    let wrong = InvitationLink {
        origin: link.origin.clone(),
        token: link.token,
        secret: SecretKey([1; 32]),
    };
    unusable(open_signed_out(&server.base, &wrong).await);
    let refused = open_signed_out("https://sync.example.com", &link)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("another sync server"));

    // Signing up enrolls Ben's first device; then the app accepts.
    let mut ben = first_desktop(&server, "ben@example.com").await;
    ben.service
        .accept_invitation(&link)
        .await
        .expect("accepted");
    assert!(
        ben.service
            .client()
            .workspaces()
            .await
            .expect("listed")
            .workspaces
            .is_empty(),
        "acceptance grants nothing"
    );
    let listed = ana
        .service
        .workspace_invitations(&mut book, workspace)
        .await
        .expect("listed");
    assert_eq!(listed[0].status, InvitationStatus::Accepted);
    assert_eq!(listed[0].accepted_by, Some(ben.credentials.account));
}

/// No request carries the secret, a lost answer gets the same bytes again
/// and the original result, the device keeps the secret until the server
/// stops listing the invitation, and a wrong secret or another server's
/// link doesn't open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_secret_stays_on_the_inviting_device() {
    let mut pair = pair().await;
    let workspace = pair.sync_a.workspace();
    let mut book = InvitationBook::open(pair.laptop.state.path(), pair.laptop.id()).unwrap();

    let mangled = Arc::new(AtomicBool::new(false));
    let once = mangled.clone();
    pair.proxy.tamper(Box::new(move |path, bytes| {
        if path.ends_with("/invitations") && !once.swap(true, Ordering::SeqCst) {
            return vec![0xff];
        }
        bytes
    }));
    let created = pair
        .service
        .create_invitation(
            &mut book,
            NewInvitation {
                workspace,
                role: Role::Viewer,
                workspace_name: "Field notes",
                inviter_name: "Ana",
            },
        )
        .await
        .expect("invited");
    pair.proxy.honest();
    assert!(mangled.load(Ordering::SeqCst));
    let link = InvitationLink::parse(&created.url).unwrap();
    let posts: Vec<_> = pair
        .proxy
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/invitations"))
        .collect();
    assert_eq!(posts.len(), 2, "the creation went out twice");
    assert_eq!(posts[0].body, posts[1].body, "with identical bytes");
    for request in pair.proxy.requests() {
        for form in forms(link.secret.expose()) {
            assert!(
                !contains(&request.body, &form) && !contains(request.path.as_bytes(), &form),
                "{} {} carries the secret",
                request.method,
                request.path
            );
        }
        assert!(
            !contains(&request.body, &link.token),
            "no request carries the token"
        );
    }
    let secret = pair
        .service
        .invitation_secret(&book, &created.invitation.invitation)
        .unwrap()
        .expect("kept");
    assert_eq!(secret.expose(), link.secret.expose());
    assert_eq!(
        pair.service
            .invitation_url(&book, &created.invitation.invitation)
            .unwrap()
            .as_str(),
        created.url.as_str()
    );
    assert!(
        pair.desktop
            .service
            .invitation_secret(&book, &created.invitation.invitation)
            .unwrap()
            .is_none(),
        "another device of the account holds no secret"
    );

    let ben = first_desktop(&pair.server, "ben@example.com").await;
    let mut ben = ben.connect_through(&pair.proxy.base).await;
    let wrong = InvitationLink {
        origin: link.origin.clone(),
        token: link.token,
        secret: SecretKey([1; 32]),
    };
    unusable(ben.open_invitation(&wrong).await);
    let elsewhere = InvitationLink {
        origin: "https://sync.example.com".into(),
        token: link.token,
        secret: SecretKey(*link.secret.expose()),
    };
    let refused = ben.open_invitation(&elsewhere).await.unwrap_err();
    assert!(refused.to_string().contains("another sync server"));

    // Expiry ends the invitation, and the device drops its secret once
    // the server stops listing it.
    pair.server
        .store
        .clock()
        .advance(std::time::Duration::from_secs(7 * 86_400 + 1));
    unusable(ben.open_invitation(&link).await);
    let listed = pair
        .service
        .workspace_invitations(&mut book, workspace)
        .await
        .unwrap();
    assert!(listed.is_empty());
    assert!(book.invitations().is_empty());
    let item = format!(
        "noura.sync.v2.invitation.{}.{}",
        pair.laptop.id().to_hex(),
        created.invitation.invitation.to_hex()
    );
    assert!(
        pair.laptop
            .secrets
            .read_optional(&item)
            .unwrap()
            .is_none_or(|t| t.is_empty()),
        "the credential store drops the secret"
    );
}

/// The fixture's link reads as its token and secret, and each of its
/// contexts, served by the real server, opens or fails as the fixture
/// says, with one generic error for every failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fixture_contexts_open_through_the_server() {
    let fixture = fixture();
    let server = Server::start().await;
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let origin = reqwest::Url::parse(&server.base)
        .unwrap()
        .origin()
        .ascii_serialization();

    let example = fixture["link"]["example"].as_str().unwrap();
    let link = InvitationLink::parse(example).expect("the fixture's link reads");
    assert_eq!(link.origin, "https://sync.noura.app");
    let cases = fixture["cases"].as_array().unwrap();
    let case = |name: &str| {
        cases
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("case {name}"))
    };
    assert_eq!(link.token.to_vec(), hex(&case("token_hash")["token"]));
    assert_eq!(
        link.secret.expose().to_vec(),
        hex(&case("invitation_key")["input"])
    );
    assert_eq!(
        invite_token_hash(&link.token).0.to_vec(),
        hex(&case("token_hash")["expect"])
    );
    assert_eq!(&*link.url(), example, "the link's text round-trips");

    let opening: Vec<&Value> = cases
        .iter()
        .filter(|c| c["kind"] == "context_open")
        .collect();
    assert!(opening.len() >= 3);
    for case in opening {
        let context = InviteContext::from_bytes(&hex(&case["context"])).expect("a context");
        let token: [u8; 16] = sync_protocol::crypto::random_array().unwrap();
        let row = InvitationRow {
            invitation: context.invitation,
            token_hash: invite_token_hash(&token),
            workspace: context.workspace,
            role: context.role,
            inviter: Id::random().unwrap(),
            accepted_by: None,
            expires_at: server.store.clock().now() + 86_400,
            invite_context: context.to_bytes(),
        };
        server
            .store
            .backend()
            .write(Box::new(
                move |txn: &mut <sync_store_sqlite::SqliteBackend as Backend>::Write| {
                    Box::pin(async move {
                        // The fixture's contexts share one invitation ID.
                        txn.delete_invitation(row.invitation).await?;
                        txn.insert_invitation(&row).await
                    })
                },
            ))
            .await
            .unwrap();
        let secret: [u8; 32] = hex(&case["secret"]).try_into().unwrap();
        let link = InvitationLink {
            origin: origin.clone(),
            token,
            secret: SecretKey(secret),
        };
        let opened = ben.service.open_invitation(&link).await;
        match case["expect"].as_str().expect("an expectation") {
            "invalid" => unusable(opened),
            payload => {
                let opened = opened.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
                let expected =
                    sync_protocol::records::InvitePayload::from_bytes(&from_hex(payload).unwrap())
                        .unwrap();
                assert_eq!(opened.workspace_name, expected.workspace_name.0);
                assert_eq!(opened.inviter_name, expected.inviter_name.0);
                assert_eq!(opened.inviter_fingerprint, expected.inviter_fingerprint);
                assert_eq!(opened.workspace, context.workspace);
            }
        }
    }
}
