//! Pure update policy and state transitions.
//!
//! Runtime integration stays outside these modules so update eligibility,
//! manifest handling, and concurrency rules can be proved without network or
//! Tauri state.
#![allow(dead_code)] // B1 deliberately lands before the B2 runtime coordinator.

pub mod capability;
pub mod manifest;
pub mod state;
pub mod version;
