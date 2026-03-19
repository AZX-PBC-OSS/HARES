# Comprehensive I/O, Configuration, and Output Comparison: OCHRE vs HARES

This document catalogues all identified differences in I/O, configuration, weather processing, output, and features between OCHRE and HARES. Organized by functional area.

---

## 1. HPXML Parsing Completeness

### OCHRE Features (vendors/OCHRE/ochre/utils/hpxml.py)

**Boundary/Envelope Parsing (extensive)**:
- Parses all 24 boundary types: Exterior Wall, Attic Wall, Garage Attached Wall, Garage Wall, Adjacent Wall, Foundation Wall, Adjacent Attic Wall, Attic Garage Wall, Adjacent Garage Wall, Adjacent Foundation Wall, Attic Floor, Foundation Ceiling, Garage Ceiling, Garage Interior Ceiling, Adjacent Ceiling, Adjacent Floor, Floor, Foundation Floor, Garage Floor, Raised Floor, Attic Roof, Roof, Garage Roof, Rim Joist, Adjacent Rim Joist, Window, Door, Garage Door
- Parses window properties: U-Factor, SHGC, interior shading coefficients, frame type
- Extracts solar absorptivity and emissivity for exterior surfaces
- Detailed roof properties: pitch, radiant barrier presence
- Foundation wall properties: height, insulation details
- Slab insulation details using dedicated logic
- Consolidates multiple windows/doors per wall by azimuth
- Validates all boundary parameters are consistent across grouped items

**Zone Parsing**:
- Recognizes 6 zone types: Conditioned, Attic, Garage, Foundation, Outdoor, Adjacent
- Maps HPXML zone names to OCHRE zones with fuzzy matching
- Calculates ceiling height from volume and floor area
- Extracts number of bedrooms/bathrooms
- Determines foundation type and basement condition (finished vs unfinished)
- Calculates indoor/above-grade floor counts

**Equipment-Related Parsing**:
- Extracts and maps schedules for 30+ equipment types
- Parses MEL/plug load categories with mapping (TV, Well Pump, Gas Grill, Gas Fireplace, etc.)
- Extracts heating/cooling/water heater setpoints
- Parses EV charging specifications
- Pool/spa equipment (pump, heater)
- Wet appliance schedules (clothes washer, dishwasher, clothes dryer)
- Occupancy schedule and drawing profiles

**Property Calculations**:
- Calculates infiltration parameters (ELA, SLA) using ResStock-derived coefficients
- Extracts and processes HPWH-specific UA values and efficiency (UEF-based)
- Boiler efficiency and minimum field annual efficiency (EAE)
- Water heater tank sizing and thermal properties
- Heating/cooling system capacity calculations

### HARES Features (crates/hares-io/src/hpxml/)

**Building Parsing** (building.rs):
- Parses basic building envelope: 6 zone types
- Extracts boundary information with types: Wall, Roof, Window, Door, FoundationWall, Slab
- Parses site information: elevation, site type, shielding, lat/lon
- Window extraction: area, azimuth, U-factor, SHGC, frame type, attachment
- Material layer parsing with thickness, conductivity, density, specific heat
- Duct system parsing (location inside/outside conditioned space)
- Basic zone temperature/humidity properties

**Validation** (validation.rs):
- Schema validation against HPXML 4.0 expectations
- Range checks on parsed values (e.g., floor area min/max)
- Basic domain validation

**Equipment Parsing** (equipment.rs):
- Generic equipment resolution by type
- Partial parameter extraction

### OCHRE Advantages on HPXML:

1. **Much more detailed window/door handling** — OCHRE parses and validates interior shading, frame types, and consolidates by azimuth
2. **Comprehensive MEL/plug load mapping** — OCHRE recognizes 30+ specific equipment/load categories from HPXML; HARES appears to skip or generalize many
3. **Roof construction details** — OCHRE parses pitch, radiant barrier, finish type; HARES only basic geometry
4. **Foundation specifics** — OCHRE distinguishes finished/unfinished basement, calculates basement height; HARES less detailed
5. **Equipment-dependent calculations** — OCHRE derives infiltration (ELA), boiler efficiency, water heater UA from HPXML context; HARES relies more on defaults
6. **Pool/spa equipment** — OCHRE extracts pool pump, heater; HARES does not appear to handle these
7. **Wet appliance schedules and Monte Carlo profiles** — OCHRE uses detailed WetAppliance class for clothes washer, dishwasher, dryer; HARES has EventBasedLoad but may not support same profile generation
8. **Slab insulation logic** — OCHRE has dedicated utility to parse slab insulation details; HARES minimal treatment
9. **Occupancy schedule parsing** — OCHRE maps occupancy to zone; HARES less integrated
10. **Backup field/calculation paths** — OCHRE provides fallback values for missing HPXML fields (e.g., "if NumberofBathrooms missing, estimate from bedrooms")

---

## 2. Output Metrics and Columns

### OCHRE Metrics (Analysis.py: calculate_metrics)

**Verbosity Level 0+**: Total power
- Total Electric Energy (kWh), Gas Energy (therms), Reactive Energy (kVARh) — optionally at verbosity 8

**Verbosity 1+**: Peak demand
- Average/Peak Electric Power (kW)
- 15-min, 30-min, 1-hour peak averages (verbosity 7+)

**Verbosity 2+**: Per-end-use energy
- Sums for all 14 end-use categories (HVAC Heating, HVAC Cooling, Water Heating, etc.)
- Both electric and gas per end-use

**Verbosity 3+**: HVAC metrics
- Unmet Heating/Cooling Load (C-hours)
- Total HVAC Delivered (kWh)
- Average COP (capacity / power)
- Average SHR (sensible / total)
- Average Duct Efficiency

**Verbosity 3+**: Envelope metrics
- Average temperature per zone (Indoor, Attic, Foundation, Garage)
- Std Dev temperature (verbosity 8)

**Verbosity 3+**: Water heater metrics
- Hot Water Unmet Demand (kWh)
- Water Heating Delivered (kWh)
- Average COP
- Hot Water Delivered (gal/day or kWh)

**Verbosity 4+**: EV metrics
- EV SOC (state of charge) average
- EV Unmet Load (kWh)

**Verbosity 4+**: Battery metrics
- Battery Charging/Discharging Energy (kWh)
- Round-trip Efficiency
- **Islanding Time (hours)** — calculated as max duration grid can sustain load from battery

**Verbosity 4+**: Gas generator metrics
- Gas Generator Efficiency (electric / gas input)

**Verbosity 5+**: Component load metrics
- Internal Heat Gain (kWh)
- Infiltration Heat Gain (kWh)
- Forced/Natural Ventilation Heat Gain (kWh)
- HVAC Duct Losses, Heating/Cooling (kWh)

**Verbosity 6+**: All end-use equipment power (summed into energy)

**Verbosity 7+**: Equipment cycling
- Mode transitions for each equipment
- Separate count for each mode (e.g., "HVAC Heating 'Heating' Cycles")

**Verbosity 8+**: Grid voltage and outage metrics
- Number of Outages
- Average Outage Duration (hours)
- Longest Outage Duration (hours)

**Verbosity 9+**: Kitchen-sink approach
- Sums ALL power columns into energy equivalents
- Averages ALL unitless (-) columns

### HARES Output Columns (crates/hares-io/src/output/columns.rs)

**Level 0**: Total power only
- Total Electric Power (kW)
- Total Gas Power (therms/hour)

**Level 1**: Per-equipment power
- {Equipment} Electric Power (kW)
- {Equipment} Gas Power (therms/hour) [if applicable]

**Level 2**: Zone temperatures and unmet load
- Temperature - Indoor (C)
- Unmet HVAC Load (C)

**Level 3**: Equipment state
- {Equipment} Mode (-)
- {HVAC/WH} Setpoint (C)
- {Battery/EV} SOC (-)

**Level 4**: Energy columns
- {Equipment} Energy (kWh)

**Level 5**: Reactive power
- {Equipment} Reactive Power (kVAR)
- {Equipment} Power Factor (-)

**Level 6+**: Component loads per boundary (structure exists but comment indicates dynamic generation)

### HARES Advantages on Output:

1. **Equipment-instance awareness** — HARES properly qualifies equipment instances (e.g., "Battery #1 SOC")
2. **Flexible verbosity** — HARES schema construction via `build_schema()` allows verbosity-based column inclusion

### OCHRE Advantages on Output:

1. **Vastly more derived metrics** — OCHRE calculates COP, SHR, DSE, unmet loads, islanding time, outages, cycling counts
2. **Energy conversion to common units** — OCHRE sums power over time to energy (kWh, kVARh, therms)
3. **Confidence/reliability metrics** — unmet hours, outage duration, islanding time quantify resilience
4. **Performance metrics** — COP, SHR, efficiency ratios for HVAC and battery
5. **Occupancy-relative metrics** — component loads broken by heat source
6. **Verbosity-gated metrics** — 10 distinct verbosity levels allow progressive detail
7. **Fault detection** — mode cycling, outage tracking
8. **Post-simulation analysis** — OCHRE's `calculate_metrics()` can be called on completed DataFrame; HARES appears to focus on streaming output

---

## 3. Weather Data Processing

### OCHRE Features (utils/schedule.py, psychrolib_jit.py, Analysis.py)

**Derived Weather Quantities** (calculated in schedule.py):
- Ambient Humidity Ratio (-) — from relative humidity
- Ambient Wet Bulb (C) — calculated via psychrolib
- Airmass (for solar calculations)
- Sky Temperature (C) — from GHI infrared or default
- Ground Temperature (C) — from water mains correlation or HPXML
- All transformations preserve units and validate

**Psychrometric Calculations** (psychrolib_jit.py — Numba-JIT compiled):
- Saturation vapor pressure from dry bulb
- Humidity ratio from relative humidity
- Dew point from vapor pressure
- Wet bulb from humidity ratio (uses iterative bisection search)
- Moist air enthalpy, volume, density
- Sensible Heat Ratio (SHR) calculation with coil bypass factor
- **All functions validated against ASHRAE standards**

**Weather File Support**:
- EPW (EnergyPlus Weather) files with header parsing
- Custom OCHRE-format CSV weather files
- Automatic sky temperature calculation from infrared if missing
- Ground temperature estimation using water mains temperature correlation
- Handles leap year detection and non-annual data

**Time Series Handling**:
- Automatic time resolution detection from file length (8760 hrs, 35040 qtr-hrs, etc.)
- Timezone-aware datetime indexing
- Time offset support for non-calendar-year data
- Resample capability (though not actively used in simulation)

### HARES Features (crates/hares-io/src/weather.rs, epw.rs)

**Basic Weather Fields** (WeatherTimeSeries):
- dry_bulb_c, dew_point_c, rel_humidity_pct, pressure_kpa
- ghi_w_m2, dni_w_m2, dhi_w_m2
- wind_speed_m_s, wind_dir_deg
- opaque_sky_cover, horizontal_infrared_w_m2
- sky_temp_c, ground_temp_c

**EPW Parser** (epw.rs):
- Reads EPW header metadata (location, lat/lon, elevation, timezone)
- Column-oriented storage for efficient access
- Zero-order-hold resampling to sub-hourly timesteps

**No Derived Quantities**:
- Wet bulb: NOT calculated
- Humidity ratio: NOT calculated
- Enthalpy: NOT calculated
- Apparent temperature: NOT mentioned
- Airmass: NOT calculated

### OCHRE Advantages on Weather:

1. **Wet bulb calculation** — OCHRE computes via psychrometric relations; HARES assumes it's in EPW data
2. **Humidity ratio (absolute humidity)** — OCHRE derives; HARES doesn't calculate
3. **Enthalpy calculation** — OCHRE computes moist air enthalpy for coil outlet conditions; HARES absent
4. **Airmass for solar** — OCHRE calculates for plane-of-array irradiance; HARES uses only GHI/DNI/DHI
5. **Sky temperature fallback** — OCHRE estimates from infrared when missing; HARES uses raw value
6. **Ground temperature derivation** — OCHRE can estimate from water mains; HARES requires input
7. **Psychrometric library integration** — OCHRE uses psychrolib (validated against ASHRAE); HARES minimal
8. **Time series flexibility** — OCHRE handles non-standard timesteps, leap years; HARES simpler
9. **Indoor humidity tracking** — OCHRE calculates indoor humidity ratio and wet bulb for comfort/control; HARES less integrated

---

## 4. Schedule Handling

### OCHRE Features (utils/schedule.py, Equipment.py)

**Schedule Sources**:
- ResStock-generated in.schedules.csv files (standard format)
- Custom OCHRE-format CSV schedules
- Event-based draw profiles for wet appliances (classes WetAppliance, ClothesWasher, Dishwasher, etc.)
- Monte Carlo profiles for hot water draws and appliance events

**Schedule Categories** (SCHEDULE_NAMES dict):
- **Occupancy**: occupants per zone
- **Power** (20+ types): Clothes Washer, Dryer, Dishwasher, Refrigerator, Freezer, Cooking Range, Indoor/Exterior/Basement/Garage Lighting, MELs, TV, Well Pump, Gas Grill, Fireplace, Lighting, Pool Pump, Pool Heater, Spa Pump, Spa Heater, Ceiling Fan
- **Water**: Fixtures, Clothes Washer, Dishwasher
- **Setpoint**: HVAC Heating/Cooling, Water Heating
- **Ignore list**: ExtraRefrig, dryer exhaust, exterior holiday lighting, vehicle, battery, vacancy, power outages, no-heat/cool flags

**Water Draw Profiles**:
- Detailed Monte Carlo hot water draw modeling (WetAppliance.py)
- Stochastic shower, fixture, clothes washer, dishwasher draws
- Temperature stratification in hot water tanks
- Draw profiles with duration, temperature, flow rate distributions

**Event-Based Loads**:
- EventBasedLoad: Stochastically-generated events (e.g., EV charging events, appliance turn-ons)
- Configurable event duration, power, and inter-event timing
- Delay-able loads (e.g., can delay laundry)

**Schedule Interpolation**:
- Linear interpolation for sub-hourly timesteps
- Proper handling of annual vs multi-year data
- Occupancy-scaled loads

### HARES Features (crates/hares-io/src/schedule.rs)

**Schedule CSV Format**:
- Column-major storage with normalized column name lookup
- Timestamp parsing with timezone awareness
- Required columns: Time (DatetimeIndex)
- Optional columns: any load/occupancy data

**Resampling**:
- Zero-order-hold upsampling only (no downsampling)
- Must match hourly source resolution to simulation timestep

**No Specialized Features**:
- No wet appliance draw profiles
- No Monte Carlo event generation
- No occupancy-dependent scaling
- No delay-able loads
- No stochastic features

### OCHRE Advantages on Schedules:

1. **Monte Carlo wet appliance draws** — OCHRE models individual draw events with realistic statistics; HARES loads from CSV
2. **Stochastic event generation** — OCHRE EventBasedLoad class generates synthetic charging/usage events; HARES doesn't
3. **Hot water tank simulation** — OCHRE models draw profiles, mixing, stratification; HARES abstracted
4. **Plug load diversity** — OCHRE can model 20+ distinct appliance types; HARES generic
5. **Occupancy scaling** — OCHRE scales loads by occupancy; HARES static values
6. **Delay-able loads** — OCHRE can defer certain tasks (e.g., laundry); HARES no control
7. **Power outage schedule** — OCHRE can inject grid disconnections; HARES relies on control signals
8. **Schedule ignore logic** — OCHRE skips certain columns (e.g., "battery") that shouldn't feed equipment; HARES loads all

---

## 5. Configuration Validation

### OCHRE Features (Dwelling.py, Equipment.py, hpxml.py)

**Equipment Multiplicity Checks**:
- Raises exception if multiple HVAC Heating/Cooling or Water Heater units (invalid)
- Warns if multiple units of other end-uses (allowed but suspicious)

**Time Resolution Validation**:
- Raises exception if non-ideal equipment used with time_res >= 15 minutes
- Warns if non-ideal equipment used with 5-15 min timesteps

**Parameter Completeness**:
- Checks that all required HPXML fields are present and valid
- Fallback logic for missing optional fields (e.g., estimate bedrooms/bathrooms)
- Validates voltage range, raises on NaN/negative values

**Zone Temperature Scheduling**:
- Validates that zone temperatures are available for equipment that require them
- Sets up zone-to-schedule-column mapping dynamically

**Grid Disconnection Logic**:
- Detects when voltage = 0 (islanded mode)
- Resets power if grid disconnects and cannot meet demand
- Zeroes internal gains/HVAC/equipment power on islanding failure

### HARES Features (validation.rs)

**Schema Validation**:
- Checks HPXML structure against 4.0 schema expectations
- Validates XML element presence/structure

**Range Checks**:
- Floor area: minimum/maximum bounds
- Basic domain validation on parsed values

**No Advanced Checks**:
- No equipment multiplicity validation
- No time resolution checks
- No parameter fallback logic
- No grid mode validation
- No control signal validation

### OCHRE Advantages on Validation:

1. **Equipment configuration rules** — enforces single heating/cooling/DHW unit
2. **Time resolution guard rails** — prevents invalid model combinations
3. **Control mode validation** — detects grid disconnection and handles it explicitly
4. **Fallback estimates** — uses industry defaults when HPXML incomplete
5. **Voltage anomaly detection** — catches NaN and out-of-range power values
6. **Zone availability** — verifies temperature inputs available before equipment init

---

## 6. Error Handling and User-Facing Messages

### OCHRE Features

**Exception Hierarchy** (utils/__init__.py):
- OCHREException: Custom base exception for all OCHRE errors
- Thrown with descriptive messages in 100+ places

**Warning Levels**:
- Uses Python logging and print() for non-fatal issues
- Controlled by verbosity parameter (0-9)
- Prefix messages with simulation name and timestamp

**File I/O Error Reporting**:
- Reports file paths in error messages
- Suggests fallback paths if files not found
- Indicates if weather file mismatch detected

**Data Validation Messages**:
- Lists non-equal parameter values when consolidating boundaries
- Warns about cascade of warnings if major issues detected
- Prints progress messages (e.g., "Saved schedule to: /path/file")

### HARES Features (HpxmlError, WeatherError, ScheduleError, OutputError)

**Error Types** (enums):
- HpxmlError: Parse, SchemaValidation, DomainValidation
- WeatherError: Io, Parse, Validation, Resample
- ScheduleError: Io, Parse, Validation, MissingRequiredColumns, ColumnNotFound, IndexOutOfRange, Resample, Coverage
- OutputError: Arrow, Parquet, Io, ColumnMismatch, InvalidChunkSize

**Error Context**:
- File path included in Io errors
- Column names in missing/not-found errors
- Index and length in out-of-range errors

**No Warnings**:
- Only errors reported (no non-fatal warnings)

### OCHRE Advantages on Error Handling:

1. **Verbose logging** — can trace execution with verbosity levels
2. **Suggested recovery** — offers fallback values or paths
3. **Cascade detection** — warns about downstream issues from single root cause
4. **File mismatch detection** — warns if weather station name doesn't match file
5. **Progress reporting** — confirms file saves and initialization steps
6. **Exception wrapping** — meaningful context on where errors occur

---

## 7. ResStock and Integration Features

### OCHRE Features (Analysis.py, cli.py)

**ResStock Download Support** (Analysis.py: download_resstock_model):
- Automatic download of HPXML and schedule files from NREL OEDI S3
- Supports ResStock 2024.2 release
- Formatted building IDs (bldg0000001 format)
- Upgrade pathway support (up00, up01, etc.)
- Automatic zip file extraction and validation

**Batch Processing** (cli.py):
- Find all OCHRE-compatible folders in directory tree
- Run multiple simulations in parallel via multiprocessing.Pool
- HPC support via Slurm (srun command wrapping)
- Track completion with ochre_complete flag files
- Limit total runs with n_max parameter
- Memory-aware HPC submission (--mem option)

**CLI Commands**:
- `ochre single <path>` — run single dwelling
- `ochre local <path> -n <parallel>` — run multiple locally
- `ochre hpc <path> --mem <GB>` — submit to Slurm cluster

**Configuration Override**:
- Command-line parameters override HPXML defaults
- Nested update logic to merge configs
- HPXML properties saved to JSON for audit trail

### HARES Features

**No ResStock Integration**:
- No automated download
- No batch runner visible in I/O layer
- No HPC submission support
- Relies on external orchestration (Python/Rust wrapper)

**Configuration Merging** (EquipmentSpec: nested_update):
- Basic nested dictionary updates supported
- Used to override equipment defaults

### OCHRE Advantages on Integration:

1. **Native ResStock support** — download and run ResStock models directly
2. **Batch automation** — built-in CLI for fleet runs
3. **HPC native support** — Slurm integration without wrapper scripts
4. **Completion tracking** — ochre_complete flag prevents re-runs
5. **Parameter sweep** — CLI options allow easy sensitivity analysis
6. **Reproducibility** — saved JSON includes all input parameters

---

## 8. Co-simulation and Advanced Interfaces

### OCHRE Features

**None identified** — OCHRE appears to be a standalone simulator with no HELICS, OpenFMI, or other co-simulation protocol support. However:

- Voltage input from external controller (resilience mode): `schedule_inputs["Voltage (-)"] = external_value`
- Net power output available for generator: `sub.current_schedule["net_power"] = self.total_p_kw`
- PV power available for battery: `sub.current_schedule["pv_power"] = sum(e.electric_kw for e in equipment)`

These are one-way data flows within a single process, not bidirectional coupling.

### HARES Features

**None identified** — no co-simulation interface visible in I/O crate.

---

## 9. Time-of-Use (TOU) Rate Structures

### OCHRE Features

**Commented-out code** (Analysis.py lines 540-551):
```python
# FUTURE: add rates, emissions, other post processing
# print('Loading rate file...')
# rate_file = os.path.join(main_path, 'Inputs', 'Rates', 'Utility Rates.csv')
# df_rates = ... load and merge with results
# df_all['Cost'] = df_all['Rate'] * df_all['Total Electric Energy (kWh)']
# annual_costs = df_all.groupby(...).sum()
```

Indicates **TOU rates were planned but not implemented**.

### HARES Features

**None identified** — no rate structures in I/O layer.

### Status:

Both systems lack active TOU rate support, though OCHRE has partial scaffolding.

---

## 10. Post-Processing and Analysis

### OCHRE Features (Analysis.py)

**Detailed Analysis Functions**:
- `load_timeseries_file()` — load OCHRE or EnergyPlus CSV/Parquet with automatic aggregation
- `get_agg_func()` — classify columns as sum/mean based on units (kWh sums, C averages)
- `add_eplus_equivalent_results()` — map E+ column names to OCHRE names for validation
- `add_eplus_detailed_results()` — extract film coefficients and fit response surfaces
- `calculate_metrics()` — compute 50+ derived annual metrics (see section 2)
- `create_comparison_metrics()` — compare OCHRE vs E+ with RMSE, percent error
- `combine_json_files()` — aggregate input parameters across fleet
- `combine_metrics_files()` — aggregate metrics across fleet
- `combine_time_series_files()` — merge results with MultiIndex (Time, House)
- `get_parent_folders()` — extract run identifiers from path hierarchy
- `find_subfolders()` — search for runs matching file patterns
- `find_files_from_ending()` — discover output files with priority matching

**Figures** (CreateFigures.py):
- `plot_time_series()` — standard line plot with legends
- `plot_time_series_detailed()` — multi-panel with colors, steps, custom formatting
- Matplotlib integration for visualization

**EnergyPlus Comparison**:
- Functions to compare OCHRE results against E+ for validation
- Detailed error metrics (RMSE, percent error)

### HARES Features (crates/hares-io/src/output/)

**Metrics** (metrics.rs):
- `AnnualEnergyKwh` — total and per-end-use energy
- `PeakPowerKw` — instantaneous and time-averaged peaks
- `GridInteractionMetrics` — peak import/export
- `GasEnergyMetrics` — total therms and kWh equivalent
- `SimulationMetrics` — comfort hours, unmet load hours, renewable fraction

**Output Writer** (writer.rs):
- Streaming CSV/Parquet writer with configurable chunk size
- Memory-bounded streaming (peak memory ∝ chunk_size, not simulation length)

**No Post-Processing**:
- No fleet aggregation functions
- No comparison utilities
- No visualization
- No time-series analysis helpers

### OCHRE Advantages on Post-Processing:

1. **Fleet aggregation** — combines JSON and metrics from multiple buildings
2. **Comparison utilities** — RMSE, percent error vs benchmark
3. **Detailed component loads** — film coefficients, response surface fitting
4. **Time-series alignment** — merges multiple runs with MultiIndex
5. **File discovery** — walks directory tree to find results
6. **Visualization** — plotting functions for common analyses
7. **EnergyPlus validation** — dedicated comparison code
8. **Metrics aggregation strategies** — smart sum/mean based on column units

---

## 11. Additional Draw Profiles and Occupancy Schedules

### OCHRE Features

**Wet Appliance Profiles** (Equipment/WetAppliance.py):
- Monte Carlo profile generator for:
  - Clothes Washer: event timing, duration, flow rate, temperature
  - Dishwasher: similar parameterization
  - Clothes Dryer: energy draw, duration
- Temperature stratification in hot water tank during draws
- Realistic draw statistics from field data

**Occupancy Integration**:
- Occupancy schedule drives many equipment behaviors
- Number of occupants can scale loads
- Occupancy-dependent setpoints

**Event-Based Loads** (Equipment/EventBasedLoad.py):
- Stochastic event generation (e.g., EV charging)
- Configurable event rate, power, duration
- Can delay events (e.g., shift laundry off-peak)
- Multiple event types in single class

### HARES Features

**Scheduled Load** (equipment/scheduled_load.rs):
- Loads from schedule file
- No profile generation

**Event Load** (equipment/event_load.rs):
- Event-based loads
- May support stochastic timing but less detailed than OCHRE

**No Stochastic Features**:
- No Monte Carlo profile generation
- No draw temperature variation
- No tank stratification model
- No draw delay logic

### OCHRE Advantages:

1. **Water heater stratification** — models tank layers and mixing
2. **Draw temperature control** — appliances can request hot water at specific temps
3. **Event delay** — can shift laundry/charging off-peak
4. **Clothes washer detail** — separate flow/thermal behavior
5. **Occupancy scaling** — loads scale with number of people
6. **Profile statistics** — based on field measurements, not synthetic

---

## 12. Unique Features by System

### OCHRE-Only Features:

1. **Psychrometric library with detailed humidity calculations** (psychrolib_jit.py) — Numba-JIT compiled, validated against ASHRAE
2. **Wet appliance Monte Carlo profiles** — stochastic hot water draws with temperature and flow
3. **Multi-speed HVAC support** — speed-dependent biquadratic curves
4. **Gas generator efficiency curves** — part-load dependent
5. **Battery degradation tracking** — capacity fade over cycles
6. **Island resilience metrics** — islanding time, outage duration, number of events
7. **Detailed duct loss modeling** — separate duct efficiency (DSE) calculation
8. **Coil bypass factor** — models partial air mixing in HVAC coils
9. **Furniture thermal mass** — models indoor furniture in envelope
10. **Command-line interface (CLI)** — single/local/HPC run modes
11. **ResStock integration** — direct download and batch run support
12. **Figure generation** — plotting functions for results
13. **Comparison utilities** — OCHRE vs EnergyPlus validation
14. **Fleet aggregation** — combine results from 100+ buildings into single DataFrame
15. **Time zone awareness** — handles DST and locale-specific calendars

### HARES-Only Features:

1. **Memory-bounded streaming output** — chunk-based Parquet/CSV write keeps RAM constant
2. **Strong type safety** — Rust prevents many OCHRE's runtime errors
3. **Compressed Parquet output** — uses Snappy compression for efficient storage
4. **Column-oriented weather** — explicit field access with error checking
5. **LSP-compatible validation** — HPXML schema validation against defined spec
6. **Instance-qualified equipment names** — "Battery #1 SOC" not just "SOC"

---

## Summary Table

| Feature | OCHRE | HARES | Gap |
|---------|-------|-------|-----|
| **HPXML Parsing** | Comprehensive, 30+ fields | Basic, ~10 fields | HARES missing: MELs, pool equipment, foundation details, slab insulation, roof properties, wet appliance mapping |
| **Output Metrics** | 50+ calculated (COP, SHR, islanding, outages) | ~5 (annual energy, peak, comfort) | HARES missing: efficiency metrics, resilience metrics, cycling counts, component loads |
| **Weather Derivations** | Wet bulb, humidity ratio, enthalpy, sky temp | Raw EPW fields only | HARES missing: psychrometric calculations, derived quantities |
| **Schedules** | Monte Carlo, event-based, occupancy-scaled | CSV-based, static | HARES missing: stochastic event generation, wet appliance profiles, delay logic |
| **Validation** | Equipment rules, time resolution, fallback logic | Schema/range checks | HARES missing: configuration rules, parameter fallbacks |
| **Error Messages** | Verbose with context, progress reporting | Concise, error-only | HARES missing: non-fatal warnings, recovery suggestions |
| **ResStock Support** | Direct download, batch processing | None | HARES needs external wrapper |
| **TOU Rates** | Scaffolding (not active) | None | Both missing |
| **Co-simulation** | Internal data flow only | None | Both missing |
| **Post-Processing** | Fleet aggregation, comparison, plotting | Streaming output only | HARES missing: analysis utilities |
| **Wet Appliance Draws** | Full Monte Carlo | Basic schedule | HARES missing: draw profiles, tank stratification |
| **CLI/Batch** | Full Slurm/local support | None | HARES needs orchestration |

---

## Recommendations for HARES

To match or exceed OCHRE, HARES should add (priority order):

1. **High Priority — Output Completeness**:
   - Calculate COP, SHR, DSE for HVAC/WH
   - Add unmet load hours, comfort hours, peak demand profiles
   - Add islanding time and grid resilience metrics

2. **High Priority — Psychrometrics**:
   - Implement wet bulb calculation
   - Add humidity ratio derivation
   - Calculate enthalpy for control logic

3. **Medium Priority — Configuration**:
   - Equipment multiplicity validation
   - Parameter fallback logic for missing HPXML fields
   - Zone temperature availability checks

4. **Medium Priority — Schedules**:
   - Add stochastic event generation (at least for EV charging)
   - Implement occupancy scaling
   - Support hot water draw profiles

5. **Medium Priority — Analysis**:
   - Fleet aggregation utilities
   - Comparison metrics (vs baseline/benchmark)
   - Plotting functions

6. **Lower Priority — Integration**:
   - ResStock downloader (if targeting ResStock usage)
   - Batch runner CLI (can be Python wrapper initially)
   - TOU rate support (if needed for cost analysis)

---

## Files for Reference

**OCHRE**:
- `vendors/OCHRE/ochre/Dwelling.py` — main dwelling orchestration
- `vendors/OCHRE/ochre/utils/hpxml.py` — HPXML parsing (2000+ lines)
- `vendors/OCHRE/ochre/Analysis.py` — metrics and post-processing
- `vendors/OCHRE/ochre/Equipment/` — all equipment classes
- `vendors/OCHRE/ochre/utils/psychrolib_jit.py` — psychrometric library

**HARES**:
- `crates/hares-io/src/hpxml/` — HPXML parsing (Rust)
- `crates/hares-io/src/output/` — output schemas and metrics
- `crates/hares-io/src/weather.rs` — weather time-series
- `crates/hares-io/src/schedule.rs` — schedule parsing
- `crates/hares-equipment/src/` — equipment implementations
