---
id: BESTEST-001
title: Standardize layer ordering to exterior-to-interior (outside-in)
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/boundary_rc.rs
  - crates/hares-core/src/dwelling/conversions.rs
  - crates/hares-core/src/dwelling/synthetic.rs
  - tests/fixtures/bestest/600.toml
  - tests/fixtures/bestest/600ff.toml
  - tests/fixtures/bestest/640.toml
  - tests/fixtures/bestest/900.toml
  - tests/fixtures/bestest/900ff.toml
references:
  - /home/rich/.claude/plans/dapper-hatching-gosling.md
  - docs/invariants-and-observability.md
  - docs/equipment/envelope-construction.md
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core --test bestest -- --nocapture
  - cargo test -p hares-core --test parity -- --nocapture
  - cargo test --workspace --exclude hares-python --lib
---

## Background/Context

HARES uses interior-to-exterior layer ordering internally, the opposite of EnergyPlus/OCHRE/ASHRAE industry standard (outside-to-inside). The BESTEST TOML fixtures list layers interior-to-exterior, but `synthetic.rs:485` has a wrong comment claiming they're exterior-to-interior and applies `.iter().rev()`, flipping the layers backwards. This puts ground insulation adjacent to the zone air node instead of the concrete slab, causing `radiation_frac` of 0.009 instead of 0.77 for floors. This is the primary root cause of BESTEST free-float overheating (600FF crash at 81C, 900FF peak 57C vs 44.8C max) and conditioned-case load undercounts (600 heating -10.5%, cooling -25%).

All changes in this ticket are in initialization code, not the simulation hot loop. Steps must be applied atomically since individually they produce wrong results.

## Work to Do

### boundary_rc.rs — flip to exterior-to-interior convention

- [ ] Update doc comment on `BoundaryInput.material_layers` (line 127): "interior to exterior" -> "exterior to interior"
- [ ] In `build_layered_boundary` (lines 753-786): swap interior/exterior end connections
  - Interior connection: use `effective_layers[n_layers - 1]` and `layer_nodes[n_layers - 1]` with film_interior R
  - Exterior connection: use `effective_layers[0]` and `layer_nodes[0]` with film_exterior R
  - Return value: `(layer_nodes[n_layers - 1], layer_nodes[0])` — inner=zone-facing, outer=exterior-facing
  - Adjacent layer loop (764-774): no change needed
- [ ] Fix same-zone truncation (lines 726-731): keep interior half (last `keep` layers) instead of first; `halve_last_cap` applies at `i == 0` instead of `i == n_layers - 1`
- [ ] Fix `r_zone_to_inner` material-layer path (line 482): `valid_layers[0]` -> `valid_layers.last().unwrap()`
- [ ] Fix `r_zone_to_inner` precomputed path (line 404): `bd.precomputed_rc.first()` -> `bd.precomputed_rc.last()`
- [ ] Fix `inner_node` precomputed path (line 397): `NodeId(nodes_before)` -> `NodeId(nodes_before + n_nodes as u32 - 1)`
- [ ] Fix `inner_node` material-layer path (line 477): same change
- [ ] Flip `build_precomputed_boundary` wiring (lines 879-914):
  - `res_abs[last] += film_interior` (was `res_abs[0]`)
  - `res_abs[0] += film_exterior` (was `res_abs[last]`)
  - Wire: `exterior_node <-> layer[0]`, `layer[n-1] <-> interior_node`
  - Return `(layer_nodes[n-1], layer_nodes[0])`
- [ ] Fix precomputed same-zone truncation (lines 813-825): keep interior half (last layers) instead of first; halve `cap_list[0]` instead of `cap_list[new_nodes]`

### conversions.rs — remove LUT layer reversal

- [ ] Remove `layers.reverse()` at line 162 and update comment (LUT layers are already exterior-to-interior)

### synthetic.rs — remove .rev()

- [ ] Remove `.iter().rev()` at line 490 and delete the wrong comment about TOML ordering

### BESTEST TOML fixtures — reorder wall/roof layers to outside-in

- [ ] 600.toml: reverse 4 wall boundaries + 1 roof boundary layer order
- [ ] 600ff.toml: same
- [ ] 640.toml: same
- [ ] 900.toml: reverse 4 wall boundaries + 1 roof boundary layer order
- [ ] 900ff.toml: same
- [ ] Floor layers in all files: verify already exterior-to-interior (insulation, then slab) — no change

### Verify adjacent paths are correct (no code changes expected)

- [ ] HPXML path: verify `parse_material_layers` in `crates/hares-io/src/hpxml/building.rs:1420` emits layers in exterior-to-interior order from XML document order. After convention change this matches boundary_rc expectation directly. `conversions.rs` material_layers path (non-LUT) does NOT reverse — confirm this is correct.
- [ ] Envelope Materials CSV: confirm `defaults/envelope/Envelope Materials.csv` is already exterior-to-interior (identical to OCHRE's convention). No change needed.
- [ ] Auto-split interaction: `split_layer_count` in `build_layered_boundary` splits thick layers into sub-layers. After convention change, sub-layers maintain order within each original layer. Verify split layers are still correctly oriented (exterior sub-layers before interior sub-layers for the same original layer).

### Add radiation_frac assertion test

- [ ] Add a unit test in `boundary_rc.rs` (or `solver_builder.rs`) that builds a floor boundary with known BESTEST 900FF materials (1.007m insulation k=0.040 exterior, 0.080m concrete k=1.130 interior) and asserts `r_zone_to_inner` and derived `radiation_frac` are in the expected range (~0.82). This catches future layer ordering regressions at the unit level.

## Files to Touch

- `crates/hares-envelope/src/boundary_rc.rs`: Flip both RC build paths to expect layers[0]=exterior, update diagnostics, same-zone truncation
- `crates/hares-core/src/dwelling/conversions.rs`: Remove `.reverse()` for precomputed RC layers
- `crates/hares-core/src/dwelling/synthetic.rs`: Remove `.iter().rev()` and wrong comment
- `tests/fixtures/bestest/600.toml`: Reverse wall/roof layer arrays
- `tests/fixtures/bestest/600ff.toml`: Reverse wall/roof layer arrays
- `tests/fixtures/bestest/640.toml`: Reverse wall/roof layer arrays
- `tests/fixtures/bestest/900.toml`: Reverse wall/roof layer arrays
- `tests/fixtures/bestest/900ff.toml`: Reverse wall/roof layer arrays

## Measures of Success

- [ ] `radiation_frac` for 900FF floor is approximately 0.82 (was 0.013)
- [ ] 600FF no longer crashes at 80C invariant violation
- [ ] 900FF peak temperature drops from 57.2C toward acceptable band (<=44.8C)
- [ ] 600/640 heating and cooling loads move toward BESTEST acceptable bands
- [ ] Parity tests do not regress
- [ ] All envelope unit tests pass

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core --test bestest -- --nocapture` — record all case results
- [ ] `cargo test -p hares-core --test parity -- --nocapture` — no regression
- [ ] `cargo test --workspace --exclude hares-python --lib` passes
