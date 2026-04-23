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
