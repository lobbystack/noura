//! Load numbers (sync-v2.md §22 and §15): each load of a note's document
//! takes the next load number from the device's reservation for the
//! workspace, so the document inserts under a client ID no other load used.
//!
//! The reservation lives in the credential store's high-water mark for the
//! workspace (§15), which the sync client keeps. The device raises it one
//! block of 1,024 at a time before it uses a number past it, and by one more
//! block at each start, so a crash between taking a number and writing the
//! journal never hands the same number out twice.

use std::sync::{Arc, Mutex};

use sync_protocol::ids::yrs_client_id;
use sync_protocol::records::MAX_LOAD;

use crate::note::ClientIds;

/// How far the device raises its reservation at a time.
pub const LOAD_BLOCK: u32 = 1_024;

/// Why the device can't take a load number.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReservationError {
    /// The credential store refused to read or write the mark.
    #[error("the credential store refused the load reservation: {0}")]
    Store(String),
    /// The device used every load number from 1 to 2^29 − 1 in this
    /// workspace.
    #[error("no load number is left in this workspace")]
    Exhausted,
}

/// Where the device keeps the highest load number it reserved for a
/// workspace: the credential store's high-water mark (§15).
pub trait ReservationStore: Send {
    /// The highest reserved load number, or 0 before the first.
    fn reserved(&self) -> Result<u32, ReservationError>;

    /// Records that every load number up to `through` is reserved, and
    /// returns once the mark is durable.
    fn reserve(&mut self, through: u32) -> Result<(), ReservationError>;
}

/// A reservation kept in memory, for tests and for a workspace that
/// doesn't sync.
#[derive(Clone, Debug, Default)]
pub struct MemoryReservation {
    mark: Arc<Mutex<u32>>,
}

impl MemoryReservation {
    /// The highest load number reserved so far.
    pub fn mark(&self) -> u32 {
        *self.mark.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

impl ReservationStore for MemoryReservation {
    fn reserved(&self) -> Result<u32, ReservationError> {
        Ok(self.mark())
    }

    fn reserve(&mut self, through: u32) -> Result<(), ReservationError> {
        *self.mark.lock().unwrap_or_else(|poison| poison.into_inner()) = through;
        Ok(())
    }
}

/// The load numbers this run of the app may hand out in one workspace.
#[derive(Debug)]
pub struct LoadReservation<S> {
    store: S,
    /// The next load number to hand out.
    next: u32,
    /// The highest load number the store holds as reserved.
    reserved: u32,
}

impl<S: ReservationStore> LoadReservation<S> {
    /// Starts a run: raises the mark by one block and hands out numbers from
    /// just above the old mark, skipping whatever the last run reserved.
    pub fn start(mut store: S) -> Result<Self, ReservationError> {
        let mark = store.reserved()?;
        if mark >= MAX_LOAD {
            return Err(ReservationError::Exhausted);
        }
        let reserved = mark.saturating_add(LOAD_BLOCK).min(MAX_LOAD);
        store.reserve(reserved)?;
        Ok(Self {
            store,
            next: mark + 1,
            reserved,
        })
    }

    /// The next load number, raising the mark by a block first when the
    /// reserved ones ran out.
    pub fn next_load(&mut self) -> Result<u32, ReservationError> {
        if self.next > MAX_LOAD {
            return Err(ReservationError::Exhausted);
        }
        if self.next > self.reserved {
            let raised = self.reserved.saturating_add(LOAD_BLOCK).min(MAX_LOAD);
            self.store.reserve(raised)?;
            self.reserved = raised;
        }
        let load = self.next;
        self.next += 1;
        Ok(load)
    }

    /// The store, for the caller that keeps the high-water mark.
    pub fn store(&self) -> &S {
        &self.store
    }
}

impl<S: ReservationStore + 'static> LoadReservation<S> {
    /// Client IDs for the loads of one note's document: `card_index × 2^29 +
    /// load` for each load number the shared reservation hands out (§22).
    /// It hands out none once the reservation fails or runs out, or for a
    /// card index outside 1 to 2^24 − 1.
    pub fn client_ids(reservation: Arc<Mutex<Self>>, card_index: u32) -> ClientIds {
        Box::new(move || {
            let load = reservation
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .next_load()
                .ok()?;
            yrs_client_id(card_index, load)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_start_skips_the_block_the_last_run_reserved() {
        let store = MemoryReservation::default();
        let mut first = LoadReservation::start(store.clone()).unwrap();
        assert_eq!(store.mark(), 1_024);
        assert_eq!(first.next_load(), Ok(1));
        assert_eq!(first.next_load(), Ok(2));
        drop(first);

        // The app quit or crashed after load 2; the next run starts above
        // every number the last one could have used.
        let mut second = LoadReservation::start(store.clone()).unwrap();
        assert_eq!(store.mark(), 2_048);
        assert_eq!(second.next_load(), Ok(1_025));
    }

    #[test]
    fn the_mark_rises_a_block_before_a_number_past_it() {
        let store = MemoryReservation::default();
        let mut reservation = LoadReservation::start(store.clone()).unwrap();
        for expected in 1..=1_024 {
            assert_eq!(reservation.next_load(), Ok(expected));
        }
        assert_eq!(store.mark(), 1_024);
        assert_eq!(reservation.next_load(), Ok(1_025));
        assert_eq!(store.mark(), 2_048);
    }

    #[test]
    fn the_last_load_number_is_2_pow_29_minus_1() {
        let mut store = MemoryReservation::default();
        store.reserve(MAX_LOAD - 2).unwrap();
        let mut reservation = LoadReservation::start(store.clone()).unwrap();
        assert_eq!(store.mark(), MAX_LOAD);
        assert_eq!(reservation.next_load(), Ok(MAX_LOAD - 1));
        assert_eq!(reservation.next_load(), Ok(MAX_LOAD));
        assert_eq!(reservation.next_load(), Err(ReservationError::Exhausted));
        drop(reservation);
        assert_eq!(
            LoadReservation::start(store).unwrap_err(),
            ReservationError::Exhausted
        );
    }

    #[test]
    fn client_ids_place_the_load_under_the_card_index() {
        let reservation = Arc::new(Mutex::new(
            LoadReservation::start(MemoryReservation::default()).unwrap(),
        ));
        let mut ids = LoadReservation::client_ids(reservation.clone(), 8);
        assert_eq!(ids(), Some(8 * (1 << 29) + 1));
        assert_eq!(ids(), Some(4_294_967_298));
        let mut widest = LoadReservation::client_ids(reservation, 16_777_215);
        assert_eq!(widest(), Some(16_777_215 * (1 << 29) + 3));
        assert_eq!(
            LoadReservation::client_ids(
                Arc::new(Mutex::new(
                    LoadReservation::start(MemoryReservation::default()).unwrap()
                )),
                0
            )(),
            None
        );
    }

    #[derive(Debug, Default)]
    struct Refusing;

    impl ReservationStore for Refusing {
        fn reserved(&self) -> Result<u32, ReservationError> {
            Ok(0)
        }
        fn reserve(&mut self, _: u32) -> Result<(), ReservationError> {
            Err(ReservationError::Store("locked".into()))
        }
    }

    #[test]
    fn no_load_number_goes_out_before_the_mark_is_durable() {
        assert_eq!(
            LoadReservation::start(Refusing).unwrap_err(),
            ReservationError::Store("locked".into())
        );
    }
}
