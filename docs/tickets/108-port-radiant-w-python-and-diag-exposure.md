# Expose `port_radiant_w` in Python `post_solvers` and Diagnostic CSV

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-python, hares-core/diagnostics

## Problem

`port_radiant_w` is missing from two observable surfaces:

1. Python `post_solvers` dict at `crates/hares-python/src/py_dwelling.rs:1939` — only `port_sensible_w` is exposed.
2. `EnvelopeDiag.port_radiant_w` field at `crates/hares-core/src/diagnostics.rs:40` — the diagnostic struct has no such field, and the diagnostic CSV omits the column.

Without these, downstream analysis cannot distinguish convective port contributions from radiant ones. The radiant-vs-convective split is precisely what determines surface MRT, surface-air heat exchange, and the BESTEST 900FF diagnostic comparison. Exposing only the convective component is a half-measure.

## Current Behavior

`crates/hares-python/src/py_dwelling.rs:1939`:
```rust
post_solvers.set_item("port_sensible_w", ...)?;
// no "port_radiant_w" entry
```

`crates/hares-core/src/diagnostics.rs:40`:
```rust
pub struct EnvelopeDiag {
    pub port_sensible_w: f64,
    // no port_radiant_w
    ...
}
```

CSV output omits `port_radiant_w`; Python users cannot read the value.

## Required Behavior

1. Add `port_radiant_w: f64` field to `EnvelopeDiag`.
2. Populate it in the diagnostic-emission path (whatever step writes `EnvelopeDiag` records each timestep).
3. Add the column to the diagnostic CSV header and row writer.
4. Add `post_solvers.set_item("port_radiant_w", ...)?;` in `py_dwelling.rs:1939`.
5. Update any Python tests / fixtures consuming `post_solvers` to assert the new key is present.

## Approach

1. Open `crates/hares-core/src/diagnostics.rs:40` and add the `port_radiant_w` field.
2. Identify the population site (likely in `Dwelling::run_timestep` or the diagnostic emission helper). Wire the radiant-port total there.
3. Open `crates/hares-python/src/py_dwelling.rs:1939` and add the `set_item` call.
4. Update CSV header/row writer to include the new column.
5. Add Rust unit test asserting `EnvelopeDiag` populates `port_radiant_w` correctly for a representative dwelling.
6. Add Python integration test asserting the dict has the new key with the expected value.

## Definition of Done

- [ ] `EnvelopeDiag.port_radiant_w` field added at `crates/hares-core/src/diagnostics.rs:40`
- [ ] Field populated in diagnostic emission path
- [ ] CSV header and row writer include `port_radiant_w` column
- [ ] Python `post_solvers["port_radiant_w"]` exposed
- [ ] Rust unit test asserts field population
- [ ] Python integration test asserts dict key and value
- [ ] No `port_radiant_w` callsite uses a missing-field workaround

## Verification

```bash
cargo test -p hares-core diagnostics
cargo test -p hares-python
uv run pytest python/tests/test_diagnostics.py
```

## References

- HARES `crates/hares-envelope/src/thermal_solver/ports.rs` — radiant port path.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" — convective and radiant components must be reported separately for analysis.

## Related Tickets

- 091-port-radiant-inputs-all-zones (radiant path correctness)
- 092-zone-sensible-breakdown-debug-includes-radiant (related debug breakdown)
- 110-port-sensible-vs-convective-naming (related naming clarity)
