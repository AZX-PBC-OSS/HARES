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

### SeededNoise

Deterministic pseudo-random draws (ChaCha8). Determinism depends on seed + call order, not on simulation time. Supports `reset()` for checkpoint restart.

```rust
ScheduleSource::SeededNoise {
    base: f64,
    std_dev: f64,
    seed: [u8; 32],
    ..
}
// result = base + std_dev * N(0,1)
```

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
