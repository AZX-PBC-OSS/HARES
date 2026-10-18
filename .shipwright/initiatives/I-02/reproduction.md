---
reproduction_outcome: confirmed
failing_tests:
  - command: "cargo test -p hares-core --test envelope_opaque_loads"
    path: tests/envelope_opaque_loads.rs
    name: opaque_surface_indoor_heat_gain_is_net_conduction
---

## What the RCA established

Per `.shipwright/initiatives/I-02/rca.md`: `opaque_solar_lwr_kwh` (and its
per-interval source, the CSV column "Opaque Surface Heat Gain - Indoor (W)")
carries the **gross radiant flux absorbed at the exterior skin** of the opaque
envelope — injected as a boundary condition on each wall/roof's own exterior
RC node — while the struct-level documentation on `EnvelopeComponentLoadsKwh`
claims every component is a net "thermal load imposed on the conditioned
zone". The true net quantity (conduction from each opaque boundary's interior
surface into zone air) is already computed every timestep and exposed as the
"Wall/Floor/Roof Heat Gain - Indoor (W)" columns; it is simply never wired
into the envelope-loads reporting.

## The test

`tests/envelope_opaque_loads.rs` (registered as a `[[test]]` target in
`crates/hares-core/Cargo.toml`, following the existing
`tests/regression/smoke_test.rs` pattern: `SimulationEngine::run` on the
BEopt example fixture, Denver TMY3 weather, 24 h summer window at 1-minute
resolution, output verbosity 6, CSV parsed by column name).

The assertion targets the broken **rule**, not the reported path: any
quantity presented to consumers as the opaque envelope's heat gain *to the
indoor zone* must agree with the net conduction from the opaque boundaries
(wall + floor + roof interior surfaces) into that zone. The test compares the
run-mean of "Opaque Surface Heat Gain - Indoor (W)" against the run-mean of
the summed Wall+Floor+Roof net conduction columns, with a tolerance
(200 W / 50% of net) that absorbs boundary-categorization detail but is two
orders of magnitude tighter than the gross-vs-net gap. The same mistake made
in a sibling component (e.g. wiring any other exterior boundary flux into an
"Indoor" heat-gain output) would fail an assertion of this shape; the
per-boundary net columns make the expected value implementation-independent,
so the test asserts observable contract, not internals.

## Observed failure (run, not traced)

```
$ cargo test -p hares-core --test envelope_opaque_loads -- --nocapture
  reported 'Opaque Surface Heat Gain - Indoor' mean: 13581.0 W
  net opaque conduction (Wall+Floor+Roof Heat Gain - Indoor) mean: 104.6 W
test tests::opaque_surface_indoor_heat_gain_is_net_conduction ... FAILED
```

13,581 W reported as a zone load vs. 104.6 W actually delivered — a ~130×
overshoot on a single summer day, the same defect shape as the ticket's
+16.11 kW mean against a 1.75 kW home, reproduced on the repository's own
fixture without the ticket's unavailable python-bindings/HPXML stack. This
independently corroborates the magnitudes the RCA observed via the
`freefloat_oracle` `--nocapture` diagnostics (73.8 kW gross vs. tens of watts
net per boundary in one timestep).

## Gate semantics for the fix phase

This gate pins the physical invariant: the opaque term *reported as a zone
load* must be the net delivered quantity. It passes once the value wired into
"Opaque Surface Heat Gain - Indoor (W)" / `opaque_solar_lwr_kwh` is sourced
from the net per-boundary conduction (RCA fix option 2, or option 1 +
rewiring). If the fix phase instead chooses the documentation-only carve-out
(RCA fix option 1: keep the gross value but label it non-summable, like
`interior_lwr_kwh`), the honest resolution is to **rename/remove the gross
value from the "Indoor" heat-gain presentation** — in which case this test's
column lookup fails and the fix phase must update the test to assert against
the renamed, clearly-gross output (and assert the *absence* of any gross
quantity from the summable zone-load contract). Leaving this test red after a
docs-only relabel that keeps the gross value under an "Indoor Heat Gain"
name would mean the defect — a gross flux masquerading as a zone load — is
still live.
