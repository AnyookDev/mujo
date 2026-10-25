//! Integration tests for mujo.
//!
//! These exercise complete user workflows against real temporary
//! directories and the actual `pass`/`git` binaries: a throwaway GPG key is
//! generated, a real password store is initialized, and filesystem/git
//! state is verified after each step.
//!
//! Run with: cargo test --test integration
//!
//! Tests that need `pass`/`gpg` skip gracefully (with a printed reason)
//! when those binaries are not available in the environment.

mod common;
mod config_reload;
mod git_workflow;
mod password_lifecycle;
mod search_navigation;
