//! One test binary for the crate's integration tests. Each module was
//! a test binary of its own, so `--test it <module>::` selects its tests.

mod ai_live;
mod ai_provider;
mod ai_provider_failures;
mod ai_provider_thoughts;
mod chat_recovery;
mod create_file;
mod domain;
mod file_copy;
mod file_import;
mod file_version;
mod links;
mod local_asset_version;
mod online_only;
mod query_conformance;
mod raw_markdown_start;
mod recent_notes;
mod remote_image_proxy;
mod synced_files;
mod typed_objects;
