//! The panic hook's feature-matrix witness.
//!
//! Built as its own workspace so the leaf compiles without the main
//! workspace's unified test features: the absence arm must read the leaf's
//! own feature, not another member's dev-dependency. Run once plain and once
//! with `--features profiling`; each run asserts the reading its feature
//! selects. Against a leaf whose guard installs ungated, the plain run reads
//! the hook installed and the location enriched, and fails: the red.

use std::panic::{self, AssertUnwindSafe};

use hares_types::panic_hook::{self, PanicHookGuard};

fn main() {
    let _guard = PanicHookGuard::new();
    let installed = panic_hook::is_installed();

    let payload = panic::catch_unwind(AssertUnwindSafe(|| panic!("gate witness panic")))
        .expect_err("the witness panic must unwind");
    let message = panic_hook::panic_payload_to_string(payload);
    // The hook's enriched form prefixes the message with `file:line`; the
    // panic site is this file, so the location marker is `main.rs:`.
    let enriched = message.contains("main.rs:");

    let expected = cfg!(feature = "profiling");
    // Stderr is unbuffered and the leaf's hook, when wrongly installed,
    // silences the panic path: print the readings before any assertion so a
    // failing arm still reports what it saw.
    eprintln!(
        "the panic hook gate: installed={installed} enriched={enriched} profiling={expected}"
    );
    assert_eq!(
        installed, expected,
        "the guard's install must follow the leaf's profiling feature: {message}"
    );
    assert_eq!(
        enriched, expected,
        "the location enrichment must follow the leaf's profiling feature: {message}"
    );
    assert!(
        message.contains("gate witness panic"),
        "the panic's payload must reach the message in both arms: {message}"
    );
}
