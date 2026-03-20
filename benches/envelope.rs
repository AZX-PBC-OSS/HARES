//! Envelope solver benchmark — see crates/hares-envelope/benches/rc_solver.rs
//! for the RC solver benchmarks (HARES-063).
//!
//! This file is intentionally minimal: the RC network and state-space benchmarks
//! live in the hares-envelope crate itself to avoid pulling the full core stack
//! into what is a pure linear-algebra micro-benchmark. Run them with:
//!
//!   cargo bench -p hares-envelope

fn main() {}
