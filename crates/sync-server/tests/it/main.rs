//! One test binary for the crate's integration tests. Each module was
//! a test binary of its own, so `--test it <module>::` selects its tests.

mod emulator;
mod support;

mod accounts;
mod api;
mod billing;
mod blob_store;
mod blobs;
mod chains;
mod checkpoints;
mod commands;
mod connections;
mod devices;
mod invitations;
mod logs;
mod notices;
mod plan_jobs;
mod postgres;
mod rate_limits;
mod realtime;
mod restore;
mod soak;
mod starts;
