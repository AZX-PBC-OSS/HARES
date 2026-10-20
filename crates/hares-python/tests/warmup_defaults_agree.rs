//! The Python warm-up default's agreement with the Rust one is pinned in
//! `src/py_dwelling.rs`'s test module (`python_and_rust_warmup_defaults_agree`):
//! the dwelling types are crate-private, so an integration test cannot
//! reach the Python surface's own constructor. This file stays to keep the
//! integration-test target anchored to the row's pin.
