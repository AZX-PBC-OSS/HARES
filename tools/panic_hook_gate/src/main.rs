//! The panic hook's feature-matrix witness.
//!
//! Built as its own workspace so the leaf compiles without the main
//! workspace's unified test features: the absence arm must read the leaf's
//! own feature, not another member's dev-dependency. Run once plain and once
//! with `--features profiling`; each run asserts the reading its feature
//! selects.
//!
//! The absence arm watches the default panic report on stderr. A child of
//! this binary holds a [`hares_types::panic_hook::PanicHookGuard`] and lets
//! a panic escape; without the feature the default hook prints the report,
//! and a guard that wrongly installs the custom hook suppresses it. Against
//! the ungated parent the plain run read the report suppressed: the red.

use std::env;
use std::panic::{self, AssertUnwindSafe};
use std::process::Command;

use hares_types::panic_hook::{self, PanicHookGuard};

const CHILD_ENV: &str = "PANIC_HOOK_GATE_CHILD";

fn main() {
    if env::var_os(CHILD_ENV).is_some() {
        // The guard is alive when the panic escapes: the process-wide hook
        // decides whether stderr carries the default report.
        let _guard = PanicHookGuard::new();
        panic!("gate witness panic");
    }

    let expected = cfg!(feature = "profiling");
    let child = Command::new(env::current_exe().expect("the witness binary's path"))
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn the witness child");
    let report = String::from_utf8_lossy(&child.stderr);
    let reported = report.contains("gate witness panic");

    // The caught-panic arm: with the feature the guard's hook enriches the
    // message with the panic site; without it the payload stays raw.
    let enriched = {
        let _guard = PanicHookGuard::new();
        #[cfg(feature = "profiling")]
        assert!(
            panic_hook::is_installed(),
            "with the profiling feature the guard must install the hook"
        );
        let payload = panic::catch_unwind(AssertUnwindSafe(|| panic!("gate witness panic")))
            .expect_err("the witness panic must unwind");
        panic_hook::panic_payload_to_string(payload).contains("main.rs:")
    };

    // Without the hook the default hook reports the panic; with it the custom
    // hook suppresses the report. The report and the enrichment therefore
    // move in opposite directions across the feature.
    eprintln!("the panic hook gate: reported={reported} enriched={enriched} profiling={expected}");
    assert_eq!(
        reported, !expected,
        "the default panic report must survive exactly when the hook is absent: {report}"
    );
    assert_eq!(
        enriched, expected,
        "the location enrichment must follow the leaf's profiling feature"
    );
}
