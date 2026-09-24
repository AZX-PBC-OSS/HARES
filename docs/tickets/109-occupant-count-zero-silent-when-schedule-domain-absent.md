# Occupant Count Silently Returns 0.0 When Schedule Domain Absent

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-core/dwelling

## Problem

`crates/hares-core/src/dwelling/mod.rs:1903` silently returns 0.0 occupants when the schedule domain that supplies occupant counts is absent. There is no `tracing::warn!`, no error, no diagnostic — the dwelling proceeds as if the building were unoccupied. This produces zero occupant heat gains, zero CO2 contribution, and zero hot-water draw scaling, all without any indication that a required schedule input was missing.

This violates `feedback_no_silent_defaults`: missing or invalid input must error loudly, not be silently substituted with a fallback value.

## Current Behavior

`crates/hares-core/src/dwelling/mod.rs:1903`:
- The occupant-count accessor returns `0.0` when the schedule domain key is not present in the schedule registry.
- No log line, no diagnostic, no error.
- Downstream consumers (occupant heat gain, CO2 actor, water draw scaling) receive a falsy value indistinguishable from "the schedule said zero occupants in this hour".

## Required Behavior

When the schedule domain that supplies occupant counts is absent at the time the dwelling first attempts to read it:

1. If the absence is a configuration error (no schedule was registered at all for the dwelling type that requires one), return `Err(DwellingError::MissingScheduleDomain { domain: "occupants" })` from the construction or step path that first observed the absence.
2. If the absence is legitimate (e.g. an unoccupied test fixture explicitly has no occupant schedule), the dwelling configuration must declare `occupants_present: false` and the accessor must `debug_assert!(false)` if it is called against such a configuration — calling it would be a programming error.
3. Either path must surface the absence; silent zero is forbidden.

## Approach

1. Identify all callers of the accessor in `dwelling/mod.rs:1903`. Determine whether they rely on the silent-zero behaviour or whether they would prefer a loud error.
2. Add a `MissingScheduleDomain { domain: &'static str }` variant to `DwellingError` (or the equivalent error enum used by the construction path).
3. Replace the silent `0.0` return with the new error, propagated through the construction path. The hot-step accessor must not return errors — instead, the absence must be detected at construction time by validating that all required schedule domains are present given the equipment and actor set the dwelling was built with.
4. For dwellings that are legitimately unoccupied (e.g. BESTEST 600/900 base cases), the dwelling builder must set an explicit `occupants_present: false` flag that bypasses the schedule lookup entirely.
5. Add a unit test that constructs a dwelling with occupant-dependent equipment but no occupant schedule and asserts the construction errors with the new variant.

## Definition of Done

- [ ] Accessor at `crates/hares-core/src/dwelling/mod.rs:1903` no longer returns silent 0.0
- [ ] Construction-time validation rejects dwellings that require an occupant schedule but lack one
- [ ] `DwellingError::MissingScheduleDomain` (or equivalent) variant added
- [ ] `occupants_present: false` flag exists for legitimately unoccupied dwellings
- [ ] Unit test asserts loud failure when occupant schedule is absent
- [ ] All existing dwelling fixtures pass either the loud-error or explicit-unoccupied path

## Verification

```bash
cargo test -p hares-core dwelling
cargo test -p hares-core --test dwelling_integration
cargo build -p hares-core
```

## References

- HARES project memory `feedback_no_silent_defaults` — never silently substitute fallback values for missing/invalid input; error loudly.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Internal Loads — People" — occupant heat gain is a primary driver of cooling load; silently zeroing it produces 200-500 W systematic underestimate per absent occupant.
- HPXML Specification v4.x §11 "Schedules" — occupant schedule is a required input for residential dwellings.

## Related Tickets

- 102-thermal-solver-init-indoor-zone-loud-error (same loud-error pattern for thermal solver init)
- 103-zone-capacitance-air-density-loud-error (same loud-error pattern for air density)
- 114-scheduled-load-sensible-fraction-loud-error (same loud-error pattern for sensible fraction)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `crates/hares-core/src/dwelling/mod.rs:1903` is confirmed as `.unwrap_or(0.0)` on the schedule-domain payload lookup inside `apply_occupancy_gains()` (lines 1890–1918). The function comment at line 1888 explicitly documents the silent-return behaviour: *"If no occupancy column is present in the schedule the method returns without side-effects, supporting synthetic TOML inputs that omit occupancy schedules."* The `.unwrap_or(0.0)` on line 1903 extends this silence further: even when `occupancy_column_idx` is `Some(…)` but the schedule-domain update is absent or the payload vector is shorter than the column index, the function silently uses 0.0.
- [x] Described logic matches current implementation — confirmed. `occupancy_column_idx` is `Option<usize>` (line 656). When `None`, the function returns early (line 1892). When `Some(col_idx)`, the chain `.find(…).and_then(…).and_then(…).copied().unwrap_or(0.0)` (lines 1895–1903) silently produces 0.0 for any of three absence conditions: no SCHEDULE_DOMAIN_ID domain, no `custom_payload`, or `col_idx` out of bounds.
- [x] Construction path also silent — `occupancy_scale` (lines 958–972) correctly errors if an `Occupancy` spec lacks `number_of_occupants`. However, there is **no validation** that an Occupancy spec's schedule column actually exists in the registered schedule. `occupancy_column_idx` is set from `environment.occupancy_column_idx()` (line 895), which returns `None` if no occupancy column exists, and no error is raised at that point.
- [x] OCHRE cross-check result: **partially matches, HARES is stricter**. OCHRE (`vendors/OCHRE/ochre/Models/Envelope.py:1266`) uses `self.current_schedule.get("Occupancy (Persons)", 0)` — the same silent-zero fallback. OCHRE also issues a warning at line 842 (`self.warn("Occupancy not in schedule. Ignoring heat gains from occupants.")`) during initialisation when the column is absent. HARES neither warns at init nor at step time. HARES diverges from OCHRE by being _more_ silent: OCHRE at least warns once; HARES warns never.
- [x] EnergyPlus cross-check result: **N/A for this specific bug**. The silent-zero is a HARES implementation policy choice, not an algorithm inherited from EnergyPlus. EnergyPlus handles occupancy through `People` objects in the IDF, where the people count is always explicitly defined at construction time via a schedule and a `Number of People` field — there is no silent default to zero. See EnergyPlus I/O Reference (Big Ladder, EnergyPlus 8.9): the `People` object requires `Number of People Calculation Method`, `Number of People`, and `Number of People Schedule Name` — all are required inputs, and a missing schedule causes a fatal error, not a silent zero. This confirms the ticket's direction: HARES should adopt the same loud-failure posture.

### Web-Verified Citations

**Citation 1:**

- **Citation**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Internal Loads — People" — occupant heat gain is a primary driver of cooling load; silently zeroing it produces 200-500 W systematic underestimate per absent occupant.
- **Source found**: Search confirmed ASHRAE HoF 2021 Chapter 18 Table 1 "Heat Gain from Occupants" (corroborated by multiple secondary sources: Scribd F21-Ch18, Engineering Toolbox, Eng-Tips forum). Direct access to the paywalled PDF was not possible.
- **Quoted passage**: From secondary sources aggregating Table 1 data: *"Seated, very light work — Offices, hotels, apartments: Adult Male total 450 Btu/h; Adjusted (M/F) total 400 Btu/h, Sensible 245 Btu/h, Latent 155 Btu/h."* This is the ASHRAE non-residential table and applies to apartment/hotel light-activity occupants. For residential loads, ASHRAE Chapter 17 (Residential) uses a formula approach rather than per-person W values; Manual J (ACCA) prescribes 230 Btu/hr sensible + 200 Btu/hr latent per person (≈ 67 W + 59 W = 126 W total per person). OCHRE (and HARES) use 400 Btu/hr total per person (≈ 117 W), which matches the Chapter 18 apartment/hotel row, with OCHRE's sensible fraction of 0.563 giving 66 W sensible (confirmed at `crates/hares-physics/src/constants.rs:127`: `OCCUPANT_SENSIBLE_GAIN_W = 66.0`).
- **Verdict**: **Partially correct**. The chapter/section attribution is imprecise — occupant heat gain for residential simulation is addressed in Chapter 17 (residential loads) via combined equations, not in Chapter 18 Table 1 (which is for non-residential). The 400 Btu/h value that OCHRE/HARES use does appear in Chapter 18 Table 1 (adjusted, light office/apartment activity), but the ticket's framing conflates non-residential and residential references. The core claim — that silently zeroing occupant gains produces a systematic underestimate — is correct. However, the "200–500 W per absent occupant" range overstates the actual values (HARES uses ≈117 W total/person; sensible only is 66 W, latent only is 51 W; combined 117 W — not 200–500 W unless multiple occupants are absent simultaneously). The 200–500 W range would apply to groups of 2–4 missing occupants, which is plausible for a typical household but should be stated as such.

**Citation 2:**

- **Citation**: HPXML Specification v4.x §11 "Schedules" — occupant schedule is a required input for residential dwellings.
- **Source found**: HPXML Data Dictionary v4.0.0 (hpxml.nlr.gov/datadictionary/4.0.0); OpenStudio-HPXML workflow inputs documentation (openstudio-hpxml.readthedocs.io); BPI HPXML v4.1 standard overview.
- **Quoted passage**: From OpenStudio-HPXML documentation: *"If NumberofOccupants is not provided, it defaults to the sum of conditioned spaces' NumberofOccupants values if provided, otherwise it defaults to the larger of NumberofBedrooms+1 and NumberofResidents."* Regarding `SchedulesFilePath`: *"Detailed schedule inputs are provided via one or more CSV file … referenced in the HPXML file as /HPXML/Building/BuildingDetails/BuildingSummary/extension/SchedulesFilePath elements."* This element is optional. *"If neither simple nor detailed inputs are provided, schedules are defaulted"* — i.e., a default schedule is generated, not an error.
- **Verdict**: **Incorrect**. The ticket asserts that "HPXML v4.x §11 makes occupant schedule a required input." This is not accurate. HPXML v4 makes the occupant count (NumberofResidents / NumberofOccupants) optional with a default rule, and the schedule CSV (`SchedulesFilePath`) is explicitly optional — if absent, default averaged schedules are generated. There is no "§11 Schedules" that mandates an occupant schedule as a required field. The standard imposes no hard requirement; the requirement cited in the ticket is a HARES project convention (the `feedback_no_silent_defaults` rule), not an external standard.

**Citation 3 (implicit):**

- **Citation**: `feedback_no_silent_defaults` project memory — never silently substitute fallback values for missing/invalid input; error loudly.
- **Source found**: This is an internal HARES project convention, not an external standard. It is consistently applied in the codebase (e.g., `HaresError::Equipment` for missing `number_of_occupants`, `HaresError::Io` for environment init failure).
- **Verdict**: **Confirmed as project policy**. The convention is real and consistently applied elsewhere; the ticket correctly identifies this gap.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is real. `crates/hares-core/src/dwelling/mod.rs:1903` does silently return 0.0 via `.unwrap_or(0.0)` when the schedule domain payload is absent during a step, and there is no construction-time validation that ensures an Occupancy spec is backed by a schedule column. This violates the project's `feedback_no_silent_defaults` convention, which is consistently applied elsewhere in the codebase (e.g., `occupancy_scale` validation at line 960). However, two details in the ticket require refinement: (1) the HPXML citation is incorrect — HPXML v4.x §11 does not require an occupant schedule as a mandatory field; the requirement is an internal project convention; (2) the "200–500 W systematic underestimate per absent occupant" overstates the actual single-occupant impact (HARES uses ~117 W total per person, 66 W sensible + 51 W latent); the 200–500 W range only applies to groups of missing occupants. The OCHRE divergence finding is also noteworthy: OCHRE issues a one-time warning when the occupancy column is absent at init — HARES is more silent than even OCHRE. The proposed fix direction (construction-time validation + `MissingScheduleDomain` error variant + `occupants_present: false` flag for intentionally unoccupied dwellings) is sound and appropriate.

### Proposed Fix Summary

**Do NOT implement this fix** — audit only.

Minimal fix: (1) In the `Dwelling::new` / `from_toml_config_with_write_output` construction path (around line 895 of `mod.rs`), after computing `occupancy_column_idx` and `occupancy_scale`, add a validation guard: if `occupancy_scale > 0.0` (an Occupancy spec exists) but `occupancy_column_idx.is_none()`, return `Err(HaresError::Equipment("Occupancy spec present but no occupancy column found in schedule".into()))` or a new `MissingScheduleDomain { domain: "occupants" }` variant. (2) Add an `occupants_present: bool` flag (default `true`) to `SyntheticTomlConfig` / `SyntheticScheduleConfig` so BESTEST fixtures can declare `occupants_present = false`, causing the construction path to set `occupancy_column_idx = None` and `occupancy_scale = 0.0` without error. (3) Replace the `.unwrap_or(0.0)` at line 1903 with a `debug_assert` that the payload is present (since construction would have already validated its existence).

### Test Written

- **File 1**: `crates/hares-core/tests/ticket_109_occupant_count_silent_zero.rs` (new integration test file)
  - **Tests**: `zero_occupancy_schedule_does_not_error_at_construction` — verifies BESTEST 600 (zero-occupancy fixture) builds without error via the public API. `zero_occupancy_step_succeeds` — verifies one step completes without error. Both document current (pre-fix) behaviour and must be updated post-fix to assert the loud-error / explicit-unoccupied path.

- **File 2**: `crates/hares-core/src/dwelling/mod.rs` (new `#[test]` added to internal `#[cfg(test)]` module)
  - **Test**: `ticket_109_absent_schedule_domain_silently_produces_zero_gains` — directly exercises `apply_occupancy_gains()` with `occupancy_column_idx = Some(0)` and `occupancy_scale = 3.0` but no SCHEDULE_DOMAIN_ID update pushed into `latest_env`. Asserts the current broken behaviour (0.0 gains) with documentation that this assertion must be removed when the fix lands.
