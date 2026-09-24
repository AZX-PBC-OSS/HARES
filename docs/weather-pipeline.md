# Weather Pipeline

How HARES ingests weather data, computes derived quantities, and delivers
per-timestep environmental state to equipment and solvers.

---

## Module Layout

```
hares-io/src/
├── epw.rs              EPW file parser, sky temp, ground temp models
├── psm3.rs             NREL PSM3/NSRDB CSV parser (5/15/30/60-min native solar)
└── weather.rs          WeatherTimeSeries, WeatherFormat dispatch, PCHIP resampling

hares-core/src/
└── environment.rs      EnvironmentManager: offset alignment, per-timestep state, DST

hares-physics/src/
├── solar.rs            Solar position (Spencer 1971), Perez tilted irradiance
├── psychrometrics.rs   Humidity ratio, wet-bulb, enthalpy, saturation pressure
└── water_mains.rs      Burch-Christensen mains water temperature model

hares-types/src/
└── environment.rs      WeatherState, SurfaceIrradiance, ZoneState
```

---

## File Format Support

HARES supports multiple weather file formats via a unified dispatch layer.

### Unified Dispatch

**Entry point**: `hares-io::weather::parse_weather(path)` — auto-detects format
and dispatches to the appropriate parser.

Detection logic (`detect_weather_format()`):
- `.epw` extension → EPW parser
- `.csv` extension → header sniffing:
  - Line 1 starts with `Source` + ≥10 fields, line 3 has temporal + solar columns → **PSM3**
  - Header contains `Dry Bulb Temperature` + `Global Horizontal Radiation` → **ResStock CSV**
- Other extensions → descriptive error

```rust
pub enum WeatherFormat {
    Epw,          // EnergyPlus Weather
    Psm3,         // NREL NSRDB PSM3 (SAM format)
    ResStockCsv,  // ResStock simplified CSV (AMY 2018, etc.)
}
```

Dwelling construction uses the unified entry point: `parse_weather(&config.weather_path)`.

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

## PSM3 Parsing

**Entry point**: `hares-io::psm3::parse_psm3()`

Parses NREL's Physical Solar Model v3 (PSM3/NSRDB) CSV format, which provides
native sub-hourly solar data at 5, 15, 30, or 60-minute intervals — eliminating
interpolation artifacts for solar fields.

### Key Differences from EPW

| Aspect               | EPW                    | PSM3                           |
|----------------------|------------------------|--------------------------------|
| Native resolution    | Hourly (3600 s)        | 5/15/30/60 min (auto-detected) |
| Solar data           | Period-average          | Period-average at native res   |
| Infrared radiation   | Measured (field 12)     | Not available                  |
| Sky temperature      | Stefan-Boltzmann from IR| Clark-Allen empirical model    |
| Ground temperature   | EPW header or DOE-2     | DOE-2 from monthly dry-bulb   |
| Pressure units       | Pa → kPa               | mbar → kPa (÷10)              |
| Surface albedo       | Not in EPW              | Available per-timestep         |

Record counts validated for the detected interval (e.g., 105120 records for
5-minute data in a standard year).

### Shared Helpers

Sky temperature and ground temperature models are `pub(crate)` functions in
`epw.rs`, shared by both parsers:
- `clark_allen_sky_temp_c()` — empirical sky temp from dry-bulb and dew-point
- `doe2_ground_temp_monthly()` — DOE-2 sinusoidal model from hourly dry-bulb
- `doe2_ground_temp_from_monthly_avg()` — DOE-2 from pre-computed monthly means
  (used by PSM3 via `compute_monthly_means_sub_hourly()`)

### WeatherMeta

Both parsers produce `WeatherMeta` with a `source_step_secs` field indicating
the native timestep (3600 for EPW, 300/900/1800/3600 for PSM3). This controls
resampling behavior — native-resolution data is not resampled unnecessarily.

---

## Storage Format

`WeatherTimeSeries` stores data column-major — one `Vec<f64>` per field.
Access is O(1) by field and timestep index:

```rust
let temp = series.get(WeatherField::DryBulbC, timestep_idx);
```

After EPW parse, fields are hourly (8760/8784 entries). PSM3 data retains its
native resolution. Both use the same column-major structure. Resampling (see
below) converts to the simulation timestep before the run begins.

---

## Sub-Hourly Resampling

`WeatherTimeSeries::resample(target_step_secs)` converts source data to the
simulation timestep. Handles both upsampling (hourly EPW → 60s) and
downsampling (5-min PSM3 → 15-min). Target step must divide 3600 evenly.
No-op when source and target resolution match.

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

**PCHIP** (implemented in `fritsch_carlson_slopes()` and `pchip_resample()`):
Piecewise Cubic Hermite Interpolating Polynomial with Fritsch-Carlson
monotonicity preservation (1980). Guarantees C¹ continuity, passes through
original knots, and prevents overshoot in monotone regions. Uses Hermite basis
functions for evaluation. Linear fallback for 2-element input. Flat
extrapolation at year boundaries (no cyclic wrap). NaN propagation through
affected intervals.

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

**DST-aware indexing** (optional, behind `dst` cargo feature): When a civil
timezone is configured via `EnvironmentManager::new(civil_timezone: Some("America/New_York"))`,
schedule indexing converts simulation time to civil time using `chrono-tz` before
computing the schedule index. This correctly handles:
- Spring forward: civil time jumps, schedule index skips one hour
- Fall back: civil time repeats, schedule index reuses one hour

Weather indexing is **never** affected by DST — it stays on the fixed UTC offset,
which is physically correct for outdoor conditions. Only occupancy/rate schedules
that follow civil time use DST conversion.

When the `dst` feature is not compiled, supplying a timezone returns
`EnvironmentManagerError::DstNotEnabled`.

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

## Source Files

| File | Purpose |
|------|---------|
| `hares-io/src/epw.rs` | EPW parser, Clark-Allen sky temp, DOE-2 ground temp |
| `hares-io/src/psm3.rs` | PSM3/NSRDB parser, sub-hourly native solar |
| `hares-io/src/weather.rs` | WeatherTimeSeries, WeatherFormat dispatch, PCHIP resampling |
| `hares-core/src/environment.rs` | EnvironmentManager, offset alignment, DST schedule indexing |
| `hares-physics/src/solar.rs` | Solar position (Spencer), Perez tilted irradiance |
| `hares-physics/src/psychrometrics.rs` | Humidity ratio, wet-bulb, enthalpy |
| `hares-physics/src/water_mains.rs` | Burch-Christensen mains water temperature |
| `hares-types/src/environment.rs` | WeatherState, SurfaceIrradiance, ZoneState |
