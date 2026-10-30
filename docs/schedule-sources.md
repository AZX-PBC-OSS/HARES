# Schedule Sources

[Back to Architecture](architecture.md)

**Source**: `crates/hares-types/src/schedule.rs`

Equipment parameters that vary over time (power draws, setpoints, deadbands, event probabilities) are driven by `ScheduleSource` — a unified enum that resolves to `f64` each timestep via `value_at(env)`.

## Variants

### Constant

Fixed scalar. Use for parameters that never change.

```rust
ScheduleSource::Constant(21.0)
```

### DailyProfile

24-hour weekday/weekend profile scaled by monthly multipliers and a maximum value.

```rust
ScheduleSource::DailyProfile {
    weekday: [f64; 24],       // fraction per hour, Mon–Fri
    weekend: [f64; 24],       // fraction per hour, Sat–Sun
    month_multipliers: [f64; 12],
    max_value: f64,
}
// result = profile[hour] * month_multipliers[month] * max_value
```

### ColumnRef

Index into the schedule CSV payload carried in `EnvironmentState.custom_domains`.

```rust
ScheduleSource::ColumnRef {
    col_idx: usize,
    boundary: BoundaryPolicy,  // Clamp | Wrap | Error
}
```

### SolarAware

Fraction selected by solar altitude (daytime / evening / overnight), then scaled by month and max value. Used for lighting and occupancy profiles.

```rust
ScheduleSource::SolarAware {
    daytime_fraction: f64,
    evening_fraction: f64,
    overnight_fraction: f64,
    month_multipliers: [f64; 12],
    max_value: f64,
    dusk_altitude_threshold_deg: f64,
}
```

### Stochastic

Stateful pseudo-random draws (ChaCha8). Determinism depends on seed + call order, not on simulation time. Supports `reset()` for checkpoint restart, and the state (seed, draw count, RNG position) rides the schedule-source snapshots and the reseed derivation.

```rust
ScheduleSource::Stochastic {
    kind: DistributionKind,  // Gaussian | Uniform | LogNormal | Exponential
    seed: [u8; 32],
    draw_count: u64,
    rng: ChaCha8Rng,
    clamp_min: Option<f64>,
    clamp_max: Option<f64>,
}
```

The noisy-TimeWindows source's RNG keystream derivation is documented
under [TimeWindows](#timewindows) below.

### Shared

Cursor-based advancement through a shared data vector. Boundary policy controls behavior when the cursor exceeds the data length.

```rust
ScheduleSource::Shared {
    data: Arc<[f64]>,
    cursor: usize,
    boundary: BoundaryPolicy,
}
```

### TimeWindows

Window-based lookup by day-of-week and time-of-day with an optional default. Intended for thermostat setpoint schedules, occupancy modes, and any parameter that follows a weekly pattern with distinct time blocks.

```rust
ScheduleSource::TimeWindows {
    windows: Vec<TimeWindow>,
    default: Option<f64>,
}
```

**Evaluation**: windows are checked in declaration order; the first match wins. Overlapping windows are allowed — use ordering to express priority (e.g. a Monday-specific override before a broad weekday rule). If no window matches, `default` is returned. If `default` is `None` and nothing matches, an error is raised.

**Noise**: a window may carry a distribution; a matching window then samples `value + noise` from the source's ChaCha8 generator. The generator's key is the configured `seed` when one is set; without one it is derived from the source's identity (`<dwelling identity>:<owner name>:<source role>`, the dwelling identity the config-stable building id the per-dwelling stream derivations key on) at conversion, so two same-configured sources never draw identical series - not within a dwelling and not across homes, whose HPXML-derived equipment names are generic type strings - and the same identity is reproducible run to run. The default is never the all-zero key: sources sharing one key would correlate their noise.

**Master-seed reach**: the fleet's master seed threads only the per-dwelling derivations (`dwelling_seed`, the key the event-load streams draw from). The DR compliance driver's draw, the registry EV driver's key material (the driver added by name, whose day roll keys on its configured seed and the building id) and a noisy TimeWindows source's keystream key key on their own configured seed or identity, and the fleet path constructs none of them today. If the fleet path gains actor or noisy-source construction, the master seed must thread into their unspecified-seed derivations the same way, or re-seeding a fleet would silently leave those streams unseeded.

**Scenario-seed reach**: the run's `scenario_seed` threads the same derivation the master seed threads: it mixes into the per-dwelling key's spare bytes (`dwelling_seed_scenario`, the scenario id at key bytes 16..24 and a presence byte at 24, so a declared scenario 0 is never the absent scenario and the absent scenario keeps the scenario-less bytes exactly). The mix reaches every surface inside the per-dwelling key: the dwelling's own stream, the built-in EV drivers' streams and every event-load interior. It does NOT reach the identity-keyed surfaces: an unspecified-seed noisy TimeWindows source's keystream key (`schedule_source_key`) and the registry EV driver's key material (`driver_key_material`) key on identity alone. Two scenarios therefore share an unspecified-seed source's keystream only when both scenarios' dwellings address the same dwelling identity (the same building id): different building ids derive different keys, whatever the configuration. The share is named and deliberate: an identity-keyed key stays stable across scenario relabeling, and a scenario that needs its own series for a source sets that source's explicit seed. The DR compliance actor's compliance draw (its configured seed plus a hash over the DR level and the time) is outside the mix too: it receives neither the master seed nor the scenario and keys on its own configured seed alone. Anything that routes the scenario through a hash instead of the injective byte layout degrades the disjointness argument from constructive to birthday-bound and is a finding, not a refactor.

**Behavioral diversity (the generated family)**: a config with no schedule source generates its schedule from the default profiles; the diversity layer draws that home's own behavior, keyed on the scenario-mixed per-dwelling key, before the generation, so two homes under one master seed (their building ids differing, as a fleet's do) no longer run identical behavioral schedules (the fleet's peaks, DR aggregates and training distributions stop being N copies of one home). The key names the boundary: one blueprint carries one building id, so two builds of one blueprint under one seed are the SAME home by design and run the identical schedule; `DwellingBlueprint::build_with_seed` re-derives the schedule only for a new seed. The drawn parameter set (`hares_types::diversity::ScheduleDiversityDraws`) covers five dimensions: the presence timing (the Occupancy profile's hours shift a whole number of hours, a discrete uniform over plus or minus 3), the occupancy level (a mean-one lognormal, sigma 0.15, clamped to [0.6, 1.6]), the appliance diversity factors (MELs, indoor lighting, dishwasher, clothes washer, clothes dryer and cooking range, each a mean-one lognormal, sigma 0.25, clamped to [0.5, 2.0]; an appliance's hot-water column shares the appliance's profile and moves with it), the water-draw timing (the Water Heating profile's own shift, plus or minus 2 hours) and the water level (a mean-one lognormal, sigma 0.30, clamped to [0.5, 2.0]). The timing shifts rotate the profiles' hourly fractions cyclically within each day-type array: the shift wraps midnight, never the day type, and the monthly multipliers pass through untouched. The draws compose with the RNG contract: each dimension draws from its own `RngStream::schedule_diversity` stream under the dwelling's scenario-mixed per-dwelling key, so the derivation is injective over (master seed, scenario, building, dimension) with no hash of the scenario (the scenario rides the key's own bytes), one dimension's draws never move another's, the same (seed, scenario, building) reproduces bitwise, and the draws consume no stream the dwelling's build or step draws from: the layer adds no runtime stochastic state, and nothing of it appears in the dwelling's seeded-stream ledger or its checkpoints. The knob is `SimulationConfig.schedule_diversity` (TOML `schedule_diversity = "on" | "off"`, the Python keyword and `schedule_diversity` property the same; the default on). A schedule CSV is NEVER diversified whatever the setting: the layer is the generated family's alone, and measured, upstream or hand-built schedules pass through untouched. Provenance: the central profiles are the ANSI/RESNET/ICC 301-2022 Addendum C defaults the occupancy provenance contract pins (below); the SPREADS are the record's calibrated choice, not published values, chosen mean-one before the clamp so the fleet aggregate's mean stays at the 301 profile's own mean up to the clamp's two folded tails (a stated low bias of at most 0.075 percent at the widest spread, where the clamped variance is 4.71 percent below the unclamped `exp(sigma^2) - 1`; the closed forms and their quadrature-referenced pins are on `hares_types::diversity`); the published scale the acceptance is stated against is the NREL End-Use Load Profiles report (Pigman, McCarty, et al., NREL 2022, https://www.osti.gov/servlets/purl/1909353): its per-home annual electricity by end use across the synthetic stock, while the witnesses pin the fleet mean at the 301 profile's integral and the cross-home spread at the clamped closed form, never at a table read from the report. The stochastic occupancy family the layer mirrors (the Markov presence chains, the seven ATUS activities 2013-2017) is OS-HPXML v1.12.0's BuildResidentialScheduleFile. The setpoint habits are deliberately NOT a schedule dimension: the setpoint resolution rule (below, the CSV column over the HPXML profiles over the defaults) pins the HPXML profiles' priority, and a generated setpoint column would silently override them; the setpoint diversity belongs to the scenario spec's sampled parameters (below), applied through the overrides path, where the declared value lands in the typed config it names.

**The scenario spec and the sampler** (`hares_types::scenario`, the Python `hares.ScenarioSpec`): a study declares a typed `ScenarioSpec`: a name and a list of parameters, each carrying its `name` (unique; the sampled record's key), its `unit` ("C", "W", "count", "fraction"), its `target`, its `distribution` and an optional `correlation`. The distributions are `uniform` (low, high), `normal` (mean, std_dev), `lognormal` (mu, sigma), `triangular` (min, mode, max) and `discrete` (values, weights); every family validates its parameters and declares its analytical mean, the moment witnesses' reference. The target is either `{"override": {"equipment": ..., "field": ...}}`: the value lands in the dwelling's overrides map under that equipment key and field, where the assembly's own loud unknown-field rejection and the removed-keys walk police it exactly as a hand-written override; or `{"patch": {"field": ...}}`: the value sets the HPXML data patches (`HpxmlDataPatches`, `number_of_bedrooms`) the assembly consumes the same way external metadata's patches arrive. An override target must name an equipment the assembly's override path accepts: the `Occupancy` spec is handled outside that path (its internal-gain load is applied directly in the simulation loop), and the assembly refuses an override of it exactly as it refuses a hand-written one, so an occupancy count is not a scenario target; the behavioral occupancy is the generated schedule's own diversity dimension. The sampler maps `(master_seed, scenario_id, bldg_id)` to the home's parameter set deterministically: the draws come from one `RngStream::scenario_sampler` stream under the scenario-mixed per-dwelling key, in the spec's declaration order (a correlation group's shared factor draws at the group's first member, so appending a parameter, a new group included, never moves the values already recorded, and a member appended to an existing group reads the group's already-drawn factor; the guarantee is append-stability alone: inserting or reordering a parameter moves every draw from the first changed position on), so the same triple reproduces bitwise, a different scenario, building or master seed draws differently, and the stream is tagged out of the dwelling's own stream space (the sampler's draws never touch the streams the build or the simulation draw from). A study names the home's scenario twice, and the two names are one key: the spec's `scenario_id` and the run's `SimulationConfig.scenario_seed` both fill the scenario bytes of `dwelling_seed_scenario`, so a sample at `(master_seed, scenario_id, bldg_id)` and a dwelling built with `master_seed`, `scenario_seed = scenario_id` and that `bldg_id` address one per-dwelling key (the sampler's tagged stream and the schedule diversity's tagged streams are disjoint identities under it, so nothing is shared but the key). Nothing checks the pairing: the sampler never reads a config and the build never reads a record, so a study that samples under one scenario or building id and builds under another applies the values to a home whose behavioral draws come from a different key. The record carries both ids for exactly that check. A correlated member's value is its distribution's quantile at the one-factor Gaussian copula's probability (the shared factor's z times sqrt(rho) plus the member's own z times sqrt(1-rho), through the normal CDF and Acklam's inverse): the transform preserves the declared marginal exactly, and the witnesses hold it: the uniform pair's KS and Gaussian-copula correlation, and a correlated Normal and LogNormal member's mean and spread. The sampled record (`ScenarioSample`) serializes: the per-home payload a run manifest records (the scenario id, the building id, every parameter's name, value and unit, with the targets for the application), plus the overrides-map form the dwelling constructor's `overrides=` accepts and the patches list the settings' patch application reads. The Python surface reads the record and the overrides form (`sample` and `sample_overrides`); the patch channel's application is the Rust `apply_sample` (no Python apply binding, the row's declared surface). The application is the EXISTING paths and nothing new: `hares_core::scenario::apply_sample` merges the Override targets into `DwellingSettings.overrides` and sets the Patch targets into `DwellingSettings.patches`; the round-trip witnesses (the Rust resolved-model read-back and the Python `resolved_model()`'s equipment read-back) prove each sampled value reached the typed config it names bit for bit. The two channels apply in different orders by design: an override lands on the typed config AFTER the assembler's autosizing (the assembler sizes every omitted capacity from the resolved inputs first, then the override is the value the equipment initializes with, the autosized value standing only where no override names the field), while a patch is read by the resolution itself (a patched bedroom count feeds the water heater's sizing).

#### TimeWindow

```rust
TimeWindow {
    day: DayFilter,        // Any | Weekdays | Weekends | Day(Weekday)
    start_minute: u16,     // 0..1440, minutes from midnight
    end_minute: u16,       // 0..=1440, half-open: [start, end)
    value: f64,
}
```

- `end_minute = 1440` represents end-of-day (24:00), so a full-day window is `(0, 1440)`.
- **Midnight wrapping**: when `start_minute > end_minute`, the window spans midnight. The post-midnight portion matches the *next* calendar day. For example, `Day(Mon), 1320, 360` covers Monday 22:00–23:59 and Tuesday 00:00–05:59.
- Zero-width windows (`start == end`) are invalid and panic in debug builds.

#### DayFilter

| Variant | Matches |
|---------|---------|
| `Any` | Every day |
| `Weekdays` | Monday–Friday |
| `Weekends` | Saturday–Sunday |
| `Day(Weekday)` | A specific day (e.g. `Day(Mon)`) |

#### Example: thermostat setback schedule

```rust
ScheduleSource::TimeWindows {
    windows: vec![
        // Weekday daytime comfort
        TimeWindow::new(DayFilter::Weekdays, 420, 1320, 21.0),  // 07:00–22:00
        // Weekday overnight setback
        TimeWindow::new(DayFilter::Weekdays, 1320, 420, 18.0),  // 22:00–07:00
        // Weekend daytime (sleep in)
        TimeWindow::new(DayFilter::Weekends, 540, 1380, 22.0),  // 09:00–23:00
        // Weekend overnight
        TimeWindow::new(DayFilter::Weekends, 1380, 540, 18.0),  // 23:00–09:00
    ],
    default: Some(20.0),
}
```

## BoundaryPolicy

Controls out-of-range index behavior for `ColumnRef` and `Shared`:

| Policy | Behavior |
|--------|----------|
| `Clamp` | Clamp to first/last valid index |
| `Wrap` | Wrap modulo data length |
| `Error` | Return an error |
