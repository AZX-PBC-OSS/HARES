---
coverage_note_outcome: written
---

# Coverage note: I-07 — EV pack thermal model alignment (gh #32)

Outcome: **tests written** (the no-tests case does not apply). This phase
has run in seventeen rounds against the fix work as it landed; eighteen
Rust tests, one gate extension (round 2), one gate conversion-and-extension
(round 10), three Python gates (round 14), one parametrized Python gate +
control (round 15), one routed-finding boundary file — four strict-xfail
gates + five passing gates/controls (round 16 + its reassessment) — are
attributable to it. **Round 17 (this round) measured the landed guards for
round 16's four routed findings and re-swept the class: all four fixed
through the shared helpers (one home, `utils.rs`), all 28 routed gates
flipped green (the strict-xfail markers self-destructed as designed), and
the class is closed — no remaining raw f64 crossing on the boundary — so
no new tests were warranted** (§Round 17 — the counts and the one
considered-and-dismissed candidate). No production code was changed by
this phase; every edit is in test files (rounds 1-12) or this note. This
phase's findings have each been routed through the flow's own division of
labor and fixed: round 1's temperature-validation gap (triage gate →
validation-class fix), round 3's Generator restore defect (triage's
15-equipment class escalation → restore-contract fix, with round 4 closing
the one unguarded sibling), the red team's F4/F5 (restore reactive and
heater-draw columns — fixed with gates), round 6 closing the F5 fix's
ungated v2l extension, round 7 measuring the F6/v8 fix the triage gated,
round 8 measuring the red team's fourth-pass F6/v9 fix (the away intake —
the restore-fidelity family's last member, gated by the red team), round 9
covering the review round's machinery (three W1-face gates + the routed
`check_soc` monitor finding), round 10 measuring the landed `soc_bounds`
fix (gate conversion + the loud-error message-contract extension), round
11 measuring the landed `classify_mode` re-keying (the `Standby` mode-value
and restore gate), round 12 measuring the landed exact-sign fix (the
export-leg sub-threshold mirror gate), round 13 measuring the landed
non-finite float guard (the `from_typed` choke point + `finite_walk` —
nothing unguarded), round 14 measuring the landed Python boundary converter
(the `DwellingConfig(overrides=...)` channel gate + the huge-int and
non-string-key faces), round 15 measuring the landed actor-param guard
(the list-element arm's gate), round 16 measuring the landed solar-override
guard and sweeping its class (four routed findings, gated), and round 17
measuring the landed guards for those findings (§Round 17).

## Round 17: the landed guards for round 16's four routed findings —
## all fixed, all gates green, the class closed, no tests added

The input this round: the fix division landed guards for round 16's four
routed findings, all through **shared helpers consolidated in one home**
(`crates/hares-python/src/utils.rs:366+` — `finite_f64_required` /
`finite_f64_optional`, beside the `validate_finite`/`validate_range`/
`validate_non_negative`/`dict_required`/`dict_optional` family):

1. **`set_price_signal`** — all three fields now `finite_f64_optional`
   (`py_dwelling.rs:875+`).
2. **`set_grid_voltage`** — guarded.
3. **`set_equipment_lut`** — the extractors now walk for non-finite.
4. **The ControlSignal construction boundary** — every raw f64 field from
   round 16's 15-face map plus the ones the map hadn't listed
   (`ramp_rate_kw_per_s`, `period_s`, `duration_s`, `kvar`,
   `power_factor`, `heating_delta_c`/`cooling_delta_c`) now guarded
   (`py_control.rs:412-540`), the typed constructors validating their
   kwargs (`thermal_setpoint`, `power_setpoint`'s `min_soc`/`max_soc`,
   `soc_target`, `humidity_setpoint`, `demand_response`,
   `thermal_setpoint_delta`, `event_delay`, `max_capacity_fraction`,
   `load_fraction`, `ideal_capacity` — verified by reading each
   constructor's validate calls).

**The routed gates flipped green as designed** — the fix removed the
strict-xfail markers (the self-destruction the form exists for) and the
28 gates now pass as permanent regression coverage
(`tests/python/test_non_finite_input_boundaries.py`, re-run this round:
28 passed; the full boundary family — the file plus the solar and
actor-param boundary files — 45 passed).

**The class re-sweep, against the tree:** every f64 field in
`ControlSignal.from_dict`'s complete field map (all arms enumerated,
`py_control.rs:412-540`) is guarded — through the finite helpers or
through `dict_required` + `validate_range`/`validate_non_negative`, both
of which check `is_finite` first; every typed constructor validates; the
dwelling's `set_*` channels are guarded; the site-location surface
(`extract_site_location_override`, `py_dwelling.rs:2174`) is pre-existing
and gated (`tests/python/test_site_location.py`, 8 gates — out-of-range,
unknown-kwarg, setter-validation). **No remaining raw f64 crossing was
found on the boundary. Unguarded count: 0.**

**Considered and dismissed — extending the permanent gate's parametrize
with per-site ids for the remaining guarded fields** (`duration_s`,
`kvar`, `power_factor`, the thermal-delta fields, …): all flow through
the same two helpers the 15 existing ids already pin on both entry
styles; a per-site id would repeat the mechanism test per site — the
exact repetition the one-test-per-mechanism rule weighs against (round
15's precedent: the arm-level gate, not per-field). Dismissed with the
reason stated.

**The reassessment's extra surface — the config setters** (an
f64-crossing surface the earlier sweeps had not enumerated; checked
before re-signalling): the 17 setters on `SimulationConfig` — 12 are
non-f64 (`String`/`i64`/`u8`/`bool`/`usize`/`u64`); the 5 f64 setters
are **all guarded and gated**: `set_setpoint_deadband_c` rejects
non-finite loudly with the constructor's own message (proven by
execution; gated at `tests/python/test_py_config.py:103`,
`test_invalid_setpoint_deadband_raises_value_error`, asserting
`match="setpoint_deadband_c must be finite"`), and the four location
setters (`set_latitude`/`set_longitude`/`set_elevation_m`/
`set_utc_offset_h`) are guarded and gated
(`tests/python/test_site_location.py::test_setters_validate_range`).
The class remains closed: **unguarded count 0**.

Conclusion: no additional tests warranted this round — the fixes landed
gated (the flipped routed gates are the permanent coverage), the class
re-sweep is closed at unguarded count 0, and the only candidate is
per-site noise.

## Round 16: the landed solar-override guard + the class sweep — four
## unguarded boundaries found, routed with strict-xfail gates

The input this round: a fix landed guarding the solar-override boundary —
`set_solar_override` (`crates/hares-python/src/py_dwelling.rs:1560`)
now runs a post-parse non-finite walk over the parsed irradiance table
(one mechanism: every container arm — dict/list/ndarray/dataframe — every
field, every timestep, every surface, rejecting with the field/step/
surface named), because the downstream finiteness screens are cfg-gated
to debug builds and this is the documented pvlib/PySAM pipeline channel
where a NaN rides in from missing satellite data. Gates landed with it
(`tests/python/test_solar_override_boundary.py`): NaN/±inf on `direct`
+ a finite control + `has_solar_override` — mechanism-level (the walk is
post-parse, so one field/one arm pins it for all arms and fields).

**The class sweep** (the same shape across the dwelling's external-input
boundaries — values crossing pyo3 `f64` extraction with no `is_finite`
guard, downstream screens cfg-gated, NaN comparisons silently stopping
the consuming conditionals), measured against the tree and by execution:

- **Fixed and gated members (4):** `set_solar_override` (this round's
  input); `add_actor_by_name` params (`py_to_config_value`, round 15);
  the tariff objects (guarded at construction via `python_to_json_value`,
  round 14); the equipment constructors (`from_typed`'s finite walk +
  the per-surface boundary files).
- **Unguarded members found — 3, each proven by execution** (the value
  silently crosses today):
  1. **`set_price_signal`** (`py_dwelling.rs:873`) — `dict_optional`'s
     f64 extraction: a NaN electricity price silently arms the price
     actors (the EvDriver's price logic). Proven: silently accepted.
  2. **`set_grid_voltage`** (`py_dwelling.rs:884`) — a raw `f64` kwarg
     with no guard. Proven: silently accepted.
  3. **`set_equipment_lut`** (`py_dwelling.rs:1375` — the
     `extract_ocv_table`/`extract_f64_axis` extractors): a NaN or `+inf`
     OCV point silently crosses and poisons the pack electrical model's
     terminal-voltage solve — the exact machinery this initiative built
     and guards at every other entry. Proven: NaN and `+inf` silently
     accepted; **`−inf` is already loud** via `OcvTable::new`'s one-sided
     "voltage_v must be positive" validation — the one-sided check
     passes both NaN and `+inf` (comparisons slip) while catching `−inf`.

**The fourth finding — found by this round's reassessment sweep** (the
`dict_optional`/`dict_required` copy in `py_control.rs` that the
round-16 sweep's grep surfaced but had not chased): the **ControlSignal
construction boundary** (`from_dict` and the typed constructors,
`crates/hares-python/src/py_control.rs`) applies the shared
`validate_finite`/`validate_range` mechanisms inconsistently. The
validated faces reject loudly today (`active_power_kw`/
`reactive_power_kvar` both styles, `max_power_kw`, the SOCTarget arm's
`target_soc`/`min_soc`/`max_soc`, `on_fraction`, `fraction`); **15 f64
fields cross raw — each proven by execution to be silently accepted**:
ThermalSetpoint's three setpoint fields (both styles), PowerSetpoint's
`min_soc`/`max_soc` (both styles — while the SOCTarget arm's
same-named fields ARE validated: the same mechanism, applied next door,
missing here — the sharpest evidence nothing prevented the gap),
HumiditySetpoint's `target_rh`/`min_rh`/`max_rh`, PowerLimit's
`ramp_rate_kw_per_s`, DutyCycle's `period_s`. A dispatched NaN silently
stops the receiving equipment's conditionals (a NaN heating setpoint
never heats; a NaN `min_soc` breaks the EV's effective-floor max).

These are production guards — outside this phase's tests-only remit —
**routed, not fixed**. The gates use `xfail(strict=True)`: the Python
translation of the constitution's self-destructing `#[should_panic]`
pattern for genuinely out-of-scope defects (an established project
convention — `tests/python/test_thermal_trace.py`): today each gate
fails (the value silently crosses), which the strict xfail expects — the
suite stays green; when the boundary guard lands, the gate passes, the
strict xfail flags the XPASS **red**, and the marker is removed — the
annotation self-destructs.

The gates added this round (22-25, a new boundary file,
`tests/python/test_non_finite_input_boundaries.py`, whose module
docstring documents the sweep and the counts):

22. `test_non_finite_price_signal_fails_loudly_at_the_boundary`
    (parametrized NaN/±inf, strict-xfail) +
    `test_finite_price_signal_attaches` (control) — the price channel's
    routed finding and its working-channel control.
23. `test_non_finite_grid_voltage_fails_loudly_at_the_boundary`
    (parametrized NaN/±inf, strict-xfail) +
    `test_finite_grid_voltage_attaches` (control) — the grid-voltage
    channel's routed finding and control.
24. `test_non_finite_ocv_table_fails_loudly_at_the_boundary`
    (parametrized NaN/+inf — the silent faces — strict-xfail) — the LUT
    channel's routed finding. The `−inf` face is deliberately **not**
    in the xfail parametrize: it is already loud.
25. `test_negative_infinite_ocv_table_is_already_rejected` +
    `test_finite_ocv_table_attaches` (passing gates) — the `−inf` face
    pinned as it stands (the `OcvTable::new` one-sided validation,
    verified by execution — the only non-finite OCV value that does
    NOT silently cross) and the channel's finite control, so the
    one-sided boundary stays loud while the silent faces route above.
26. `test_non_finite_control_signal_fields_fail_loudly_at_the_boundary`
    (parametrized over the 15 proven-silent faces — 10 `from_dict` + 5
    typed-constructor, with per-face ids — strict-xfail) — the fourth
    routed finding: the shared mechanisms' missing applications, one
    gate on the mechanism (not per-site), the parametrize carrying the
    full face map.
27. `test_validated_control_signal_fields_stay_loud` (passing control)
    — the validated faces (`SOCTarget`'s `target_soc`, the typed
    `power_setpoint`'s `kw`) reject loudly today (`match="finite"`),
    pinning that the mechanism works where applied — the routed gate is
    about missing applications, not a broken mechanism — and that these
    faces stay loud through the fix.

## Round 15: the landed actor-param boundary guard — the list-element
## arm's gate added

The input this round: a fix landed guarding the actor-param boundary —
`py_to_config_value` (`crates/hares-python/src/py_dwelling.rs`) now
rejects non-finite floats at conversion, the same contract the equipment
typed-config boundary (`from_typed`'s finite walk) and the shared
Python→JSON converter enforce: actor params feed built-in seeds (the
IdealThermostat's setpoints), so an unguarded NaN arms a real actor with
a poisoned value (`with_heating_setpoint(NaN)` — NaN comparisons are
always false, so the conditionals silently stop firing), surfacing only
mid-simulation (a debug-build telemetry panic, a silent never-acting
controller behind a `tracing::error!` line in release builds). Two arms
landed: the scalar rejection and an **element-wise list rejection**
("same class, one deeper" — `FloatArray` values). Gates landed with it
(`tests/python/test_actor_param_boundary.py`): NaN and ±inf scalars on
the four thermostat params, and a finite control that observes the lost
observable (a finite setpoint attaches, simulates, and heats). Measured
the guard's arms, against the tree:

- **The scalar arm** — gated (9 landed parametrized cases). ✓
- **The list-element arm — ungated.** The landed gates reach only scalar
  params (the thermostat's); the element-wise check can regress
  independently (dropped in a refactor), and a dropped check silently
  stores `FloatArray([..., NaN, ...])`, poisoning every
  `get_f64_array` consumer — `actor_registry.rs:297`'s `occupancy_column`
  (the Occupant's presence schedule) is the real consumer. Closed with
  the parametrized gate + control below, on the real consumer rather
  than a synthetic key.
- **The catch-all arm** ("config value must be bool, float, string, or
  list of floats") — pre-existing, and the loud landing spot for
  unsupported types (an int too large for f64 extraction lands there,
  the same disposition round 14 observed for the shared converter).
- **The bool-first ordering** — dismissed with round 14's stated reason
  (documented ordering in the converter; no cheap end-to-end observable;
  an internal-representation assertion would test implementation
  detail).

The gate added this round (21, in the boundary's own gate home,
`tests/python/test_actor_param_boundary.py`):

21. `test_non_finite_list_element_param_fails_loudly_at_the_boundary`
    (parametrized over NaN/±inf) +
    `test_finite_list_element_param_still_attaches_and_simulates`
    (the control) — the list-element arm's gate on the real consumer
    (`occupancy_column`, the Occupant's presence schedule): the
    parametrized cases assert the boundary raise (`match="non-finite"` —
    the invariant both arms' messages carry); the control proves the
    rejection is value-triggered, not blanket (a finite column attaches
    and the dwelling simulates). Red on dropping the element-wise check
    (the NaN element silently stores and poisons the presence schedule).
    The control's first run failed on an unrelated loud premise error of
    my own (the column shorter than the default run — "presence
    schedule exhausted", the occupant runtime's own loud error); fixed
    by lengthening the column, not by weakening anything.

## Round 14: the landed unified Python→JSON boundary converter — two
## Python gates added, one comment observation

The input this round: a fix landed unifying the Python→JSON boundary
into one shared converter — `python_to_json_value`
(`crates/hares-python/src/utils.rs`), replacing the three per-module
copies (the DRY failure the fix's own header names: "one reviewed
converter, not two reviewed and one missed"), rejecting non-finite
floats loudly (serde_json writes them as `null` and the field's
documented default would silently apply — the silent-substitution
class, measured end to end: a pandas-fed NaN silently runs a
differently-configured dwelling or tariff); plus `make_spec`'s
`reject_nulled_value` (a `null` anywhere in a spec-parameter tree,
named by key path — the spec channel's guard, one choke point for
every `*_spec_from_py` builder). Gates landed with it: the spec channel
(`test_non_finite_spec_parameter_fails_loudly`, blueprint), the
dwelling-override channel
(`test_non_finite_override_value_fails_loudly`), and the tariff channel
(`test_non_finite_value_fails_loudly`). Also landed: comment-only changes
(`ev/config.rs`'s documented deliberate non-validation of
`heater_threshold_c` vs `min_charge_temp_c`; `degradation.rs`'s citation
correction to SSC trunk), the `py_hvac`/`py_water_heater` routing through
the new `make_spec`, and the operator's removal of the `i07_diag.rs`
tombstone (its contracts live in `ev_thermal_regressions.rs`). Measured
the converter's channels and faces, against the tree:

- **The free-form Value channels through the shared converter: 3.**
  `Dwelling.from_hpxml(overrides=...)` (`py_dwelling.rs:2467`) — gated
  (landed); the tariff `from_dict` channels (`py_tariff.rs:151`,
  `:704`) — gated (landed); **`DwellingConfig(overrides=...)`
  (`py_config.rs:632`, the config-object entry point for the same
equipment-override semantics) — ungated** → closed this round (gate 19).
  The spec channel (`make_spec`) — gated (landed, the blueprint gate).
- **The huge-int face — ungated** → closed (in gate 19): the converter's
  f64-arm comment names it ("Python ints too large for i64 that extract
  as f64 infinite (e.g. 10**400)"). **Observed by execution:** `10**400`
  is rejected loudly by the converter's **unsupported-type catch-all**
  (`"cannot convert Python object of type 'int' to JSON"`, `utils.rs:78`)
  — pyo3's f64 extraction of a too-large int fails rather than yielding
  inf in this pyo3 version, so the comment's mechanism claim is
  inaccurate for this case while the observable contract (loud, never
  silent) holds. Comment-only observation, reported here — not fixed
  (production code is outside this phase's remit).
- **The non-string-key faces — ungated** → closed (in gate 20): a
  non-string key **nested** inside a value dict is rejected by the
  converter's own dict arm (`"dict keys must be strings"` — verified by
  execution); a non-string **top-level** key is rejected loudly at
  pyo3's string extraction in the config channel's own key loop
  (`py_config.rs:627`, a raw `TypeError`) — loud by a different layer,
  both pinned per layer with the boundary-test convention ("never which
  layer raised it" where the layer is not the contract).
- **Measured and dismissed, with reasons:** the **bool-first ordering**
  (the converter extracts `bool` before `i64`, because Python `bool` is
  an `int` subclass — a reorder would silently coerce `True` to `1` and
  the downstream `json_value_to_config_value` would hand the raw
  converter a `Float` where a `Bool` field's `get_bool` returns `None`:
  a silent override miss) — no cheap end-to-end observable exists (the
  fixture's override-reachable equipment has no bool field with a
  step-level observable; an assertion on the converter's internal JSON
  form would test the representation, an implementation detail); the
  **nested `null` value** inside a provided overrides dict — deliberate
  documented design (a `null` value means the key is unset, distinct
  from a hostile non-finite float that looks like a number); the
  **`overrides=None`** face — gated (the existing
  `test_overrides_none_builds_and_non_object_overrides_raise`).

The gates added this round (19-20, Python — the channel's own test home,
`tests/python/test_py_config.py`):

19. `test_non_finite_overrides_value_fails_loudly`
    (`TestDwellingConfig`) — the ungated channel's non-finite face: a
    finite control constructs; NaN/±inf raise the converter's
    `"non-finite"` ValueError; the huge-int face (`10**400`) raises
    loudly (layer unpinned per the boundary-test convention — the
    catch-all arm, observed by execution). Red on removing the
    converter's non-finite rejection (the NaN cases silently construct)
    or its catch-all (the huge-int face silently coerces).
20. `test_non_string_overrides_key_fails_loudly`
    (`TestDwellingConfig`) — the non-string-key faces, pinned per layer:
    a string-key control constructs; a nested non-string key raises the
    converter's `"dict keys must be strings"` ValueError; a top-level
    non-string key raises loudly at the channel's key extraction.
    Red on a silent-skip regression at either layer.

Both gates' first draft over-pinned the raising layer for two faces and
went red — the investigation (not assertion-weakening: the
loud-never-silent contract is what is pinned) confirmed both faces are
loud at different layers, and the gates now pin each layer's actual
contract, with the layer-unpinned form where the layer is not the
contract (the landed boundary tests' own convention).

## Round 13: the landed non-finite-float guard (the `from_typed` choke
## point + `finite_walk`) — measured, nothing unguarded, no tests added

The input this round: a fix landed guarding the typed-config entry path
against non-finite floats — `EquipmentConfig::from_typed`
(`crates/hares-equipment/src/config.rs:462+`) now serializes every typed
config through a new `finite_walk::Walker`
(`crates/hares-equipment/src/config/finite_walk.rs`, one module, one
choke point) and rejects a non-finite float loudly with its field path,
because serde_json silently nulls non-finite floats (JSON cannot
represent them) and the field's documented default would then apply with
no error anywhere — measured end to end at the Python boundary: a NaN
`battery_temp_c` silently attached a differently-configured vehicle.
Also landed: `away_arm_zeroes_a_latched_var_setpoint_off_mode_flows_all_zero`
(`ev/tests.rs`) — a probe-turned-gate with **no production change** (the
`ev/mod.rs` diff is comment churn only on this topic; the away arm
already zeroes the latched var setpoint — the gate pins it). Measured the
guard's surface level by level, against the tree:

- **The walker mechanism** — 5 unit tests in `finite_walk.rs` (finite
  walks clean; a NaN leaf rejected with its field path; non-finite inside
  nested containers rejected with the full path; absent fields
  path-transparent; negative infinity rejected like NaN). ✓
- **The choke point** — 3 gates in `config.rs`'s tests (a NaN top-level
  optional field rejected loudly with the path `'value'`; an Infinity
  nested in a container rejected with `'inner.list[1]'`; finite and
  absent optional floats accepted — the guard's no-false-positive face).
  The choke point is generic over `T: EquipmentTypedConfig + Serialize` —
  one mechanism, every typed config, so per-equipment repeats would be
  noise; the gates run on a synthetic config exercising the generic. ✓
- **The Python boundary end-to-end** — 2 new boundary test files
  (`tests/python/test_ev_pack_thermal_boundary.py`,
  `test_battery_pack_thermal_boundary.py`): 26 parametrized hostile
  cases (NaN/±inf on every thermal field, out-of-range and zero topology
  counts, zero/negative thermal mass, UA, cell resistance) asserting an
  exception by attach time, plus the dwelling-usable-after-rejection
  face. Re-run this round: 26 passed. ✓
- **The remaining entry paths — structurally excluded, verified by
  reading, not assumed:** the HPXML path resolves through serde_json
  `Value`s (`hares-io/src/hpxml/resolve_loads.rs` — `params.get(…)
  .and_then(Value::as_f64)` throughout), and serde_json **cannot
  represent non-finite floats** — its parser rejects the tokens, the
  same mechanism the fix's own rationale documents; the overrides
  channel converts through `serde_json::Value` (`dwelling/conversions.rs`
  `python_to_json_value` → `Value::Number`), which can never carry NaN
  or ±inf. No external boundary can deliver a non-finite float into a
  raw config; the raw path's reachable fields are additionally covered
  by the per-field `validate()` class (round 1) and the boundary gates.
  **Unguarded count: 0.**
- **Boundary values:** −0.0 and subnormals are finite — accepted by
  design (the guard rejects only what serde_json would null); ±Infinity
  gated on both signs. Concurrent access N/A (config construction).

The away-arm var gate: probe-turned-gate, no production change — its own
coverage is itself (the away face's reactive zeroing, mode `Off`, and the
guard-valid pair, through the real `ReactiveSetpoint` → away transition
→ step path).

Conclusion: no additional tests warranted this round — the guard landed
gated at all three levels (mechanism, choke point, end-to-end boundary),
the remaining entry paths are structurally excluded by the very
mechanism the fix documents, and a forced gate on an externally
unreachable path (a Rust-constructed raw NaN, or a per-equipment
`from_typed` repeat) would assert nothing any existing gate does not
already catch.

## Round 12: the landed exact-sign `classify_mode` fix — one gate added

The input this round: a fix landed removing the `±1e-9` "treat-as-
nothing" tolerances from `classify_mode`
(`crates/hares-equipment/src/ev/mod.rs:1740-1760`) — keying `Charging`
on `net > 0.0`, `Discharging` on `export < 0.0`, and the `Heating`/
`Standby` split on `net >= 0.0`, exactly — because a tolerance there
classified sub-threshold power as `Off` while the published electric
flow carried the actual value, and Rule 2 (Off forbids nonzero flows)
then failed the dwelling step; the taper limit is a continuum
(`(soc_limit − soc)·capacity/dt/η`), so an ordinary landing that rounds
even one ULP short of the target produces a nonzero charge leg inside
(0, 1e-9] kW. Two gates landed with it —
`sub_threshold_charge_leg_off_mode_still_publishes_a_nonzero_flow`
(the charge-leg face: a 1e-12-below-target premise) and
`taper_landings_across_a_soc_sweep_publish_contract_valid_outputs`
(the natural-reachability sweep) — plus a `with_published_soc` probe
builder in `equipment_id_regressions.rs` (test harness). Measured the
fix's faces, against the tree:

- **`net > 0.0` (the charge leg's sub-threshold face)** — gated by both
  landed gates. ✓
- **`export < 0.0` (the export leg's sub-threshold face) — ungated.**
  The mirror corner: a V2L landing that rounds short of the reserve
  floor leaves the next step's export capped to the float-dust headroom
  (inside (−1e-9, 0) kW, nonzero at the port), and the old
  `< −1e-9` epsilon classified it `Off` with the nonzero published flow —
  the same Rule 2 violation, from the other leg. The landed charge-side
gates cannot catch an export-leg epsilon regression (their premises are
  charge legs), and `v2l_discharge_lands_on_soc_reserve_floor` asserts
  the SOC landing only. Closed with the discharge-side mirror gate
  below — one gate on the shared `classify_mode` choke point (one call
  site, both discharge legs through `compute_discharge`); a distinct
  face, not a repeat.
- **`net >= 0.0` (the `Heating`/`Standby` boundary band (−1e-9, 0) with
  the heater running)** — not discriminatingly reachable, verified by
  reading: while connected the charge arm's net is
  `charge_leg·η ≥ 0` (the heater is a parallel DC load, excluded from
  the net); on the discharge arm `net = −(export/η + heater)`, and any
  running heater makes |net| kW-scale, far outside the band — while
  with the heater off, `export < 0` is checked before the heater branch
  (`Discharging`). Dismissed with the reachability analysis, stated
  here.
- **`reactive != 0.0` (`On`) and exactly-zero `Off`** — unchanged by
  this fix (already exact), gated
  (`commanded_vars_at_zero_real_power_report_on_mode`, the
  disconnected/no-power tests). ✓

The gate added this round (18):

18. `sub_threshold_export_leg_discharge_mode_still_publishes_a_nonzero_flow`
    (`crates/hares-equipment/src/ev/tests.rs`, beside the landed
    charge-side gate) — the discharge-side mirror premise: SOC 1e-12
    above the reserve floor with a −3 kW V2L setpoint, so the discharge
    budget caps the export to the float-dust headroom (~2e-10 kW,
    inside (−1e-9, 0)). Asserts the observable corner shape (the port
    flow in the sub-threshold window — the precondition verified by
    execution), the mode value `Some(OperatingMode::Discharging)`
    (red on the `Off` misclassification), and `validate_core_contract`
    (red on the Rule 2 pair). Red on the export-leg epsilon regression
    twice over — the label assertion and the contract assertion —
    neither of which any landed gate would have caught.

## Round 11: the landed `classify_mode` re-keying (export-leg
## `Discharging`, the `Standby` corner) — one gate added

The input this round: a fix landed refining the mode classification
(`crates/hares-equipment/src/ev/mod.rs:1740-1760`, the single
`classify_mode` choke point called once at `:1947`, published at
`:1964` and at the checkpoint-restore output at `:2179`):
`Discharging` is re-keyed on the **export leg** (never the pack-side net —
a negative net from the heater alone is internal preconditioning, not
a grid-facing discharge), and a new **`Standby`** corner covers
pack-fed preconditioning — a commanded V2L/V2G export curtailed to zero
(e.g. a GridEmergency DR event) while the discharge-side heater draws
from the pack: net negative, zero port, mode `Standby` (energized and
connected, exchanging nothing at the port — the same label the
stationary Battery reports for a commanded-zero discharge and the
guard's own `resolve_idle` maps active-with-zero-flow states to). Two
gates landed with it —
`grid_emergency_discharge_with_running_heater_publishes_a_guard_valid_output`
and the V2G-leg sibling — asserting the corner's shape (zero port,
running heater) and `validate_core_contract` acceptance. Also landed:
a behavior-preserving hoist of the LUT divisor above the LUT's `as_mut`
borrow (a compile-fix refactor — pinned by the existing W2 gates), the
checkpoint version-comment update, and the two docs files. Measured
the fix's surface for unguarded occurrences, against the tree:

- **The mode classification is one shared choke point** — one
  `classify_mode` call site (`:1947`, with the away arm's `Off` special
case), both discharge legs sharing `compute_discharge` — so one test on
  the shared mechanism covers both legs (the V2G gate proves the corner
  is reached on that leg; no per-leg repetition).
- **The mode VALUE of the corner was ungated — the landed gates pin
  only contract validity, which `Off` satisfies equally with zero
  flows**, so a regression relabeling the pack-fed preconditioning pack
  as `Off` (losing the fix's semantic content — the label is the fix)
  would have stayed green. Closed with the mode-value assertion in the
  new gate below.
- **The corner's checkpoint-restore face was ungated** — the restore
  publishes the checkpointed `last_mode` verbatim (`:2110`, `:2179`); a
  restore that re-derived the mode from the zero port would publish
  `(Off, zero flows)` — guard-valid, silently losing the saved state's
  discharge dispatch, uncaught by the mid-discharge restore gate (whose
  premise exports nonzero). Closed with the restore round-trip in the
  same gate (the restore family's established pattern —
  `mid_discharge_checkpoint_restores_island_source_availability` does
  step + save/restore in one test).
- **The other faces were verified gated, not assumed:** `Charging`
  (`update_control_reports_charging_when_connected_and_drawing_power`
  and the L2 session tests), plain `Discharging` (the mid-discharge
  restore gate asserts the mode value at step and restore, `ev/tests.rs`
  `mid_discharge_checkpoint_restores_island_source_availability`);
  `Heating` (`preconditioning_pack_reports_heating_mode`), `On`
  (`commanded_vars_at_zero_real_power_report_on_mode`), away-`Off`
  (`away_activity_reports_off_mode_consistent_with_zero_dwelling_flows`).
  **Unguarded count: 0 after this round's gate.**
- **The `Discharging`-with-heater-and-nonzero-export face is
  value-identical under both the old (pack-net) and new (export-leg)
  keys** — both classify it `Discharging` — so no discriminating test is
  possible or needed; the keys differ exactly in the two corners the
  gates cover (export-zeroed and the plain-discharge restore).

The gate added this round (17):

17. `pack_fed_preconditioning_reports_and_restores_standby_mode`
    (`crates/hares-equipment/src/ev/tests.rs`, beside the two landed
    corner gates) — premise mirrors the V2L corner gate (cold pack
    discharging, heater running, GridEmergency-curtailed export);
    asserts non-vacuous preconditions (zero port, running heater), the
    **mode value `Some(OperatingMode::Standby)` at the step**, then a
    real `save_state`/`load_state` round-trip asserting the restored
    mode value and `validate_core_contract` on the restored output.
    Red on an `Off` relabel of the corner and on an `Off`-rederiving
    restore — both of which every other gate, including the two landed
    corner gates, would have let pass green.

## Round 10: the landed `soc_bounds` fix (round 9's routed finding) —
## one gate extension, the class closed

The input this round: the fix division landed round 9's routed finding —
`check_soc`'s silent skip is now a loud gate
(`crates/hares-core/src/dwelling/mod.rs:7105-7121`: a `let-else` that
fails the step with a `HaresError::Dwelling` naming the check
(`soc_bounds`), the equipment, and the population rule), the two
spy-harness files whose BATTERY/EV spies stand in for real storage
equipment were adapted to publish SOC
(`equipment_ordering_tests.rs`, `multi_instance_naming.rs`), and the
round-9 must_fail gate was converted to a direct assertion (it flipped
green with the fix — verified by execution this round). Measured the
fix's surface for unguarded occurrences, against the tree:

- **The monitor silent-skip class is closed.** Full sweep of
  `check_invariants` (`dwelling/mod.rs:7026+`): every checker invocation
  (`check_temperatures`, `check_soc`, `check_ev_capacity_degraded`,
  `check_nan_screen`, `check_electrical`, `check_reactive`,
  `check_fuel_electric_absent`, `check_fuel_coverage`, `check_thermal`,
  `check_heating_accumulator`, `check_hvac_power_non_negative`,
  `check_moisture`) invokes directly on computed values — no other
  optional-channel-gated monitor exists in the population. The class's
  total count stands at 2 (the two gated occurrences); **unguarded
  count: 0**.
- **The fake-equipment sweep is complete.** Grepped `hares-core/tests`
  and `hares-fleet` for fake equipment with BATTERY/EV end uses: the
  two spy files the fix adapted, and no others (`core_output_regressions.rs:91`
  searches real built equipment, not a fake). The full workspace green
  (below) confirms no fake trips the loud gate anywhere.
- **One gap found and closed — the loud-error message contract.** The
  converted gate asserted the step fails but not what the failure says;
  the sibling `ev_capacity_degraded` gate pins both the check name and
  the equipment name in the message
  (`msg.contains("ev_capacity_degraded") && msg.contains("ProbeEvAbsentTelemetry")`),
  and the constitution's loud-error rule requires exactly that — so a
  regression to a generic error message would have stayed green here
  while degrading the debuggability the loud gate exists to provide.
  Closed by extending the round-9 gate (the established gate-extension
  pattern, one assertion on the shared mechanism):
  `ev_soc_check_fails_loudly_when_core_output_soc_is_absent`
  (`crates/hares-core/tests/equipment_id_regressions.rs`) now asserts
  `msg.contains("soc_bounds") && msg.contains("ProbeEvAbsentSoc")`,
  mirroring the sibling gate exactly, and its doc comment (which still
  described the pre-fix "red by design" state) was refreshed to the
  landed-fix reality. Re-run green alongside the sibling gate.
- **The NaN feeder stands as documented.** The loud gate's own comment
  names `Soc::try_from(..).ok()` as a channel-nulling path "itself a
  defect" — a non-finite internal SOC now surfaces as the loud
  absent-SOC error at the dwelling boundary, closing round 9's feeder
  observation downstream; not separately injectable through the public
  surface without a real internal defect, so no forced test.

Conclusion: the class is closed (verified against the tree, not assumed
from the fix's word), the one gap on the fix's surface is gated, and no
further tests are warranted this round.

## Round 9: the review round's fixes (W1/W2/W3 + loud monitor) — four
## tests added, one routed finding

The input this round: the review round (`review.md`, 2026-09-25) fixed
three warnings in the machinery this initiative introduced and made the
dwelling `ev_capacity_degraded` gate loud on absent keys. Measured each
fix's surface for untested occurrences, against the tree:

- **W1 (the shared I²R solve's basis in `apply_soc_and_thermal`)** —
  the mechanism lives at `crates/hares-equipment/src/ev/mod.rs:1668-1705`
  (the pack-side net `net_dc_kw` at `:1668-1673`, the pending-drive
  consumption at `:1690-1700`, the solve call and published ohmic at
  `:1704-1705`). 2 solve callers (EV, Battery); the Battery's heater is
  an AC standby load that never enters its ohmic solve or its SOC
  (verified in `battery/mod.rs:1335-1380` — no sibling instance). The EV
  mechanism has **four faces; one was gated** (the heater-only state, `ohmic_loss_w`
  exactly 0). The other three were unguarded — a regression on any of
  them changed no observable any existing test asserts (the ohmic
  deltas are ~0.2–7 W, invisible to every thermal-band and SOC
  assertion). Closed with three tests on the same shared mechanism, one
  per branch (each branch regresses independently — an arm-specific
  "symmetry cleanup" or a dropped `pending_drive_kwh` consumption does
  not break the others' gates):
  13. `heater_current_never_enters_cell_ohmic_loss_while_charging`
      (`ev/tests.rs`) — exercises the charging arm `step.charge_leg_ac_kw
      * eta` (`ev/mod.rs:1672`) as the solve's sole basis: with the supply
      cap slack, a simultaneous charge + heater step reports the same
      cell ohmic loss as the same step with the heater off (the pair-test
      premise, which already proves the charge leg identical — so any
      ohmic difference is attribution, not allocation). Red on the pre-W1
      shape (the heater-inclusive bus draw fed to the solve — ~6× the
      charge-leg-only I²R).
  14. `cold_discharge_ohmic_loss_covers_export_and_heater_terminal_currents`
      (`ev/tests.rs`) — exercises the discharge arm `-(export_dc_kw +
      heater_dc_kw)` (`ev/mod.rs:1670`): the pack is the source for both
      the export conversion and the heater's DC draw, so the oracle (the
      same shared solve production uses) is evaluated on the pack-side
      total (export/η + heater) at the post-debit SOC's OCV, with a
      non-vacuous discrimination assertion (the 5 kW heater's ~14 A is
      a large share of the terminal current — excluding it drops the
      ohmic loss by its full I²R, > 1 W at this premise). Red on a
      future "symmetry cleanup" that excludes the heater on both arms.
  15. `drive_energy_heats_the_pack_at_its_equivalent_discharge_current`
      (`ev/tests.rs`) — exercises the pending-drive consumption
      (`ev/mod.rs:1690-1700`, `pending_drive_kwh` → equivalent discharge
      power → the `net_dc_kw - drive_dc_kw` solve basis at `:1704`):
      a 15 kWh `EvDrive` consumed by one 15-min step warms the pack by
      exactly I²R·dt/C (oracle: the shared solve on the drive's
      equivalent discharge power; UA 0, pack at ambient, so the entire
      rise is the drive's I²R). Red on dropping the `pending_drive_kwh`
      mechanism — material for the cold-charge contract (at the
      post-alignment day-scale τ, most of a commute's heat is still in
      the pack at the charge-window start).
- **W2 (the shared `charging_lut_c_rate` divisor)** — 2 callers (EV,
  Battery), each gated by the review round's own gates
  (`lut_c_rate_divisor_is_temperature_independent`, the Battery
  sibling, the SOH-once companion — verified present and passing).
  **Unguarded count: 0.**
- **W3 (the actor-seed rating)** — 1 producer
  (`degradation_adjusted_capacity_kwh`), both consumers gated
  (`actor_seed_capacity_is_independent_of_init_weather` + the updated
  seed test). **Unguarded count: 0.**
- **The loud-monitor class — one same-class occurrence the fix's own
  sweep missed, proven by execution:** the review round made
  `ev_capacity_degraded` loud on absent keys; ten lines up in the same
  invariant loop, `check_soc`'s `if let Some(soc)`
  (`crates/hares-core/src/dwelling/mod.rs:7105-7107`) silently skips
  when an EV-end-use equipment's core-output SOC is absent — and the
  EV/Battery publish sites use `Soc::try_from(self.soc).ok()`
  (`ev/mod.rs:1943`, `:2158`; `battery/mod.rs:1608`, `:1792`), so a
  non-finite internal SOC silently nulls the channel too. Same loop,
  same filtered population (BATTERY/EV, which always publish SOC), same
  anti-pattern ("a monitor must detect the absence of expected input,
  not merely anomalous values"). Closed with a routed gate:
  16. `ev_soc_check_fails_loudly_when_core_output_soc_is_absent`
      (`equipment_id_regressions.rs`, beside the ev_capacity gate) —
      an EV-end-use probe carrying the four static keys **present and
      consistent** (so the adjacent now-loud gate passes, isolating the
      `check_soc` skip) but no core-output SOC. **Finding, proven by
      execution: today the step succeeds** — the gate silently skips —
      so the test is `#[should_panic]` per the constitution's
      known-failure pattern (the expected string is the `expect_err`
      message; the annotation self-destructs — goes red — when a fix
      phase makes the gate loud, which is the fix this test routes).
      This phase adds tests only; the production fix (make the
      `check_soc` gate loud on absent SOC for the BATTERY/EV population,
      mirroring the ev_capacity pattern ten lines below) is routed to
      the flow's fix division.
      **Superseded disposition (coverage triage, `coverage-note-quality.md`
      §Third-pass):** the triage routed this finding as `defect_found`,
      whose must_fail gate requires a genuinely failing command — a
      `#[should_panic]` test exits 0 while the defect exists and failed
      the hook. Since the defect is routed to this initiative's own fix
      division (in scope, so the known-failure pattern's out-of-scope
      precondition does not hold), the triage removed the `#[should_panic]`
      attribute; the test is now a plain red gate (exit 101), green
      unedited once the fix lands — the same must_fail/must_pass shape as
      this initiative's reproduction gates.

## Round 8: the fourth-pass F6 fix (v9 — away intake verbatim) —
## measured, the family closed, no tests added

The input this round: the fourth red-team pass found the v7/v8 verbatim
sweep **one member short** — `away_charge_actual_kw`, published as
`AWAY_CHARGE_POWER_KW` (the documented way away charging is observed,
"the actual intake after taper"), meets the same underivable-basis
criterion (the away state publishes zero port power, so no checkpointed
column reconstructs the away charge leg) yet was left at its init zero
by the restore — misreporting an active away session as drawing nothing
(the asymmetry: `v2l_power_kw`, the discharge-side actual, was
checkpointed; the charge-side away actual was not). Previous dispositions
of the column as a "documented lossy placeholder" were superseded by
the red team's proof that it is a published transfer observable, not an
internal diagnostic. The fix run closed it with the established verbatim
pattern: checkpointed (v9, `ev/checkpoint.rs:85`) and restored verbatim
(`load_state`: `self.away_charge_actual_kw = cp.away_charge_actual_kw`,
`ev/mod.rs:2099`), the red team's gate passing unedited. Measured the
fix's surface for untested occurrences:

- **The restore-fidelity family is closed.** Verified independently of
  the red team's sweep: the restore's remaining zeroed columns are
  exactly the two the red team's held list affirms as internal per-step
  diagnostics (`ohmic_loss_w`, `pack_net_charge_kw` — recomputed next
  step, not published transfer/state observables); every published
  transfer/state column is now checkpointed and restored verbatim —
  reactive (v6), heater draw (v7), V2L dispatch (v8), away intake (v9) —
  each gated (seven gates re-run and passing this round: the red team's
  away-intake and floor-held gates, round 6's island gate, both F4
  reactive gates, the F5 heater-draw gate, the F1 var-support gate).
  **Unguarded count: 0.**
- **Checkpoint v9 version-bump error path** — covered on the shared
  mechanism, unchanged: `version_mismatch_is_rejected_with_descriptive_error`
  (`hares-equipment/src/serial.rs:220`). **Unguarded count: 0.**
- The tapered-intake face (the "after taper" semantics) is the same
  verbatim mechanism the red team's gate pins at the rating value — a
  regression to zeroing breaks it regardless of the restored value; no
  repeat test.

Conclusion: no additional tests warranted this round — the fix landed
gated, the family is closed (verified against the tree, not assumed
from the red team's word), and the version-bump error path is covered
at the shared mechanism.

## Round 7: the F6 fix (v8 — v2l verbatim) — measured, nothing
## unguarded, no tests added

The input this round: the round-6 triage's reassessment probe found **F6**
— the v2l restore derivation unfaithful in the floor-held discharge
corner (the step keys `v2l_active` on dispatch presence, not the
computed export, so a dispatched step at the reserve floor publishes
`V2L_ACTIVE = 1` with zero export and mode `Off`; the derivation keyed
on the mode flipped false exactly there) — and left a failing gate
(`floor_held_discharge_checkpoint_restores_the_published_v2l_state`).
The fix run closed it with the established verbatim pattern: checkpoint
**v8** persists `v2l_active`/`v2l_power_kw` (`ev/checkpoint.rs:68,75`)
and restores them verbatim (`load_state`). The triage's gate passes
unedited. Measured the fix's surface for untested occurrences:

- **The v2l verbatim mechanism's faces** — the restore is now one code
  path (`self.v2l_active = cp.v2l_active`); its faces are gated:
  dispatched-and-exporting (round 6's test 12), floor-held zero-export
  (the triage's F6 gate), and the heater/reactive/var faces by the
  F4/F5/F1 gates — all six re-run and passing this round. The triage's
  named DR-zeroed export corner (GridEmergency during a latched
  discharge) publishes the identical restored state through the same
  checkpointed fields — a separate test would repeat the F6 gate's
  assertion on the same mechanism, which the one-test-per-mechanism
  rule weighs against. **Unguarded count: 0.**
- **Checkpoint v8 version-bump error path** — covered on the shared
  mechanism, re-run and passing:
  `version_mismatch_is_rejected_with_descriptive_error`
  (`hares-equipment/src/serial.rs:220`). The schema round-trip
  (`latched_hold_and_target_survive_equipment_checkpoint_round_trip`)
  covers the v8 fields' persistence. **Unguarded count: 0.**
- The triage's dismissed items (`away_charge_actual_kw` lossy
  placeholder; furnace/battery `q = 0`; the carried dismissals) hold
  with their reasons on record.

Conclusion: no additional tests warranted this round — the fix landed
gated, the remaining faces are the same mechanism already pinned, and
the version-bump error path is covered at the shared mechanism.

## Round 6: the F5 fix's ungated extension (v2l restore derivation) —
## one gate added

The input this round: the third red-team pass found **F5** (the F4 fix
was correct for the reactive column but the same restore block still
zeroed `heater_draw_w` while publishing the heater-funded
`active_power_kw` and `Heating` mode — a state no live step can
produce), and the fix run closed it with the same lossless pattern
(checkpoint v7, `heater_draw_w` restored verbatim, `heater_active`
derived from the restored draw). **The fix's own post-fix audit then
went beyond the red team's finding and fixed `v2l_active`/
`v2l_power_kw` derivation at restore** — `island_source_available`
reads `v2l_active`, so a checkpoint-restore mid-discharge was reporting
no island source for the whole restore-to-first-step window — with the
lossless derivation `v2l_active ⟺ last_mode == Discharging`,
`v2l_power_kw = −active_power_kw`. **That extension landed with no
gate**: the red team had named the v2l columns a weaker unification
candidate and declined to force a test; the fix fixed them anyway.

Measured the changed surface: the restore-fidelity family's columns —
reactive (F4: 2 gates), heater draw (F5: 1 gate), v2l/island (**0
gates**). Count of unguarded instances: **1**. Closed this round with
one gate on the shared mechanism (test 12), mirroring the F4/F5 gates'
round-trip pattern. The audit's other disposition — `away_charge_actual_kw`
standing as a documented lossy placeholder (self-healing on the next
away step, no published-flow infidelity) — stands; no test forced
against a documented design.

## Round 5: the F4 fix (EV restore reactive basis) — measured, nothing
## unguarded, no tests added

The input this round: the second red-team pass found **F4 — a defect in
the F1 fix itself** (the EV restore recomputed the reactive flow over
the heater-inclusive port total, the basis the step's inverter-leg rule
forbids: 2.69 kVAR of vars from a resistive load in idle preconditioning;
a commanded 6.5 kVAR clamped to zero at a bound-binding session), and the
fix run closed it with the water heaters' lossless pattern — the
step-published `reactive_power_kvar` is checkpointed (v6) and restored
verbatim (`ev/mod.rs` `load_state`; the only `compute_reactive_kvar`
call remaining is the step's inverter-leg call at `:1838`). Measured the
fix's surface for untested occurrences:

- **Wrong-basis restore shape** — the red team's own sweep read every
  restore site and found the EV was the only wrong-basis site in the
  population; post-fix, its faces are covered: both heater-active faces
  (the only states where port ≠ inverter leg) by the red team's two
  gates (`heater_active_checkpoint_restores_the_reactive_flow_the_step_published`,
  `commanded_vars_served_at_a_bound_binding_checkpoint_survive_restore`),
  the var-support face by the F1 gate
  (`var_support_checkpoint_restores_an_output_the_mode_flow_guard_accepts`),
  and the discharging/away faces are value-identical under either
  mechanism (port = export = inverter leg; q = 0) — verified by the red
  team's held list. All four gates re-run and passing this round.
  **Unguarded count: 0.**
- **Checkpoint version-bump error path** (a v5 blob under the v6
  restore) — covered on the shared mechanism, the right level:
  `version_mismatch_is_rejected_with_descriptive_error`
  (`hares-equipment/src/serial.rs:220`) pins `load_versioned`'s loud
  rejection for every equipment's old blobs — one test on the shared
  mechanism rather than one per equipment. Re-run and passing.
  **Unguarded count: 0.**
- **The unification candidate** (furnace/battery restores' lossy `q = 0`
  placeholder vs the water heaters' verbatim pattern) — a documented
  design whose restores claim no q fidelity; the red team itself rejected
  attacking it ("a test demanding verbatim q restoration there would
  demand the water heaters' design for equipment whose restore makes no
  fidelity claim"), and the fix assessed it as documented, not a defect.
  Adding such a test now would force noise against a documented decision.

Conclusion: no additional tests warranted this round — the fix landed
with gates on both defect faces, the version-bump error path is covered
at the shared mechanism, and the class search (the red team's sweep,
re-verified against the tree this round) finds zero unguarded
occurrences.

## What changed (the surface covered, round by round)

- **Original fix:** EV pack thermal alignment with the stationary Battery
  (D1 thermal mass/UA, D2 I²R-only heating, D3 temperature+supply heater
  gate, supply-cap allocation, DR multiplier, `Heating`/`On` mode
  classification, ambient init, LUT composition, degradation guard,
  override validation, D4). Rounds 1-2 covered its boundary arms, error
  paths, and input domains (tests 1-9).
- **Validation-class fix:** temperature-field finiteness + ramp ordering
  across EV/Battery/ventilation/heat-pump, each with gates.
- **Deadline-raise exception:** round 2 covered the combined arm
  (raise + heater).
- **Red-team round fixes (F1-F3)** with their gates; round 3 extended the
  F2 gate with the `Some(Value::Null)` face.
- **Restore-contract class fix (round 4's input):** the round-3
  triage escalated this phase's Generator finding to a **15-equipment
  class** (Shape A: persisted mode contradicting restored flows —
  Generator, Ventilation; Shape B: `CoreOutput::default()` published at
  restore against declared capabilities — 13 equipment) and the fix run
  closed every enumerated restore with per-equipment sibling gates
  (3 triage gates + 14 sibling gates; workspace 7005 at its signal).
- **Red-team round-2/3 fixes (F4/F5, rounds 5-6's input):** the EV
  restore's reactive and heater-draw columns checkpointed and restored
  verbatim (v6/v7) — the wrong-basis recompute and the zeroed-draw
  infidelity, both gated by the red team's tests; the F5 fix's v2l
  extension derived at restore (round 6's gate) and then made verbatim
  at v8 after the triage's floor-held-corner finding (F6, gated by the
  triage).
- **The restore-fidelity family's close (rounds 7-8's input):** the
  triage's floor-held gate (v8) and the red team's fourth-pass
  away-intake gate (v9 — the family's last member, closing the family:
  reactive, heater draw, V2L dispatch, away intake all verbatim, with
  only the two affirmed per-step diagnostics zeroed at restore).
- **The review round's machinery (round 9's input):** W1's I²R-basis
  rule (heater excluded on the charge arm, included on the discharge
  arm, drive I²R), W2's shared LUT c-rate divisor, W3's actor-seed
  rating, and the loud `ev_capacity_degraded` monitor — measured in
  round 9; three ungated W1 faces closed, one same-class monitor gap
  routed.

## Class measurement (round 4 — the restore-contract class audit)

The changed surface is the restore contract itself. Audited the fix's
gate inventory against the triage's enumeration, equipment by equipment,
plus the `load_state` sites absent from both sweeps:

- **1 unguarded instance found: `water_heater/indirect_tank.rs`.** It is
  in the triage's Shape-B enumeration; its production restore was fixed
  (rebuilds a populated `CoreOutput`, mode re-derived by the step's own
  rule — `load_state`, `indirect_tank.rs:620-676`), but it was the one
  enumerated equipment whose sibling gate did not land with the fix. Its
  test module had no restore round-trip at all. Closed in round 4
  (test 11).
- **`protocol_bridge`** (`protocol_bridge/mod.rs:266`): restores only
  counters; declares `CoreCapabilities::empty()` (`:121`) — nothing
  presence-required, no mode/flows published. Consistent by reading;
  honestly excluded, no gate needed.
- **`macros.rs:65`**: a pure delegation wrapper (`self.$inner.load_state`)
  — the gated inner equipment covers it. Excluded.
- All 15 enumerated instances + wrappers otherwise gated (verified by
  name in round 4: generator ×2, ventilation, gas, tankless,
  heat_pump_wh, resistance, event_load, baseboard, dehumidifier,
  ideal_hvac, pv, boiler ×2, furnace ×2, air_conditioner, cooler ×2
  [ASHP + WSHP], heater, plus EV/Battery from earlier rounds);
  `scheduled_load` verified consistent by the triage (rebuilds a
  populated literal, declares only ELECTRIC).

## Tests added by this phase (18 Rust + 3 Python gates + 1 parametrized
## Python gate & control + 4 strict-xfail routed-finding gates + 5 Python
## passing gates/controls + 1 gate extension + 1 gate conversion-and-extension)

Tests 1-10 (rounds 1-3, detailed in prior notes and verified passing on
this tree): the heater gate's no-supply and outage arms, D3
demand-independence, DR composed-total and both commanded-zero entries,
the six pack-thermal/topology validation rules, the thermal-default
scaling law, the deadline-raise combined arm, the override
unhandled-names branch, and the Generator mid-generation restore
invariant. Plus the F2 gate extension (`Some(Value::Null)` fourth
payload, `crates/hares-core/tests/ev_thermal_regressions.rs`).

Round 4:

11. `restored_checkpoint_output_satisfies_the_core_contract`
    (`crates/hares-equipment/src/water_heater/indirect_tank.rs`,
    `control_domain_tests` module) — the missing sibling gate for the
    one unguarded enumerated equipment: a real
    `save_state`/`load_state` round-trip through a stepped tank,
    asserting the restored output publishes its mode and thermal flow
    (non-vacuous preconditions naming the Shape-B regression face) and
    satisfies `validate_core_contract`. Mirrors the twelve sibling
    gates exactly, including their `ports()` harness pattern (the tank
    declares a zone-2 thermal port and a loop-3 fluid port — a bare
    `PortSlots::default()` fails the step, which the helper documents).

Round 6:

12. `mid_discharge_checkpoint_restores_island_source_availability`
    (`crates/hares-equipment/src/ev/tests.rs`, beside the F4/F5 gates) —
    the ungated v2l restore derivation: a real mid-discharge
    save/load round-trip asserting the restored
    `island_source_available()` (the observable the fix's own audit
    named — the dwelling's islanding reads it), the `V2L_ACTIVE`
    telemetry, and the `V2L_POWER_KW` magnitude equal to the
    step-published export. A regression reverting to the placeholder
    `false` would make a mid-outage dwelling see no island source in
    the restore window, silently — this goes red instead.

## Verification (run this round — round 17 — measurement only, no code
## added by this phase)

- The boundary family, together → `uv run pytest
  tests/python/test_non_finite_input_boundaries.py
  tests/python/test_solar_override_boundary.py
  tests/python/test_actor_param_boundary.py -q` → **45 passed** (the 28
  flipped routed gates green as permanent regressions, plus the solar and
  actor-param gates).
- The full Python suite → `uv run pytest tests/python/ -q -n 4` →
  **1249 passed, 12 skipped, 0 failed** — **no xfails remain**: the strict
  markers self-destructed with the landed guards exactly as the form
  designed (the count moved 1226 + 23 xfailed → 1249 with the fix's own
  gates and the flipped markers removed).
- `cargo nextest run --workspace --no-fail-fast` → **7037 passed, 0
  failed, 12 skipped** (no Rust code added or changed by this phase this
  round); `cargo clippy --workspace --tests -- -D warnings` → clean;
  `cargo fmt --check --all` → clean.
- No code was added or changed by this phase this round (measurement
  only — the note edits aside), so the runs above also verify the tree
  this round measured.

Round 16's verification (superseded in the Python suite count, which
moved 1226 + 23 xfailed → 1249 with the landed guards' conversion of
the routed gates): the round-16 gates flipped green unedited in
substance (the markers removed per the designed self-destruction),
verified inside the 45-passed run above.

Round 4's verification (superseded only in the workspace count, which
moved 7006 → 7008 with the F4 fix's two gates):
`cargo nextest run -p hares-equipment --lib water_heater::indirect_tank`
→ 2 passed. Honest note retained from that round: the gate's first run
failed on my own harness (a bare `PortSlots::default()` — "undeclared
thermal zone: ZoneId(2)"), not on production; fixed by mirroring the
sibling water heaters' `ports()` helper.

## Production paths not covered, and why

- **The `check_soc` silent skip (round 9's routed finding — landed and
  closed in round 10)** — the one production path round 9 could not
  close (a production-code change, outside its tests-only remit). The
  gate is `dwelling/mod.rs:7105-7121`, now a loud `let-else` failing the
  step naming the check (`soc_bounds`) and the equipment, for the
  BATTERY/EV population — with the channel-nulling feeder
  `Soc::try_from(self.soc).ok()` at `ev/mod.rs:1943`, `:2158` and
  `battery/mod.rs:1608`, `:1792` now surfacing downstream as the loud
  absent-SOC error. Gated by
  `ev_soc_check_fails_loudly_when_core_output_soc_is_absent`
  (§Round 9, item 16) — since the coverage triage's disposition
  correction (`coverage-note-quality.md` §Third-pass) a plain red test,
  **green unedited when the fix landed (verified by execution in
  round 10)**; extended in §Round 10 with the loud-error message contract
  (`msg.contains("soc_bounds") && msg.contains("ProbeEvAbsentSoc")`,
  mirroring the sibling `ev_capacity_degraded` gate's message assertion).
  The monitor silent-skip class is closed: the round-10 full sweep of
  `check_invariants` found no other optional-channel-gated monitor in
  the population.
- The four documented modeling boundaries (Bernardi entropic term,
  road-speed convective cooling, constant-power PTC heater, sub-zero
  degradation extrapolation) — Known Limitations with their constraints
  and error directions on record; a test forced against any of them
  would assert against documented design, not catch a real failure
  (the test-quality concern's removal test fails for them).
- The restore-contract class is now fully gated — every enumerated
  instance (15 + wrappers) carries a round-trip gate, including
  indirect_tank as of round 4; `protocol_bridge` and the macro
  wrapper excluded with reasons above.
- Discharge-leg DR × heater and degradation-guard exact envelope edges:
  dismissed by triage and red team, reasons on record.
- Below-cutoff preconditioning under a binding low setpoint: pinned by
  `commanded_power_setpoint_bounds_total_draw_with_heater_running` and
  `heater_only_grid_draw_when_fully_cold_derated`; a variation.
- Python surface: covered by the fix's adapter tests including
  `test_overrides_none_builds_and_non_object_overrides_raise`.
