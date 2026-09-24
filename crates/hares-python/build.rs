//! Build script: embed the Python shared-library directory as an rpath so
//! `cargo test -p hares-python` (and any dev binary) can load `libpython`
//! without `LD_LIBRARY_PATH` gymnastics. Without this, the test harness
//! aborts at startup with `error while loading shared libraries:
//! libpython3.XX.so.1.0` (observed 2026-09-11; the suite was silently
//! un-runnable in a bare shell — a broken window).
//!
//! Only applies to targets that link libpython at all (the `auto-initialize`
//! feature). Paths come from pyo3's build configuration, which resolves the
//! interpreter the same way the link step does.

fn main() {
    // pyo3-build-config exposes the resolved interpreter's lib dir. When it
    // is absent (e.g. cross-compiles with a static libpython), emit nothing.
    if let Some(lib_dir) = pyo3_build_config::get().lib_dir.as_deref() {
        // Re-run if the configured interpreter changes.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
        println!("cargo:rustc-link-search=native={lib_dir}");
    }
}
