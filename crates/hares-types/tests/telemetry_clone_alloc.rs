//! The snapshot types' `clone_from` paths allocate nothing when the
//! destination already holds the source's shape, and always copy the
//! latches. This is a separate test binary because `#[global_allocator]`
//! applies to the whole binary: the counts must come from the counting
//! allocator, not from an uninstrumented process.

use std::hint::black_box;

use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use hares_types::{DomainId, DomainUpdate, Telemetry, ZoneId};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn telemetry_with_keys(keys: &[&str], offset: f64) -> Telemetry {
    keys.iter()
        .enumerate()
        .map(|(i, key)| (key.to_string(), offset + i as f64))
        .collect()
}

/// Latches the non-finite write the entry's test names: a rejected `insert`
/// leaves the value out of the map and the `(key, value)` in the latch.
fn telemetry_latched_non_finite() -> Telemetry {
    let mut t = telemetry_with_keys(&["power_kw"], 1.0);
    t.insert("power_kw", f64::NAN);
    t
}

/// Latches the unknown key: the first `set` of an unregistered key.
fn telemetry_latched_unknown_key() -> Telemetry {
    let mut t = telemetry_with_keys(&["power_kw"], 1.0);
    t.set("unregistered", 2.0);
    t
}

fn assert_clone_from_allocates_zero(dst: &mut Telemetry, src: &Telemetry) {
    black_box(&dst);
    let before = thread_allocations().expect("the test installs the counting allocator");
    dst.clone_from(src);
    let after = thread_allocations().expect("the test installs the counting allocator");
    assert_eq!(
        after - before,
        0,
        "clone_from between matched-shape maps must not allocate"
    );
}

fn assert_clone_from_copies_latches(dst: &mut Telemetry, src: &Telemetry) {
    dst.clone_from(src);
    // The map's values are never non-finite and compare directly.
    assert_eq!(dst.0, src.0, "clone_from must copy every value");
    // The non-finite latch carries NaN, which is never equal to itself:
    // compare the key and the bit pattern.
    let latch = |t: &Telemetry| {
        t.non_finite_latch()
            .map(|(key, value)| (key.clone(), value.to_bits()))
    };
    assert_eq!(
        latch(dst),
        latch(src),
        "clone_from must copy the non-finite latch"
    );
    assert_eq!(
        dst.unknown_key_latch(),
        src.unknown_key_latch(),
        "clone_from must copy the unknown-key latch"
    );
}

#[test]
fn telemetry_clone_from_same_keys_allocates_nothing() {
    let keys = ["power_kw", "soc", "reactive_power_kvar"];

    // Same keys, no latch: the in-place path overwrites the f64 slots and
    // allocates nothing. The values equal the source afterwards.
    let mut dst = telemetry_with_keys(&keys, 0.0);
    let src = telemetry_with_keys(&keys, 10.0);
    assert_clone_from_allocates_zero(&mut dst, &src);
    assert_eq!(dst, src, "same-key clone_from must copy every value");
    assert_eq!(dst.non_finite_latch(), None);
    assert_eq!(dst.unknown_key_latch(), None);

    // Both latches clean on both sides is the steady state above; the
    // same assertion with the latches explicitly set on neither side but
    // the destination holding a stale latch: a clean source clears both
    // latches of a latched destination.
    let mut latched_dst = telemetry_with_keys(&keys, 0.0);
    latched_dst.insert("power_kw", f64::NAN);
    latched_dst.set("unregistered", 1.0);
    assert!(latched_dst.non_finite_latch().is_some());
    assert!(latched_dst.unknown_key_latch().is_some());
    let clean_src = telemetry_with_keys(&keys, 10.0);
    assert_clone_from_allocates_zero(&mut latched_dst, &clean_src);
    assert_eq!(latched_dst, clean_src);
    assert_eq!(
        latched_dst.non_finite_latch(),
        None,
        "a clean source clears the non-finite latch"
    );
    assert_eq!(
        latched_dst.unknown_key_latch(),
        None,
        "a clean source clears the unknown-key latch"
    );

    // Different key sets: the fallback path. The result equals the source.
    let mut dst = telemetry_with_keys(&["power_kw", "extra"], 0.0);
    let src = telemetry_with_keys(&["power_kw", "soc"], 5.0);
    dst.clone_from(&src);
    assert_eq!(
        dst, src,
        "a different key set falls back and still equals the source"
    );
}

#[test]
fn telemetry_clone_from_copies_a_latched_source_on_both_paths() {
    // The in-place path (same keys): a latched source yields a latched copy.
    let mut dst = telemetry_with_keys(&["power_kw"], 0.0);
    let src = telemetry_latched_non_finite();
    assert_clone_from_copies_latches(&mut dst, &src);

    // The fallback path (different keys): same latch rule.
    let mut dst = telemetry_with_keys(&["other_key"], 0.0);
    let src = telemetry_latched_non_finite();
    assert_clone_from_copies_latches(&mut dst, &src);

    // The unknown-key latch, in place (same keys).
    let mut dst = telemetry_with_keys(&["power_kw"], 0.0);
    let src = telemetry_latched_unknown_key();
    assert_clone_from_copies_latches(&mut dst, &src);

    // The unknown-key latch, fallback path (different keys).
    let mut dst = telemetry_with_keys(&["other_key"], 0.0);
    let src = telemetry_latched_unknown_key();
    assert_clone_from_copies_latches(&mut dst, &src);
}

#[test]
fn domain_update_clone_from_reuses_allocations() {
    // Capacities suffice (same shapes): both vectors are reused in place
    // and nothing allocates.
    let mut dst = DomainUpdate {
        domain_id: DomainId(7),
        zone_temperatures_c: vec![(ZoneId(1), 20.0), (ZoneId(2), 21.0)],
        custom_payload: Some(vec![0.0; 5]),
    };
    let src = DomainUpdate {
        domain_id: DomainId(7),
        zone_temperatures_c: vec![(ZoneId(1), 25.0), (ZoneId(2), 26.0)],
        custom_payload: Some(vec![1.0; 5]),
    };
    black_box(&dst);
    let before = thread_allocations().expect("the test installs the counting allocator");
    dst.clone_from(&src);
    let after = thread_allocations().expect("the test installs the counting allocator");
    assert_eq!(
        after - before,
        0,
        "clone_from with sufficient capacities must not allocate"
    );
    assert_eq!(dst, src);

    // A shorter destination grows through the fallback and still equals
    // the source.
    let mut short = DomainUpdate {
        domain_id: DomainId(7),
        zone_temperatures_c: vec![(ZoneId(1), 20.0)],
        custom_payload: None,
    };
    short.clone_from(&src);
    assert_eq!(short, src);
}
