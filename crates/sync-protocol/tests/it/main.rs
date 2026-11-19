//! One test binary for the crate's integration tests. Each module was
//! a test binary of its own, so `--test it <module>::` selects its tests.

mod support;

mod account_chain;
mod blob;
mod chain;
mod decoders_never_panic;
mod decoding;
mod ids;
mod keys;
mod live_and_manifest;
mod padding;
mod policy_chain;
mod published_vectors;
mod spec_literals;
mod v2_fixtures;
