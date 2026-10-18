---
diagnosis_outcome: root_cause_found
complexity: moderate
passing_commands:
  - command: "cargo test -p hares-core ev_driver"
  - command: "cargo test --workspace"
  - command: "cargo clippy --workspace --all-targets -- -D warnings"
  - command: "cargo fmt --all -- --check"
---

## Symptom

I-04 reports that `EvDriverActor`'s own telemetry channels do not reflect
charging, on both the DailyCommuterL1 archetype route and the HPXML route:
`charge_kw` is identically `0.000`, `needed_charge_hours` is identically
`0.000`, and `soc` is monotone non-increasing (never rises on a charging
interval), even though the vehicle visibly draws real energy (confirmed via
the equipment's own "EV End Use Electric Power (kW)" column). `plugged_in`
and `phase` are correct. The ticket asks whether `charge_kw` /
`needed_charge_hours` are meant to be populated, and whether `soc` should
reflect charging as well as driving.

## Investigation

Code: `crates/hares-core/src/actors/ev_driver/{mod.rs,composer.rs,preference.rs,soc_target.rs,soc_gate.rs,departure.rs,price.rs,solar.rs,v2g.rs,v2h.rs}`,
equipment: `crates/hares-equipment/src/ev/mod.rs`, reference:
`vendors/OCHRE/ochre/Equipment/EV.py`.

`EvDriverActor` dispatches *goals* to the EV equipment (`SOCTarget`,
`EvSetReadyBy`) or *rates* (`PowerSetpoint`, `EvAwayCharge`) — see
`build_preferences()` (mod.rs:96-226) and `ChargingComposer::emit_vote`
(composer.rs:193-324). The EV equipment is self-contained: it runs its own
BMS/CC-CV physics each step to decide the actual charging power that
satisfies whatever goal it was last given (`hares-equipment/src/ev/mod.rs`,
`calculate_power`/`bms_ready_by_power` region, `soc` field around line 117).
This driver/equipment split is intentional per the module doc comment
(mod.rs:1-21): the driver never mutates equipment state directly, and its
`estimated_soc` is explicitly allowed to diverge from the equipment's
ground-truth SOC.

Three concrete implementation gaps compound to produce the exact symptom
reported, and none of them are inherent to the driver/equipment split — they
are gaps in how the driver reports its own state:

### 1. `charge_kw` only recognizes rate signals, but most strategies never emit one

`populate_telemetry` (mod.rs:391-439) sets `charge_kw` by scanning only the
dispatch requests newly emitted *this step* (`out[before_out..]`) for
`ControlSignal::PowerSetpoint` or `ControlSignal::EvAwayCharge`
(mod.rs:404-424). But of the nine `ChargingStrategy` variants, the
preferences that drive six of them — `SocTarget` (soc_target.rs:9-26, used by
`Immediate`, `Nightly`, `PreDeparture`, `TouAware`), `SocGate`
(soc_gate.rs:46-61, used by `LowSoc`, `QuickThenWait`), and
`DepartureDeadline`'s non-urgent `score()` path (departure.rs:136-160, used
by `PreDeparture`/`TouAware` outside the urgency buffer) — vote only
`target_soc`/`departure_hour`, never `power_kw`. `ChargingComposer::emit_vote`
therefore emits only `SOCTarget` or `EvSetReadyBy` for these
(composer.rs:201-212, 312-323), which `populate_telemetry`'s match arm
ignores entirely (`ControlSignal::EvDrive { .. } => {} // driving, not
charging` is the only other arm before the catch-all `_ => {}`,
mod.rs:421-423). Only `PriceOptimizer` (price.rs:73-104), `SolarTracking`
(solar.rs:10-29), `V2GExport` (v2g.rs:20-27), and `V2HDischarge`
(v2h.rs:20-37) vote `power_kw`, and `DepartureDeadline`'s urgency/buffer
*override* (departure.rs:105-133) also emits `power_kw`. So `charge_kw`
telemetry is structurally `0.0` for `Immediate`, `Nightly`, `LowSoc`,
`QuickThenWait`, and for the ready-by lead time of `PreDeparture`/`TouAware`
outside the urgent window — i.e. for the strategies that actually govern
DailyCommuterL1-style scenarios. The equipment computes and delivers real
power from the `SOCTarget`/`EvSetReadyBy` goal on its own BMS clock, but that
resulting power is never read back into the driver's telemetry.

### 2. `needed_charge_hours` defaults to a sentinel that is indistinguishable from "already done"

`ChargingPreference::needed_charge_hours` defaults to `f64::INFINITY`
("no estimate available", preference.rs:73-79). Only `DepartureDeadline`
overrides it with a real calculation (departure.rs:40-67, wired through
`ChargingPreference::needed_charge_hours` at departure.rs:162-164).
`ChargingComposer::evaluate` folds the minimum across all active preferences
into `last_needed_charge_hours` (composer.rs:65-71), and
`last_needed_charge_hours()` maps a still-infinite result (no preference
provided one) to `0.0` (composer.rs:50-56) — a getter comment even says
"the getter maps non-finite results to `0.0` so the telemetry value is
always finite." `DepartureDeadline` is used by `PreDeparture`, `TouAware`,
and `SolarSurplus` — so `SolarSurplus` reported a real (to-full) estimate
all along and is **not** affected. The strategies whose stacks contain no
estimating preference — `Immediate`, `Nightly`, `LowSoc`, `QuickThenWait`,
`V2G`, `V2H` — report `needed_charge_hours
== 0.0` regardless of the actual SOC gap, because "no estimate" and "zero
hours needed" collapse to the same value. (Correction: an earlier version
of this section listed `SolarSurplus` among the affected strategies;
`build_preferences` has always installed a `DepartureDeadline` in its stack,
whose estimate the fold picked up — see the decision log.)

### 3. `soc` telemetry only decreases in real time; it only catches up at two sparse events

`estimated_soc` is decremented every driving step (mod.rs:903-904,
`Driving` phase) but is never incremented while `HomePluggedIn` and
charging. It is only pulled back up to the equipment's real SOC by
`reconcile_soc()` (mod.rs:613-630), and that is only called from two sites:
on arrival (`Away → HomePluggedIn` transition, mod.rs:977) and once per
calendar day, only if the driver is already `HomePluggedIn` at the moment
the day rolls over (mod.rs:465-478). There is no per-step update of
`estimated_soc` for the (typically many-hour) span during which the vehicle
is actually charging at home between those two trigger points. On the
DailyCommuterL1 schedule (arrive evening, depart next morning, charging
spans the day boundary at most once), the reported metric — "does `soc` rise
on a step where the equipment is actively charging" — is essentially never
true: the one legitimate jump per day happens at whatever instant the day
rolls over, decoupled from the actual charging profile, and the fields
already established in (1) mean the same step also reports `charge_kw ==
0.0`, so it doesn't read as a "charging interval" either. The HPXML-route
detail (pack reads `0.000` from day 8 onward while the equipment still
delivers 234 of 318 kWh) is the same gap taken to its extreme: once
`estimated_soc` clamps to `0.0` during a `Driving` step
(`.max(0.0)`, mod.rs:904), it can persist for many days if the specific
departure/arrival/day-boundary timing on that route keeps missing both
reconciliation triggers, with nothing in between to correct it even though
the equipment (and its real SOC, readable via `actual_soc()`,
mod.rs:600-605) is charging normally the whole time.

`resolve_equipment_id` is called identically for both construction routes
from the same `build_actors` code path (`crates/hares-core/src/dwelling/mod.rs:6931`,
keyed by the same `name` used for both the dispatch target and equipment
registry lookup), so this is not a route-specific wiring bug — it is the
same architectural gap in both cases, just surfaced with different severity
depending on each route's day/trip timing relative to the two reconciliation
trigger points.

### Cross-check against EnergyPlus and OCHRE (parity references)

EnergyPlus (`vendors/EnergyPlus`) has no native electric-vehicle/EV-charger
object (`grep`-ing the EnergyPlus source for "ElectricVehicle" turns up
nothing); it isn't a source of EV telemetry conventions here. OCHRE is the
applicable reference since it implements EV charging physics on top of
EnergyPlus-driven building loads.

`vendors/OCHRE/ochre/Equipment/EV.py` has no driver/equipment split: `soc`
and `electric_kw` are both properties of the single `EV` equipment object,
and both are recomputed every timestep directly from the applied control
(`calculate_power_and_heat`, EV.py:290-305: `self.electric_kw = ac_power`;
`self.next_soc = self.soc + dc_power * hours / self.capacity`, applied via
`update_results`, EV.py:355-359). There is no telemetry sentinel collapse
and no "estimate that only catches up at two events" — OCHRE's SOC and power
outputs are live and correct on every step. HARES's driver/equipment split is
a deliberate, documented design choice (mod.rs:11-21) and is not itself the
defect, but it does mean the driver's *own* telemetry needs to be
deliberately kept faithful to what's really happening — which it currently
is not for the majority of strategies. Matching (or exceeding) OCHRE means
the caller-visible EV state should not require falling back to the
equipment's own output column to know whether the car is charging.

## Alternative causes considered and ruled out

- **Equipment never actually charges (a real physics bug, not a telemetry
  bug).** Ruled out: the ticket's own workaround ("we read charging from the
  EV End Use Electric Power (kW) column instead, so we're unblocked") shows
  real, nonzero charging power is present in the equipment's own output;
  only the driver actor's telemetry is flat. `hares-equipment/src/ev/mod.rs`
  writes `CoreState.soc` and the electric flow every `step()`
  (mod.rs:1395-1411, `soc: Soc::try_from(self.soc).ok()` at line 1406), so
  the equipment side is live and was not the thing investigated further.

- **`Telemetry::set`/`get` has a key-mismatch or plumbing bug** (e.g. the
  actor writes one key and the reader reads another, or `set` silently
  no-ops). Ruled out by direct inspection: `populate_telemetry` writes
  `self.telemetry.set("soc", self.estimated_soc)` (mod.rs:392) as a direct,
  unconditional passthrough of `estimated_soc` — no key indirection, no
  conditional skip. The reproduction below also reads the same `"soc"` and
  `"charge_kw"` keys back out via `Telemetry::get` and observes the exact
  values the source predicts, confirming the read path is not the fault.

- **Route-specific wiring: `equipment_id` fails to resolve on the HPXML
  construction route, so `actual_soc()`/reconciliation silently no-ops only
  there** (this would explain the day-8-to-empty HPXML detail without
  implicating the archetype route). Ruled out as the *primary* cause,
  though not fully as a contributing one: both routes build EV actors
  through the same `build_actors` function in
  `crates/hares-core/src/dwelling/mod.rs`, and both call
  `actor.resolve_equipment_id(equipment_id_by_name)` (line 6931) keyed by
  the same `name` used for the dispatch target — there is no route-specific
  branch in this path. This alone doesn't prove resolution *always*
  succeeds on the HPXML route (that would require running the actual HPXML
  fixture, which is out of scope for a driver-level reproduction), but it
  does rule out "the two routes wire EV lookup differently" as the
  explanation, since the ticket already reports the identical symptom (flat
  `charge_kw`/`needed_charge_hours`) on the archetype route, where wiring is
  known-good ("plugged_in and phase work"). The reproduction below isolates
  the mechanism without depending on either route's wiring at all — it
  drives `EvDriverActor` directly, so it is unaffected by whatever
  route-specific equipment-lookup risk remains open.

- **The three fields are simply not intended to be populated for
  target-based strategies, and the report is a misreading of a documented
  divergence-by-design.** Considered because `mod.rs:11-21` explicitly
  documents that `estimated_soc` is allowed to diverge from equipment SOC.
  Ruled out as a full explanation because the module doc only justifies
  *SOC estimation error* (missed CC-CV taper, thermal derating) — a bounded
  drift — not "never updates for the entire duration of a charging session."
  The reproduction below shows the estimate doesn't drift from truth, it is
  completely inert (identical `0.5` for 200 consecutive steps) while ground
  truth moves from `0.5` to `0.95`, which is a different and much larger
  effect than the documented divergence tolerance (mod.rs:1008-1029 asserts
  a 15% divergence warning threshold; the reproduction's terminal gap is
  45 percentage points with zero corrective mechanism in view).

## Reproduction

Reproduced directly against `EvDriverActor` (bypassing full dwelling/HPXML
setup, to isolate the actor's own telemetry logic from route-specific
wiring). A temporary `#[test]` was added to
`crates/hares-core/src/actors/ev_driver/mod.rs`'s test module, run once, and
then removed — no test or code change is left in the tree (`git status`
confirms the file is clean).

Test body: build an `EvDriverActor` with `ChargingStrategy::Immediate {
target_soc: 0.9 }`, put it in `DriverPhase::HomePluggedIn` at `estimated_soc
= 0.5`, and step it 200 times at one-minute resolution while independently
driving the mocked equipment-core SOC (`set_core_soc`) from `0.5` up to
`0.95` — i.e. simulating the equipment actually charging the whole time.
After each `decide()` call, read `telemetry().get("charge_kw")` and
`telemetry().get("soc")`.

Command:
```
cargo test -p hares-core --lib actors::ev_driver::tests::rca_i04_repro_charge_kw_and_soc_flat_while_charging -- --nocapture
```

Captured output:
```
running 1 test
charge_kw ever nonzero: false
soc series (first 10): [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]
soc series (last 10): [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]
soc rose at any step: false
```

This confirms both of the mechanisms named above directly: across 200 steps
of active charging (equipment SOC moving 0.5 → 0.95), the driver's own
`charge_kw` telemetry never leaves `0.0` (mechanism 1 — `Immediate` routes
through `SocTarget`, which never votes `power_kw`, so `populate_telemetry`'s
`out[before_out..]` scan at mod.rs:404-424 never sees a `PowerSetpoint`), and
the driver's own `soc` telemetry never moves off its stale `0.5` (mechanism
3 — no day boundary or arrival occurred in this window, so `reconcile_soc`
at mod.rs:613-630 was never invoked, and nothing else updates
`estimated_soc` while `HomePluggedIn`). `needed_charge_hours` was not
re-verified experimentally beyond the source trace in mechanism 2, since
`Immediate`'s only preference (`SocTarget`) demonstrably has no
`needed_charge_hours` override (soc_target.rs has no such method, so it
inherits the `f64::INFINITY` default at preference.rs:77-79) — the
composer-level fold-and-collapse to `0.0` (composer.rs:50-56) is a pure
function of that input and was already exercised by the existing
`composer.rs` unit tests (e.g. `last_needed_charge_hours` test at
composer.rs:559-561 confirms the fold arithmetic), so a redundant runtime
check would not add evidence beyond the source trace.

## Root cause

`EvDriverActor`'s telemetry for `charge_kw`, `needed_charge_hours`, and
`soc` was written under an implicit assumption that the driver's own
per-step `PowerSetpoint`/`EvAwayCharge` dispatches and its own SOC estimator
would track real charging, but for goal-based strategies (`Immediate`,
`Nightly`, `LowSoc`, `QuickThenWait`, and the ready-by/non-urgent portion of
`PreDeparture`/`TouAware`) the driver never emits a rate signal at all — the
equipment's independent BMS computes and applies the real charging power —
and the driver's own `estimated_soc` is only reconciled to that ground truth
at two sparse, schedule-dependent trigger points (arrival and
day-start-while-plugged-in) rather than every step. This is not a single
line bug; it's three related telemetry gaps in the driver actor
(`crates/hares-core/src/actors/ev_driver/mod.rs` and `composer.rs`) that
each independently zero out one of the three reported fields, and together
they make the driver's own state channels an unreliable proxy for whether
the vehicle is actually charging — a violation of the "actors must emit
telemetry for inspecting dispatched actions/controls" principle for exactly
the class of dispatches (`SOCTarget`, `EvSetReadyBy`) most strategies use.

No failing test is left in the tree by this diagnosis (the scratch
reproduction above was added, run, and reverted). The reproduce step owns
turning the reproduction above into a permanent failing test; the fix step
owns deciding and implementing the correction — neither is this document's
job.
