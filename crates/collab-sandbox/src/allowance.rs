//! Each writer's fuel allowance per note (sync-v2.md §22): a collaborator who
//! floods a note, or sends one huge record, slows only their own edits.

use std::time::{Duration, Instant};

/// A writer's allowance per note, in fuel units, and what each minute
/// refills it up to.
pub const ALLOWANCE_FUEL: i64 = 2_000_000_000;
/// How often an allowance refills.
pub const REFILL_PERIOD: Duration = Duration::from_secs(60);

/// One writer's balance on one note. Both stages charge it the fuel a
/// record used, so one record may overdraw it; while it's negative, the
/// writer's further records wait.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Allowance {
    balance: i64,
    /// When the last refill period began.
    refilled_at: Instant,
}

impl Allowance {
    pub(crate) fn new(now: Instant) -> Self {
        Self::with_balance(ALLOWANCE_FUEL, now)
    }

    pub(crate) fn with_balance(balance: i64, now: Instant) -> Self {
        Self {
            balance,
            refilled_at: now,
        }
    }

    /// Adds one allowance for each whole period since the last refill, up
    /// to [`ALLOWANCE_FUEL`].
    pub(crate) fn refill(&mut self, now: Instant) {
        let periods =
            now.saturating_duration_since(self.refilled_at).as_secs() / REFILL_PERIOD.as_secs();
        if periods == 0 {
            return;
        }
        let added = ALLOWANCE_FUEL.saturating_mul(i64::try_from(periods).unwrap_or(i64::MAX));
        self.balance = self.balance.saturating_add(added).min(ALLOWANCE_FUEL);
        self.refilled_at += REFILL_PERIOD * u32::try_from(periods).unwrap_or(u32::MAX);
    }

    pub(crate) fn charge(&mut self, fuel: u64) {
        self.balance = self
            .balance
            .saturating_sub(i64::try_from(fuel).unwrap_or(i64::MAX));
    }

    pub(crate) fn balance(&self) -> i64 {
        self.balance
    }

    /// Whether the writer's records may run now.
    pub(crate) fn in_credit(&self) -> bool {
        self.balance >= 0
    }

    /// When the next refill comes.
    pub(crate) fn next_refill(&self) -> Instant {
        self.refilled_at + REFILL_PERIOD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debt_takes_a_refill_per_allowance_to_clear() {
        let start = Instant::now();
        let mut allowance = Allowance::new(start);
        allowance.charge(5_000_000_000);
        assert_eq!(allowance.balance(), -3_000_000_000);
        assert!(!allowance.in_credit());

        allowance.refill(start + Duration::from_secs(59));
        assert_eq!(allowance.balance(), -3_000_000_000);
        allowance.refill(start + Duration::from_secs(60));
        assert_eq!(allowance.balance(), -1_000_000_000);
        allowance.refill(start + Duration::from_secs(150));
        assert_eq!(allowance.balance(), 1_000_000_000);
        assert!(allowance.in_credit());
        allowance.refill(start + Duration::from_secs(600));
        assert_eq!(allowance.balance(), ALLOWANCE_FUEL);
    }
}
