//! The enrollment proof (§7). `sync-key-envelope` builds it and exports the
//! verifier the server runs.

use super::Card;
use crate::codec::{Nested, Text};
use crate::types::Key;

crate::wire_struct! {
    /// `enroll`, signed with the card's signing key: proves that a signed-in
    /// device holds its key.
    pub struct Enroll: tag DeviceEnroll {
        /// The server's public origin in ASCII, such as
        /// `https://sync.noura.app`.
        pub origin: Text,
        /// The encoded card.
        pub card: Nested<Card>,
        /// The server's 32-byte challenge.
        pub challenge: Key,
    }
}
