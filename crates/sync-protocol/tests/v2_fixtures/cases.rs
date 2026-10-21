//! The behavior cases of the v2 fixtures, by kind.

use serde_json::{Value, json};
use sync_protocol::codec::{Decode, Encode, List};
use sync_protocol::crypto::{context, derive_key};
use sync_protocol::manifest::Manifest;
use sync_protocol::messages::*;
use sync_protocol::records::*;
use sync_protocol::tag::Tag;
use sync_protocol::types::{Key, from_hex, to_hex};

use super::{digest_of, hex, id_of, secret_of};

type Outcome = Result<(), String>;

fn same<T: PartialEq + std::fmt::Debug>(ours: T, theirs: T) -> Outcome {
    if ours == theirs {
        Ok(())
    } else {
        Err(format!("ours {ours:?}, fixture {theirs:?}"))
    }
}

fn key(value: &Value) -> Key {
    Key(hex(value).try_into().unwrap())
}

fn expect_str(case: &Value) -> &str {
    case["expect"].as_str().unwrap_or("")
}

pub fn run_document(file: &str, doc: &Value) -> Outcome {
    if file != "sync-v2-encoding.json" {
        return Ok(());
    }
    let tags = doc["tags"].as_array().ok_or("no tags")?;
    same(tags.len(), Tag::ALL.len())?;
    for t in tags {
        let tag = Tag::from_domain(t["domain"].as_str().unwrap()).ok_or("unknown tag")?;
        same(
            to_hex(&tag.bytes()),
            t["bytes"].as_str().unwrap().to_owned(),
        )?;
    }
    let contexts = doc["contexts"].as_array().ok_or("no contexts")?;
    same(contexts.len(), context::ALL.len())?;
    for c in contexts {
        let name = c["context"].as_str().unwrap();
        if !context::ALL.contains(&name) {
            return Err(format!("unknown context {name}"));
        }
        same(
            to_hex(derive_key(name, &hex(&c["input"])).expose()),
            c["output"].as_str().unwrap().to_owned(),
        )?;
    }
    let errors: Vec<(String, u64)> = doc["server_errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["code"].as_str().unwrap().to_owned(),
                e["status"].as_u64().unwrap(),
            )
        })
        .collect();
    let ours: Vec<(String, u64)> = SERVER_ERRORS
        .iter()
        .map(|(c, s)| ((*c).to_owned(), u64::from(*s)))
        .collect();
    same(ours, errors)?;
    for code in [
        "authorization_pending",
        "slow_down",
        "expired_token",
        "access_denied",
    ] {
        same(server_error_status(code), Some(400))?;
    }
    same(json!(CLIENT_ERRORS), doc["client_errors"].clone())?;
    same(
        RESTORE_GENERATION_HEADER,
        doc["restore_generation_header"].as_str().unwrap(),
    )?;
    Ok(())
}

pub fn run(file: &str, _doc: &Value, case: &Value) -> Outcome {
    match case["kind"].as_str().unwrap() {
        "page_shape" => {
            let page = Page::from_bytes(&hex(&case["page"])).map_err(|e| e.to_string())?;
            let result = match page.check_shape() {
                Ok(()) => "ok",
                Err(PageShapeError::NotEmpty) => "not_empty",
                Err(PageShapeError::OutOfRange) => "out_of_range",
                Err(PageShapeError::Range) => "range",
            };
            same(result, expect_str(case))
        }
        "records_frame_action" => {
            let n = |k: &str| case[k].as_u64().unwrap();
            let action =
                records_frame_action(n("cursor"), n("previous_seq"), n("first"), n("last"));
            same(json!(action), case["expect"].clone())
        }
        "buckets_body" => {
            let leaves = case["leaves"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| {
                    let refs: Vec<Ref> = serde_json::from_value(l["refs"].clone()).unwrap();
                    (id_of(&l["rid"]), refs)
                })
                .collect();
            let body = ManifestBucketsResponse {
                buckets: List(Manifest::new(leaves).bucket_hashes()),
            }
            .to_bytes();
            same(body.len() as u64, case["length"].as_u64().unwrap())?;
            same(
                sync_protocol::crypto::hash(&body),
                digest_of(&case["digest"]),
            )?;
            same(ManifestBucketsResponse::from_bytes(&body).is_ok(), true)
        }
        "commit" => {
            let head: Option<AccountHead> =
                serde_json::from_value(case["candidate_head"].clone()).unwrap();
            same(
                approval_commit(
                    &digest_of(&case["candidate_card_digest"]),
                    head.as_ref(),
                    &key(&case["nc"]),
                ),
                digest_of(&case["expect"]),
            )
        }
        "code" => {
            let code = approval_code(&digest_of(&case["offer_digest"]), &key(&case["nc"]));
            same(u64::from(code), case["expect"].as_u64().unwrap())?;
            same(
                approval_code_text(code),
                case["text"].as_str().unwrap().to_owned(),
            )
        }
        "code_text" => same(
            approval_code_text(case["code"].as_u64().unwrap() as u32),
            expect_str(case).to_owned(),
        ),
        "reveal_check" => {
            let offer =
                Signed::<Offer>::from_bytes(&hex(&case["offer"])).map_err(|e| e.to_string())?;
            let card =
                Card::from_bytes(&hex(&case["candidate_card"])).map_err(|e| e.to_string())?;
            let head: Option<AccountHead> =
                serde_json::from_value(case["candidate_head"].clone()).unwrap();
            let result = check_offer(&offer.body, &card, head.as_ref(), &key(&case["nc"]));
            same(
                if result.is_ok() {
                    "ok"
                } else {
                    "sync_approval_mismatch"
                },
                expect_str(case),
            )
        }
        "offer_size" => {
            let length = case["length"].as_u64().unwrap();
            let floor =
                case["workspaces"].as_u64().unwrap() * case["anchor_bytes"].as_u64().unwrap();
            if length > case["max_body"].as_u64().unwrap() || length < floor {
                return Err(format!("length {length}"));
            }
            Ok(())
        }
        "invite_proof" => same(
            invite_proof(&secret_of(&case["secret"]), &hex(&case["card"])),
            digest_of(&case["expect"]),
        ),
        "kit_key" => {
            let mut material = hex(&case["secret"]);
            material.extend_from_slice(&hex(&case["salt"]));
            same(
                to_hex(derive_key(case["context"].as_str().unwrap(), &material).expose()),
                expect_str(case).to_owned(),
            )
        }
        "kit_open" => {
            let kit = RecoveryKit::from_bytes(&hex(&case["kit"])).map_err(|e| e.to_string())?;
            let opened = parse_kit_secret(case["secret_text"].as_str().unwrap())
                .and_then(|secret| kit.open(&secret));
            match (opened, expect_str(case)) {
                (Err(_), "sync_recovery_kit_invalid") => Ok(()),
                (Ok(payload), _)
                    if case["expect"].as_str().and_then(from_hex) == Some(payload.to_bytes()) =>
                {
                    Ok(())
                }
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "kit_secret_text" => {
            let secret: [u8; KIT_SECRET_LEN] = hex(&case["secret"]).try_into().unwrap();
            same(kit_secret_text(&secret), expect_str(case).to_owned())?;
            same(parse_kit_secret(expect_str(case)).ok(), Some(secret))
        }
        "kit_secret_parse" => same(
            parse_kit_secret(case["text"].as_str().unwrap()).is_err(),
            true,
        ),
        "kit_card_matches_seeds" => {
            let payload =
                KitPayload::from_bytes(&hex(&case["payload"])).map_err(|e| e.to_string())?;
            let signing = sync_protocol::crypto::SigningKey::from_seed(&payload.signing_seed);
            same(signing.public(), payload.card.signing_public)
        }
        "token_hash" => {
            let token: [u8; 16] = hex(&case["token"]).try_into().unwrap();
            let (tag, ours) = if file == "sync-v2-share.json" {
                (Tag::ShareToken, share_token_hash(&token))
            } else {
                (Tag::InviteToken, invite_token_hash(&token))
            };
            let mut input = tag.bytes();
            input.extend_from_slice(&token);
            same(input, hex(&case["input"]))?;
            same(ours, digest_of(&case["expect"]))
        }
        "derive_key" => same(
            to_hex(derive_key(case["context"].as_str().unwrap(), &hex(&case["input"])).expose()),
            expect_str(case).to_owned(),
        ),
        "context_open" => {
            let ctx =
                InviteContext::from_bytes(&hex(&case["context"])).map_err(|e| e.to_string())?;
            match (ctx.open(&secret_of(&case["secret"])), expect_str(case)) {
                (Err(_), "invalid") => Ok(()),
                (Ok(p), e) if from_hex(e) == Some(p.to_bytes()) => Ok(()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "share_open" => {
            let (link_key, verify_key) = parse_share_fragment(case["fragment"].as_str().unwrap())
                .map_err(|e| e.to_string())?;
            match (
                open_share(&hex(&case["share"]), &link_key, &verify_key),
                expect_str(case),
            ) {
                (Err(_), "invalid") => Ok(()),
                (Ok((_, p)), e) if from_hex(e) == Some(p.to_bytes()) => Ok(()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "presence_channel" => same(
            presence_channel(&secret_of(&case["epoch_key"]), &id_of(&case["rid"])),
            id_of(&case["expect"]),
        ),
        "presence_open" => {
            let record = Signed::<Presence>::from_bytes(&hex(&case["presence"]))
                .map_err(|e| e.to_string())?;
            let opened = record.body.open_payload(
                &secret_of(&case["epoch_key"]),
                &digest_of(&case["epoch_record"]),
            );
            match (opened, expect_str(case)) {
                (Err(_), "invalid") => Ok(()),
                (Ok(p), e) if from_hex(e) == Some(p.to_bytes()) => Ok(()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "clock_window" => same(
            within_clock_window(
                case["minute"].as_u64().unwrap() as u32,
                case["now"].as_u64().unwrap() as u32,
            ),
            case["expect"].as_bool().unwrap(),
        ),
        "profile_open" => {
            let record =
                Signed::<Profile>::from_bytes(&hex(&case["profile"])).map_err(|e| e.to_string())?;
            let opened = record.body.open_payload(
                &secret_of(&case["epoch_key"]),
                &digest_of(&case["epoch_record"]),
            );
            match (opened, expect_str(case)) {
                (Err(_), "invalid") => Ok(()),
                (Ok(p), e) if from_hex(e) == Some(p.to_bytes()) => Ok(()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        other => super::behavior::run(other, case),
    }
}
