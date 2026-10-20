# Axis A: co-simulation and grid operations (the rows' full specs)

Source: the I-09 planning round, 2026-10-17, verified against the tree at
`0911b711`. The HELICS surface exists and is hardened on the lifecycle side
(timeout-bounded federate calls `python/hares/helics/federate.py`, broker
port hygiene, single-dwelling and fleet-as-one-federate with per-dwelling
subscriptions and per-equipment publications, a HELICS-CLI runner, ~5,300
pytest lines). The gaps are grid-facing semantics, not lifecycle plumbing.

## A1: the grid-signal input channel (M)

**Problem.** Frequency does not exist as an input: `set_grid_voltage`
overwrites `frequency_hz` with a 60 Hz constant every call
(`dwelling/mod.rs:4991-4998`), no `set_frequency` anywhere; the fleet
federate has no price subscription (`fleet.py:295-349`), the dwelling
federate never applies `export_price` or `ghg_intensity`
(`dwelling.py:747-758`); boundary guarding is inconsistent (the dwelling
refuses out-of-range voltage, `dwelling.py:725-741`; the fleet warns and
applies anyway, `fleet.py:494-522`; a negative price warns then applies).

**Change.** One boundary contract applied identically in both federates:
validate once at the HELICS boundary, apply never on violation, count every
violation in the per-run `HELICSDiagnostics` row. Add the
`grid/frequency` channel end to end (subscription, `set_frequency` on
`Dwelling` and `SteppableFleet`, stored in `GridState`); price and
export-price topics on both federates, per-dwelling price topics on the
fleet mirroring the per-dwelling voltage pattern; the fleet's apply-anyway
paths brought in line.

**Acceptance.** Mock-aggregator federation tests drive a 0.7 pu sag, a 0.0
outage, a 59.3 Hz event and import/export price into both federates; every
out-of-band value counted once, never applied; a fleet run's per-dwelling
price changes the dwelling-level bill.

**Measurement.** The per-run unapplied-signal counts; the per-step log
volume before/after on a 1-year federation.

**Dependencies.** I-08 hardening Task 1.6 first (the guard is built once);
the frequency channel matters downstream of Task 4.6.

## A2: grid-event scenario scripting (M)

**Problem.** No scenario surface exists: outage reaches co-simulation only
through the setter (`outage-behavior.md:15`; the topic refuses 0.0 pu),
frequency cannot be scripted at all, and every study is hand-written Python
calling setters in a loop with no shared event semantics, validation or
replay discipline.

**Change.** A declarative event script (TOML or JSON) with time-windowed
entries: `voltage_pu` (including 0.0), `frequency_hz`, import/export price,
DR events, named control signals (the existing vocabulary). One engine
evaluates per step and drives either the direct setters (standalone, the
gym, batch) or a small HELICS source federate publishing the same signals,
so the identical script runs standalone and inside a federation.
Deterministic replay by construction; malformed scripts fail with typed
errors naming the field and line.

**Acceptance.** A scripted brownout-then-outage-then-restoration scenario
produces bitwise-identical telemetry standalone and through an in-process
broker federation; a malformed script (overlapping events, unknown signal,
non-monotone times) fails naming the entry.

**Measurement.** Replay equality is the gate; the per-step engine overhead
vs hand-rolled setter calls recorded once.

**Dependencies.** A1 (the frequency/price vocabulary; a voltage-plus-price
subset can land first); Task 1.6.

## A3: the co-simulation contract (M)

**Problem.** During an island the published `total_power_kw` is the
island's internal imbalance (`dwelling/mod.rs:5252`, `:7683-7685`), not the
grid interchange (physically zero at the disconnect): a feeder federate
sees a phantom load exactly during the disturbances a grid study exists to
examine. Nothing publishes `islanded`, `bus_voltage_pu`,
`island_unserved_kw`, `island_excess_kw`. The temporal contract is
folklore: the period inferred from the first two timesteps
(`dwelling.py:972-1001`), intermediate grants publishing without stepping
(`:583-588`), drift accumulating silently (`:592-606`).

**Change.** (a) The new grid-facing publications (`grid_interchange_kw`
exactly 0.0 whenever the utility is out, `islanded`, `bus_voltage_pu`,
`island_unserved_kw`, `island_excess_kw`) on both federates, per-dwelling
on the fleet. (b) `docs/cosim.md`: every topic, unit, range, stale-value
semantics at intermediate grants, control queueing rules, the
outage-via-setter rule, the 1 s cadence floor, the fixed-cadence
(not event-driven) coupling statement. (c) A criterion bench measuring the
per-step publish/subscribe overhead against standalone stepping.

**Acceptance.** An outage-then-island federation scenario: the
feeder-side interchange reads exactly 0.0 from island formation while the
premise-balance topic continues; the multi-rate test extended to staleness
and control timing; the drift guard green.

**Measurement.** The federation tax (ns/step, allocations/step) in the
measurement log; the allocation-free rule applies to the hot path.

**Dependencies.** None hard; the doc is written against A1's topic names.

## A4: per-DER trip observability end to end (M)

**Problem.** I-08's Task 4.6 lands IEEE 1547 trip/ride-through with
per-DER trip state in engine telemetry, but nothing else sees it: no trip
keys in `telemetry_keys.rs`, no trip/island columns in the catalogue
(`catalogue.rs:1227-1232`, `:1385`), the federates publish
power/SOC/mode only, the fleet fold has no trip aggregation. A sag study
tripping 40 percent of a feeder's PV cannot produce the trip table the
study exists for.

**Change.** Carry the per-DER trip state (state, trip time,
time-to-reconnect) through the recorded output columns, the HELICS
publication surface (A3's contract), the gym observation plan, and a
fleet-level fold (per-step tripped counts by DER type, trip-duration
distributions). No new physics.

**Acceptance.** A 0.6 pu sag scenario recorded end to end: the output
columns match engine telemetry bitwise; a 100-dwelling fleet run yields
per-step trip counts and a trip-duration histogram; the scenario script is
stored with the run for provenance.

**Measurement.** The column set rides the existing pipeline; the fleet
histogram is the deliverable.

**Dependencies.** I-08 Task 4.6; A3; A2 for the scenario provenance.

## A5: load behavior below 0.9 pu (M)

**Problem.** The channel admits [0.5, 1.5] pu and the outage doc admits
sustained brownouts, but the load model clamps at [0.9, 1.1] pu with a
per-step warn (`electrical_solver.rs:13-32`, `:135-162`): a 0.7 pu sag
behaves as 0.9 pu every step forever, motor loads never stall, and a
brownout run emits one warning line per dwelling per step for the run's
duration. DERs trip correctly (Task 4.6) while the loads behind the meter
respond wrongly.

**Change.** Decide and implement the sub-0.9 pu contract family by family,
recorded in `docs/PHYSICS_DECISIONS.md`: Z/I/P components scale unclamped
across the admitted band for ZIP-governed loads; motor-driven families get
an explicit stall/lockout behavior with its draw and thermal consequence or
a documented non-model; the per-step warn becomes a counted event (the
Task 2.11 clamp-census pattern).

**Acceptance.** Brownout vectors at 0.5/0.7/0.88 pu: a pure-Z load draws
V-squared scaled, a pure-P load holds, the motor families show the decided
behavior, no per-step warn fires; goldens at 1.0 pu stay bitwise.

**Measurement.** Clamp/warn counts before/after on a sag scenario; the
nominal goldens' diffs empty and any sag fixture explained.

**Dependencies.** After Task 4.6 (shares its sag vectors); must precede A7
(same file).

## A6: the named co-sim surface and the reference pandapower adapter (L)

**Problem.** The topic schema is positional and heuristic: equipment topics
are `equipment_{i}/...` by index (`dwelling.py:479-506`, `fleet.py:258-283`)
and PV is "identified heuristically from equipment name and negative power"
(`dwelling.py:919-931`), so a PV array named anything but pv/solar drops
out of the co-sim export and every consumer hardcodes indices. No
discovery, no worked federation example, no reference adapter.

**Change.** (a) A stable, named publication schema: equipment topics carry
the equipment name and a role tag (pv, battery, ev, hvac, wh, load); index
topics remain for backward compatibility. (b) Role discovery: one
registered JSON publication listing each federate's roles, names, units.
(c) A reference adapter federate for pandapower (pure Python,
pip-installable): subscribes dwelling interchange and reactive, runs a
power flow per step, publishes per-dwelling bus voltage back; CI runs it
as an in-process federation; OpenDSS follows the pattern later.

**Acceptance.** A 3-dwelling 4-to-6-bus pandapower federation steps end to
end in CI; a PV named `roof_array` lands under the pv role; the adapter
README journey works copy-paste from a clean venv; the A2 scenario engine
drives the federation's disturbance.

**Measurement.** The adapter round-trip cost per step recorded once; CI
runtime bounded.

**Dependencies.** A3; A1; A2 for the CI scenario.

## A7: the service-entrance electrical model (L)

**Problem.** The dwelling's electrical surface is one aggregate load scale
and net P/Q (`electrical_solver.rs:194-219`): panel congestion, breaker
headroom, per-leg imbalance and apparent-power curtailment are unanswerable
from HARES, and they are premise physics, exactly what a
service-transformer or panel-upgrade study needs from every premise.

**Change.** A service-entrance layer at the dwelling boundary: a 120/240
split-phase model mapping each equipment's connection to legs, per-leg RMS
currents, a configurable main-breaker apparent-power limit with curtailment
that is counted (never silent), and an S-squared-equals-P-squared-plus-Q-
squared consistency invariant joining the existing electrical checks.
Recorded columns and observation fields per the A3/A4 pattern.

**Acceptance.** An unbalanced-leg scenario yields per-leg currents
consistent with the equipment roster; exceeding the panel limit curtails
with the curtailment counted and the invariant green; a zero-curtailed run
reproduces the existing goldens bitwise.

**Measurement.** Before/after on the single-building bench (the panel
solve must be allocation-free).

**Dependencies.** After A5 (both touch `electrical_solver.rs`).

## A8: the HELICS wheel-workaround watchdog (S)

**Problem.** The helics 3.6.1 wheel defect's workaround depends on the
wrapper's private `_finalizer` and `PYHELICS_FREE_ON_DESTRUCTION`
(`federate.py:259-276`); a wheel update silently degrades to a leak path.

**Change.** Import-time feature detection of each assumed behavior, one
loud warning listing what was not found; a CI job running the
federate/broker teardown tests against each supported 3.6.x wheel; drop
the workaround and widen the pin when the fixed wheel ships.

**Acceptance.** The CI matrix green across 3.6.x; a simulated wrapper
without `_finalizer` produces exactly one warning and correct behavior.

**Dependencies.** None; lands any time, ideally first.

## Deliberately not proposed

No service-transformer or secondary-feeder model (the power-flow partner
owns it; HARES's boundary is the meter); no built-in power-flow solver
(co-simulation is the mission; A6's adapter is the bridge); no sub-second
or event-driven re-stepping (the honest floor is 1 s; E+ transients belong
to dedicated tools; A3 documents the fixed-cadence contract); no
volt-VAR/volt-Watt curve configuration API yet (propose once 4.6's layer
has been exercised by A4); no GLM/DSS file parsers (the runner config is
the coupling point); no gas-grid coupling.
