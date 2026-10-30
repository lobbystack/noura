//! Invitations over HTTP at seam 4 (§11, §26): create, list and revoke
//! from the owner's device, the context by token without a session,
//! acceptance from the invitee's session, the invitee's approval request
//! in `pending-devices`, and the invitation page, which loads no script.

mod support;

use reqwest::StatusCode;
use support::Client;
use sync_protocol::codec::{Encode, Nested};
use sync_protocol::messages::{
    CreateInvitationRequest, ExpiresAtResponse, InvitationResponse, InvitationStatus,
    InvitationsResponse, PendingResponse,
};
use sync_protocol::records::Role;
use sync_protocol::types::to_base64url;
use sync_store::conformance::{Invite, Invitee, World};
use sync_store::{INITIAL_RESTORE_GENERATION, Session, Store};
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};

async fn world() -> (World<SqliteBackend>, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join(DATABASE_FILE);
    sync_store_sqlite::migrate(&path).await.expect("migrated");
    sync_store_sqlite::store_secret_check(&path, [1; 32], INITIAL_RESTORE_GENERATION)
        .await
        .expect("secret check");
    let store = Store::open(SqliteBackend::open(&path).await.expect("opened"))
        .await
        .expect("store");
    let world = World::new(store, 1).await;
    let address = sync_server::spawn(world.store.clone(), "127.0.0.1:0")
        .await
        .expect("serving");
    (world, format!("http://{address}"), dir)
}

/// A client signed in as `session`, through a session token the store
/// issues for it.
async fn signed_in(world: &World<SqliteBackend>, base: &str, session: Session) -> Client {
    let token: [u8; 32] = sync_protocol::crypto::random_array().expect("random");
    world
        .store
        .create_session(token, session)
        .await
        .expect("a session");
    Client::new(base.to_owned(), Some(to_base64url(&token)))
}

fn page_text(reply: &support::Reply) -> String {
    String::from_utf8(reply.body.clone()).expect("UTF-8")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitations_follow_section_11_over_http() {
    let (world, base, _dir) = world().await;
    let owner = Client::new(base.clone(), Some(world.devices[0].bearer()));
    let anonymous = Client::new(base.clone(), None);
    let w = world.workspace.to_base64url();
    let invite = Invite::new(Role::Editor);
    let creation = invite.creation(world.workspace);
    let body = CreateInvitationRequest {
        invitation: creation.invitation,
        token_hash: creation.token_hash,
        role: creation.role,
        invite_context: Nested(creation.invite_context.clone()),
    }
    .to_bytes();

    let path = format!("/v2/workspaces/{w}/invitations");
    assert_eq!(
        anonymous.post(&path, body.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );
    let created: ExpiresAtResponse = owner.post(&path, body.clone()).await.decode();
    let again: ExpiresAtResponse = owner.post(&path, body).await.decode();
    assert_eq!(created.expires_at, again.expires_at);

    let token = to_base64url(&invite.token);
    let read: InvitationResponse = anonymous
        .get(&format!("/v2/invitations/{token}"))
        .await
        .decode();
    assert_eq!(read.invitation, invite.invitation);
    assert_eq!(read.status, InvitationStatus::Pending);
    let unknown = anonymous
        .get(&format!("/v2/invitations/{}", to_base64url(&[3u8; 16])))
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error().code.as_str(), "invitation_invalid");

    // The page sends a browser visitor to the app and loads no script, so
    // nothing reads the fragment.
    let page = anonymous.get(&format!("/invite/{token}")).await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page_text(&page);
    assert!(html.contains("Open this invitation in Noura"));
    assert!(!html.contains("<script"), "{html}");

    let invitee = Invitee::new(&world.store).await;
    let invitee_client = signed_in(&world, &base, invitee.session).await;
    let accept = format!("/v2/invitations/{token}/accept");
    assert_eq!(
        anonymous.post(&accept, Vec::new()).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        invitee_client.post(&accept, Vec::new()).await.status,
        StatusCode::OK
    );
    let (request, _) = invitee.request(&invite);
    assert_eq!(
        invitee_client
            .post("/v2/approval-requests", request.to_bytes())
            .await
            .status,
        StatusCode::OK
    );
    let pending: PendingResponse = owner
        .get(&format!("/v2/workspaces/{w}/pending-devices"))
        .await
        .decode();
    assert_eq!(pending.pending.len(), 1);
    assert_eq!(pending.pending[0].invitation, Some(invite.invitation));

    let listed: InvitationsResponse = owner.get(&path).await.decode();
    assert_eq!(listed.invitations.len(), 1);
    assert_eq!(listed.invitations[0].status, InvitationStatus::Accepted);
    assert_eq!(listed.invitations[0].accepted_by, Some(invitee.account));

    let revoke = format!(
        "/v2/workspaces/{w}/invitations/{}",
        invite.invitation.to_base64url()
    );
    assert_eq!(owner.delete(&revoke).await.status, StatusCode::OK);
    let gone = anonymous.get(&format!("/v2/invitations/{token}")).await;
    assert_eq!(gone.error().code.as_str(), "invitation_invalid");
    let page = anonymous.get(&format!("/invite/{token}")).await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert!(!page_text(&page).contains("<script"));
    let listed: InvitationsResponse = owner.get(&path).await.decode();
    assert!(listed.invitations.is_empty());
}
