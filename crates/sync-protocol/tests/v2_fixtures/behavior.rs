//! The verifier cases of the v2 fixtures: identifiers, padding, account
//! and policy chains, key domains, operations, device chains, live sets,
//! the manifest, blobs, markers and text mode.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sync_protocol::account::{AccountChain, KeptHead};
use sync_protocol::blob::{
    BlobError, blob_ciphertext_len, ciphertext_digest, decrypt_blob, encrypt_blob, header,
    nonce_prefix,
};
use sync_protocol::chain::{
    ChainError, ChainFetcher, ChainRecord, ChainVerifier, Fetched, NoFetch,
};
use sync_protocol::codec::{Bytes, Decode, Encode};
use sync_protocol::crypto::{derive_key, hash};
use sync_protocol::ids::{
    account_id, device_confirmation, device_confirmation_matches, device_id, secret_check,
    text_lineage_id, yrs_client_id,
};
use sync_protocol::keys::{
    EnvelopeVerdict, EpochAcceptance, EpochChain, LogJudge, after_failed_envelopes, check_envelope,
    judge_skipped_epoch_record, key_check, key_matches, open_lineage, recipients_digest,
    walk_lineage,
};
use sync_protocol::live::{LiveError, LiveOp, LiveSet, covers_for};
use sync_protocol::manifest::{
    CheckpointChain, CheckpointError, Manifest, bucket_hash, bucket_of, descent_path, heads_digest,
    leaf_hash, node_hash, skip_number, verify_bucket, verify_descent, verify_manifest,
};
use sync_protocol::matching::{
    DomainRoot, HeldRoot, JournalObject, LiveObject, LocalFile, LocalMarker, archived, held_roots,
    hold_ends, match_files, refuses_marker_path,
};
use sync_protocol::messages::LiveCursor;
use sync_protocol::padding::{is_valid_padded_length, pad, padded_length, padme, unpad};
use sync_protocol::policy::{AccountChains, PolicyChain, RecoveryStage};
use sync_protocol::records::*;
use sync_protocol::tag::Tag;
use sync_protocol::types::{Digest, Id, Key, from_base64url, from_decimal, from_hex, to_hex};

use super::{digest_of, hex, id_of, secret_of};

type Outcome = Result<(), String>;

fn same<T: PartialEq + std::fmt::Debug>(ours: T, theirs: T) -> Outcome {
    if ours == theirs {
        Ok(())
    } else {
        Err(format!("ours {ours:?}, fixture {theirs:?}"))
    }
}

fn u64_of(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("not an integer: {value}"))
}

fn links(value: &Value) -> Vec<Signed<AccountLink>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|l| Signed::<AccountLink>::from_bytes(&hex(l)).unwrap())
        .collect()
}

fn account_chains(value: &Value) -> AccountChains {
    let mut chains = AccountChains::new();
    for chain in value.as_array().unwrap() {
        chains.insert(AccountChain::verify(&links(chain), &BTreeSet::new()).unwrap());
    }
    chains
}

fn changes(value: &Value) -> Vec<Signed<PolicyChange>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Signed::<PolicyChange>::from_bytes(&hex(c)).unwrap())
        .collect()
}

fn policy(case: &Value) -> PolicyChain {
    PolicyChain::verify(
        &changes(&case["changes"]),
        &account_chains(&case["account_chains"]),
    )
    .expect("the case's policy verifies")
}

fn ids(value: &Value) -> Vec<Id> {
    value.as_array().unwrap().iter().map(id_of).collect()
}

fn sorted_ids<'a>(iter: impl Iterator<Item = &'a Id>) -> Vec<Id> {
    let mut v: Vec<Id> = iter.copied().collect();
    v.sort();
    v
}

fn error_name<E: serde::Serialize>(e: &E) -> String {
    let v = serde_json::to_value(e).unwrap();
    match v {
        Value::String(s) => s,
        Value::Object(m) => m["error"].as_str().unwrap().to_owned(),
        other => other.to_string(),
    }
}

fn role_name(role: Role) -> String {
    serde_json::to_value(role)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

fn live_op(value: &Value) -> LiveOp {
    LiveOp {
        rid: id_of(&value["rid"]),
        device: id_of(&value["device"]),
        counter: u64_of(&value["counter"]),
        seq: u64_of(&value["seq"]),
        digest: digest_of(&value["digest"]),
        delete: value["delete"].as_bool().unwrap(),
        covers: serde_json::from_value(value["covers"].clone()).unwrap(),
        weight: u64_of(&value["weight"]),
    }
}

fn leaves(value: &Value) -> BTreeMap<Id, Vec<Ref>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                id_of(&l["rid"]),
                serde_json::from_value(l["refs"].clone()).unwrap(),
            )
        })
        .collect()
}

struct Store {
    records: BTreeMap<u64, ChainRecord>,
    pruned: bool,
}

impl ChainFetcher for Store {
    fn fetch(&mut self, _: &Id, counter: u64) -> Fetched {
        if self.pruned {
            return Fetched::Pruned;
        }
        self.records
            .range(counter..)
            .next()
            .map_or(Fetched::NotFound, |(_, r)| Fetched::Record(*r))
    }
}

fn chain_record(value: &Value) -> ChainRecord {
    serde_json::from_value(value.clone()).unwrap()
}

pub fn run(kind: &str, case: &Value) -> Outcome {
    let expect = &case["expect"];
    match kind {
        // Padding.
        "padded_length" => {
            same(padme(u64_of(&case["framed"])), u64_of(&case["padme"]))?;
            same(padded_length(u64_of(&case["framed"])), u64_of(expect))
        }
        "valid_padded_length" => same(
            is_valid_padded_length(u64_of(&case["length"])),
            expect.as_bool().unwrap(),
        ),
        "pad" => same(pad(&hex(&case["content"])).unwrap(), hex(expect)),
        "unpad" => {
            let padded = hex(&case["padded"]);
            match (unpad(&padded), expect.as_str().unwrap()) {
                (Ok(content), e) if from_hex(e).as_deref() == Some(content) => Ok(()),
                (Err(err), e) => same(
                    error_name(&format!("{err:?}").to_lowercase()),
                    e.replace('_', ""),
                ),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "operation_ciphertext_limit" => same(
            operation_ciphertext_len(u64_of(&case["payload_length"]) as usize),
            u64_of(expect),
        ),

        // Identifiers.
        "base64url" => {
            let bytes = hex(&case["bytes"]);
            let text = expect.as_str().unwrap();
            same(text.len() as u64, u64_of(&case["length"]))?;
            same(sync_protocol::types::to_base64url(&bytes), text.to_owned())?;
            let back = match bytes.len() {
                16 => from_base64url::<16>(text).map(|b| b.to_vec()),
                _ => from_base64url::<32>(text).map(|b| b.to_vec()),
            };
            same(back.ok(), Some(bytes))
        }
        "base64url_invalid" => {
            let text = case["text"].as_str().unwrap();
            same(from_base64url::<16>(text).is_err(), true)
        }
        "decimal" => {
            let text = case["text"].as_str().unwrap();
            same(from_decimal(text).ok(), expect.as_u64())?;
            if let Some(v) = expect.as_u64() {
                same(sync_protocol::types::to_decimal(v), text.to_owned())?;
            }
            Ok(())
        }
        "live_cursor" => {
            let cursor: LiveCursor = serde_json::from_value(case["cursor"].clone()).unwrap();
            same(cursor.to_bytes(), hex(&case["encoding"]))?;
            same(cursor.to_query(), expect.as_str().unwrap().to_owned())?;
            same(
                LiveCursor::from_query(expect.as_str().unwrap()).ok(),
                Some(cursor),
            )
        }
        "device_id" => {
            let public = Key(hex(&case["signing_public"]).try_into().unwrap());
            same(device_id(&public), id_of(expect))?;
            let card = Card::from_bytes(&hex(&case["card"])).map_err(|e| e.to_string())?;
            same(card.device, id_of(expect))
        }
        "account_fingerprint" => {
            let link = Signed::<AccountLink>::from_bytes(&hex(&case["genesis"])).unwrap();
            let chain =
                AccountChain::verify(&[link], &BTreeSet::new()).map_err(|e| e.to_string())?;
            same(chain.genesis(), digest_of(expect))
        }
        "card_indices" => {
            let verified = policy(case);
            let ours: BTreeMap<String, u64> = verified
                .head_snapshot()
                .card_indices
                .iter()
                .map(|(d, i)| (d.to_hex(), u64::from(*i)))
                .collect();
            same(json!(ours), expect.clone())
        }
        "secret_check" => same(
            to_hex(secret_check(&secret_of(&case["secret"])).expose()),
            expect.as_str().unwrap().to_owned(),
        ),
        "account_id" => same(
            account_id(&secret_of(&case["secret"]), case["email"].as_str().unwrap()),
            id_of(expect),
        ),
        "device_confirmation" => {
            let secret = secret_of(&case["secret"]);
            let account = id_of(&case["account"]);
            let code = case["user_code"].as_str().unwrap();
            let expires_at = u64_of(&case["expires_at"]);
            let confirmation = device_confirmation(&secret, &account, code, expires_at);
            same(confirmation, digest_of(expect))?;
            same(
                device_confirmation_matches(&secret, &account, code, expires_at, &confirmation.0),
                true,
            )?;
            same(
                device_confirmation_matches(
                    &secret,
                    &account,
                    code,
                    expires_at + 1,
                    &confirmation.0,
                ),
                false,
            )
        }

        // Devices.
        "fingerprint" => {
            let card = Card::from_bytes(&hex(&case["card"])).map_err(|e| e.to_string())?;
            same(card.fingerprint(), digest_of(expect))?;
            same(
                fingerprint_text(&card.fingerprint()),
                case["text"].as_str().unwrap().to_owned(),
            )
        }
        // X-Wing and the enrollment and recovery-proof verifiers run in
        // sync-key-envelope.
        "hpke_card" | "enrollment" | "envelope_unwrap" | "recovery_proof_check" => Ok(()),
        "recovery_order" => {
            let chain = AccountChain::verify(&links(&case["links"]), &BTreeSet::new()).unwrap();
            let active = |seqno: u64| -> BTreeSet<Id> {
                chain
                    .active_at(seqno)
                    .iter()
                    .map(|s| s.card.device)
                    .collect()
            };
            let mut before = active(u64_of(&case["kit_seqno"]));
            for step in ["after_recover", "after_removes"] {
                let want = &expect[step];
                let now = active(u64_of(&want["seqno"]));
                let adds: Vec<Id> = now.difference(&before).copied().collect();
                let removes: Vec<Id> = before.difference(&now).copied().collect();
                same(adds, ids(&want["adds"]))?;
                same(removes, ids(&want["removes"]))?;
                before = now;
            }
            Ok(())
        }

        // Account chains.
        "account_chain" => {
            let listed: BTreeSet<Id> = ids(&case["listed_elsewhere"]).into_iter().collect();
            let result = AccountChain::verify(&links(&case["links"]), &listed);
            match (result, expect.get("error")) {
                (Ok(chain), None) => {
                    let active: Vec<Id> = sorted_ids(
                        chain
                            .active_at(chain.head_seqno())
                            .iter()
                            .map(|s| &s.card.device),
                    );
                    same(active, ids(&expect["active"]))?;
                    same(chain.head_seqno(), u64_of(&expect["head_seqno"]))?;
                    same(chain.genesis(), digest_of(&expect["fingerprint"]))
                }
                (Err(e), Some(name)) => {
                    same(error_name(&e), name.as_str().unwrap().to_owned())?;
                    same(e.seqno(), u64_of(&expect["seqno"]))
                }
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "active_at" => {
            let chain = AccountChain::verify(&links(&case["links"]), &BTreeSet::new()).unwrap();
            for (seqno, active) in expect.as_object().unwrap() {
                let ours = sorted_ids(
                    chain
                        .active_at(seqno.parse().unwrap())
                        .iter()
                        .map(|s| &s.card.device),
                );
                same(ours, ids(active))?;
            }
            Ok(())
        }
        "kept_head" => {
            let chain = AccountChain::verify(&links(&case["links"]), &BTreeSet::new()).unwrap();
            let kept = KeptHead {
                genesis: digest_of(&case["kept"]["genesis"]),
                seqno: u64_of(&case["kept"]["seqno"]),
                head: digest_of(&case["kept"]["head"]),
            };
            let result = match case["recorded"].as_object() {
                None => kept.check(&chain),
                Some(_) => {
                    let recorded: AccountHead =
                        serde_json::from_value(case["recorded"].clone()).unwrap();
                    kept.check_recorded(&chain, &recorded)
                }
            };
            same(
                if result.is_ok() {
                    "ok"
                } else {
                    "sync_account_forked"
                },
                expect.as_str().unwrap(),
            )
        }

        "kept_head_extend" => {
            let kept = KeptHead {
                genesis: digest_of(&case["kept"]["genesis"]),
                seqno: u64_of(&case["kept"]["seqno"]),
                head: digest_of(&case["kept"]["head"]),
            };
            let own = case["own_account"].as_bool().unwrap();
            match kept.judge(&links(&case["links"]), &BTreeSet::new()) {
                Ok(chain) => {
                    same("ok", expect["outcome"].as_str().unwrap())?;
                    same(chain.head_seqno(), u64_of(&expect["head_seqno"]))
                }
                Err(refusal) => {
                    same(refusal.client_code(), expect["outcome"].as_str().unwrap())?;
                    same(
                        serde_json::to_value(refusal.stops(own)).unwrap(),
                        expect["stops"].clone(),
                    )
                }
            }
        }

        // Policy.
        "policy_chain" => {
            let result = PolicyChain::verify(
                &changes(&case["changes"]),
                &account_chains(&case["account_chains"]),
            );
            match (result, expect.get("error")) {
                (Err(e), Some(name)) => {
                    same(error_name(&e), name.as_str().unwrap().to_owned())?;
                    let revision = serde_json::to_value(e).unwrap()["revision"].clone();
                    same(revision, expect["revision"].clone())
                }
                (Ok(chain), None) => check_policy(&chain, &expect["ok"]),
                (other, _) => Err(format!("{:?}", other.map(|c| c.head()))),
            }
        }
        "chain_read" => {
            let verified = policy(case);
            let reader_account = id_of(&case["reader_account"]);
            let current = u64_of(&case["current_seqno"]);
            let through_policy = verified.chain_read_bound(
                &[id_of(&case["reader"])],
                &reader_account,
                &id_of(&case["account"]),
                &digest_of(&case["genesis"]),
                current,
            );
            // A waiting approval request that names an invitation to the
            // workspace serves the current chain in full to its members.
            let through_request = (!case["approval_request"].is_null()
                && verified.reads_invitees(&reader_account))
            .then_some(current);
            same(json!(through_policy.max(through_request)), expect.clone())
        }
        "recovery_anchor" => {
            let verified = policy(case);
            let own = AccountChain::verify(&links(&case["own_chain"]), &BTreeSet::new()).unwrap();
            let kit_head = own.head_at(u64_of(&case["kit_seqno"])).unwrap();
            let stage = match case["stage"].as_str().unwrap() {
                "review" => RecoveryStage::Review,
                "removing" => RecoveryStage::Removing,
                _ => RecoveryStage::Done,
            };
            same(
                json!(verified.recovery_anchor(&own, &kit_head, stage)),
                expect.clone(),
            )
        }
        "policy_alerts" => {
            let verified = policy(case);
            let own = AccountChain::verify(&links(&case["own_chain"]), &BTreeSet::new()).unwrap();
            same(
                json!(verified.alerted_changes(
                    &own,
                    &id_of(&case["this_device"]),
                    u64_of(&case["trusted_after"])
                )),
                expect.clone(),
            )
        }
        "removed_card_changes" => {
            let verified = policy(case);
            let removed: BTreeSet<Id> = ids(&case["removed"]).into_iter().collect();
            let listed: Vec<Value> = verified
                .removed_card_changes(&id_of(&case["viewer"]), &removed)
                .into_iter()
                .map(|c| json!({"revision": c.revision, "kind": c.kind, "undo": c.undo}))
                .collect();
            same(json!(listed), expect.clone())
        }
        "anchored_by" => {
            let verified = policy(case);
            let own = AccountChain::verify(&links(&case["own_chain"]), &BTreeSet::new()).unwrap();
            let reference = own.head_at(u64_of(&case["reference_seqno"])).unwrap();
            same(
                json!(verified.anchored_by(&own, &reference)),
                expect.clone(),
            )
        }

        // Keys.
        "key_check" => same(
            key_check(
                &secret_of(&case["epoch_key"]),
                &id_of(&case["workspace"]),
                &id_of(&case["domain"]),
                u64_of(&case["epoch"]),
            ),
            digest_of(expect),
        ),
        "recipients_digest" => same(
            recipients_digest(&ids(&case["recipients"]).into_iter().collect()),
            digest_of(expect),
        ),
        "lineage" => {
            let records: Vec<Signed<Epoch>> = case["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| Signed::<Epoch>::from_bytes(&hex(r)).unwrap())
                .collect();
            match (walk_lineage(&records, &secret_of(&case["key"])), expect) {
                (Ok(keys), Value::Array(want)) => same(
                    keys.iter().map(|k| to_hex(k.expose())).collect::<Vec<_>>(),
                    want.iter()
                        .map(|w| w.as_str().unwrap().to_owned())
                        .collect(),
                ),
                (Err(e), Value::String(name)) => same(error_name(&e), name.clone()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "rotation_secrecy" => {
            let epoch = Signed::<Epoch>::from_bytes(&hex(&case["epoch"])).unwrap();
            let earlier = secret_of(&case["earlier_key"]);
            same(open_lineage(&epoch.body, &earlier).is_err(), true)?;
            same(key_matches(&epoch.body, &earlier), false)
        }
        "epoch_chain" => {
            let verified = policy(case);
            let workspace = verified.workspace();
            let mut chain = EpochChain::new(workspace, id_of(&case["domain"]));
            for step in case["steps"].as_array().unwrap() {
                let record = Signed::<Epoch>::from_bytes(&hex(&step["epoch"])).unwrap();
                let result = chain.accept(&record, &secret_of(&step["key"]), &verified);
                let want = &step["expect"];
                match (result, want) {
                    (Ok(EpochAcceptance::Next), Value::String(s)) if s == "next" => {}
                    (Ok(EpochAcceptance::Replacement { skipped }), _)
                        if want.get("replacement").is_some() =>
                    {
                        same(
                            json!([skipped.start(), skipped.end()]),
                            want["replacement"].clone(),
                        )?;
                    }
                    (Err(e), _) if want.get("error").is_some() => {
                        same(error_name(&e), want["error"].as_str().unwrap().to_owned())?;
                    }
                    (other, _) => return Err(format!("step {want}: {other:?}")),
                }
            }
            Ok(())
        }
        "envelope_check" => {
            let verified = policy(case);
            let epoch = Signed::<Epoch>::from_bytes(&hex(&case["epoch"])).unwrap();
            let envelope = Signed::<Envelope>::from_bytes(&hex(&case["envelope"])).unwrap();
            let card = Card::from_bytes(&hex(&case["recipient_card"])).unwrap();
            let result = check_envelope(
                &envelope,
                &epoch.body,
                &secret_of(&case["unwrapped_key"]),
                &card,
                &verified,
                u64_of(&case["accepted_head"]),
                case["entry_revision"].as_u64(),
            );
            let ours = match result {
                Ok(verdict) => error_name(&verdict),
                Err(e) => error_name(&e),
            };
            same(ours, expect.as_str().unwrap().to_owned())
        }
        "rotation_pending" => {
            let verified = policy(case);
            same(
                json!(verified.rotation_required(
                    &id_of(&case["domain"]),
                    u64_of(&case["latest_epoch_policy_revision"]),
                    u64_of(&case["accepted_head"]),
                )),
                expect.clone(),
            )
        }
        "envelope_choice" => {
            let verified = policy(case);
            let epoch = Signed::<Epoch>::from_bytes(&hex(&case["epoch"])).unwrap();
            let card = Card::from_bytes(&hex(&case["recipient_card"])).unwrap();
            let keys = case["unwrapped_keys"].as_array().unwrap();
            let accepted = case["envelopes"]
                .as_array()
                .unwrap()
                .iter()
                .zip(keys)
                .position(|(envelope, key)| {
                    let envelope = Signed::<Envelope>::from_bytes(&hex(envelope)).unwrap();
                    check_envelope(
                        &envelope,
                        &epoch.body,
                        &secret_of(key),
                        &card,
                        &verified,
                        u64_of(&case["accepted_head"]),
                        None,
                    ) == Ok(EnvelopeVerdict::Accept)
                });
            let then = accepted
                .is_none()
                .then(|| after_failed_envelopes(case["accepted_earlier_epoch"].as_bool().unwrap()));
            same(json!({"accepted": accepted, "then": then}), expect.clone())
        }
        "epoch_entry_due" => {
            let record = Signed::<Epoch>::from_bytes(&hex(&case["epoch"])).unwrap();
            let latest = Digest::from_hex(case["latest"].as_str().unwrap()).unwrap();
            same(
                sync_protocol::keys::makes_checkpoint_due(&latest, &record.body),
                expect.as_bool().unwrap(),
            )
        }
        "skipped_epoch_record" => {
            let verified = policy(case);
            let rotator = id_of(&case["rotator"]);
            let verdict = judge_skipped_epoch_record(
                &verified,
                &id_of(&case["domain"]),
                &id_of(&case["signer"]),
                Some(&rotator),
                u64_of(&case["replacement_revision"]),
            );
            same(error_name(&verdict), expect.as_str().unwrap().to_owned())
        }
        "log_judge" => {
            let verified = policy(case);
            let domain = id_of(&case["domain"]);
            let mut judge = match case["start"].as_object() {
                None => LogJudge::new(domain),
                Some(start) => LogJudge::from_checkpoint(
                    domain,
                    u64_of(&start["policy_revision"]),
                    u64_of(&start["epoch"]),
                    u64_of(&start["epoch_policy_revision"]),
                ),
            };
            for step in case["steps"].as_array().unwrap() {
                if let Some(r) = step.get("policy_entry") {
                    judge
                        .policy_entry(u64_of(r), &verified)
                        .map_err(|e| e.to_string())?;
                } else if let Some(e) = step.get("epoch_entry") {
                    judge.epoch_entry(&Signed::<Epoch>::from_bytes(&hex(e)).unwrap().body);
                } else if let Some(p) = step.get("rotation_pending") {
                    same(judge.rotation_pending(&verified), p.as_bool().unwrap())?;
                } else {
                    let op = &step["operation"];
                    let result = judge.check_operation(
                        &id_of(&op["device"]),
                        u64_of(&op["policy_revision"]),
                        u64_of(&op["epoch"]),
                        &verified,
                    );
                    let ours = match result {
                        Ok(()) => "ok".to_owned(),
                        Err(e) => error_name(&e),
                    };
                    same(ours, step["expect"].as_str().unwrap().to_owned())
                        .map_err(|e| format!("{step}: {e}"))?;
                }
            }
            Ok(())
        }

        // Operations.
        "monotonic" => {
            let previous = Signed::<Operation>::from_bytes(&hex(&case["previous"])).unwrap();
            let next = Signed::<Operation>::from_bytes(&hex(&case["next"])).unwrap();
            let ours = match check_monotonic(&previous.body, &next.body) {
                Ok(()) => "ok",
                Err(_) => "sync_policy_regressed",
            };
            same(ours, expect.as_str().unwrap())
        }
        "operation_aad" => {
            let op = Signed::<Operation>::from_bytes(&hex(&case["operation"])).unwrap();
            same(op.body.aad(&digest_of(&case["epoch_record"])), hex(expect))
        }
        "payload_open" => {
            let op = Signed::<Operation>::from_bytes(&hex(&case["operation"])).unwrap();
            let class = match case["class"].as_str().unwrap() {
                "state" => OpClass::State,
                _ => OpClass::Patch,
            };
            let result = op.body.open_payload(
                &secret_of(&case["epoch_key"]),
                &digest_of(&case["epoch_record"]),
                class,
            );
            let ours = match result {
                Ok(payload) => to_hex(&payload.to_bytes()),
                Err(OpenError::Decrypt(_)) => "decrypt".to_owned(),
                Err(OpenError::ClassMismatch) => "class_mismatch".to_owned(),
                Err(OpenError::DeleteFlagMismatch) => "delete_flag_mismatch".to_owned(),
                Err(OpenError::RidMismatch) => "rid_mismatch".to_owned(),
                Err(other) => format!("{other:?}"),
            };
            same(ours, expect.as_str().unwrap().to_owned())
        }
        "payload_limit" => {
            let n = u64_of(&case["encoded_length"]) as usize;
            // A file payload whose inline content brings the encoding to n.
            let mut payload = Payload {
                rid: Id([1; 16]),
                kind: PayloadKind::File {
                    object_id: Default::default(),
                    path: SyncPath::new("a").unwrap(),
                    revision: Digest::ZERO,
                    content: Content::Inline {
                        bytes: Bytes(Vec::new()),
                    },
                    origin: None,
                },
            };
            let overhead = payload.to_bytes().len();
            if let PayloadKind::File {
                content: Content::Inline { bytes },
                ..
            } = &mut payload.kind
            {
                bytes.0 = vec![0x61; n - overhead];
            }
            same(payload.to_bytes().len(), n)?;
            let mut op = Operation {
                workspace: Id([1; 16]),
                domain: Id([2; 16]),
                device: Id([3; 16]),
                counter: 1,
                previous: Digest::ZERO,
                epoch: 1,
                policy_revision: 1,
                policy_head: Digest::ZERO,
                previous_attempt: Digest::ZERO,
                generation: 1,
                checkpoint_seq: 0,
                checkpoint: Digest::ZERO,
                rid: Id([1; 16]),
                delete: false,
                supersedes: Default::default(),
                blobs: Default::default(),
                nonce: sync_protocol::types::Nonce::ZERO,
                ciphertext: Bytes::default(),
            };
            let result = op.seal_payload(
                &payload,
                &sync_protocol::types::SecretKey([7; 32]),
                &Digest::ZERO,
                sync_protocol::types::Nonce::ZERO,
            );
            match (result, expect) {
                (Ok(()), Value::Number(len)) => {
                    same(op.ciphertext.len() as u64, len.as_u64().unwrap())
                }
                (Err(SealError::TooLarge), Value::String(s)) if s == "too_large" => Ok(()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "path" => same(
            validate_path(&hex(&case["path"])).is_ok(),
            expect.as_bool().unwrap(),
        ),

        // Device chains.
        "chain_schedule" => {
            let mut store = Store {
                records: case["server"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(chain_record)
                    .map(|r| (r.counter, r))
                    .collect(),
                pruned: case.get("server_pruned").is_some(),
            };
            let heads: Vec<Ref> = case["start_heads"]
                .as_array()
                .unwrap()
                .iter()
                .map(chain_record)
                .map(|r| Ref {
                    device: r.device,
                    counter: r.counter,
                    seq: r.seq,
                    op_digest: r.digest,
                })
                .collect();
            let mut verifier = ChainVerifier::from_heads(&heads);
            let mut verdicts = Vec::new();
            for (p, page) in case["pages"].as_array().unwrap().iter().enumerate() {
                let records: Vec<ChainRecord> =
                    page.as_array().unwrap().iter().map(chain_record).collect();
                match verifier.process_page(&records, &mut store) {
                    Ok(v) => verdicts.push(serde_json::to_value(v).unwrap()),
                    Err((i, e)) => {
                        return same(
                            json!({"error": e.client_code().trim_start_matches("sync_chain_"), "page": p, "index": i}),
                            expect.clone(),
                        );
                    }
                }
            }
            same(json!(verdicts), expect.clone())
        }

        "fork_resolution" => {
            let records = |v: &Value| -> Vec<ChainRecord> {
                v.as_array().unwrap().iter().map(chain_record).collect()
            };
            let mut verifier = ChainVerifier::new();
            for page in case["applied"].as_array().unwrap() {
                verifier
                    .process_page(&records(page), &mut NoFetch)
                    .map_err(|e| format!("an applied page failed: {e:?}"))?;
            }
            // A checkpoint the reader accepted after the applied pages.
            if let Some(covered) = case.get("accepted_covered_seq") {
                verifier.forget_below(u64_of(covered));
            }
            let forked = records(&case["forked"]);
            match verifier.clone().process_page(&forked, &mut NoFetch) {
                Err((_, ChainError::Forked)) => {}
                other => return Err(format!("the page should fork: {other:?}")),
            }
            let kept = chain_record(&case["kept"]);
            let kept = Ref {
                device: kept.device,
                counter: kept.counter,
                seq: kept.seq,
                op_digest: kept.digest,
            };
            match verifier.resolve_fork(&kept, &records(&case["records"])) {
                Err(e) => same(
                    json!({"error": e.client_code().trim_start_matches("sync_chain_")}),
                    expect.clone(),
                ),
                Ok(reverted) => match verifier.process_page(&forked, &mut NoFetch) {
                    Ok(verdicts) => {
                        let reverted: Vec<String> =
                            reverted.iter().map(|r| r.digest.to_hex()).collect();
                        let verdicts = serde_json::to_value(verdicts).unwrap();
                        if expect.get("verdicts").is_some() {
                            same(
                                json!({"reverted": reverted, "verdicts": verdicts}),
                                expect.clone(),
                            )
                        } else if reverted.is_empty() {
                            same(verdicts, expect.clone())
                        } else {
                            Err(format!("settling reverted {reverted:?}"))
                        }
                    }
                    Err((i, e)) => Err(format!("record {i} failed after settling: {e:?}")),
                },
            }
        }

        "checkpoint_walk" => {
            use sync_protocol::manifest::{
                ChainLink, CheckpointFetcher, CheckpointWalk, WalkEntry,
            };
            let link = |v: &Value| ChainLink {
                number: u64_of(&v["number"]),
                digest: Digest::from_hex(v["digest"].as_str().unwrap()).unwrap(),
                previous: Digest::from_hex(v["previous"].as_str().unwrap()).unwrap(),
            };
            struct Served(Vec<ChainLink>);
            impl CheckpointFetcher for Served {
                fn checkpoint(&mut self, digest: &Digest) -> Option<ChainLink> {
                    self.0.iter().find(|l| l.digest == *digest).copied()
                }
            }
            let mut served = Served(
                case["server"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(link)
                    .collect(),
            );
            let reached: Vec<(u64, Digest)> = case["reached"]
                .as_array()
                .unwrap()
                .iter()
                .map(link)
                .map(|l| (l.number, l.digest))
                .collect();
            let entries: Vec<WalkEntry> = case["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    if let Some(c) = e.get("checkpoint") {
                        WalkEntry::Checkpoint(link(c))
                    } else {
                        let op = &e["operation"];
                        WalkEntry::Operation {
                            generation: u64_of(&op["generation"]),
                            checkpoint_seq: u64_of(&op["checkpoint_seq"]),
                            checkpoint: Digest::from_hex(op["checkpoint"].as_str().unwrap())
                                .unwrap(),
                        }
                    }
                })
                .collect();
            let mut walk =
                CheckpointWalk::new(reached, u64_of(&case["floor"]), u64_of(&case["generation"]));
            let ours = match walk.plan(&entries, &mut served) {
                Ok(planned) => json!({"fetched": planned
                    .iter()
                    .map(|(before, walk)| json!({
                        "before": before,
                        "checkpoints": walk.iter().map(|l| l.digest.to_hex()).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>()}),
                Err((index, _)) => json!({"error": "gap", "index": index}),
            };
            same(ours, expect.clone())
        }

        // Live sets.
        "live_set" => {
            let ops: Vec<LiveOp> = case["ops"]
                .as_array()
                .unwrap()
                .iter()
                .map(live_op)
                .collect();
            let floors = case["floors"].as_array().unwrap();
            let mut set = LiveSet::new();
            let mut classes = Vec::new();
            let apply_floors = |set: &mut LiveSet, after: usize| -> Outcome {
                for floor in floors {
                    let at = floor
                        .get("after_op")
                        .map_or(ops.len(), |a| u64_of(a) as usize);
                    if at != after {
                        continue;
                    }
                    let dropped = set.apply_floor(
                        u64_of(&floor["previous_floor"]),
                        u64_of(&floor["floor"]),
                        u64_of(&floor["covered_seq"]),
                    );
                    let ours: Vec<Value> = dropped
                        .iter()
                        .map(|(rid, r)| json!({"rid": rid.to_hex(), "device": r.device.to_hex(), "counter": r.counter}))
                        .collect();
                    same(json!(ours), floor["dropped"].clone())?;
                }
                Ok(())
            };
            for (i, op) in ops.iter().enumerate() {
                apply_floors(&mut set, i)?;
                classes.push(error_name(&set.apply(op.clone())));
            }
            apply_floors(&mut set, ops.len())?;
            same(json!(classes), case["classes"].clone())?;
            for (rid, want) in expect.as_object().unwrap() {
                let mut ours: Vec<(Id, u64)> = set
                    .live(&Id::from_hex(rid).unwrap())
                    .iter()
                    .map(|(o, _)| (o.device, o.counter))
                    .collect();
                ours.sort();
                let ours: Vec<Value> = ours
                    .iter()
                    .map(|(d, c)| json!({"device": d.to_hex(), "counter": c}))
                    .collect();
                same(json!(ours), want.clone())?;
            }
            Ok(())
        }
        "live_check" => {
            let mut set = LiveSet::new();
            for op in case["ops"].as_array().unwrap() {
                set.apply(live_op(op));
            }
            let ours = match set.check(&live_op(&case["next"])) {
                Ok(class) => error_name(&class),
                Err(e) => error_name::<LiveError>(&e),
            };
            same(ours, expect.as_str().unwrap().to_owned())
        }
        "covers_for" => {
            let mut set = LiveSet::new();
            let ops: Vec<LiveOp> = case["ops"]
                .as_array()
                .unwrap()
                .iter()
                .map(live_op)
                .collect();
            for op in &ops {
                set.apply(op.clone());
            }
            let rid = ops[0].rid;
            let live = set.live(&rid);
            let refs: Vec<&LiveOp> = live.iter().map(|(o, _)| *o).collect();
            let chosen: BTreeSet<(Id, u64)> = case["chosen"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| (id_of(&c["device"]), u64_of(&c["counter"])))
                .collect();
            let covers = covers_for(&refs, &chosen, id_of(&case["own_device"]));
            same(serde_json::to_value(covers).unwrap(), expect.clone())
        }

        // Markers.
        "marker" => same(
            parse_marker(&hex(&case["content"])),
            expect.as_str().map(|e| Id::from_hex(e).unwrap()),
        )
        .and_then(|()| match expect.as_str() {
            Some(e) => same(
                marker_content(&Id::from_hex(e).unwrap()).into_bytes(),
                hex(&case["content"]),
            ),
            None => Ok(()),
        }),

        // Matching without a journal.
        "journal_less_match" => {
            let objects: Vec<LiveObject> = serde_json::from_value(case["objects"].clone()).unwrap();
            let locals: Vec<LocalFile> = serde_json::from_value(case["locals"].clone()).unwrap();
            same(
                serde_json::to_value(match_files(&objects, &locals)).unwrap(),
                expect.clone(),
            )
        }
        "archived" => {
            let objects: Vec<JournalObject> =
                serde_json::from_value(case["objects"].clone()).unwrap();
            same(
                serde_json::to_value(archived(&objects)).unwrap(),
                expect.clone(),
            )
        }
        "stray_marker" => {
            let markers: Vec<Id> = serde_json::from_value(case["marker_rids"].clone()).unwrap();
            same(
                refuses_marker_path(
                    &id_of(&case["rid"]),
                    case["path"].as_str().unwrap(),
                    &markers,
                ),
                expect.as_bool().unwrap(),
            )
        }
        "held_roots" => {
            let locals: Vec<LocalMarker> = serde_json::from_value(case["locals"].clone()).unwrap();
            let applied: Vec<DomainRoot> = serde_json::from_value(case["applied"].clone()).unwrap();
            same(
                serde_json::to_value(held_roots(&locals, &applied)).unwrap(),
                expect.clone(),
            )
        }
        "hold_ends" => {
            let held: HeldRoot = serde_json::from_value(case["held"].clone()).unwrap();
            let roots: Vec<DomainRoot> = serde_json::from_value(case["roots"].clone()).unwrap();
            same(hold_ends(&held, &roots), expect.as_bool().unwrap())
        }

        // The manifest.
        "leaf_hash" => {
            let refs: Vec<Ref> = serde_json::from_value(case["refs"].clone()).unwrap();
            same(leaf_hash(&id_of(&case["rid"]), &refs), digest_of(expect))
        }
        "bucket_of" => same(u64::from(bucket_of(&id_of(&case["rid"]))), u64_of(expect)),
        "bucket_hash" => {
            let hashes: Vec<Digest> = case["leaf_hashes"]
                .as_array()
                .unwrap()
                .iter()
                .map(digest_of)
                .collect();
            same(
                bucket_hash(u64_of(&case["index"]) as u16, &hashes),
                digest_of(expect),
            )
        }
        "node_hash" => {
            let children: Vec<Digest> = case["children"]
                .as_array()
                .unwrap()
                .iter()
                .map(digest_of)
                .collect();
            same(
                node_hash(
                    u64_of(&case["level"]) as u8,
                    u64_of(&case["index"]) as u16,
                    &children,
                ),
                digest_of(expect),
            )
        }
        "heads_digest" => {
            let heads: Vec<Ref> = serde_json::from_value(case["heads"].clone()).unwrap();
            same(heads_digest(&heads), digest_of(expect))
        }
        "root" => {
            let manifest = Manifest::new(leaves(&case["leaves"]));
            same(manifest.root(), digest_of(expect))?;
            same(manifest.live_objects(), u64_of(&case["live_objects"]))
        }
        "verify_manifest" => {
            let manifest = Manifest::new(leaves(&case["leaves"]));
            let result = verify_manifest(
                &manifest,
                &digest_of(&case["root"]),
                u64_of(&case["live_objects"]),
            );
            same(
                result.is_err(),
                expect.as_str() == Some("sync_manifest_mismatch"),
            )
        }
        "verify_bucket" => {
            let leaves: Vec<(Id, Vec<Ref>)> = leaves(&case["leaves"]).into_iter().collect();
            let result = verify_bucket(
                u64_of(&case["index"]) as u16,
                &leaves,
                &digest_of(&case["hash"]),
            );
            same(
                if result.is_ok() {
                    "ok"
                } else {
                    "sync_manifest_mismatch"
                },
                expect.as_str().unwrap(),
            )
        }
        "skip_number" => same(skip_number(u64_of(&case["number"])), u64_of(expect)),
        "descent_path" => same(
            json!(descent_path(u64_of(&case["from"]), u64_of(&case["to"]))),
            expect.clone(),
        ),
        "verify_descent" => {
            let walk: Vec<Signed<Checkpoint>> = case["walk"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| Signed::<Checkpoint>::from_bytes(&hex(c)).unwrap())
                .collect();
            let result = verify_descent(&walk, &digest_of(&case["anchor"]));
            same(
                if result.is_ok() {
                    "ok"
                } else {
                    "sync_manifest_rollback"
                },
                expect.as_str().unwrap(),
            )
        }
        // The server's checks need a store, so the store conformance suite
        // runs these on each store.
        "server_check" | "server_heads" => Ok(()),
        "heads_may_leave" => same(
            policy(case).card_gone(&id_of(&case["device"]), u64_of(&case["revision"])),
            expect.as_bool().unwrap(),
        ),
        "checkpoint_check" => {
            let mut chain = CheckpointChain::new();
            for c in case["accepted"].as_array().unwrap() {
                chain.accept(&Signed::<Checkpoint>::from_bytes(&hex(c)).unwrap());
            }
            let next = Signed::<Checkpoint>::from_bytes(&hex(&case["next"])).unwrap();
            let heads: Vec<Ref> = serde_json::from_value(case["heads"].clone()).unwrap();
            let previous: Vec<Ref> =
                serde_json::from_value(case["previous_heads"].clone()).unwrap();
            let manifest = Manifest::new(leaves(&case["leaves"]));
            let ours = match chain.check(&next, &heads, &manifest, &previous, |_| false) {
                Ok(()) => "ok".to_owned(),
                Err(e) => error_name::<CheckpointError>(&e),
            };
            same(ours, expect.as_str().unwrap().to_owned())
        }

        // Blobs.
        "nonce_prefix" => {
            let mut material = hex(&case["blob_key"]);
            material.extend_from_slice(&hex(&case["blob"]));
            let derived = derive_key(case["context"].as_str().unwrap(), &material);
            same(derived.expose()[..19].to_vec(), hex(expect))?;
            same(
                nonce_prefix(&secret_of(&case["blob_key"]), &id_of(&case["blob"])).to_vec(),
                hex(expect),
            )
        }
        "blob_header" => {
            let blob = id_of(&case["blob"]);
            let prefix = nonce_prefix(&secret_of(&case["blob_key"]), &blob);
            let mut ours = Tag::Blob.bytes();
            ours.extend_from_slice(&blob.0);
            ours.extend_from_slice(&prefix);
            same(header(&blob, &prefix), ours)?;
            same(header(&blob, &prefix), hex(expect))
        }
        "chunk_nonce" => {
            let mut nonce = hex(&case["prefix"]);
            nonce.extend_from_slice(&(u64_of(&case["index"]) as u32).to_be_bytes());
            nonce.push(u8::from(case["last"].as_bool().unwrap()));
            same(nonce, hex(expect))
        }
        "blob_lengths" => {
            let len = u64_of(&case["file_length"]);
            let p = padded_length(len + 8);
            same(p, u64_of(&case["padded"]))?;
            same(p.div_ceil(65_536), u64_of(&case["chunks"]))?;
            same(blob_ciphertext_len(len), u64_of(&case["ciphertext_length"]))
        }
        "blob" => {
            let file = hex(&case["file"]);
            let blob_ref: BlobRef = serde_json::from_value(case["blob_ref"].clone()).unwrap();
            let ciphertext = hex(&case["ciphertext"]);
            let result = decrypt_blob(&ciphertext, &blob_ref, Some(&digest_of(&case["revision"])));
            match (result, expect.as_str().unwrap()) {
                (Ok(plain), "ok") => {
                    same(plain, file.clone())?;
                    // The file and its blob_ref rebuild the blob byte for byte.
                    let (rebuilt, rebuilt_ref) =
                        encrypt_blob(&file, blob_ref.blob, blob_ref.blob_key.clone()).unwrap();
                    same(rebuilt, ciphertext)?;
                    same(rebuilt_ref, blob_ref)
                }
                (Err(BlobError::PlaintextLength), "plaintext_length") => Ok(()),
                (Err(BlobError::Digest), "digest") => Ok(()),
                (other, _) => Err(format!("{other:?}")),
            }
        }
        "blob_digest" => {
            let len = u64_of(&case["file_length"]) as usize;
            let file: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
            let (ciphertext, blob_ref) =
                encrypt_blob(&file, id_of(&case["blob"]), secret_of(&case["blob_key"])).unwrap();
            same(ciphertext.len() as u64, u64_of(&case["ciphertext_length"]))?;
            same(
                ciphertext.len().div_ceil(1_048_576) as u64,
                u64_of(&case["segments"]),
            )?;
            same(ciphertext_digest(&ciphertext), digest_of(expect))?;
            same(blob_ref.ciphertext_digest, digest_of(expect))
        }

        // Text mode.
        "text_lineage" => {
            let rid = id_of(&case["rid"]);
            let base = digest_of(&case["base"]);
            let document = hex(&case["document"]);
            if let Some(input) = case["input"].as_str() {
                let mut ours = Tag::TextLineage.bytes();
                ours.extend_from_slice(&rid.0);
                ours.extend_from_slice(&base.0);
                ours.extend_from_slice(&document);
                same(to_hex(&ours), input.to_owned())?;
                same(
                    Id(hash(&ours).0[..16].try_into().unwrap()),
                    text_lineage_id(&rid, &base, &document),
                )?;
            }
            same(text_lineage_id(&rid, &base, &document), id_of(expect))
        }
        "client_id" => same(
            json!(yrs_client_id(
                u64_of(&case["card_index"]) as u32,
                u64_of(&case["load"]) as u32
            )),
            expect.clone(),
        ),
        // Re-wrap schedules need a server, so the store conformance suite
        // replays them.
        "rewrap_schedule" => Ok(()),
        // The collaboration sandbox's cases need its guest module, so
        // crates/collab-sandbox runs them.
        "verdict" | "client_round_trip" | "surrogate" | "allowance" | "trap" | "pending"
        | "verdict_limit" | "verdict_trap" => Ok(()),
        other => Err(format!("unknown case kind {other}")),
    }
}

fn check_policy(chain: &PolicyChain, checks: &Value) -> Outcome {
    let head = chain.head_snapshot();
    if let Some(owner) = checks.get("owner") {
        same(head.owner(), Some(id_of(owner)))?;
    }
    if let Some(roles) = checks.get("roles").and_then(Value::as_object) {
        for (account, role) in roles {
            same(
                head.role(&Id::from_hex(account).unwrap()).map(role_name),
                role.as_str().map(str::to_owned),
            )?;
        }
    }
    if let Some(devices) = checks.get("devices").and_then(Value::as_object) {
        for (revision, want) in devices {
            let snapshot = chain
                .at(revision.parse().unwrap())
                .ok_or("no such revision")?;
            same(sorted_ids(snapshot.devices.keys()), ids(want))
                .map_err(|e| format!("devices at {revision}: {e}"))?;
        }
    }
    if let Some(indices) = checks.get("card_indices") {
        let ours: BTreeMap<String, u64> = head
            .card_indices
            .iter()
            .map(|(d, i)| (d.to_hex(), u64::from(*i)))
            .collect();
        same(json!(ours), indices.clone())?;
    }
    if let Some(rotations) = checks.get("rotations").and_then(Value::as_object) {
        for (revision, want) in rotations {
            let snapshot = chain
                .at(revision.parse().unwrap())
                .ok_or("no such revision")?;
            same(sorted_ids(snapshot.rotations.iter()), {
                let mut w = ids(want);
                w.sort();
                w
            })
            .map_err(|e| format!("rotations at {revision}: {e}"))?;
        }
    }
    if let Some(access) = checks.get("access").and_then(Value::as_object) {
        for (domain, want) in access {
            let d = &head.domains[&Id::from_hex(domain).unwrap()];
            same(sorted_ids(d.access.keys()), ids(want))?;
        }
    }
    if let Some(readers) = checks.get("readers").and_then(Value::as_object) {
        for (revision, domains) in readers {
            let snapshot = chain
                .at(revision.parse().unwrap())
                .ok_or("no such revision")?;
            for (domain, want) in domains.as_object().unwrap() {
                same(
                    snapshot
                        .readers(&Id::from_hex(domain).unwrap())
                        .into_iter()
                        .collect::<Vec<_>>(),
                    ids(want),
                )?;
            }
        }
    }
    Ok(())
}
