//! Credential storage adapters.
//!
//! Production credentials are encrypted in the workspace-local vault. The in-memory adapter is
//! an explicit test double and never selected by production composition.

pub mod local;
pub mod memory;
