# Occupancy sensible and latent gain schedules and diversity
**Review ID**: core-10
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/actors/occupant.rs`
- `crates/hares-io/src/hpxml/resolve_loads.rs`
- `crates/hares-core/src/dwelling/mod.rs` (lines 690-706, 1055-1090, 2070-2130)
- `crates/hares-physics/src/constants.rs` (lines 170-198)
- `crates/hares-io/src/schedule_resolve.rs` (lines 44-50, 374-406, 730-779)
- `crates/hares-core/src/dwelling/synthetic.rs` (lines 900-914)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/utils/schedule.py`
- `vendors/OCHRE/ochre/Models/Envelope.py` (lines 895-908)
- `defaults/Default Schedule Parameters.csv`

## Findings

### Finding 1: [Severity: high]
**Description**: HPXML-embedded occupancy schedule fractions are parsed and stored but never used to generate the occupancy timeseries. The `parse_schedule_extension_params` call in `resolve_loads.rs:50-52` extracts `weekday_schedule_fractions`, `weekend_schedule_fractions`, and `month_multipliers` from HPXML `<BuildingOccupancy>/<extension>` elements and stores them in the Occupancy equipment spec. However, `schedule_resolve.rs:396-404` explicitly drops `ScheduleCategory::Occupancy` via the `_ => {}` fallback arm. The mapping filter at line 374-383 also excludes Occupancy from the equipment lookup map. The parsed HPXML schedule data is dead data -- it is never converted into an 8760-hour occupancy timeseries.

In contrast, OCHRE's `import_occupancy_schedule` in `schedule.py:428-436` calls `create_simple_schedule(**ochre_dict)` with those same HPXML-extracted fractions, generating the full 8760-hour occupancy schedule. When weekday/weekend fractions are absent, OCHRE falls back to the default schedule file (line 431-433), ensuring an occupancy schedule always exists.

**Code Location**:
- `crates/hares-io/src/hpxml/resolve_loads.rs:50-52` (schedule fractions stored but unused)
- `crates/hares-io/src/schedule_resolve.rs:404` (Occupancy category silently discarded)
- `crates/hares-io/src/schedule_resolve.rs:374-383` (Occupancy excluded from mapping filter)

**Root Cause**: The schedule resolution pipeline was designed only for power and event-based schedules. Occupancy is expected to arrive as a pre-populated column in the schedule CSV file (from ResStock or a user-provided schedule file). There is no code path that builds an occupancy timeseries from the Default Schedule Parameters.csv or from HPXML extension-provided schedule fractions.

**Impact**: Users providing a custom HPXML file with building-specific occupancy schedule fractions in `<extension>` elements will get incorrect results -- the occupancy schedule will not reflect their custom weekday/weekend/multiplier data. The occupancy timeseries will default to whatever column is present in the external schedule CSV file. If the schedule CSV lacks an "occupants" column and no synthetic default applies, the dwelling constructor hard-errors at `dwelling/mod.rs:1081-1089` with "Occupancy spec configured but no occupancy column found in schedule."

---

### Finding 2: [Severity: medium]
**Description**: The occupant gain per-person values use OCHRE's 400 BTU/h convention (66.0 W sensible, 51.2 W latent) rather than ASHRAE Standard 55's typical values for seated activity (~70 W sensible, ~45 W latent). The total is comparable (117.2 W vs ~115 W), but the sensible/latent split differs meaningfully: HARES/OCHRE uses 56%/44% sensible/latent, while ASHRAE 55 uses approximately 61%/39% for seated activity. There is no metabolic rate scaling factor to adjust gains for different activity levels (sleeping, light activity, moderate activity) across the day.

**Code Location**:
- `crates/hares-physics/src/constants.rs:180-186` (`OCCUPANT_SENSIBLE_GAIN_W = 66.0`, `OCCUPANT_LATENT_GAIN_W = 51.2`)
- `crates/hares-core/src/dwelling/mod.rs:2112-2114` (gain application)

**Root Cause**: The constants are derived from OCHRE's `Gain per Occupant (W) = convert(400, "Btu/hour", "W")` (Envelope.py:904), which is a common residential simulation convention. The OCHRE derivation (`Convective Gain Fraction (-) = 0.563`, `Latent Gain Fraction (-) = 0.437`) targets a 400 BTU/h total that corresponds to ~117.2 W. ASHRAE 55 Table 5.2.1.2 specifies typical metabolic rates of 1.0-1.2 met for seated/light activity, yielding ~70 W sensible and ~45 W latent.

**Impact**: The 4 W difference in sensible gain (66 vs 70) represents ~6% lower sensible cooling load per occupant. For a 4-person household this is a 16 W sensible deficit, which cumulatively affects HVAC sizing and annual energy predictions. The 6.2 W higher latent (51.2 vs 45) overestimates dehumidification load. Neither OCHRE nor HARES implements time-of-day metabolic variation (sleeping vs active), which per ASHRAE 90.2 could further differentiate gains by occupancy state.

---

### Finding 3: [Severity: medium]
**Description**: The HPXML `NumberofResidents` field is the sole driver of the occupant count. When `NumberofResidents` is absent from HPXML, the Occupancy spec may not be created (if no extension schedule params are present), or it will fail construction because `number_of_occupants` is required by `dwelling/mod.rs:1061-1065`. There is no fallback to derive occupant count from the number of bedrooms as specified by ANSI/RESNET 301-2014 §4.2.2.2.1 (2 occupants for the first bedroom + 1 for each additional). OCHRE has a similar limitation (it reads `NumberofResidents` directly at `hpxml.py:822`), but also adjusts bedroom count *from* occupants (`hpxml.py:791-800`) for appliance energy calculations -- a bidirectional link HARES does not match.

**Code Location**:
- `crates/hares-io/src/hpxml/resolve_loads.rs:47-48` (`NumberofResidents` read)
- `crates/hares-core/src/dwelling/mod.rs:1058-1072` (occupancy_scale from spec)

**Root Cause**: The HPXML parser reads `NumberofResidents` but has no fallback logic. The `NumberofBedrooms` is available in the same XML subtree but is not used to impute occupant count.

**Impact**: HPXML files that omit `NumberofResidents` (which is optional per HPXML schema) will either fail to create an Occupancy spec or fail at dwelling construction with a hard error. For compliant but minimal HPXML files, this prevents simulation.

---

### Finding 4: [Severity: low]
**Description**: The default occupancy schedule fractions (from ANSI/RESNET/ICC 301-2022 Addendum C Table C.3(5)) have identical weekday and weekend fractions in both the HARES and OCHRE default schedule CSV files. While this is faithful to the standard reference, it means the default occupancy model has no behavioral weekday/weekend differentiation (the numerical fractions are literally the same 24-value array for both). The schedule does exhibit appropriate morning (6-7 AM, fraction 0.082) and evening (17-18 + 19-20 PM, fractions 0.068-0.082 each) peaks, consistent with typical residential occupancy patterns.

The `parse_schedule_extension_params` function in `resolve_loads.rs:888-943` correctly supports distinct weekday and weekend fraction arrays from HPXML extensions, and `schedule_resolve.rs:730-779` correctly builds `DefaultScheduleProfile` structs that apply a 5:2 weekday/weekend weighting via `annual_mean_fraction`. However, as noted in Finding 1, this infrastructure is unused for occupancy.

**Code Location**:
- `defaults/Default Schedule Parameters.csv:2-4` (identical weekday/weekend fractions for occupants)
- `crates/hares-io/src/schedule_resolve.rs:341-356` (annual_mean_fraction with 5:2 weighting)

**Root Cause**: The ANSI/RESNET/ICC 301-2022 standard provides the same fractions for both weekday and weekend occupancy. This is a standard-design choice, not a HARES implementation bug.

**Impact**: Minimal for standard compliance. Only matters when using the default profile to generate occupancy schedules (which Finding 1 indicates doesn't happen anyway, as the occupancy profile is never applied through schedule resolution).

---

### Finding 5: [Severity: low]
**Description**: Fractional occupancy is correctly supported. The schedule column provides a 0-1 fraction that is multiplied by `occupancy_scale` (total `number_of_occupants`) to produce a floating-point person count at each timestep. The resulting gain calculation (`n_occupants × OCCUPANT_SENSIBLE_GAIN_W`) handles fractional occupant counts correctly, enabling partial-occupancy hours (e.g., 0.5 effective persons). This is validated by the test at `dwelling/mod.rs:5684-5716` (`occupancy_gains_scaled_by_number_of_occupants`).

The `apply_occupancy_gains` method at `dwelling/mod.rs:2085-2130` correctly deposits gains only into the conditioned (indoor) zone, matching OCHRE's behavior where occupant heat is applied solely to `indoor_zone`. The gain is split into convective sensible (70%), radiative sensible (30%), and latent (100% convective moisture) components per ASHRAE HoF 2021 Ch.18 Table 1 recommendations.

**Code Location**:
- `crates/hares-core/src/dwelling/mod.rs:2093-2106` (fractional occupancy scaling)
- `crates/hares-core/src/dwelling/mod.rs:2112-2114` (gain calculation)
- `crates/hares-physics/src/constants.rs:188-198` (convective/radiative split)

**Root Cause**: N/A -- this is correct behavior.

**Impact**: N/A -- confirmation of correct behavior.

---

### Finding 6: [Severity: low]
**Description**: The `Occupant` actor in `occupant.rs` models behavioral control (turning equipment on/off based on presence) but does **not** contribute to thermal zone gains. The actor has a `Presence` enum (`Home`, `Away`, `Sleeping`) and dispatches equipment control signals but has no connection to the heat gain model in `apply_occupancy_gains`. The thermal occupancy gains are completely independent of the behavioral actor model. While this is by design (separation of concerns), it means the `Presence::Sleeping` state has no thermal implication (no reduced metabolic rate), and occupancy diversity for internal gains is driven purely by the schedule fraction -- not by actor state.

**Code Location**:
- `crates/hares-core/src/actors/occupant.rs:40-60` (Presence enum)
- `crates/hares-core/src/dwelling/mod.rs:2085-2130` (apply_occupancy_gains)

**Root Cause**: Architectural separation between behavioral modeling (actors) and thermal modeling (dwelling gains). The thermal model reads from the schedule domain directly, not from actor state.

**Impact**: The `Presence::Sleeping` state could theoretically correspond to a lower metabolic rate (~0.7 met vs 1.0-1.2 met for seated), but this is not modeled. This is a future enhancement opportunity, not a defect.

---

## Summary
- Total findings: 6
- Critical: 0
- High: 1 (HPXML occupancy schedule fractions silently dropped)
- Medium: 2 (gain constants differ from ASHRAE 55, no occupant count fallback from bedrooms)
- Low: 3 (default schedules lack weekday/weekend differentiation, fractional occupancy confirmed correct, actor/thermal separation)

## Recommendations

1. **Wire HPXML occupancy schedule fractions into schedule generation** (addresses Finding 1). Add a code path in `schedule_resolve.rs` or a new resolver that converts HPXML-parsed `weekday_schedule_fractions` / `weekend_schedule_fractions` / `month_multipliers` into the occupancy column in the schedule timeseries, mirroring OCHRE's `create_simple_schedule` logic. When CSV column "occupants" is present, prefer it; when absent, fall back to HPXML extension fractions; when neither exists, fall back to the default schedule profile for "Occupancy" from `Default Schedule Parameters.csv`.

2. **Add HPXML `NumberofResidents` fallback from `NumberofBedrooms`** (addresses Finding 3). Per ANSI/RESNET 301-2014 §4.2.2.2.1, derive `number_of_occupants = NumberofBedrooms + 1` when `NumberofResidents` is absent. This matches the HERS reference home convention.

3. **Document or reconcile occupant gain constants** (addresses Finding 2). Either adopt ASHRAE 55 values (70 W sensible, 45 W latent for seated activity) or explicitly document that the 66/51.2 W split is aligned with OCHRE's 400 BTU/h convention. Consider adding configurable metabolic rate scale factor for future sleeping/active differentiation.

4. **Consider tying actor `Presence` state to metabolic rate** (addresses Finding 6). A future enhancement could allow the `apply_occupancy_gains` method to accept an optional metabolic rate multiplier based on the current presence state (e.g., Sleeping = 0.7×, Away = 0.0× as already handled by schedule fraction).

## References / Citations

- ANSI/RESNET/ICC 301-2022 Addendum C, Table C.3(5) -- Default occupancy schedule fractions
- ANSI/RESNET 301-2014 §4.2.2.2.1 -- Occupant count from bedrooms
- ASHRAE Handbook of Fundamentals 2021, Chapter 18, Table 1 -- Radiative/convective split (~30%/70% for residential occupancy)
- ASHRAE Standard 55-2020, Table 5.2.1.2 -- Metabolic rates for typical activities
- OCHRE `Envelope.py:904-908` -- Occupancy gain derivation (400 BTU/h, 0.563/0.437 split)
- OCHRE `schedule.py:372-470` -- Occupancy schedule generation from HPXML fractions and defaults
- OCHRE `hpxml.py:818-824` -- HPXML occupancy parsing
