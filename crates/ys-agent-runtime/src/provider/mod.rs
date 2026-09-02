//! Provider-management runtime boundary.
//!
//! This module reserves the approved application-layer seams without adding
//! provider behavior before the core contracts are available.

pub mod api;
pub mod catalog;
pub mod evidence;
pub mod evidence_collector;
pub mod evidence_gate;
pub mod resolver;
pub mod service;
pub mod validation;
