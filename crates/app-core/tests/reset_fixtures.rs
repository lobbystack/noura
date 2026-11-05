//! The reset cases of `sync-v2-account.json` and `sync-v2-approval.json`
//! (§7, §10, §14), through the client's own code: its kept heads, the
//! alerts' inputs and the checks a reset device makes before it trusts a
//! workspace.

mod sync_support;

use std::collections::BTreeSet;

use app_core::sync::joining::{reset_committed, reset_request};
use app_core::sync::removal::reset_fingerprint;
use app_core::sync::{Anchor, KeptHeads, SyncError};
use serde_json::Value;
use sync_protocol::account::AccountChain;
use sync_protocol::codec::Decode;
use sync_protocol::messages::Pending;
use sync_protocol::policy::{AccountChains, PolicyChain};
use sync_protocol::records::{AccountHead, AccountLink, Offer, PolicyChange, Signed};
use sync_protocol::types::{Digest, Id, from_hex};
use sync_support::MemorySecrets;

fn fixture(file: &str) -> Value {
    let path = format!(
        "{}/../../docs/workspace-format/fixtures/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn hex(value: &Value) -> Vec<u8> {
    from_hex(value.as_str().unwrap()).unwrap()
}

fn digest(value: &Value) -> Digest {
    Digest::from_hex(value.as_str().unwrap()).unwrap()
}

fn links(value: &Value) -> Vec<Signed<AccountLink>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|link| Signed::from_bytes(&hex(link)).unwrap())
        .collect()
}

fn policy_of(case: &Value) -> PolicyChain {
    let mut chains = AccountChains::new();
    for chain in case["account_chains"].as_array().unwrap() {
        chains.insert(AccountChain::verify(&links(chain), &BTreeSet::new()).unwrap());
    }
    let changes: Vec<Signed<PolicyChange>> = case["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| Signed::from_bytes(&hex(change)).unwrap())
        .collect();
    PolicyChain::verify(&changes, &chains).expect("the case's policy verifies")
}

fn head(value: &Value) -> AccountHead {
    serde_json::from_value(value.clone()).unwrap()
}

/// A device that keeps `kept`, of its own account or another's.
fn device_keeping(
    kept: &AccountHead,
    own_account: bool,
    dir: &std::path::Path,
    secrets: &MemorySecrets,
) -> KeptHeads {
    let own = if own_account {
        kept.account
    } else {
        Id::random().unwrap()
    };
    let device = Id::random().unwrap();
    KeptHeads::rebuild(dir, own, device, [kept.clone()], [], secrets).unwrap()
}

#[test]
fn the_account_reset_cases_reach_their_outcomes() {
    let doc = fixture("sync-v2-account.json");
    let cases = doc["reset_cases"].as_array().expect("reset cases");
    assert!(cases.len() >= 10);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let secrets = MemorySecrets::default();
        match case["kind"].as_str().unwrap() {
            "new_genesis" => {
                let kept = head(&case["kept"]);
                let mut heads = device_keeping(&kept, false, dir.path(), &secrets);
                let new =
                    AccountChain::verify(&links(&case["new_chain"]), &BTreeSet::new()).unwrap();
                let policy = policy_of(case);
                let outcome = heads.enter(&new, Anchor::Workspace(&policy), &secrets);
                match (outcome, case["expect"]["outcome"].as_str().unwrap()) {
                    (Ok(()), "ok") => {}
                    (Err(SyncError::Check(code)), expect) => assert_eq!(code, expect, "{name}"),
                    (other, expect) => panic!("{name}: {other:?}, wanted {expect}"),
                }
                assert_eq!(
                    new.genesis(),
                    digest(&case["expect"]["fingerprint"]),
                    "{name}"
                );
            }
            "retired_device" => {
                let served = links(&case["served"]);
                let genesis = served[0].digest();
                let found = reset_fingerprint(
                    &hex(&case["detail"]),
                    &digest(&case["own_genesis"]),
                    served[0].body.account,
                    genesis,
                    &served,
                )
                .unwrap();
                match case["expect"].as_str() {
                    Some(_) => assert_eq!(found, Some(digest(&case["expect"])), "{name}"),
                    None => assert_eq!(found, None, "{name}"),
                }
            }
            "changed_devices" => {
                let pending = Pending::from_bytes(&hex(&case["pending"])).unwrap();
                let workspace = Id::from_hex(case["workspace"].as_str().unwrap()).unwrap();
                let shown = reset_request(pending, workspace).map(|r| r.fingerprint);
                match case["expect"].as_str() {
                    Some(_) => assert_eq!(shown, Some(digest(&case["expect"])), "{name}"),
                    None => assert_eq!(shown, None, "{name}"),
                }
            }
            "two_chains" => {
                let account = Id::from_hex(case["account"].as_str().unwrap()).unwrap();
                let mut heads =
                    KeptHeads::rebuild(dir.path(), account, account, [], [], &secrets).unwrap();
                let first = Id::random().unwrap();
                let second = Id::random().unwrap();
                heads.name(first, account, digest(&case["first"])).unwrap();
                heads
                    .name(second, account, digest(&case["second"]))
                    .unwrap();
                assert_eq!(
                    heads.two_current_chains().contains(&account),
                    case["expect"].as_bool().unwrap(),
                    "{name}"
                );
            }
            "retired_chain" => {
                let kept = head(&case["kept"]);
                let own_account = case["own_account"].as_bool().unwrap();
                let mut heads = device_keeping(&kept, own_account, dir.path(), &secrets);
                let served = links(&case["served"]);
                let refused = heads.accept(
                    kept.account,
                    kept.genesis,
                    &served,
                    &BTreeSet::new(),
                    &secrets,
                );
                let Err(SyncError::Check(code)) = refused else {
                    panic!("{name}: the short chain was taken");
                };
                assert_eq!(code, case["expect"]["before"].as_str().unwrap(), "{name}");
                assert!(heads.stopped([kept.account]).is_some(), "{name}");
                let workspace = Id::random().unwrap();
                let genesis = digest(&case["new_genesis"]);
                heads
                    .drop_retired(&kept.account, &genesis, workspace, &secrets)
                    .unwrap();
                assert!(heads.kept_of(&kept.account).is_empty(), "{name}");
                assert_eq!(heads.stopped([kept.account]), None, "{name}");
            }
            kind => panic!("{name}: unknown kind {kind}"),
        }
    }
}

#[test]
fn the_approval_reset_cases_reach_their_outcomes() {
    let doc = fixture("sync-v2-approval.json");
    let cases = doc["reset_cases"].as_array().expect("reset cases");
    assert!(cases.len() >= 6);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert_eq!(case["kind"], "reset_commit", "{name}");
        let offer = Signed::<Offer>::from_bytes(&hex(&case["offer"])).unwrap();
        let outcome = reset_committed(&policy_of(case), &offer.body, &head(&case["head"]));
        match (outcome, &case["expect"]) {
            (Ok(done), Value::Bool(expect)) => assert_eq!(done, *expect, "{name}"),
            (Err(SyncError::Check(code)), Value::String(expect)) => {
                assert_eq!(code, expect, "{name}")
            }
            (other, expect) => panic!("{name}: {other:?}, wanted {expect}"),
        }
    }
}
