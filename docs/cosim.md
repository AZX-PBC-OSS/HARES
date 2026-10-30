# The co-simulation contract

HARES's coupling surface for grid co-simulation: the HELICS value
federates (`python/hares/helics/`), the direct Python surface
(`hares.Dwelling`, `hares.SteppableFleet`), and the boundary rules both
paths share. This document is the contract the federate sources
implement; where the code and this document disagree, the code is the
defect and one of them changes.

## The time base and the stepping semantics

- **The time base is the dwelling's own local standard time** (naive
  wall-clock at the site's fixed UTC offset; daylight saving shifts
  schedules, not the weather or the clock). A federate's timesteps are
  the dwelling's configured run: `start_time + n * period`.
- **The period** is the dwelling's `time_res` (fixed for the run). The
  federate infers it from the first two timesteps and warns while doing
  so; a caller can pin it by configuring the run and reading the warning
  once. The floor is 1 s: no sub-second coupling is offered, and no
  event-driven re-stepping exists. The coupling is **fixed-cadence**:
  every federate steps on the same period, and a slower federate's
  intermediate grants publish without stepping (below).
- **The step's semantics:** a timestep's state is the state *before* the
  step advances it. The federate requests the step's *exit* time
  (`(n+1) * period`): a federate is already at time 0 immediately after
  entering execution mode, and HELICS never re-grants a time at or
  before the federate's current time, so requesting the entry time would
  silently shift every subsequent request one period late.
- **Intermediate grants (publish-without-step):** when HELICS grants a
  time *before* the requested exit time, the federate publishes its
  current state and requests the exit time again. The published values
  at an intermediate grant are the LAST COMPLETED STEP's values - the
  stale-value semantics: a subscriber reading between steps sees the
  last stepped state, never a partial step.
- **Drift:** the federate accumulates the absolute difference between
  requested and granted times; a cumulative-drift tolerance can be set
  (`max_time_drift_tolerance_s`), and exceeding it raises rather than
  silently skewing the federation.
- **Completion:** a federate with no further steps signals completion
  with `request_time(HELICS_TIME_MAXTIME)` so the broker can coordinate
  shutdown without every peer stalling through the disconnect timeout.

## The boundary's variables and their units

Publications (per dwelling; the fleet federate prefixes
`dwelling_{index}/` and adds fleet aggregates):

| Topic | Unit | Meaning |
|---|---|---|
| `total_power_w` | W | The premise balance: the net active power at the meter (import positive, export negative), the aggregate of the premise's own equipment. During an island this is the island's internal imbalance - it is a premise quantity, not a grid quantity. |
| `grid_interchange_w` | W | The grid interchange: the net active power crossing the service connection. Exactly 0.0 whenever the utility is out (the breaker is open); the meter's net flow otherwise. A feeder federate's demand channel reads THIS topic, never `total_power_w`. |
| `reactive_power_var` | var | The premise's net reactive power. |
| `islanded` | flag (0.0/1.0) | 1.0 when the utility is out and an on-site source holds the bus energized. |
| `bus_voltage_pu` | pu | The bus voltage the premise's equipment sees: the utility voltage when up, the island-formed bus voltage during an outage with an island source, 0.0 on a de-energized bus. |
| `island_unserved_w` | W | The island's internal imbalance, load side: the load the island sources failed to cover. 0.0 when not islanded. |
| `island_excess_w` | W | The island's internal imbalance, generation side: the surplus the island could not absorb. 0.0 when not islanded. |
| `zone_{i}/temp_air_c` | degC | Zone air temperature. |
| `equipment_{i}/power_w`, `/soc_pct`, `/operating_mode` | W, pct, enum | Per-equipment power, state of charge (0-100), operating-mode code. |
| `pv_generation_w` | W | PV generation magnitude (positive W). |

Subscriptions:

| Topic | Unit | Meaning |
|---|---|---|
| `grid/voltage` | pu | The utility service voltage. Exactly 0.0 signals a utility outage; otherwise a finite value in [0.5, 1.5] pu. |
| `grid/frequency` | Hz | The grid frequency. A finite value in [40, 80] Hz. |
| `grid/price` | currency/kWh | The real-time electricity price, non-negative. |

## The boundary contract (one behavior, both federates)

Both federates (the single-dwelling federate and the fleet federate)
enforce the SAME rules at the HELICS boundary:

- **Validate once, apply only in-band, count every violation.** An
  out-of-band value is warned, counted in the per-run
  `HELICSDiagnostics` row (`helics_range_violations_total`), and NEVER
  applied: the dwelling keeps its last valid value. This is true for the
  voltage (both federates), the frequency (both federates) and the
  non-finite prices.
- **A negative price warns, counts, and is still applied**: a negative
  tariff is a real scenario; the warning is the diagnostic. Both
  federates do exactly this.
- The per-run diagnostic row carries the per-timestep validity flags
  (`helics_voltage_valid`, `helics_frequency_valid`,
  `helics_price_valid`), the clamped-control count, the stale-subscription
  count and the update mask.

## The island's semantics

- `Dwelling.set_grid_voltage(0.0)` (the `grid/voltage` topic's 0.0) is
  the utility-outage signal and the only way an outage enters the
  engine.
- On an outage the dwelling resolves bus energization each step from the
  equipment state (a battery with usable charge, a generator, a
  discharging V2G/V2L EV): an available source islands the home and
  holds the bus at nominal (1.0 pu); no source means a de-energized bus
  and every load force-offs at the control level. Island formation
  takes effect with a one-step lag: the formation condition reads the
  previous step's outage and the source availability (both
  end-of-previous-step state) beside the step's own utility voltage, so
  an outage's first step is a dead-bus step and the island forms on the
  step after. A formed island collapses one step after its source dies;
  the utility's restoration collapses it on the restoration step itself
  (docs/outage-behavior.md).
- The islanding model is intentionally minimal: the island source
  regulates the bus at nominal; no island power-balance, droop or
  frequency physics is modeled.
- **The interchange is zero at the breaker.** From island formation the
  published `grid_interchange_w` reads exactly 0.0: the service
  connection carries no power. The island's internal imbalance
  (unserved load, unabsorbable surplus) surfaces as the island's
  internal response - `island_unserved_w` / `island_excess_w` and the
  premise balance `total_power_w` - never as a phantom grid load a
  feeder federate would read as demand.
- Restoration (a non-zero in-band voltage after an outage) collapses the
  island; the interchange is the meter's net flow again.

## The federates' obligations

- Register every publication and subscription before entering execution
  mode; after entering, verify the registration completeness against the
  caller's required-key sets and the HELICS-reported counts, and log an
  error naming every missing key.
- Publish every step and at every intermediate grant; subscribe and read
  every declared topic each step (`is_updated()` exactly once per
  subscription per step so the stale tracking and the application agree).
- Track stale subscriptions: a subscription silent for
  `STALE_SUBSCRIPTION_THRESHOLD` consecutive steps warns (once, at the
  threshold) and the count rides the diagnostics row.
- The fleet federate applies fleet-wide signals to every dwelling and
  exposes per-dwelling topics mirroring the per-dwelling voltage
  pattern; its boundary behavior is the single-dwelling federate's
  exactly (the section above).
- Timeouts: every blocking HELICS call runs under a wall-clock timeout
  (`connect_timeout_s`, `grant_timeout_s`); a timeout tears the federate
  down non-blocking and raises. `finalize()` is idempotent.
