# Weather Pipeline

How HARES ingests weather data, computes derived quantities, and delivers
per-timestep environmental state to equipment and solvers.

---

## Module Layout

```
hares-io/src/
├── epw.rs              EPW file parser, sky temp, ground temp models
└── weather.rs          WeatherTimeSeries storage, field access, resampling

hares-core/src/
└── environment.rs      EnvironmentManager: offset alignment, per-timestep state

hares-physics/src/
├── solar.rs            Solar position (Spencer 1971), Perez tilted irradiance
├── psychrometrics.rs   Humidity ratio, wet-bulb, enthalpy, saturation pressure
└── water_mains.rs      Burch-Christensen mains water temperature model

hares-types/src/
└── environment.rs      WeatherState, SurfaceIrradiance, ZoneState
```

---

## EPW Parsing

**Entry point**: `hares-io::epw::parse()`

### Fields Extracted

| EPW Field               | Stored As             | Units   | Validation Range |
|-------------------------|-----------------------|---------|------------------|
| Dry-bulb temperature    | `dry_bulb_c`          | °C      | [−60, 55]        |
| Dew-point temperature   | `dew_point_c`         | °C      | ≤ dry-bulb       |
| Relative humidity       | `rel_humidity_pct`    | %       | [0, 100]         |
| Barometric pressure     | `pressure_kpa`        | kPa     | [60, 110]        |
| GHI                     | `ghi_w_m2`            | W/m²    | [0, 1500]        |
| DNI                     | `dni_w_m2`            | W/m²    | ≥ 0              |
| DHI                     | `dhi_w_m2`            | W/m²    | ≥ 0              |
| Wind speed              | `wind_speed_m_s`      | m/s     | [0, 60]          |
| Wind direction          | `wind_dir_deg`        | °       | [0, 360)         |
| Opaque sky cover        | `opaque_sky_cover`    | 0–10    | [0, 10]          |
| Horizontal infrared     | `horizontal_infrared_w_m2` | W/m² | [0, 700]    |
| Liquid precipitation    | `liquid_precip_m`     | m       | ≥ 0              |

EPW pressure field is in Pa; converted to kPa at parse time. Precipitation is
mm → m. Record count must be exactly 8760 (standard year) or 8784 (leap year).

### Computed at Parse Time

**Sky temperature** — derived from horizontal infrared radiation:
- Primary: Stefan-Boltzmann inversion: `T_sky_K = (IR / σ)^0.25`
- Fallback (IR < 50 W/m²): Clark-Allen empirical model:
  `T_sky_K = T_db_K × [0.787 + 0.764 × ln(T_dp_K / 273.15)]^0.25`

**Ground temperature** — hybrid approach:
- Preferred: Monthly values from EPW header (GROUND TEMPERATURES), interpolated
  to each hour
- Fallback: DOE-2 sinusoidal damped model from monthly dry-bulb averages:
  `T_ground(day) = T_avg − ΔT × gm × cos(2π·day/365 − 0.6 − atan(z))`
- Default: 10.0°C constant if all else fails

---

## Storage Format

`WeatherTimeSeries` stores data column-major — one `Vec<f64>` per field.
Access is O(1) by field and timestep index:

```rust
let temp = series.get(WeatherField::DryBulbC, timestep_idx);
```

All fields are hourly after parse, starting Jan 1 00:00 (EPW hour-ending
convention: hour 1 = 00:00–01:00 LST).

---

## Sub-Hourly Resampling

`WeatherTimeSeries::resample(target_step_secs)` converts hourly data to
sub-hourly resolution. Target step must divide 3600 evenly.

| Field Type                   | Algorithm     | Rationale                              |
|------------------------------|---------------|----------------------------------------|
| Continuous instantaneous     | PCHIP         | Smooth C¹ curves, no overshoot         |
| (dry-bulb, dew-point, pressure, infrared, sky_temp, ground_temp) | | |
| Bounded continuous           | PCHIP + clamp | Preserve physical bounds after interp  |
| (relative humidity [0–100%], sky cover [0–10]) | | |
| Period-average energy flux   | Zero-order hold | Preserve integrated energy            |
| (GHI, DNI, DHI)             |               |                                        |
| Turbulent/stochastic         | Zero-order hold | Sub-hourly interpolation meaningless  |
| (wind speed, wind direction) |               |                                        |
| Accumulated depth            | Distribute    | `depth / factor` preserves total       |
| (precipitation)              |               |                                        |

**PCHIP**: Piecewise Cubic Hermite Interpolating Polynomial with Fritsch-Carlson
monotonicity preservation (1980). Guarantees C¹ continuity, passes through
original knots, and prevents overshoot in monotone regions.

---

## Timestep Alignment

### EPW Midpoint Shift

EPW data uses hour-ending convention. A 30-minute (1800 s) midpoint shift is
applied to align weather values with the center of each period, matching
pvlib/OCHRE convention. This ensures solar position is computed at the period
midpoint rather than the end.

### Offset Computation

For mid-year simulation starts, the annual offset into the weather series is:

```
seconds_into_year = ordinal0 × 86400 + h × 3600 + m × 60 + s   (ordinal0: 0 = Jan 1)
shifted = (seconds_into_year + year_secs − 1800) % year_secs
offset = shifted / step_secs
```

Weather wraps annually via modular indexing:
`idx = (current_step + weather_start_offset) % weather.len()`

Schedule offset uses the same pattern without the midpoint shift.

---

## Per-Timestep State Construction

`EnvironmentManager::update()` produces a fresh `WeatherState` each timestep
in five sequential steps.

### Step 1: Weather Lookup + Psychrometrics

Direct index into `WeatherTimeSeries` for all stored fields. Derived quantities
computed from dry-bulb, dew-point, and pressure:

**Humidity ratio** (from dew-point):
```
w = (M_v / M_a) × p_ws(T_dp) / (P − p_ws(T_dp))
```
where `p_ws` is ASHRAE saturation pressure (piecewise polynomial, split at 0°C).

**Wet-bulb** — bisection solve on [T_dp, T_db]:
- Convergence: ±0.01°C bracket, ±1e-7 humidity ratio tolerance
- Inverts `humidity_ratio_from_twb(T_db, T_wb, P)` iteratively

**Enthalpy** (moist air):
```
h = (c_pa × T_db + w × (L_v + c_pv × T_db)) × 1000  [J/kg dry air]
```
Constants: c_pa = 1.006 kJ/(kg·K), c_pv = 1.86 kJ/(kg·K), L_v = 2501.0 kJ/kg.

### Step 2: Per-Surface Solar Irradiance

**Solar position** (Spencer 1971 — Fourier declination/EOT):

1. Day-of-year fraction: `γ = 2π(day − 1 + (minutes_utc − 720)/1440) / 365`
2. Declination (3-harmonic Fourier):
   `δ = 0.006918 − 0.399912·cos(γ) + 0.070257·sin(γ) − ...` [rad]
3. Equation of time:
   `EoT = 229.18 × (0.0000075 + 0.001868·cos(γ) − 0.032077·sin(γ) + ...)` [min]
4. True solar time: `TST = UTC_min + EoT + 4·longitude_deg`
5. Hour angle: `h = TST × 0.25 − 180` [°, clamped to [−180, 180)]
6. Altitude: `sin(alt) = sin(lat)·sin(δ) + cos(lat)·cos(δ)·cos(h)`
7. Azimuth: north-referenced via `atan2(sin(h), cos(h)·sin(lat) − tan(δ)·cos(lat)) + π`

Accuracy: ±0.01° for altitude. Suitable for building simulation timesteps.

**Perez tilted irradiance** (Perez et al. 1990) for each building surface:

Three components:
- **Beam**: `dni × cos(AOI)` (zero-clipped when AOI > 90°)
- **Diffuse** (anisotropic 3-term decomposition):
  - Isotropic: `(1 − f₁) × (1 + cos(tilt))/2 × dhi`
  - Circumsolar: `f₁ × cos(AOI) / cos(zenith)`
  - Horizon brightening: `f₂ × sin(tilt) × dhi`
- **Ground-reflected**: `ghi × albedo × (1 − cos(tilt))/2` (isotropic, albedo = 0.2)

Sky coefficients f₁, f₂ selected from 8 clearness bins:
- Clearness: `ε = ((dhi + dni)/dhi + 1.041·θ_z³) / (1 + 1.041·θ_z³)` (θ_z in radians)
- Brightness: `Δ = dhi × airmass / I₀`

**Fallback to isotropic** (Liu & Jordan 1963) when:
- Zenith > 87° (Perez singular near horizon)
- DHI < 1.0 W/m² (insufficient diffuse signal)

Output: `Vec<SurfaceIrradiance>` with `direct_w_m2`, `diffuse_w_m2`,
`reflected_w_m2`, `angle_of_incidence_rad` per surface.

### Step 3: Schedule Values

Index into schedule columns at `(current_step + schedule_offset) % len`.
All columns extracted for the current row.

### Step 4: Zone State Feedback

Zone temperatures and humidity ratios from the previous timestep's solver
output are fed back into `EnvironmentState.zones`.

### Step 5: Grid Defaults

Voltage (per-unit) and frequency (Hz) with optional override.

---

## Mains Water Temperature

**Model**: Burch-Christensen (2007), calibrated for continental US. Used by
EnergyPlus and OCHRE.

**Inputs**: Annual average outdoor dry-bulb, peak-to-peak monthly range (full
swing), day of year, hemisphere.

```
T_mains_F = T_avg_F + 6 + ratio × (ΔT_F / 2) × sin(angle)

ratio = clamp(0.4 + 0.01 × (T_avg_F − 44), 0, 1)
lag = 35 − 1.0 × (T_avg_F − 44)                      [days]
angle = 0.986 × (day − 15 − lag) + sign × 90°         [degrees]
```

`sign = −1` for Northern hemisphere (summer peak), `+1` for Southern.
The +6°F offset accounts for ground buffering above outdoor average.

---

## Planned Improvements (WEATHER Tickets)

### WEATHER-001: PCHIP Sub-Hourly Interpolation

Implement Fritsch-Carlson monotonicity-preserving PCHIP to replace zero-order
hold for continuous weather fields. Defensive clamping for bounded fields.
ZOH preserved for solar/wind; distribution for precipitation.

### WEATHER-002: Interpolation Quality Tests

Comprehensive test coverage for PCHIP at 60s, 300s, 900s timesteps: knot
pass-through, C¹ continuity, monotonicity, physical bounds, edge cases, NaN
propagation, year boundary behavior.

### WEATHER-003: PSM3/NSRDB Parser

Native support for NREL PSM3 CSV format (5/15/30/60-minute solar data).
Extract shared helpers (Clark-Allen sky temp, DOE-2 ground temp) from `epw.rs`
to `pub(crate)`. Add `source_step_secs` to `WeatherMeta`. Rewrite
`resample()` to handle non-hourly source data (upsampling + downsampling).

### WEATHER-004: PSM3 Parser Tests

Test coverage for PSM3 parsing, unit conversions, resolution detection,
resampling round-trips, and validation of out-of-range values.

### WEATHER-005: Unified Weather Dispatch

Single `parse_weather()` entry point with automatic format detection:
- `.epw` → EPW parser
- `.csv` → header sniffing for PSM3 (`Source`, `Year,Month,Day,...,GHI,DNI,DHI`)
- `WeatherFormat` enum and `detect_weather_format()` function

### WEATHER-006: DST-Aware Schedule Indexing

Add optional `chrono-tz` dependency behind `dst` cargo feature. Schedule
indexing converts simulation time to civil time for DST-aware lookup (spring
forward skips, fall back repeats). Weather indexing stays on fixed UTC offset
(physically correct).

### Dependency Chain

```
WEATHER-001 (PCHIP interpolation) ──→ WEATHER-002 (interpolation tests)
                                        ↓
WEATHER-003 (PSM3 parser) ──→ WEATHER-004 (PSM3 tests) ──→ WEATHER-005 (unified dispatch)

WEATHER-006 (DST schedules)  [independent]
```
