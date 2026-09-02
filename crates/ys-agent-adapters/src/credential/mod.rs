//! Credential storage adapter boundary.
//!
//! Concrete native-store behavior belongs in `keyring`; this module owns no
//! production fallback or configuration discovery.

pub mod keyring;
