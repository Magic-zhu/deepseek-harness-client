//! dsh-profile — filesystem and CLI seams of a dsh profile.
//!
//! Everything here is plain logic over paths/text/processes with zero Tauri
//! imports, so it is unit-testable end to end.

pub mod home;
pub mod patch;
