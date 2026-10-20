---
red_team_outcome: clean
failing_tests: []
---

# I-01 red-team, fourth pass: adversarial pass over the magnitude-bound fix

Outcome: **clean** — no adversarial gaps remain open, reported per the
designed "nothing to run" declaration (`failing_tests: []`). One gap was
found this pass and closed within it, with the operator's direction to
fix in-pass rather than route another round (see Resolution below); its
demonstrating test is green in the suite as the permanent regression pin
(`cargo test -p hares-equipment --lib
config::tests::validate_zip_sums_rejects_out_of_range_power_factor` →
1 passed) and re-fails if the guard regresses — a passing command is not
failing evidence, so it is not listed under `failing_tests`. The
remaining observation (`set_grid_voltage` input validation) is a
pre-existing runtime API outside this pass's changed surface, recorded
below for its owner. The record of the finding and its resolution
follows.

The third red-team pass left one red test (near-maximum cancellation row
NaNs at 1.05 pu). The fix run that followed (`v2-fix-c3ff46c1`) made it
green — re-verified on this checkout before attacking: `cargo test -p
hares-equipment --lib` → 2184 passed 0 failed, `cargo test -p hares-core
--lib` → 904 passed 0 failed, `uv run pytest
tests/python/test_equipment_zip_voltage_response.py -q` → 10 passed (all
nine prior red-team regression tests green). The fix replaced the
point probes with magnitude bounds — `validate_plausible_magnitudes`
(crates/hares-types/src/zip.rs:113-167): every polynomial coefficient
within ±100 (literature rows top out near 28.6), `v0` within [0.1, 10] —
wired into both the init funnel (`validate_zip_sums`,
crates/hares-equipment/src/config.rs:540) and the `"zip"` override
channel's earliest stage (`merge_zip_override`,
crates/hares-core/src/dwelling/conversions.rs:756-772, single source of
truth with the init backstop). Sound approach: bounds make the overflow
class unrepresentable instead of sampling it. This pass attacks that
guard.

## Finding — the plausibility guard omits `pf`: a finite power factor
outside [-1, 1] is fully validated and silently clamps at every use

`validate_plausible_magnitudes` bounds the six polynomial coefficients
(zp/ip/pp/zq/iq/pq) and `v0` — but not `pf`. Power factor is cos(φ) and
physically lies in [-1, 1] (the module's sign convention documents
negative = capacitive), yet a finite pf outside that range passes every
guard in the funnel:

- field finiteness (1.5 is finite);
- the magnitude bound — `pf` is not in its coefficient array;
- the sum checks (pf only gates the reactive-sum check);
- the nominal-voltage probe — `clamp(1.5, ±1)` lands on 1.0, and
  `tan(acos(1.0))` = 0, so the reactive multiplier probes as a finite 0.

The row is then **silently clamped at every use**: Q is zeroed (pf > 1)
with no signal, `equipment_zip` publishes the garbage pf, and a caller
applying the module's own documented Q formula
(`Q = P · tan(acos(pf)) · base`, power-factor.md) to the published pf
gets `acos(1.5)` = NaN while the engine computes 0 — the published model
and the engine's behavior diverge with nothing to show for it.

**Executed end-to-end on this checkout** (recon, 2026-09-11):
`Dwelling.from_hpxml(..., overrides={"Indoor Lighting": {"zip": {"pf":
1.5}}})` — the documented user channel — builds and initializes with no
signal, and `equipment_zip("Indoor Lighting")` publishes `pf: 1.5`; the
premise aggregate mixes it as if it were pf = 1.0. The pass-1 guard
rejected *non-finite* pf (NaN/±inf arms); finite-but-out-of-range pf is
the surviving sibling, and every DER path already validates pf in (0, 1]
(`BatteryConfig::validate`, EV/PV equivalents, all three
`PowerFactorSetpoint` arms) — the ZIP channel is the one seam without a
pf range check.

`validate_zip_sums_rejects_out_of_range_power_factor`
(crates/hares-equipment/src/config.rs) pins it at the funnel every real
config flows through: pf ∈ {1.5, 50.0, -1.2} must be rejected — was red
at discovery on the first arm:

```
pf = 1.5 is outside the physical [-1, 1] range and must fail validation,
not pass silently and clamp at every use
```

Control arms pin the legitimate domain against an over-broad fix:
pf ∈ {0.84, 1.0, -0.5, 0.0} — in-range, unity, the signed capacitive
convention, and the "no reactive" sentinel — must stay accepted.

## Resolution: fixed in this pass with the operator's direction

The operator asked whether this finding was small enough to fix in-pass
rather than spend another fix round. It was: a one-field omission from
the existing shared guard — ~8 lines, purely additive (no valid row
changes behavior), and the red test above already specified the full
contract. The red-team phase's test-only rule was set aside deliberately
for this finding (operator direction; proportionality), and the fix
landed where the guard already lives, `validate_plausible_magnitudes`
(crates/hares-types/src/zip.rs): a `|pf| <= 1` domain check between the
coefficient bounds and the v0 range, naming the field, the range, and
both conventions in the rejection. Both wired seams (the init funnel and
the `"zip"` override boundary) inherit it from the shared function —
single source of truth, unchanged.

Verification after the fix (this checkout, 2026-09-11):

- `cargo test -p hares-equipment --lib config::tests::
  validate_zip_sums_rejects_out_of_range_power_factor` → 1 passed
  (rejection arms and domain controls both green).
- Full suites: `cargo test -p hares-equipment --lib` → 2185 passed;
  `hares-types` → 394; `hares-core` → 904; `hares-io` → 918; `uv run
  pytest` (three files, against the rebuilt extension) → 194 passed;
  `cargo fmt --check --all` clean; `cargo clippy --workspace -- -D
  warnings` clean.
- End-to-end re-run of the discovery recon after `uv run maturin
  develop`: `Dwelling.from_hpxml(..., overrides={"Indoor Lighting":
  {"zip": {"pf": 1.5}}})` now **raises at build** — `HaresEquipmentError:
  equipment 'Indoor Lighting': "zip" override produced an implausible
  row: ZIP power factor pf = 1.5 is outside the physical [-1, 1] range
  (cos(phi); 0.0 is the no-reactive sentinel, negative values the
  capacitive sign convention)` — the rejection names the equipment, the
  field, and the domain, at the boundary where the typo was made. (The
  first post-fix recon still built cleanly because the installed
  extension predated the fix — the stale-`_hares.*.so` failure mode the
  coverage note documented in an earlier round; the rebuild restored the
  honest signal.)

The demonstrating test stays in the suite as the regression pin; the
finding is closed within this pass rather than routed to another fix
round.

## Hypotheses attacked and ruled out (no test forced)

- **Merge-boundary wiring**: the plausibility check is wired into
  `merge_zip_override` (the override channel's earliest stage) with the
  init funnel as backstop, sharing one source of truth. Complete; the pf
  gap above is the omission in the shared guard itself, not a wiring
  hole.
- **Legitimate rows rejected by the new bounds**: literature rows top
  out near 28.6 (Refrigerator `iq`), an order of magnitude below the ±100
  bound; negative coefficients within the bound are legitimate
  (literature has -28.62); `v0 ∈ [0.1, 10]` admits references an order
  of magnitude off base. The fix's own
  `magnitude_bounds_accept_every_literature_row` test plus a green suite
  confirm no collateral rejection.
- **The nominal probe's removal** (replaced by the bounds): the
  cancellation class it existed for is now unrepresentable — the pass-3
  test is green through the magnitude rejection, and the subnormal/tiny
  v0 rows are covered by the v0 range bound.

## Noticed, recorded, not pinned (with reasons)

- **pf = 1.0000001** (one-past-maximum of the physical domain): clamped
  to 1.0 before the fix, which is its own behavior — a harmless
  degenerate; the `[-1, 1]` range guard now rejects it as acceptable
  boundary tightness.
- **Premise-aggregate mixing of an out-of-range pf**: the aggregate
  weights the load's `tan_phi()` — clamped, so it mixed as unity —
  consistent with the clamped behavior; the root cause was the missing
  guard (closed by the in-pass fix), and the facet followed it. No
  separate test (same root cause, same fix).
- **`set_grid_voltage` accepts any f64 unvalidated** (pre-existing,
  untouched by this fix): flagged in the pass-3 note for the runtime-API
  owner; unchanged this pass.

## Verification (this checkout, 2026-09-11)

At discovery — the finding, red:

- `cargo test -p hares-equipment --lib` → `2184 passed; 1 failed`
  (`validate_zip_sums_rejects_out_of_range_power_factor`, failing on the
  `pf = 1.5` arm with the output quoted above).

After the in-pass fix — everything green:

- `cargo test -p hares-equipment --lib` → 2185 passed; `hares-types` →
  394; `hares-core` → 904; `hares-io` → 918.
- `uv run pytest tests/python/test_equipment_zip_voltage_response.py
  tests/python/test_reactive_power.py tests/python/test_py_control.py -q`
  → `194 passed` (all ten red-team regression tests green, including this
  pass's).
- `cargo fmt --check --all` clean; `cargo clippy --workspace -- -D
  warnings` clean.

## Scope discipline

Production code touched exactly once, for this pass's finding, with the
operator's explicit direction to fix in-pass rather than route another
round: the pf domain check in `validate_plausible_magnitudes`
(crates/hares-types/src/zip.rs), described under Resolution above. Every
other edit is test code — this pass's test function inside the `mod
tests` block of config.rs — plus this note. The tree-wide `cargo fmt
--all` run was audited: a copy of config.rs with this pass's test
function stripped is `rustfmt --check` clean, proving the received
fix-phase tree was already fmt-clean and the formatter could only have
reformatted the red-team's own test code. The end-to-end reachability
recon (pf = 1.5 through real `from_hpxml`) lives under `/tmp/opencode/`
outside the repo; its findings are pinned by the in-suite test above.
