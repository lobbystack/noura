//! The limiter behind the rate limits of §26: a fixed-window counter per
//! key and window length, so two limits on one key count apart. SQLite mode runs one instance, so [`MemoryLimiter`] keeps its
//! counters in memory; PostgreSQL mode keeps them in the database, so a
//! limit used up through one instance also holds on the others. Ticket 52
//! applies limits to sign-in and sync.
//!
//! Windows start at multiples of their length since the Unix epoch, so every
//! instance that shares a clock agrees on them.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{BoxFut, Result};

/// How much a key may use in each window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    /// The allowance per window, in the caller's unit: requests or bytes.
    pub allowance: u64,
    /// The window's length; at least a millisecond counts.
    pub window: Duration,
}

impl Limit {
    /// The window's length in milliseconds, at least 1.
    pub fn window_ms(&self) -> u64 {
        u64::try_from(self.window.as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    }

    /// When the window around `now_ms` ends, in milliseconds since the Unix
    /// epoch.
    pub fn window_ends(&self, now_ms: u64) -> u64 {
        let window = self.window_ms();
        (now_ms / window).saturating_add(1).saturating_mul(window)
    }
}

/// What [`Limiter::take`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// The cost fits the key's allowance, which now counts it.
    Allowed,
    /// The cost doesn't fit; the counter didn't change.
    Refused {
        /// How long until the window ends.
        retry_after: Duration,
    },
}

/// Counts what each key uses per window.
pub trait Limiter: Send + Sync {
    /// Takes `cost` from `key`'s allowance in the current window when it
    /// fits, and refuses it otherwise. A cost above the whole allowance
    /// never fits.
    fn take<'a>(&'a self, key: &'a [u8], cost: u64, limit: Limit) -> BoxFut<'a, Result<Taken>>;

    /// Deletes the counts of every window that has ended, and returns how
    /// many it deleted. The server runs it every [`PRUNE_EVERY`].
    fn prune(&self) -> BoxFut<'_, Result<u64>>;
}

/// How often the server deletes the counts of ended windows.
pub const PRUNE_EVERY: Duration = Duration::from_secs(60);

/// Milliseconds since the Unix epoch on this computer's clock.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Entries the memory limiter holds before it drops ended windows.
const SWEEP_AT: usize = 16_384;

/// The limiter of a single instance, with its counters in memory.
#[derive(Debug, Default)]
pub struct MemoryLimiter {
    counters: Mutex<HashMap<(Vec<u8>, u64), Counter>>,
}

#[derive(Clone, Copy, Debug)]
struct Counter {
    window_ends: u64,
    used: u64,
}

impl MemoryLimiter {
    fn take_at(&self, key: &[u8], cost: u64, limit: Limit, now: u64) -> Taken {
        let ends = limit.window_ends(now);
        let refused = Taken::Refused {
            retry_after: Duration::from_millis(ends - now),
        };
        if cost > limit.allowance {
            return refused;
        }
        let mut counters = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
        if counters.len() >= SWEEP_AT {
            counters.retain(|_, counter| counter.window_ends > now);
        }
        let counter = counters
            .entry((key.to_vec(), limit.window_ms()))
            .or_insert(Counter {
                window_ends: ends,
                used: 0,
            });
        if counter.window_ends != ends {
            *counter = Counter {
                window_ends: ends,
                used: 0,
            };
        }
        match counter.used.checked_add(cost) {
            Some(used) if used <= limit.allowance => {
                counter.used = used;
                Taken::Allowed
            }
            _ => refused,
        }
    }
}

impl Limiter for MemoryLimiter {
    fn take<'a>(&'a self, key: &'a [u8], cost: u64, limit: Limit) -> BoxFut<'a, Result<Taken>> {
        Box::pin(async move { Ok(self.take_at(key, cost, limit, now_ms())) })
    }

    fn prune(&self) -> BoxFut<'_, Result<u64>> {
        Box::pin(async move {
            let now = now_ms();
            let mut counters = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
            let before = counters.len();
            counters.retain(|_, counter| counter.window_ends > now);
            Ok((before - counters.len()) as u64)
        })
    }
}
