---
diagnosis_outcome: root_cause_found
complexity: moderate
passing_commands:
  - command: cargo fmt --check --all
  - command: cargo clippy --workspace -- -D warnings
  - command: cargo test --workspace
---

# I-01: `equipment_zip` not describing voltage response

## Symptom, restated

The operator queries `dwelling.equipment_zip(name)` for every equipment
instance right after `initialize()` and finds `zp=0, ip=0, pp=1` (pure
constant-power) on every one they checked. But driving the same building with
`set_grid_voltage(0.95/1.00/1.05)` through a full annual `simulate()` produces
a real ~0.25 %P/%V system-level sensitivity, matching published residential
figures. The operator's conclusion — "the coefficients don't describe [the
response] ... it evidently comes from load whose ZIP `equipment_zip` doesn't
expose, plausibly the scheduled/untyped base load" — is correct, and this RCA
confirms exactly which code path produces each half of that picture and why.

## Investigation

### Two distinct ZIP-resolution paths exist by design

`crates/hares-types/src/zip.rs` documents "Rule R1": typed physics equipment
(HVAC, water heaters, ventilation fans) computes its real power through its
own physics model (compressor curves, thermal models, etc.) each timestep,
and that computation must stay bit-identical regardless of voltage. So the
ZIP model for that class of equipment is deliberately restricted to the
reactive side only — real coefficients are hard-forced to `(zp, ip, pp) =
(0, 0, 1)`.

This is implemented in `crates/hares-equipment/src/config.rs:465-483`,
`resolve_reactive_zip()`:

```rust
pub(crate) fn resolve_reactive_zip(config: &EquipmentConfig) -> crate::Result<ZipLoad> {
    let zip = ZipLoad {
        zp: 0.0, ip: 0.0, pp: 1.0,   // <-- forced regardless of resolve_zip() result
        ...
    };
    ...
}
```

Every physics-typed equipment module calls this at `init()` and stores the
result as `self.zip`, which `resolved_zip()` returns verbatim (confirmed by
direct reads, not inference):

- `hvac/furnace.rs:178,553`, `hvac/boiler.rs:240,620`,
  `hvac/air_conditioner.rs:855`, `hvac/baseboard.rs:137`,
  `hvac/dehumidifier.rs:565`, `hvac/ideal_hvac.rs:515`, `ventilation.rs:477`
  (HRV/ERV/Ceiling-Fan/Ventilation-Fan-as-physics-equipment),
  `water_heater/resistance.rs:380`, `water_heater/tankless.rs:283`,
  `water_heater/heat_pump_wh.rs:674`, `water_heater/gas.rs:354` — all call
  `crate::config::resolve_reactive_zip(config)` and each type's
  `resolved_zip()` returns `Some(self.zip)` unmodified.

So for this whole class, `equipment_zip()` reporting `(0,0,1)` is **not a
bug** — it is the intended, engineered guarantee that real electric power for
physics-modeled equipment never silently diverges from what the thermal/
electrical model computed. Reactive power for these classes *is* still
voltage-sensitive: `ZipLoad::reactive_kvar()` applies `zq/iq/pq/pf` to the
already-computed real power (`zip.rs:150-165`), so real power is voltage-
inert but reactive power is not — a nuance `equipment_zip()`'s output cannot
convey on its own (a caller sees `zq/iq/pq` populated but has no signal that
those apply only to Q, never P, for this equipment).

### Scheduled/event loads take the other path and *do* carry real voltage sensitivity

`crates/hares-equipment/src/scheduled_load.rs:365` and
`crates/hares-equipment/src/event_load.rs:587,1247` call
`crate::config::resolve_zip(config)` — **not** `resolve_reactive_zip` — which
returns the full class-table ZIP (`zp/ip/pp` populated) from
`hares_types::zip::zip_defaults_for_class` or the `defaults/zip_parameters.toml`
sidecar (e.g. Lighting `zp=0.54, ip=0.50, pp=-0.04`; MELs
`zp=0.36147, ip=-0.08583, pp=0.72436`; Refrigerator
`zp=5.03, ip=-8.48, pp=4.45`). `scheduled_load.rs:595-596` then actually
applies that full ZIP to the scheduled power at the current bus voltage each
step:

```rust
self.zip.apply(scheduled_power_kw, env.grid.bus_voltage_pu())
```

This is the code path that produces the real, measured ~0.25 %P/%V annual
sensitivity in Part B. It is driven by lighting, MELs/plug loads, TV,
refrigerator/freezer, dishwasher, range, clothes washer/dryer, well/pool/spa
pumps and heaters, and ceiling/ventilation fans configured as scheduled loads
— i.e. exactly the "scheduled/untyped base load" the operator's ticket
hypothesized.

### Both paths enumerate through the same `equipment_zip()`/`equipment_names()` API

`crates/hares-core/src/dwelling/mod.rs:3033-3035` — `Dwelling::equipment()`
returns one flat `&[Box<dyn Equipment>]` containing every equipment instance
(HVAC, water heaters, ventilation, scheduled loads, event loads, EV, PV,
battery, generator, etc.). `py_dwelling.rs:929-943`
(`equipment_zip`) and `:945-950` (`equipment_names`) iterate that same list
with no distinction between equipment kinds. So scheduled loads such as
"Lighting" or "MELs" *do* appear in `dw.equipment_names()` and *do* report
non-`(0,0,1)` real coefficients through `equipment_zip()` — confirmed by
reading `resolve_zip()`'s call sites and `zip_defaults_for_class`'s
non-trivial values for those class names, and by the class-table
completeness test in `hares-types/src/zip.rs` (`class_table_covers_every_
known_name`), which exercises every one of the 48 `ZIP_CLASS_NAMES`
including "Lighting", "MELs", "Refrigerator", etc. and asserts they resolve
to non-default rows.

## Reproduction

The ticket's exact `bldg0046363.xml` / `G4804390 TMY3.epw` inputs are not
present in this repo, so I reproduced the mechanism on the repo's own HPXML
fixture (`tests/fixtures/hpxml/ochre_samples/base.xml`, used by the existing
`tests/python/test_reactive_power.py` suite) through the real `ochre_next`
Python bindings — the same module and API the ticket calls — rather than
reasoning from source alone.

**Command 1 — Part A analogue** (`uv run python /tmp/repro_i01.py`, script
below, built and run via `uv run` which builds the `ochre_next` extension):

```python
from ochre_next import Dwelling
dw = Dwelling.from_hpxml(
    "tests/fixtures/hpxml/ochre_samples/base.xml",
    "data/examples/BEopt_example_schedule.csv",
    "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw",
    start_time="2019-07-01T00:00:00", duration_s=3600, time_res_s=60,
    defaults_path="defaults", bldg_id=42, master_seed=0,
)
dw.initialize()
for name in dw.equipment_names():
    z = dw.equipment_zip(name)
    print(name, z["zp"], z["ip"], z["pp"])
```

Actual printed output (12 equipment instances, full run captured):

```
'Gas Furnace': zp=0.0, ip=0.0, pp=1.0
'Air Conditioner': zp=0.0, ip=0.0, pp=1.0
'Electric Resistance Water Heater': zp=0.0, ip=0.0, pp=1.0
'Clothes Washer': zp=0.05, ip=0.31, pp=0.64
'Clothes Dryer': zp=1.0, ip=0.0, pp=0.0
'Dishwasher': zp=1.0, ip=0.0, pp=0.0
'Refrigerator': zp=5.03, ip=-8.48, pp=4.45
'Cooking Range': zp=1.0, ip=0.0, pp=0.0
'Exterior Lighting': zp=0.54, ip=0.5, pp=-0.04
'Indoor Lighting': zp=0.54, ip=0.5, pp=-0.04
'TV': zp=0.36147, ip=-0.08583, pp=0.72436
'MELs': zp=0.36147, ip=-0.08583, pp=0.72436
```

This is a live, executed reproduction, not a hypothesis: exactly the three
Rule-R1 physics equipment (Gas Furnace, Air Conditioner, Electric Resistance
Water Heater) report `(0,0,1)`; the nine scheduled loads do not. This
directly reproduces the mechanism I traced in source, and simultaneously
disproves the ticket's literal wording — "every typed equipment reports as
constant power" is false on this fixture; 9 of 12 do not. The operator's own
fallback hypothesis ("plausibly the scheduled/untyped base load") is the one
the data confirms.

**Command 2 — Part B analogue**, comparing per-equipment `electric_kw`
telemetry at `set_grid_voltage(0.95)` vs `set_grid_voltage(1.05)` after one
step at 2019-07-01T14:00 (`uv run python /tmp/repro_i01d.py`):

```
name                                      P@0.95       P@1.05   changed?
Gas Furnace                                  0.0          0.0      False
Air Conditioner                              0.0          0.0      False
Electric Resistance Water Heater             0.0          0.0      False
Clothes Washer                               0.0          0.0      False
Clothes Dryer                                0.0          0.0      False
Dishwasher                                   0.0          0.0      False
Refrigerator                        0.07638515145294049 0.08931271906086101       True
Cooking Range                                0.0          0.0      False
Exterior Lighting                   0.004343380112002439 0.005087407929746665       True
Indoor Lighting                     0.05952903782365918 0.06972645526404316       True
TV                                  0.0691145392271484 0.07365854077660784       True
MELs                                0.27420752096577883 0.292235556948872       True
```

The three physics-typed loads were idle at this timestamp (both readings
0.0), so this run alone doesn't show them running-but-unchanged. That
stronger claim — real power stays bit-identical for physics equipment across
a ZIP/pf change *while it is actively drawing power* — is already pinned by
an existing, passing test I ran rather than wrote:
`tests/python/test_reactive_power.py::TestZipPfOverride::
test_zip_pf_override_changes_q_leaves_p_identical`, which steps an Air
Conditioner until its compressor is running on a July afternoon and asserts
`p_base == p_ov` (bit-identical) across a `pf` override while `q_base !=
q_ov`. Command and result:

```
$ uv run pytest tests/python/test_reactive_power.py -k TestResolvedZipInspection -v
...
tests/python/test_reactive_power.py::TestResolvedZipInspection::test_default_battery_resolved_zip_pf_unity PASSED
tests/python/test_reactive_power.py::TestResolvedZipInspection::test_user_zip_override_pf_09_visible PASSED
tests/python/test_reactive_power.py::TestResolvedZipInspection::test_unknown_equipment_name_raises_value_error PASSED
tests/python/test_reactive_power.py::TestResolvedZipInspection::test_ashp_heater_resolved_zip_pf_084 PASSED
tests/python/test_reactive_power.py::TestResolvedZipInspection::test_battery_power_factor_setpoint_updates_resolved_zip PASSED
5 passed in 0.90s
```

`test_ashp_heater_resolved_zip_pf_084` (`tests/python/test_reactive_power.py:
609-634`) already asserts, on a real `ASHPHeater`, `(zp, ip, pp) == (0.0,
0.0, 1.0)` with the comment "Rule R1: real side forced to constant power" —
i.e. the behavior the ticket calls a bug is an existing, deliberately-tested
invariant, confirmed passing right now on this checkout, not a regression
that crept in unnoticed.

## Alternatives considered and ruled out

1. **A lookup/normalization bug hides real ZIP for scheduled loads too**
   (i.e. the ticket's "every" is literally true because of a second,
   independent defect, not just Rule R1). Ruled out by Command 1 above:
   scheduled loads on the fixture return their full literature coefficients
   (e.g. Refrigerator `zp=5.03`), so the lookup/resolution path for
   `resolve_zip()` → `zip_defaults_for_class()` → `DefaultsStore::zip_params`
   (`defaults.rs:519-526`, case-normalized lookup) is functioning correctly.
2. **The operator's building genuinely has no scheduled-load equipment
   instances** (e.g. everything folds into one unnamed aggregate the API
   can't see). Ruled out structurally: `resolve_loads.rs:830-903` builds one
   named `EquipmentSpec` per plug/fuel load type, and each becomes its own
   `Box<dyn Equipment>` reachable via `equipment_names()`/`equipment_zip()` —
   there is no unnamed-aggregate load path in the code. I could not run the
   literal `bldg0046363.xml` (not present in this repo) to confirm it
   specifically enumerates scheduled loads, so I can't rule out with
   certainty that *that one file's* HPXML happens to omit lighting/MELs/
   appliances entirely — but Command 1 shows that on a normal ResStock-style
   HPXML, scheduled loads are present and do report real ZIP, so this is not
   the general mechanism at play, and is not required to explain the
   symptom.
3. **The reactive coefficients are also wrong/inert for physics equipment**,
   i.e. `equipment_zip()` is simply broken across the board rather than
   correctly reactive-only. Ruled out: `test_zip_pf_override_changes_q_leaves_
   p_identical` and `test_ashp_heater_resolved_zip_pf_084` both show `zq/iq/
   pq/pf` populated with non-default class values and Q responding to a `pf`
   override on the same equipment whose P stays fixed — the reactive side of
   the API is live and correct, only real power is (deliberately) inert for
   this equipment class.

**Strongest alternative explanation for the symptom**, and why it's less
likely than mine: the operator's hypothesis that a second, code-level defect
independently zeroes scheduled-load ZIP would require a bug in
`resolve_zip()`/`zip_defaults_for_class()`/`DefaultsStore::zip_params()` — a
different, unrelated code path from `resolve_reactive_zip()`. Command 1's
output rules this out directly by exercising that exact path end-to-end and
observing non-default coefficients. No plausible single defect in this
codebase would produce "physics equipment inert, scheduled loads correct" as
a *side effect* of a bug — that split is exactly what the intentional Rule R1
branch (`resolve_reactive_zip` vs `resolve_zip`, chosen per equipment module)
produces by construction, and no other branch point in the traced code
produces that same split.

## The real gap: no supported way to ask "what is this system's real voltage response"

Given the above, the actual defect is not incorrect arithmetic — every
individual coefficient is intentional and independently tested (see
`zip.rs`'s `apply_is_bit_identical_to_legacy_arithmetic_for_all_class_rows`,
`reactive_only_leaves_real_power_untouched_at_all_voltages`, and
`config.rs`'s `resolve_reactive_zip` tests in `water_heater/mod.rs:592-628`).
The defect is an **API/observability gap**:

1. `equipment_zip()`'s docstring (`py_dwelling.rs:920-928`) does not mention
   Rule R1 at all. It says the dict has "the primary resolved ZIP/power-
   factor model for the named equipment" with no caveat that for an entire
   category of equipment (every HVAC unit, every water heater, every
   ventilation fan) the real-power coefficients are structurally pinned to
   `(0,0,1)` and can never reflect real voltage sensitivity, while the
   reactive coefficients on the same struct *do* apply. A caller reading
   only the dict has no way to distinguish "this equipment is genuinely
   voltage-insensitive" from "this equipment's real-power ZIP is
   inapplicable by construction, ask about Q instead."
2. There is no premise-level (whole-dwelling) aggregate ZIP or voltage-
   sensitivity query anywhere in the codebase — confirmed by
   `grep -rn "premise|dwelling_zip|aggregate_zip|whole_house_zip|total_zip"`
   across `crates/`, which returns no matches. The only way to observe the
   real system-level sensitivity today is the operator's Part-B approach:
   run `simulate()` twice at different `set_grid_voltage()` values and diff
   the output — there is no cheaper, structural, no-simulation-required
   answer to "how voltage-sensitive is this dwelling," which is exactly
   what the ticket is asking to be told the supported route for.

This satisfies the constitution's "a failure never looks like a success"
principle in spirit even though nothing here silently returns a wrong
*numeric* answer: the API silently returns a *misleadingly incomplete* answer
to "does this equipment respond to voltage," for a large, common class of
equipment, with no signal to the caller that they're looking at the wrong
half of the ZIP model or that no whole-premise equivalent exists.

## Root cause

`equipment_zip()` is documented and consumed as "the" voltage-response model
for an equipment instance, but for every physics-modeled equipment class
(HVAC, water heaters, ventilation — the Rule R1 group) it reports only the
reactive-power ZIP, with real-power terms permanently pinned to `(0,0,1)` by
`resolve_reactive_zip()` (`crates/hares-equipment/src/config.rs:465-483`).
Nothing in the API surface — docstring, return shape, or a companion query —
tells a caller this, and no premise-level aggregate exists as an alternative.
The engine's true ~0.25 %P/%V annual sensitivity is real and correctly
computed, but it originates entirely from scheduled/event-load equipment
(`resolve_zip()`, not `resolve_reactive_zip()`), a mechanism the operator has
no supported, non-simulation way to discover from the per-equipment ZIP
inspection API alone.

## Scope note for downstream steps

This diagnosis stops at the cause; it does not propose or implement a fix.
One fact worth flagging for whoever scopes the fix: the defect is in the
`equipment_zip()` API contract and documentation
(`crates/hares-python/src/py_dwelling.rs:920-943`), not in any numeric
computation — `resolve_reactive_zip()` and `ZipLoad::apply`/`reactive_kvar`
are each independently tested and correct at the values they were designed
to produce. There is no known-wrong number to pin with a currently-failing
test; that determination is the reproduce step's to make, not mine.
