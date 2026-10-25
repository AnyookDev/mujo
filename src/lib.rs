//! mujo library crate.
//!
//! The TUI binary is a thin `main.rs` on top of these modules; exposing
//! them as a library lets the integration tests in `tests/` drive the same
//! code paths (config loading, search, pass/git wrappers) that the UI uses.

pub mod app;
pub mod config;
pub mod diagnose;
pub mod git;
pub mod handlers;
pub mod keys;
pub mod models;
pub mod pass;
pub mod password;
pub mod search;
pub mod ui;
pub mod utils;

#[cfg(test)]
mod testenv;
