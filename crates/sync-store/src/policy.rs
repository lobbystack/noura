//! Verified policy chains, rebuilt from the stored changes and the account
//! chains they record, and kept per workspace while their head stays the
//! stored one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use hashlink::LruCache;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::Decode;
use sync_protocol::policy::{AccountChains, PolicyChain};
use sync_protocol::records::{AccountHead, AccountLink, PolicyChange, Signed};
use sync_protocol::types::{Digest, Id};

use crate::{Error, ReadTxn, Result, WorkspaceRow};

/// The most chains each cache keeps; the least recently used one goes
/// first, so a server with many accounts and workspaces keeps the busy
/// ones and verifies the others again when they come back.
pub(crate) const CACHED_CHAINS: usize = 1_024;

/// Verified chains by their account or workspace ID, at most
/// [`CACHED_CHAINS`] of them.
struct Bounded<T>(Arc<Mutex<LruCache<Id, Arc<T>>>>);

impl<T> Clone for Bounded<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Bounded<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(LruCache::new(CACHED_CHAINS))))
    }
}

impl<T> Bounded<T> {
    /// The chain cached for `id` when `current` accepts it; one it refuses
    /// stays until a newer one replaces it.
    fn get(&self, id: &Id, current: impl FnOnce(&T) -> bool) -> Option<Arc<T>> {
        let mut chains = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        chains.get(id).filter(|chain| current(chain)).cloned()
    }

    fn put(&self, id: Id, chain: Arc<T>) {
        let mut chains = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        chains.insert(id, chain);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// The verified policy chain of each workspace, keyed by its head. A chain
/// whose head differs from the stored one is verified again, so a stale
/// entry never serves a check.
#[derive(Clone, Default)]
pub struct PolicyCache {
    chains: Bounded<PolicyChain>,
}

impl PolicyCache {
    fn get(&self, workspace: &Id, revision: u64, digest: &Digest) -> Option<Arc<PolicyChain>> {
        self.chains
            .get(workspace, |chain| chain.head() == (revision, *digest))
    }

    pub(crate) fn put(&self, chain: Arc<PolicyChain>) {
        self.chains.put(chain.workspace(), chain);
    }
}

/// A workspace's row and its verified policy chain at the head the
/// transaction sees, or `None` for a workspace the store doesn't hold.
pub(crate) async fn load(
    txn: &mut impl ReadTxn,
    cache: &PolicyCache,
    workspace: Id,
) -> Result<Option<(WorkspaceRow, Arc<PolicyChain>)>> {
    let Some(row) = txn.workspace(workspace).await? else {
        return Ok(None);
    };
    if let Some(chain) = cache.get(&workspace, row.policy_revision, &row.policy_head) {
        return Ok(Some((row, chain)));
    }
    let changes = txn
        .policy_changes(workspace, 0)
        .await?
        .iter()
        .map(|bytes| Signed::<PolicyChange>::from_bytes(bytes).map_err(Error::internal))
        .collect::<Result<Vec<_>>>()?;
    let accounts: BTreeSet<Id> = changes
        .iter()
        .flat_map(|change| change.body.heads.iter().map(|head| head.account))
        .collect();
    let chains = account_chains(txn, accounts).await?;
    let chain = PolicyChain::verify(&changes, &chains).map_err(Error::internal)?;
    if chain.head() != (row.policy_revision, row.policy_head) {
        return Err(Error::internal("the stored policy head isn't the chain's"));
    }
    let chain = Arc::new(chain);
    cache.put(chain.clone());
    Ok(Some((row, chain)))
}

/// Every stored chain of each account, verified.
pub(crate) async fn account_chains(
    txn: &mut impl ReadTxn,
    accounts: impl IntoIterator<Item = Id>,
) -> Result<AccountChains> {
    let mut chains = AccountChains::new();
    for account in accounts {
        for chain in chains_of(txn, account).await?.into_values() {
            chains.insert(chain);
        }
    }
    Ok(chains)
}

/// Every stored chain of `account`, verified, keyed by genesis digest.
pub(crate) async fn chains_of(
    txn: &mut impl ReadTxn,
    account: Id,
) -> Result<BTreeMap<Digest, AccountChain>> {
    let mut grouped: BTreeMap<Digest, Vec<Signed<AccountLink>>> = BTreeMap::new();
    for row in txn.account_links(account).await? {
        let link = Signed::<AccountLink>::from_bytes(&row.link).map_err(Error::internal)?;
        grouped.entry(row.genesis).or_default().push(link);
    }
    grouped
        .into_iter()
        .map(|(genesis, links)| {
            AccountChain::verify(&links, &BTreeSet::new())
                .map(|chain| (genesis, chain))
                .map_err(Error::internal)
        })
        .collect()
}

/// Each account's verified current chain, keyed by the head the store
/// holds. Links only append and each names its predecessor's digest, so a
/// chain whose genesis, head seqno and head digest match the stored ones is
/// the stored chain; any other head is verified again.
#[derive(Clone, Default)]
pub(crate) struct ChainCache {
    chains: Bounded<AccountChain>,
}

impl ChainCache {
    fn get(&self, head: &AccountHead) -> Option<Arc<AccountChain>> {
        self.chains.get(&head.account, |chain| {
            chain.genesis() == head.genesis
                && chain.head_seqno() == head.seqno
                && chain.head_digest() == head.head
        })
    }

    fn put(&self, chain: Arc<AccountChain>) {
        self.chains.put(chain.account(), chain);
    }
}

/// The account's current chain, verified, from `cache` while the stored
/// head is the cached chain's.
pub(crate) async fn cached_current_chain(
    txn: &mut impl ReadTxn,
    cache: &ChainCache,
    account: Id,
) -> Result<Option<Arc<AccountChain>>> {
    let Some(head) = txn.current_head(account).await? else {
        return Ok(None);
    };
    if let Some(chain) = cache.get(&head) {
        return Ok(Some(chain));
    }
    let Some(chain) = current_chain(txn, account).await? else {
        return Ok(None);
    };
    let chain = Arc::new(chain);
    if chain.head() == head {
        cache.put(chain.clone());
    }
    Ok(Some(chain))
}

/// The account's current chain, verified.
pub(crate) async fn current_chain(
    txn: &mut impl ReadTxn,
    account: Id,
) -> Result<Option<AccountChain>> {
    let Some(genesis) = txn.current_genesis(account).await? else {
        return Ok(None);
    };
    Ok(chains_of(txn, account).await?.remove(&genesis))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sync_protocol::types::Id;

    use super::{Bounded, CACHED_CHAINS};

    fn id(n: usize) -> Id {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
        Id(bytes)
    }

    #[test]
    fn the_caches_keep_the_most_recently_used_chains() {
        let cache = Bounded::<usize>::default();
        let limit = CACHED_CHAINS;
        for n in 0..limit {
            cache.put(id(n), Arc::new(n));
        }
        // Reading the first chain makes the second the oldest.
        assert!(cache.get(&id(0), |_| true).is_some());
        for n in limit..limit * 3 {
            cache.put(id(n), Arc::new(n));
            assert!(cache.len() <= limit);
        }
        assert_eq!(cache.len(), limit);
        assert!(cache.get(&id(1), |_| true).is_none());
        assert!(cache.get(&id(limit * 3 - 1), |_| true).is_some());
        // A chain that is no longer current serves no check.
        assert!(cache.get(&id(limit * 3 - 1), |n| *n == 0).is_none());
    }
}
