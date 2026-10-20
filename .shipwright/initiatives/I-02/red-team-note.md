---
red_team_outcome: issues_found
failing_tests: []
---

# Red-team pass — I-02, run 3 (the P15 orientation-aware beam distribution)

`failing_tests` is empty because **no finding remains unresolved**: the one
defect this run found was witnessed red on two seams (outputs quoted
below), fixed concurrently mid-pass, and both of its tests now pass as
permanent pins with assertions intact — fresh runs quoted in the
Verification section. A `must_fail` entry here would be false: the gate
re-runs these commands and both exit 0.

Target: the interior beam-solar distribution rewritten to the cosine-weighted
parallel-flood model (`beam_cosine_factor`,
`compute_solar_distribution_into_generic`, tilt/azimuth on the interior
surface structs, the builder wiring). One finding landed and completed its
full lifecycle inside this pass (red → concurrent fix → same assertions
green); one green pin was added for a semantic no existing test actually
executed.

## Finding — the attic floor was sun-lit as a floor; it is the zone's ceiling (found red, fixed in-flight, now green)

The pure distribution function was correct and well-tested; the BUILDER fed
it a geometrically wrong tilt for the surface class that is largest in real
houses:

- `solver_builder.rs` tilt default: `Slab | Floor => 180.0` unconditionally
  by boundary type — but an attic floor's interior face points DOWN (it is
  the conditioned zone's ceiling), and the model's own documentation says
  "ceilings never receive direct beam".
- Live on the shipped fixtures: `BEopt_example.xml` and
  `tests/fixtures/hpxml/ochre_samples/base.xml` both contain **zero tilt
  elements** (every boundary takes the type default), and both carry attic
  floors (`base.xml` line 206-210: ExteriorAdjacentTo "attic - unvented",
  `FloorOrCeiling` = ceiling) — so the zone's largest horizontal interior
  surface entered the beam distribution at tilt 180, stealing the floor's
  share of transmitted window beam onto the ceiling mass node. The
  `FloorOrCeiling` element was parsed (`hares-io/src/hpxml/building.rs`)
  but consumed by nothing.

**Attack 1 (unit seam):**
`attic_floor_boundary_tilt_presents_ceiling_to_beam_distribution`
(`crates/hares-core/src/dwelling/solver_builder.rs` test module) — the
fixture's exact shape (Floor type, exterior Attic, tilt omitted,
FloorOrCeiling = ceiling). Watched red:

```
attic-floor boundary (Floor type, exterior Attic, FloorOrCeiling=ceiling)
must present tilt 0 (ceiling) to the beam distribution — its interior face
points down and can never receive direct beam — got tilt 180 (floor): the
conditioned zone's ceiling is sun-lit as a floor
```

**Attack 2 (assembled-solver seam):**
`indoor_zone_has_exactly_one_beam_receiving_floor_on_ochre_base`
(`crates/hares-core/tests/wiring_properties.rs`) — the ochre base fixture
end-to-end: the indoor zone's interior-solar surface list may contain at
most one beam-receiving floor. Watched red on the pre-fix tree
(`test result: FAILED. 2 passed; 1 failed`).

**Fix lifecycle (concurrent implementer, during this pass):** the fix routes
the interior-face tilt through the previously-unused `FloorOrCeiling` tag
(`Ceiling → 0.0`, type default otherwise; solver_builder.rs) — exactly the
signal my finding named. My unit assertion is untouched and now passes:

```
$ cargo test -p hares-core --lib dwelling::solver_builder::tests::attic_floor
test result: ok. 1 passed; 0 failed
```

The implementer also corrected my end-to-end test's fixture premise — and
the correction is TRUE (verified against `base.xml`, not taken on trust):
the ground slab (line 225-227) is the basement-conditioned zone's boundary,
and the living zone's only horizontal boundary is the attic floor, so the
living zone legitimately has ZERO tilt-180 surfaces — my original
"exactly one floor in the living zone" expectation was factually wrong for
this fixture. Their replacement is stronger, not weaker: living-zone
ceilings == 1 (the attic floor pinned AT tilt 0), floors <= 1, and a
whole-building exactly-one-tilt-180 assertion (the slab, in the Foundation
zone) — which was itself red against their incomplete first fix draft and
drove the fix to completion. Current state, all green:

```
$ cargo test -p hares-core --test wiring_properties
test result: ok. 3 passed; 0 failed
```

## Pin — the zero-weight fallback was never executed by its own test (GREEN)

The implementer's
`beam_no_lit_face_falls_back_isotropically_and_conserves` ("sun due north
at the horizon → nothing lit") never reaches the fallback branch: its
surfaces are south-facing walls, and a due-NORTH sun lights them (cosine
factor −cos0°·cos(180°) = +1) — the normalized branch runs; with equal
areas the two branches give identical splits, so the test cannot
distinguish them. Dumping the unlit-face beam to zone air as "reflected"
would pass that test and every assertion in the randomized conservation
suite — the behavioral difference (mass-mediated absorption vs instant
air gain) was unpinned.

Attack: `beam_parallel_to_all_walls_executes_fallback_and_lands_on_surfaces`
(`crates/hares-envelope/src/thermal_solver/solar.rs` test module) — a sun
parallel to every wall (cosine factor identically zero) with differing
area×absorptance weights forces the fallback branch; asserts the full beam
lands on surfaces with the isotropic 500/300 W split and reflected == 0.

```
$ cargo test -p hares-envelope --lib thermal_solver::solar::tests::beam_parallel_to_all_walls_executes_fallback_and_lands_on_surfaces
test result: ok. 1 passed; 0 failed
```

## Failure modes investigated and dismissed (with evidence)

- **Azimuth default 180° for orientation-less surfaces** — would silently
  make all walls identical to the beam model; killed as live risk: both
  shipped fixtures carry wall azimuths (16 `<Azimuth>` elements each in
  `BEopt_example.xml` and the ResStock `home.xml`). Noted, not tested.
- **Inter-story floors between two conditioned zones** (the generalization
  of the finding) — unrepresentable in the current data model: zones
  resolve by `ZoneType` (`conversions.rs::find_zone_idx`), so two
  vertically-stacked conditioned zones are indistinguishable and such a
  boundary collapses toward same-zone internal mass. The `FloorOrCeiling`
  fix covers the representable cases; if multi-story zone identity ever
  lands, the tilt rule needs revisiting — stated so the boundary of the
  fix is visible.
- **Sun altitude < 0 with nonzero beam** (malformed weather) —
  garbage-in; the max(0, ·) clamp keeps weights finite.
- **Window-position agnosticism of the parallel-flood model** — documented
  design (Prop-10; E+ FullInteriorAndExterior named as the later step).
- **Absorptance cancelling in the normalized beam ratio** — the documented
  "all solar is distributed" model, pinned by the zero/full-absorptance
  tests.
- **`is_floor` now vestigial** (trait dropped it; field set by the builder,
  read by nothing in the distribution) — housekeeping, not a defect.
- **`deposit_solar_to_*` with an out-of-range `input_index`** —
  pre-existing, unreachable in production wiring.

## Verification (final state, all run fresh at close of pass)

```
cargo test -p hares-core --lib            → 895 passed, 0 failed
cargo test -p hares-core --test wiring_properties
                                          → 4 passed, 0 failed
cargo test -p hares-core --test envelope_opaque_loads
                                          → 6 passed, 0 failed
cargo test -p hares-envelope --lib        → 374 passed, 0 failed
cargo test -p hares-io --lib              → 917 passed, 0 failed
cargo fmt --check                         → clean
cargo clippy -p hares-core --all-targets -- -D warnings
                                          → clean
```

The two finding tests above remain in the suite as permanent pins; both
were witnessed red (outputs quoted in their sections) and both now pass on
the landed fix. The tree moved throughout this pass (the concurrent
implementer's fix refactor transiently broke compilation several times
mid-fix); every result quoted is from a moment the relevant target
compiled. Not run: the full `cargo test -p hares-core` integration surface
(parity/resstock/bestest) and the python pytest suite — this pass added
tests only, no production code touched by me (the `FloorOrCeiling` tilt
routing and the end-to-end test's fixture-fact correction are the
implementer's, the latter verified against the fixture XML by me).

## Post-fix attack on the fix itself (held, then pinned)

The landed fix flips the single shared `SolverBoundary.tilt_deg` to 0 for
ceiling-tagged boundaries, with the implementer's safety claim that "these
zone-below boundaries never take the exterior path." Attacked by reading
all consumers: `is_exterior` is `exterior_zone == Outdoor` only
(solver_builder.rs:149-153), so an attic floor (exterior = Attic zone)
never enters `config.exterior_surfaces` — the sky-view/beta exterior LWR
geometry cannot read the flipped value. The remaining tilt consumers (the
beam distribution and the interior TARP convection injections) both want
the interior-face convention the fix now provides; pre-fix the attic
floor's interior convection ran with the floor tilt — the flip repairs
that consistency too. No new hole opened.

The safety premise then promoted from reading to a permanent pin
(reassessment round): `attic_floor_never_takes_the_exterior_sky_exposed_
path` (`crates/hares-core/tests/wiring_properties.rs`) — exactly one
Roof-category exterior surface (Roof1, 140.3 m²) and no exterior surface
at the attic floor's 125.4 m². If `is_exterior` ever widens to
zone-adjacent boundaries, the attic floor would silently exchange T⁴ LWR
with the sky temperature through the attic — the pin fails then, instead
of a comment eroding silently.

```
$ cargo test -p hares-core --test wiring_properties
test result: ok. 4 passed; 0 failed
```
