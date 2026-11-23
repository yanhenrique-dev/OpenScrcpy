//! One running copy per user session.
//!
//! `single-instance` takes a per-user lock; a second launch must exit before it
//! opens a window or starts polling. Surfacing the existing window needs an
//! explicit activation channel and is not implemented yet.

/// Held for the process lifetime so the lock is not released early.
pub struct InstanceGuard {
    _inner: single_instance::SingleInstance,
}

/// Returns `Ok(None)` when another instance already owns `name`.
pub fn acquire(name: &str) -> Result<Option<InstanceGuard>, String> {
    let instance = single_instance::SingleInstance::new(name).map_err(|error| error.to_string())?;
    if instance.is_single() {
        Ok(Some(InstanceGuard { _inner: instance }))
    } else {
        Ok(None)
    }
}
