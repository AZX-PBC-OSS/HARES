# Scheduled & Event Load Equipment Models

[Back to Architecture](../architecture.md)

## Scheduled Load

**Source**: `crates/hares-equipment/src/scheduled_load.rs`

Deterministic power consumption driven by time-indexed schedules (CSV columns or daily profiles).

### Operating Model

- Power schedule (electrical) + optional gas schedule
- Monthly multipliers (`month_multiplier_0` to `month_multiplier_11`) for
  seasonal variation (e.g., ceiling fans off in winter); they scale the
  electric and gas schedules whatever their source, a daily profile's own
  month factors included; a negative or non-finite multiplier is an error
  (OCHRE only zeroes a month whose multiplier is 0: divergence D-013 in
  [DIVERGENCES.md](../alignment/DIVERGENCES.md))
- ZIP voltage-dependent load model (full ZIP with byte-identical arithmetic per [power-factor.md](./power-factor.md)):
  ```
  P = P_base * (zp·V² + ip·V + pp)    where V = voltage_pu / v0
  Q = P_actual * tan(acos(pf)) * (zq·V² + iq·V + pq)
  ```
  Default: constant-power (zp=0, ip=0, pp=1.0, pf=0 sentinel → Q=0). Non-zero pf and reactive coefficients from class table when available.

- ScheduledLoad with `ochre_class = "Ventilation Fan"` or `"Ceiling Fan"` now produces the same reactive power as the typed `ventilation.rs` model (both use PF 0.87 from the FAN class — the previous inconsistency is resolved)

### Zone Thermal Routing

Zone assignment (`load_zone.rs`):
- An explicit `zone_id` names the zone (the HPXML resolver sets it from the
  appliance's `Location`)
- EV charging, and a load named "Exterior"/"Outdoor" without a `zone_id`, gives
  no zone heat
- Otherwise the zone of the role the name implies (garage, basement,
  crawlspace or attic, else the conditioned zone) from the dwelling's zone map
- A load that gives zone heat with no zone to give it to, or a zone the
  environment does not have, fails the build

Heat distribution (`gain_fractions.rs`), per watt of electric plus fuel input:
- `sensible_gain_fraction` of it is sensible heat, `latent_gain_fraction` latent
- Of the sensible heat, `radiant_share_of_sensible` is long-wave radiant (to the
  interior surfaces), `visible_share_of_sensible` short-wave (absorbed by the
  interior surfaces as transmitted diffuse solar), the rest convective (to the
  zone air); an absolute `radiative_gain_fraction` replaces the radiant share
- OpenStudio-HPXML v1.12.0 defaults: appliances and plug loads 0.6 radiant
  share; lights 0.6 radiant and 0.2 visible; ceiling fans 0.558 radiant (see
  `docs/alignment/DIVERGENCES.md`, D-005 to D-007)
- The HPXML extension's `FracSensible` and `FracLatent` set the sensible and
  latent fractions; an override may give either spelling, never both
- Category: `ThermalCategory::InternalGain`

### Control Signals

| Signal | Effect |
|--------|--------|
| `LoadFraction` | Direct multiplier on output power |
| `ModeOverride(Off)` | Forces zero output |
| `PowerSetpoint` | Overrides schedule for one step |

### Port Interactions

- **Electrical**: the scheduled power in W (schedule x load_fraction x ZIP), the reactive in VAR; the telemetry keys keep their kW/kvar reporting spellings with W/VAR values
- **Fuel**: optional gas consumption
- **Thermal**: sensible + latent to assigned zone

---

## Event-Driven Load

**Source**: `crates/hares-equipment/src/event_load.rs`

Stochastic appliance cycles with probabilistic triggering.

### State Machine

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Active: event_window open AND probability trigger
    Active --> Cooldown: active_duration elapsed
    Cooldown --> Idle: cooldown_duration elapsed
    Active --> Idle: cooldown = 0
```

### Event Triggering

- `event_window_source`: schedule controlling when events can start (>0 = open)
- `event_probability_source`: probability of starting within one hour of
  open window (0-1); the per-step chance is derived from it and the step
  length, so the same config starts at the same hourly rate at any
  resolution
- `ForcedMode::Active` overrides stochastic draw; `ForcedMode::Idle` blocks start

### Wet Appliance Variant

Multi-phase cycles (e.g., washer: fill -> wash -> spin), the powers in W (SI):
- `phase_len` phases, each `phase_<n>_power_w` and `phase_<n>_duration_s`;
  with no `phase_len`, one phase of `active_power_w` for `active_duration_s`
- The electric cycle replays the appliance's power column's events; the hot
  water is a separate schedule-driven draw: the `hot_water_clothes_washer`
  and `hot_water_dishwasher` columns normalized to the appliance's daily
  volume (`daily_hot_water_m3` on the spec) and drawn untempered through the
  water heater's appliance draw source

### Control Signals

| Signal | Effect |
|--------|--------|
| `LoadFraction` | Multiplies active phase power |
| `ModeOverride(Off)` | Forces Idle |
| `ModeOverride(Standby)` | Starts/holds Active |
| `PowerSetpoint` | Overrides active power for one step |

### Port Interactions

- **Electrical**: phase-based or setpoint-overridden power
- **Thermal**: sensible + latent gains to zone
- **Fuel**: optional if `fuel_type != Electric`

## Parameters and Overrides

**Source**: `crates/hares-equipment/src/raw_params.rs`

Each kind of load (scheduled load, event load, wet appliance) has one list
of the parameters it reads and the kind each is read as (a number, text,
true or false, a list of numbers), beside the constants its code reads them
by; in debug and test builds a read of a key off its list, or as another
kind, fails, so the list cannot fall behind the code. The dwelling's
equipment overrides use the same lists:

- An equipment's own override entry may set only parameters its kind reads,
  `zip` (the ZIP coefficients) aside; any other key, and the reserved
  `equipment_id`, fails the build naming the key and listing what the load
  reads. The HPXML spellings `frac_sensible` and `frac_latent` stand for
  `sensible_gain_fraction` and `latent_gain_fraction`; both spellings of one
  fraction in a layer are an error.
- The wildcard override (`all` or `*`, one of them; both is an error) reaches
  every equipment: each takes the wildcard parameters it reads once every
  override layer is applied (a numbered key such as `phase_4_power_w` only
  within its family's count; a typed config the fields its schema has) and
  skips the rest. A wildcard parameter no equipment of the dwelling reads
  fails the build.
- A parameter a load reads given a value of another kind (a string for a
  number, a null, an object) fails the build rather than reading as absent.
- A raw-parameter spec of a class with no list (equipment that is not a load,
  added through a blueprint without a typed config) takes the whole wildcard
  and its own entry unchecked, and vouches for no wildcard parameter.

## Grid Outage Behaviour

Scheduled and event-based loads draw nothing and deposit no gains while the
home bus is de-energized; event timers and schedule cursors keep advancing so
missed cycles are not deferred. Gas scheduled loads are also zeroed (modern
gas appliances need electricity for ignition/controls). Islanded homes keep
their loads running. See [outage-behavior.md](../outage-behavior.md).
