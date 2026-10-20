---
reproduction_outcome: confirmed
failing_tests:
  - command: uv run pytest tests/python/test_equipment_zip_voltage_response.py -v
    path: tests/python/test_equipment_zip_voltage_response.py
    name: equipment_zip voltage-response observability contract
---

# I-01 reproduction: `equipment_zip` cannot express the dwelling's real voltage response

## Cause, restated

Per `rca.md`: no numeric computation is wrong. Rule R1 deliberately pins the
real-power ZIP of every physics-modeled equipment class (HVAC, water heaters,
ventilation fans) to `(zp, ip, pp) = (0, 0, 1)` via `resolve_reactive_zip()`
(`crates/hares-equipment/src/config.rs:465-483`), so real power stays
bit-identical to the physics model. The defect is an **API/observability
gap** (`crates/hares-python/src/py_dwelling.rs:920-943`): a caller reading
`equipment_zip()` cannot distinguish "this equipment is genuinely constant
power" from "this equipment's real-power ZIP is inapplicable by construction
— ask about Q instead," and no premise-level aggregate query exists as an
alternative. Anything computed from the published real coefficients silently
returns the wrong answer for the entire Rule-R1 class — the ticket's "the
coefficients don't describe the response."

## What the test asserts

`tests/python/test_equipment_zip_voltage_response.py` pins the observable
contract that closes the gap, pitched at the broken rule rather than the one
reported path:

1. **Every** ZIP-bearing equipment dict returned by `equipment_zip()` must
   carry an explicit `real_power_zip_applies` flag — the test iterates all
   of `equipment_names()`, so the same mistake made on any sibling equipment
   class (a new HVAC type, a new scheduled load) fails too, not just the
   Air Conditioner the RCA happened to trace.
2. The flag is **False** for Rule-R1 physics equipment (Gas Furnace, Air
   Conditioner, Electric Resistance Water Heater — whose published `(0,0,1)`
   is a structural pin, not a measured response) and **True** for
   scheduled/event loads (Indoor Lighting, MELs, Refrigerator — whose real
   coefficients genuinely govern P at the bus voltage).
3. A supported, no-simulation **premise-level** query (`dw.premise_zip()`)
   exists and reports the dwelling's real aggregate voltage response, which
   is measurably non-constant-power (~0.25 %P/%V per the ticket and the
   RCA's Part B), answering the ticket's closing question: "what is the
   supported route to premise-level ZIP?"

The flag name and the `premise_zip()` method name are the proposed contract;
the fix phase owns the final shape, but any fix must make the Rule-R1 pin
programmatically distinguishable from a genuine constant-power model and
provide a premise-level answer — a docstring-only change cannot satisfy the
constitution's "a failure never looks like a success" for an API consumer.

## Confirmation

Ran `uv run pytest tests/python/test_equipment_zip_voltage_response.py -v`
on this checkout: **4 failed in 0.94s**. Each failure is on the intended
assertion, not setup:

- `test_every_zip_dict_declares_whether_real_power_zip_applies` — no
  `real_power_zip_applies` key in any returned dict.
- `test_physics_equipment_declared_real_power_zip_inapplicable` — same
  missing key (the Rule-R1 `(0,0,1)` pin itself is confirmed present and
  unchanged, as the RCA established).
- `test_scheduled_loads_declared_real_power_zip_applies` — same missing key.
- `test_premise_zip_reflects_real_voltage_sensitivity` —
  `AttributeError: 'builtins.Dwelling' object has no attribute 'premise_zip'`.

The dwelling fixture is the repo's own
`tests/fixtures/hpxml/ochre_samples/base.xml` via the `conftest.make_dwelling`
helper — the same fixture and bindings the RCA used for its live
reproduction, so the 12-equipment mix (3 Rule-R1 physics, 9 scheduled loads)
is exercised exactly as documented there.
