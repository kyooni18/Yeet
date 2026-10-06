//! Compatibility names for hosts migrating to [`crate::harness`].
//! Runtime implementation and transport ownership live in `Harness/`.

pub use crate::harness::{HarnessClient as Backend, ServiceEvent as BackendEvent, forward_cli};
