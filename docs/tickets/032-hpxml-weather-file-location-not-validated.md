# HPXML Weather File Reference: Site Location Not Cross-Validated Against File Metadata

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml, hares-core/environment
**Weather-cluster note**: This ticket belongs adjacent to the 024–034 weather pipeline cluster. Do not move — renumber when the cluster is sequenced.

## Problem

When an HPXML file specifies a weather file via `<ClimateandRiskHazards>/<WeatherStation>/<WMOStationNumber>` or a file path, and that file is EPW or PSM3 with embedded latitude/longitude, HARES does not verify that the HPXML site location matches the weather file. Three distinct gaps:

### Gap 1: Caller-provided coordinates silently discarded for EPW/PSM3

`parse_weather_with_location` at `hares-io/src/weather.rs:190` accepts `elevation_m`, `latitude`, `longitude`, `timezone_offset_h` but ignores them for EPW and PSM3 (`weather.rs:199–200`):

```rust
WeatherFormat::Epw => crate::epw::parse_epw(path),
WeatherFormat::Psm3 => crate::psm3::parse_psm3(path),
```

If the HPXML file says the building is in Denver but the EPW covers Phoenix, no error or warning fires. Per project policy `feedback_no_silent_defaults.md`: silently discarding caller-provided coordinates is prohibited.

### Gap 2: Solar position computed from weather file's embedded location

`EnvironmentManager` receives a pre-parsed `WeatherTimeSeries` whose `meta.latitude`/`meta.longitude` may differ from the HPXML site coordinates. Solar position is computed from the weather file's location, not the building's. For ResStock CSV (no embedded location), the caller-supplied coordinates are used correctly; EPW/PSM3 do not benefit from the caller's coordinates.

### Gap 3: No timezone consistency check

There is no validation that the weather file's `timezone_offset_h` matches the `start_time` offset. If `start_time` is constructed from a DST-adjusted wall-clock time, solar position will be computed one hour off. Project policy `feedback_local_time_only.md`: all internal times are local to the building site.

## Current Behavior

`hares-io/src/weather.rs:199–200`: EPW and PSM3 paths ignore caller-provided lat/lon/elevation/timezone.

## Required Behavior

1. **Coordinate mismatch warning**: In `parse_weather_with_location`, after the format-match block resolves an EPW or PSM3 result, compare `meta.latitude` against caller-provided `latitude` and `meta.longitude` against caller-provided `longitude`. When either absolute difference exceeds 1.0°, emit `tracing::warn!` with both coordinate pairs. When caller-provided coordinates are all zero (no HPXML site override), skip the check. Do not error — the weather file's embedded location is authoritative if the caller does not supply one.

2. **Override function**: Provide `parse_weather_override_location(path, latitude, longitude, elevation_m, timezone_offset_h)` that parses the file and then replaces `meta.latitude`, `meta.longitude`, `meta.elevation_m`, and `meta.timezone_offset_h` with the caller-supplied values. For cases where the file's embedded metadata is known wrong.

3. **Timezone consistency check**: In `EnvironmentManager::new` (or its construction path), validate that `start_time`'s local offset matches `weather.meta.timezone_offset_h × 3600` within 1 800 s (0.5 h). Emit `tracing::warn!` when they differ; do not error (synthetic simulations may intentionally mismatch).

Do not silently discard coordinates or time offsets without a diagnostic. Do not add a shim that accepts mismatched coordinates without emitting a warning.

References:
- HPXML v4.2 specification §4.3 "Site" — latitude, longitude, timezone fields
- EnergyPlus Input-Output Reference §"Site:Location" — location must match the weather data file
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.1 — weather data must represent the site's actual climate
- Project policy `feedback_no_silent_defaults.md`; `feedback_local_time_only.md`

## Approach

In `parse_weather_with_location` (`weather.rs:190`): after the `match detect_weather_format(path)?` resolves to an EPW or PSM3 parsed result, add coordinate comparison using the returned `meta`. Emit `tracing::warn!` when triggered. The ResStock CSV path already uses caller-provided coordinates; no change required there.

## Definition of Done

- [ ] `parse_weather_with_location` emits `tracing::warn!` when EPW/PSM3 embedded location differs > 1.0° from non-zero caller-provided coordinates
- [ ] `parse_weather_override_location` function exists; replaces all four `meta` fields with caller-supplied values
- [ ] `EnvironmentManager::new` emits `tracing::warn!` when `start_time` offset differs from `weather.meta.timezone_offset_h × 3600` by more than 1 800 s
- [ ] Test: EPW fixture with mismatched caller coordinates (> 1°) → warning fires
- [ ] Test: `parse_weather_override_location` → `meta` fields match caller-provided values exactly
- [ ] `cargo test -p hares-io` and `cargo test -p hares-core` pass

## Verification

```bash
cargo test -p hares-io
cargo test -p hares-core weather_integration
```

## References

- HPXML v4.2 specification §4.3 "Site" — latitude, longitude, timezone fields
- EnergyPlus Input-Output Reference §"Site:Location" — location must match the weather data file
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.1 — weather data must represent the site's actual climate
- Project policy `feedback_no_silent_defaults.md` — never silently discard caller-provided inputs
- Project policy `feedback_local_time_only.md` — all internal times are local to the building site

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match — `parse_weather_with_location` at `hares-io/src/weather.rs:190–210`; EPW/PSM3 arms at lines 199–200 exactly as described.
- [x] Described logic matches current implementation — EPW and PSM3 arms call `parse_epw(path)` / `parse_psm3(path)` with no caller parameters; `elevation_m`, `latitude`, `longitude`, `timezone_offset_h` are silently ignored for these formats. TMY3 (line 201) is in the same position — also ignores caller coordinates, though the ticket does not mention this format.
- [x] OCHRE cross-check: **diverges — OCHRE does check station identity.** In `vendors/OCHRE/ochre/utils/schedule.py:125–130`, `import_weather()` compares the HPXML `weather_station` name against the weather file path and prints a `WARNING` string when they differ. For EPW files (line 169) OCHRE reads location from the file via `pvlib.iotools.read_epw()` — it does not cross-validate numeric coordinates from the HPXML `Site` element against EPW metadata. For the `weather_metadata` (custom CSV) path (lines 160–165), OCHRE *requires* `latitude` and `longitude` to be present or raises `OCHREException`. HARES does not perform the station-name check or the lat/lon validation.
- [x] EnergyPlus cross-check: **EnergyPlus emits a warning; HARES does not.** See quoted passage below.

### Web-Verified Citations

**Citation 1**: EnergyPlus Input-Output Reference §"Site:Location" — location must match the weather data file

- **Source found**: Big Ladder Software EnergyPlus 9.6 Input-Output Reference — Group: Location – Climate – Weather File Access ([https://bigladdersoftware.com/epx/docs/9-6/input-output-reference/group-location-climate-weather-file-access.html](https://bigladdersoftware.com/epx/docs/9-6/input-output-reference/group-location-climate-weather-file-access.html)); EnergyPlus 8.4 Tips and Tricks — Example Error Messages ([https://bigladdersoftware.com/epx/docs/8-4/tips-and-tricks-using-energyplus/example-error-messages-from-module-getinput.html](https://bigladdersoftware.com/epx/docs/8-4/tips-and-tricks-using-energyplus/example-error-messages-from-module-getinput.html))
- **Quoted passages**:
  - Site:Location object description: *"Weather data file location, if it exists, will override any location data in the IDF. Thus, for an annual simulation, a Location does not need to be entered."*
  - EnergyPlus warning emitted when coordinates differ: *"Weather file location will be used rather than entered Location object. ..Location object = ATLANTA ..Weather File Location = Tampa International Ap FL USA TMY3 WMO# = 722110 ..due to location differences, Latitude difference = [5.68] degrees, Longitude difference = [1.89] degrees. ..Time Zone difference = [0.0] hour(s), Elevation difference = [98.10] percent, [309.00] meters."*
  - This warning fires even for sub-1° differences (confirmed via Unmet Hours community post): a Barcelona example showed a warning at 0.10° latitude / 0.10° longitude difference.
- **Verdict**: **Partially correct.** The ticket correctly states that EnergyPlus has a `Site:Location` concept requiring the location to match the weather file, and that EnergyPlus emits a warning for mismatches. However, the ticket's framing ("location must match the weather data file") overstates EnergyPlus's requirement — EnergyPlus's actual stance is that the weather file's embedded location *overrides* the IDF location object and a warning is issued. EnergyPlus does not error; it warns. The ticket's proposed behavior (warn, not error) is fully consistent with EnergyPlus precedent.

**Citation 2**: HPXML v4.2 specification §4.3 "Site" — latitude, longitude, timezone fields

- **Source found**: HPXML v4.2 Data Dictionary at [https://hpxml.nlr.gov/datadictionary/4.2.0/Building/Site/GeoLocation/Latitude](https://hpxml.nlr.gov/datadictionary/4.2.0/Building/Site/GeoLocation/Latitude)
- **Quoted passage**: *"[deg] North-south position of a point on the Earth's surface. Use negative values for southern hemisphere."* Data type: xs:double; Min Inclusive: −90; Max Inclusive: 90; Min Occurrences: 0 (optional). Parallel fields exist for Longitude and UTCOffset. The ClimateandRiskZones/WeatherStation element (confirmed via OpenStudio-HPXML docs) specifies the EPW file by WMO station number or file path but does not cross-validate against Site/GeoLocation.
- **Verdict**: **Correct.** HPXML v4.2 does define Site latitude, longitude, and timezone fields, and the ticket accurately identifies this as the source of "building location" data that should be cross-validated. Section reference "§4.3" cannot be directly verified from the public data dictionary (which is organised by element path, not section number), but the element existence and semantics are confirmed.

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.1 — weather data must represent the site's actual climate

- **Source found**: ASHRAE.org table of contents for 2021 ASHRAE Handbook—Fundamentals ([https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals](https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals)); ASHRAE news article on Chapter 14 ([https://www.ashrae.org/news/ashraejournal/data-from-more-than-9-000-climate-stations-included-in-2021-handbook-chapter](https://www.ashrae.org/news/ashraejournal/data-from-more-than-9-000-climate-stations-included-in-2021-handbook-chapter))
- **Quoted passage**: Chapter 14 is titled *"Climatic Design Information"* and is under the "LOAD AND ENERGY CALCULATIONS" section. The 2021 edition covers 9,237+ climate stations and states it is concerned with "identification, analysis and tabulation of climatic data for use in analysis and design of heating, refrigeration, ventilation and air-conditioning systems."
- **Verdict**: **Plausible but not fully verifiable.** Chapter 14 of the 2021 ASHRAE Handbook—Fundamentals is confirmed to be about climatic design information. The cited principle — that weather data must represent the site's actual climate — is a reasonable reading of the chapter's stated purpose. However, the specific subsection §14.1 and the exact quoted language cannot be independently confirmed from public sources; the handbook is paywalled. The principle cited is sound engineering practice but the specific citation cannot be verified verbatim.

**Citation 4**: Project policy `feedback_no_silent_defaults.md`; `feedback_local_time_only.md`

- **Source found**: Neither file exists on disk at any path under `/Users/rich/source/HARES/`. Searched via `grep -r feedback_no_silent_defaults`. References to the policy appear in 26 other ticket files and review documents (e.g., `docs/tickets/031-epw-liquid-precip-silent-zero.md`, `docs/findings/reviews/02_rc_envelope_solver.md`) and in project conversation history, confirming the policies are real working norms for this project even though no dedicated policy markdown files have been created yet.
- **Verdict**: **Policy exists in practice but files are missing.** The policy names are real and consistently applied across the ticket corpus. The absence of the actual markdown files means future tickets cannot `cat` them for reference, but the policy's meaning is clear and well-evidenced.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: All three code gaps described in the ticket are confirmed present in the current implementation. Gap 1 (caller coordinates silently discarded for EPW/PSM3) is verified at `weather.rs:199–200`; the regression test (run with `--include-ignored`) fails with *"ticket-032: parse_weather_with_location silently discarded caller latitude 33.45; got file latitude 39.74 instead"*, proving the bug is live. Gap 2 (solar position uses weather file location) is a direct downstream consequence of Gap 1 — the `EnvironmentManager` (`environment.rs:196–202`) reads `weather.meta.latitude` / `meta.longitude` for solar calculations, so mismatched coordinates propagate unchanged into `solar_position()`. Gap 3 (no timezone consistency check) is also confirmed absent: `EnvironmentManager::new_with_resample` computes `weather_start_offset` from `start_time` directly without comparing `start_time`'s UTC offset to `weather.meta.timezone_offset_h`. The EnergyPlus precedent confirms a warn-not-error approach is correct (EnergyPlus has done this since at least v8.0). OCHRE performs a station-name identity check but not a numeric coordinate comparison, so HARES diverges from both reference implementations in the direction of less diagnostic coverage. The TMY3 format also silently ignores caller coordinates (same `match` arm pattern) but is not mentioned in the ticket — a minor scope gap that does not affect legitimacy.

### Proposed Fix Summary

In `parse_weather_with_location` (`weather.rs:190`), after the format-dispatch `match` block resolves an EPW, PSM3, or TMY3 result, add coordinate comparison: when `latitude.abs() + longitude.abs() > 0.0` (caller provided real coords, not the default zero-sentinel), and `(result.meta.latitude - latitude).abs() > 1.0 || (result.meta.longitude - longitude).abs() > 1.0`, emit `tracing::warn!` with both coordinate pairs. Do not modify `meta`; the file remains authoritative unless the caller explicitly uses `parse_weather_override_location`. Additionally, add `parse_weather_override_location(path, lat, lon, elev, tz)` that parses and then overwrites the four `meta` fields. In `EnvironmentManager::new_with_resample`, after the `weather.meta.clone()` at line 196, compare `start_time.offset().local_minus_utc()` against `(weather_meta.timezone_offset_h * 3600.0) as i32` and emit `tracing::warn!` when they differ by more than 1800 s.

### Test Written

- **File**: `crates/hares-io/src/epw.rs` (within `#[cfg(test)] mod tests`, at end of file)
- **Test name**: `epw::tests::epw_caller_coordinates_not_silently_discarded`
- **Marked**: `#[ignore]` — runs only with `--include-ignored`; will not break CI until the fix is implemented
- **What it tests**: Calls `parse_weather_with_location` with a synthetic Denver EPW (lat 39.74°) and Phoenix caller coordinates (lat 33.45°, Δ ≈ 6.3°). Asserts that after the fix, `meta.latitude` equals the caller-supplied 33.45 — currently fails because the function returns 39.74 (file value), proving caller coordinates are silently discarded.
