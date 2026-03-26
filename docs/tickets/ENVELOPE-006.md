---
id: ENVELOPE-006
title: Per-node RC network dump for deep debugging
kind: implement
depends_on: [ENVELOPE-001]
files_to_touch:
  - crates/hares-envelope/src/boundary_rc.rs
  - crates/hares-core/src/dwelling/solver_builder.rs
references:
  - tests/python/extract_ochre_rc.py
  - vendors/OCHRE/ochre/Models/RCModel.py
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core
  - cargo clippy --all-targets -- -D warnings
---

## Background/Context

The OCHRE extraction script (`extract_ochre_rc.py`) dumps the complete RC network:
- Per-node capacitances (J/K)
- Per-edge resistances (K/W) with node pair labels
- State-space matrix A_c diagonal (time constants)
- A_c off-diagonal coupling terms
- B_c input sensitivities

HARES computes all of this internally (`BuildingRC`, `RCNetwork`) but discards it
after matrix construction. To diagnose the remaining UA discrepancies, we need to
dump the full RC network at the same granularity as OCHRE.

Note: `BoundaryDiagnostic` in `boundary_rc.rs` already stores per-boundary aggregate
data (UA, R_total, capacitance, node count). This ticket adds the per-NODE level:
individual layer capacitances and inter-node resistances. The two are complementary,
not duplicative.

## Work to Do

- [ ] Add `RCNetworkDump` struct (in `boundary_rc.rs` or a new `rc_dump.rs`):
  ```rust
  pub struct RCNetworkDump {
      pub node_capacitances: Vec<(String, f64)>,           // (label, J/K)
      pub edge_resistances: Vec<(String, String, f64)>,    // (node_a, node_b, K/W)
      pub a_c_diagonal: Vec<(String, f64)>,                // (node, 1/s)
      pub a_c_couplings: Vec<(String, String, f64)>,       // (from, to, 1/s)
  }
  ```
- [ ] Generate human-readable node labels from boundary metadata:
  - Labels like `"zone_1"`, `"ExtWall_layer_0"`, `"outdoor"`, `"ground"`
  - Requires passing boundary names/categories through `assemble_building_rc()`.
    Currently `BoundaryInput` has no `name` field — either add one or generate labels
    from category + index (e.g., `"Wall_0_layer_1"`)
- [ ] Store the raw RC network data (capacitances + resistances) in `BuildingRC` before
  matrix construction, or reconstruct from the A_c/B_c matrices after construction
- [ ] Add `Serialize` derives for JSON export
- [ ] Surface via `Dwelling::rc_network_dump() -> &RCNetworkDump`
- [ ] Add a test that dumps the network for BEopt_example and compares key values against
  the OCHRE extraction (`extract_ochre_rc.py` output or hardcoded expected values)

## Files to Touch

- `crates/hares-envelope/src/boundary_rc.rs`: Store raw RC network, generate labels, add `RCNetworkDump`
- `crates/hares-core/src/dwelling/solver_builder.rs`: Pass boundary names/categories, forward dump to `Dwelling`

## Measures of Success

- [ ] `dwelling.rc_network_dump()` returns all node/edge data with human-readable labels
- [ ] JSON output can be diffed against OCHRE extraction to pinpoint exact disagreements
- [ ] Node count matches OCHRE (4 for ext wall, 3 for attic wall, etc.)
- [ ] Resistance and capacitance values can be traced to specific material layers

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
