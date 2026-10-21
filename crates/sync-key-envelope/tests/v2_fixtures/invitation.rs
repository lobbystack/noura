//! `sync-v2-invitation.json`: the invitation token hash, context and
//! payload (§11).

use serde_json::json;
use sync_protocol::codec::{Encode, Text};
use sync_protocol::crypto::{context as contexts, derive_key};
use sync_protocol::records::{InviteContext, InvitePayload, Role, invite_token_hash};
use sync_protocol::tag::Tag;
use sync_protocol::types::to_hex;

use super::common::{Fixture, Shape, id, nonce, secret};
use super::world::{World, world};

pub const TOKEN: [u8; 16] = [0x41; 16];

pub fn payload(w: &World) -> InvitePayload {
    InvitePayload {
        workspace_name: Text::new("Garden plans").unwrap(),
        inviter_name: Text::new("Alice").unwrap(),
        inviter_fingerprint: w.alice_laptop.card().fingerprint(),
    }
}

pub fn context(w: &World) -> InviteContext {
    InviteContext::seal(
        w.workspace,
        id("invitation"),
        Role::Editor,
        &payload(w),
        &secret("invitation secret"),
        nonce("invitation"),
    )
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-invitation.json",
        "Invitations (sync-v2.md §11): the token hash the server stores, the encrypted context and its padded payload, and the invitation key.",
    );
    let mut input = Tag::InviteToken.bytes();
    input.extend_from_slice(&TOKEN);
    f.case(json!({
        "kind": "token_hash",
        "name": "token_hash",
        "token": to_hex(&TOKEN),
        "input": to_hex(&input),
        "expect": invite_token_hash(&TOKEN).to_hex(),
    }));
    let invite_secret = secret("invitation secret");
    f.case(json!({
        "kind": "derive_key",
        "name": "invitation_key",
        "context": contexts::INVITATION_KEY,
        "input": to_hex(invite_secret.expose()),
        "expect": to_hex(derive_key(contexts::INVITATION_KEY, invite_secret.expose()).expose()),
    }));
    let payload = payload(&w);
    f.accept(
        "invite_payload",
        "invite_payload",
        &payload,
        None,
        Shape::Tagged,
    );
    let ctx = context(&w);
    f.accept(
        "invite_context",
        "invite_context",
        &ctx,
        None,
        Shape::Tagged,
    );
    f.case(json!({
        "kind": "context_open",
        "name": "open_with_the_secret",
        "context": to_hex(&ctx.to_bytes()),
        "secret": to_hex(invite_secret.expose()),
        "expect": to_hex(&payload.to_bytes()),
    }));
    f.case(json!({
        "kind": "context_open",
        "name": "wrong_secret",
        "context": to_hex(&ctx.to_bytes()),
        "secret": to_hex(secret("another secret").expose()),
        "expect": "invalid",
    }));
    let mut tampered = ctx.clone();
    tampered.role = Role::Admin;
    f.case(json!({
        "kind": "context_open",
        "name": "tampered_role",
        "context": to_hex(&tampered.to_bytes()),
        "secret": to_hex(invite_secret.expose()),
        "expect": "invalid",
    }));
    f.set(
        "link",
        json!({
            "format": "{origin}/invite/{base64url(token)}#{base64url(secret)}",
            "example": format!(
                "https://sync.noura.app/invite/{}#{}",
                sync_protocol::types::to_base64url(&TOKEN),
                sync_protocol::types::to_base64url(invite_secret.expose())
            ),
        }),
    );
    f
}
