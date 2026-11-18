//! One test binary for the crate's integration tests. Each module was
//! a test binary of its own, so `--test it <module>::` selects its tests.

mod support;

mod ai;
mod approvals;
mod archives;
mod calendar_plugin;
mod capabilities;
mod clock_and_navigation;
mod command_calls;
mod dictation_plugin;
mod install;
mod interface_versions;
mod limits;
mod manifest;
mod network;
mod object_types;
mod outside_project;
mod projects_plugin;
mod random;
mod releases;
mod s7_trial;
mod s7_ui;
mod speech;
mod surfaces;
mod tasks_plugin;
mod transcription_plugin;
mod views;
mod wasmtime_link;
mod workspace_reads;
