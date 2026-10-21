//! Noura's encrypted sync protocol, version 2: the binary encoding, wire
//! types, signing and verification that the desktop client, the sync server
//! and `account-web` share.
//!
//! `docs/workspace-format/sync-v2.md` is the specification; section numbers
//! in this crate's documentation refer to it. The crate holds no filesystem,
//! socket or database code, so it builds for WebAssembly, and every check that
//! server and client share is a pure function here.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod account;
pub mod blob;
pub mod chain;
pub mod codec;
pub mod crypto;
pub mod ids;
pub mod keys;
pub mod live;
pub mod manifest;
pub mod messages;
pub mod padding;
pub mod policy;
pub mod records;
pub mod tag;
pub mod types;

/// The protocol version every tag carries (§4).
pub const PROTOCOL_VERSION: u16 = 2;
