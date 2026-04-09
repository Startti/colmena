/// Global verbose flag — set once at startup via `set_verbose(true)` or by
/// setting the `COLMENA_VERBOSE=1` environment variable.
/// When false (default), all `colmena_log!` calls are no-ops.
use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Enable or disable verbose output at runtime (called from main/api on startup).
pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
}

/// Returns true if verbose output is currently enabled.
#[inline]
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}
