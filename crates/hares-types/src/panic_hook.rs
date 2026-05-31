//! Custom panic hook that suppresses default backtrace output and stores
//! [`PanicHookInfo`] metadata in a thread-local cell for enrichment of error messages.
//!
//! In fleet simulations with thousands of dwellings, the default Rust panic hook
//! floods stderr with backtraces. This module replaces it with a silent hook that
//! captures payload, file, line, and column into a per-thread cell. The shared
//! [`panic_payload_to_string`] function retrieves this metadata to produce
//! location-enriched error strings.
//!
//! ## Usage
//!
//! ```ignore
//! use hares_types::panic_hook::PanicHookGuard;
//!
//! fn run_simulation() {
//!     let _guard = PanicHookGuard::new();
//!     // ... simulation code that may panic ...
//!     // guard restores the previous hook on Drop
//! }
//! ```

use std::cell::RefCell;
use std::panic::{self, PanicHookInfo};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "observe")]
use std::sync::atomic::AtomicU64;

/// Type alias for the boxed panic hook function, factoring out the complex
/// `Fn(&PanicHookInfo<'_>) + Send + Sync + 'static` bound.
type PanicHookFn = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

// ---------------------------------------------------------------------------
// thread-local panic info storage
// ---------------------------------------------------------------------------

thread_local! {
    /// Per-thread storage for the most recent panic's metadata.
    /// Populated by the custom hook before the panic unwinds;
    /// consumed by [`panic_payload_to_string`] after `catch_unwind`.
    static PANIC_INFO: RefCell<Option<PanicInfoCapture>> = const { RefCell::new(None) };
}

/// Owned snapshot of [`PanicHookInfo`] fields extractable inside a panic hook.
///
/// `PanicHookInfo` itself is not `Send`, so we copy out the fields we need
/// into this owned struct. All fields are `Send + Sync`.
#[derive(Debug, Clone)]
pub struct PanicInfoCapture {
    /// String-form payload (the panic message).
    pub payload: String,
    /// Source file where the panic originated, if available.
    pub file: Option<String>,
    /// Source line where the panic originated, if available.
    pub line: Option<u32>,
    /// Source column where the panic originated, if available.
    pub column: Option<u32>,
}

// ---------------------------------------------------------------------------
// installation tracking (invariant + observer support)
// ---------------------------------------------------------------------------

/// Set to `true` when our custom hook is the current process-wide hook.
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Counter of panics caught via `catch_unwind` in the current process.
#[cfg(feature = "observe")]
static PANIC_COUNT: AtomicU64 = AtomicU64::new(0);

/// Counter of panics where file/line metadata was successfully captured.
#[cfg(feature = "observe")]
static PANIC_WITH_LOCATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns `true` when the HARES custom panic hook is believed to be installed.
///
/// Used by `cfg(any(debug_assertions, feature = "check_invariants"))` assertions.
#[inline]
pub fn is_installed() -> bool {
    HOOK_INSTALLED.load(Ordering::Acquire)
}

/// Returns the total number of panics caught in this process.
#[cfg(feature = "observe")]
pub fn panic_counter() -> u64 {
    PANIC_COUNT.load(Ordering::Relaxed)
}

/// Returns the number of panics where file/line metadata was captured.
#[cfg(feature = "observe")]
pub fn panic_with_location_counter() -> u64 {
    PANIC_WITH_LOCATION_COUNT.load(Ordering::Relaxed)
}

/// Counter of secondary panics caught during post-`catch_unwind` error handling
/// (double-panic prevention events). An elevated value is an early-warning signal
/// of allocator corruption or systemic post-panic instability in error handlers.
#[cfg(feature = "observe")]
static DOUBLE_PANIC_PREVENTED_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the total number of double-panic prevention events in this process.
#[cfg(feature = "observe")]
pub fn double_panic_prevented_counter() -> u64 {
    DOUBLE_PANIC_PREVENTED_COUNT.load(Ordering::Relaxed)
}

/// Records a double-panic prevention event. In non-observe builds this is a no-op.
pub fn record_double_panic_prevented() {
    #[cfg(feature = "observe")]
    DOUBLE_PANIC_PREVENTED_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// hook lifecycle
// ---------------------------------------------------------------------------

/// Installs the HARES custom panic hook, replacing whatever hook is currently
/// registered. The previous hook is consumed into the hook chain and will NOT
/// be called by the replacement.
///
/// Prefer [`PanicHookGuard`] for RAII-style installation and restoration.
fn install() {
    HOOK_INSTALLED.store(true, Ordering::Release);
    panic::set_hook(Box::new(custom_hook));
}

/// Removes the current panic hook (ours) and restores it to the given `previous`
/// hook. If `previous` is `None`, the Rust default hook takes effect.
fn uninstall(previous: Option<PanicHookFn>) {
    HOOK_INSTALLED.store(false, Ordering::Release);
    let _removed = panic::take_hook(); // discard ours
    if let Some(prev) = previous {
        panic::set_hook(prev);
    }
    // If previous is None, no hook is re-registered — the default Rust hook
    // will be used for subsequent panics. This is the correct fallback.
}

/// The custom panic hook installed by this module.
///
/// Suppresses stderr output (no backtrace, no default hook invocation) and
/// stores the panic metadata in the per-thread [`PANIC_INFO`] cell.
fn custom_hook(info: &PanicHookInfo<'_>) {
    let payload_str = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        String::new()
    };

    let location = info.location();

    PANIC_INFO.with(|cell| {
        *cell.borrow_mut() = Some(PanicInfoCapture {
            payload: payload_str,
            file: location.map(|l| l.file().to_string()),
            line: location.map(|l| l.line()),
            column: location.map(|l| l.column()),
        });
    });
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// RAII guard that installs the HARES custom panic hook on construction
/// and restores the previous hook on [`Drop`].
///
/// Safe to use in any thread; the hook is process-wide but the stored
/// [`PanicHookInfo`] is per-thread via `thread_local!`.
///
/// ```ignore
/// let _guard = PanicHookGuard::new();
/// // panics here are silent and captured
/// // guard restores previous hook when it goes out of scope
/// ```
#[must_use = "PanicHookGuard is a resource guard — dropping it restores the previous hook"]
pub struct PanicHookGuard {
    previous: Option<PanicHookFn>,
}

impl PanicHookGuard {
    /// Saves the current panic hook (via [`panic::take_hook`]), installs the
    /// HARES custom hook, and returns a guard that restores the original on
    /// [`Drop`].
    pub fn new() -> Self {
        let previous = panic::take_hook(); // Save — could be default or custom
        install();
        Self {
            previous: Some(previous),
        }
    }
}

impl Default for PanicHookGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        uninstall(self.previous.take());
    }
}

/// Takes the stored [`PanicInfoCapture`] from the thread-local, clearing it
/// so subsequent panics are not shadowed by stale metadata.
pub fn take_panic_info() -> Option<PanicInfoCapture> {
    PANIC_INFO.with(|cell| cell.borrow_mut().take())
}

/// Converts a panic payload (from [`panic::catch_unwind`]) to a string,
/// enriched with file and line context from the thread-local [`PANIC_INFO`]
/// cell when available.
///
/// If the custom hook captured location metadata, the output has the form
/// `file.rs:42 — panic message`. Otherwise, the raw panic message or a
/// fallback string is returned.
pub fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    let base_msg = if let Some(msg) = payload.downcast_ref::<&'static str>() {
        (*msg).to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        "simulation panicked with non-string payload".to_string()
    };

    if let Some(capture) = take_panic_info() {
        // Increment observer counters when the observe feature is active.
        #[cfg(feature = "observe")]
        {
            PANIC_COUNT.fetch_add(1, Ordering::Relaxed);
            if capture.file.is_some() && capture.line.is_some() {
                PANIC_WITH_LOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        if let (Some(file), Some(line)) = (capture.file, capture.line) {
            return format!("{file}:{line} — {base_msg}");
        }
    }

    base_msg
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};

    // --- unit: hook capture and restore ---

    #[test]
    fn hook_captures_file_and_line_for_panic_macro() {
        let _guard = PanicHookGuard::new();

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            panic!("test message at known location");
        }));

        assert!(result.is_err());
        let msg = panic_payload_to_string(result.unwrap_err());
        // The payload from panic!("{msg}") is a String containing "{msg}".
        // The file/line are added by the hook metadata.
        assert!(
            msg.contains("panic_hook.rs"),
            "expected file path in: {msg}"
        );
        assert!(
            msg.contains("test message at known location"),
            "expected panic text in: {msg}"
        );
        // Should include line number
        assert!(
            msg.contains(" — "),
            "expected ' — ' separator between location and message in: {msg}"
        );
    }

    #[test]
    fn hook_captures_file_and_line_for_assert_failure() {
        let _guard = PanicHookGuard::new();

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            // Use a runtime bool condition to avoid clippy::assertions_on_constants
            // while still exercising the full assert! macro path (including
            // location metadata capture via PanicHookInfo).
            let condition = true;
            assert!(!condition, "assertion that must fail");
        }));

        assert!(result.is_err());
        let msg = panic_payload_to_string(result.unwrap_err());
        assert!(
            msg.contains("panic_hook.rs"),
            "expected file path in: {msg}"
        );
        assert!(
            msg.contains("assertion that must fail"),
            "expected assertion text in: {msg}"
        );
    }

    #[test]
    fn hook_is_restored_after_guard_drops() {
        // Record the state before we install
        let before = panic::take_hook();
        panic::set_hook(before); // put it back

        {
            let _guard = PanicHookGuard::new();
            assert!(is_installed());
        }
        // After guard drops, our hook should be uninstalled
        assert!(!is_installed());
    }

    #[test]
    fn guard_constructs_cleanly_in_child_thread() {
        // Run in a child thread to isolate from other test hooks.
        let outcome = std::thread::spawn(PanicHookGuard::new).join();
        assert!(
            outcome.is_ok(),
            "guard should construct cleanly in child thread"
        );
    }

    #[test]
    fn panic_payload_still_readable_when_hook_not_installed() {
        // Simulate a panic *without* the hook installed.
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            panic!("raw message without hook");
        }));

        assert!(result.is_err());
        let msg = panic_payload_to_string(result.unwrap_err());
        // Should fall back to the raw payload string.
        assert_eq!(msg, "raw message without hook");
    }

    #[test]
    fn non_string_payload_returns_fallback() {
        // Create a payload that isn't &str or String.
        struct NotAString;
        let payload: Box<dyn std::any::Any + Send> = Box::new(NotAString);
        let msg = panic_payload_to_string(payload);
        assert_eq!(msg, "simulation panicked with non-string payload");
    }

    #[test]
    fn owned_string_payload_is_handled() {
        let _guard = PanicHookGuard::new();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            panic!("{}", "owned string result");
        }));
        assert!(result.is_err());
        let msg = panic_payload_to_string(result.unwrap_err());
        assert!(
            msg.contains("owned string result"),
            "expected owned string in: {msg}"
        );
    }

    #[test]
    fn hook_works_across_multiple_panics_in_sequence() {
        let _guard = PanicHookGuard::new();

        // First panic
        let r1 = panic::catch_unwind(AssertUnwindSafe(|| panic!("first")));
        assert!(r1.is_err());
        let m1 = panic_payload_to_string(r1.unwrap_err());
        assert!(m1.contains("first"));

        // Second panic — should get fresh metadata
        let r2 = panic::catch_unwind(AssertUnwindSafe(|| panic!("second")));
        assert!(r2.is_err());
        let m2 = panic_payload_to_string(r2.unwrap_err());
        assert!(m2.contains("second"));
    }

    #[test]
    fn take_panic_info_clears_after_read() {
        let _guard = PanicHookGuard::new();

        // Trigger a panic
        let r = panic::catch_unwind(AssertUnwindSafe(|| panic!("data")));
        assert!(r.is_err());
        let _msg = panic_payload_to_string(r.unwrap_err());

        // After panic_payload_to_string consumes the info, it should be cleared.
        assert!(take_panic_info().is_none());
    }

    #[test]
    fn thread_local_isolation_each_thread_has_own_panic_info() {
        let t1 = std::thread::spawn(|| {
            let _guard = PanicHookGuard::new();
            let r = panic::catch_unwind(AssertUnwindSafe(|| panic!("t1 panic")));
            panic_payload_to_string(r.unwrap_err())
        });

        let t2 = std::thread::spawn(|| {
            let _guard = PanicHookGuard::new();
            let r = panic::catch_unwind(AssertUnwindSafe(|| panic!("t2 panic")));
            panic_payload_to_string(r.unwrap_err())
        });

        let m1 = t1.join().unwrap();
        let m2 = t2.join().unwrap();

        assert!(m1.contains("t1 panic"));
        assert!(m2.contains("t2 panic"));
    }

    #[test]
    fn hook_captures_file_and_line_for_unwrap_on_none() {
        let _guard = PanicHookGuard::new();

        let result = panic::catch_unwind(AssertUnwindSafe(
            #[allow(clippy::unnecessary_literal_unwrap)]
            // Why: clippy sees `None` → `unwrap()` in the same scope; the test
            // intentionally triggers `unwrap()` panic on `None` to verify the hook
            // captures PanicHookInfo::location() from the actual unwrap site.
            || {
                let x: Option<i32> = None;
                x.unwrap();
            },
        ));

        assert!(result.is_err());
        let msg = panic_payload_to_string(result.unwrap_err());
        assert!(
            msg.contains("panic_hook.rs"),
            "expected file path in: {msg}"
        );
        assert!(
            msg.contains("unwrap"),
            "expected unwrap reference in: {msg}"
        );
        assert!(msg.contains(" — "), "expected location separator in: {msg}");
    }

    #[test]
    fn hook_captures_file_and_line_for_expect_on_err() {
        let _guard = PanicHookGuard::new();

        let result = panic::catch_unwind(AssertUnwindSafe(
            #[allow(clippy::unnecessary_literal_unwrap)]
            // Why: clippy sees `Err` → `expect()` in the same scope; the test
            // intentionally triggers `expect()` panic on `Err` to verify the hook
            // captures PanicHookInfo::location() from the actual expect site.
            || {
                let r: Result<i32, &str> = Err("expect test error");
                r.expect("custom expect message");
            },
        ));

        assert!(result.is_err());
        let msg = panic_payload_to_string(result.unwrap_err());
        assert!(
            msg.contains("panic_hook.rs"),
            "expected file path in: {msg}"
        );
        assert!(
            msg.contains("custom expect message"),
            "expected custom expect text in: {msg}"
        );
        assert!(msg.contains(" — "), "expected location separator in: {msg}");
    }

    // --- unit: double-panic prevention ---

    #[test]
    fn record_double_panic_prevented_is_callable() {
        // record_double_panic_prevented must not panic (it is a no-op
        // in non-observe builds and increments a counter in observe builds).
        record_double_panic_prevented();
        record_double_panic_prevented();
        // No assertion needed: the test passes if it does not panic.
    }

    #[test]
    #[cfg(feature = "observe")]
    fn double_panic_prevented_counter_increments() {
        let before = double_panic_prevented_counter();
        record_double_panic_prevented();
        record_double_panic_prevented();
        assert_eq!(double_panic_prevented_counter(), before + 2);
    }

    #[test]
    fn nested_catch_unwind_absorbs_secondary_panic_and_returns_fallback() {
        // Simulate the double-panic prevention pattern: an outer catch_unwind
        // catches a simulation panic, and the error handler inside it uses
        // a nested catch_unwind to guard against the handler itself panicking.
        let fallback = "panic handling failed (double-panic prevented)";
        let outer = panic::catch_unwind(AssertUnwindSafe(|| {
            let sim_payload = panic::catch_unwind(AssertUnwindSafe(|| {
                panic!("simulated equipment panic");
            }));
            // Error handler: guard interior allocations with nested catch_unwind.
            let handler_result = panic::catch_unwind(AssertUnwindSafe(|| {
                let _payload = sim_payload.unwrap_err();
                // Simulate allocation-heavy error handling that itself panics
                // (e.g., allocator corruption causes String::clone to panic).
                panic!("simulated error handler panic");
            }));
            match handler_result {
                Ok(_) => unreachable!(),
                Err(_) => {
                    record_double_panic_prevented();
                    fallback.to_string()
                }
            }
        }));
        assert!(outer.is_ok());
        assert_eq!(outer.unwrap(), fallback);
    }
}
