# Review 4/6: Grid, EV, PV, battery, tariff

Read-only deep review, post-I-08. Read `docs/outage-behavior.md`, `docs/equipment/{battery,ev,pv,power-factor}.md` first; verified against `crates/hares-equipment/src/{battery,ev,pv,pack_electrical}.rs`, `crates/hares-core/src/{dwelling,actors/ev_driver,actors/bms}.rs`, `crates/hares-tariff/src/`. Confirmed pre-fixed: tariff step-end time (`dwelling/mod.rs:7988-7991`), fleet Mode folding. Positives: outage gating architecture (control-level gating, `effective_load_scale` leak visibility), island unserved/excess accounting, trip-cancel honesty (`drive_cancelled`, `drive_shortfall_kwh`), tariff proration/ratchet/demand-window machinery - defects concentrate at integration seams, not core step physics.

## CRITICAL

**C1. EV can form an island while delivering zero watts - island formation keys on a commanded flag, not capability.**
- `crates/hares-equipment/src/ev/mod.rs:1754-1764` - `island_source_available()` = `HomePluggedIn && v2l_active && soc > 0.0`. But `v2l_active` is set to `power.is_discharge` (`:1812`), true whenever a negative setpoint is latched with V2G/V2L enabled - even when the export was curtailed to 0 W by the SOC floor (`compute_discharge` returns `(0.0, 0.0)` at `:1142-1145`) or by DR (`-(export * dr)` with `dr = 0`, `:1175`).
- The dwelling forms the island from end-of-previous-step availability (`crates/hares-core/src/dwelling/mod.rs:6697-6703`). A car drained to its V2G reserve (0.3) or V2L floor with a latched V2H export vote holds the bus at 1.0 pu for the entire outage while delivering nothing: every load runs at full power, delivers real heat/water, and the deficit appears only as `island_unserved_kw` (energy from nothing, every step). Violates the outage doc's own invariant ("A dead bus ... implies the battery is empty/cold/disconnected") and is inconsistent with the battery's rigorous availability check (`battery/mod.rs:1159-1176`: SOC above floor, derate > 0, DR fraction > 0).
- Fix: require delivered export - `self.v2l_power_kw > 0.0` plus `soc > effective_discharge_floor(leg)` and `dr_power_fraction() > 0.0`, mirroring the battery's check. Dwelling test in `crates/hares-core/tests/grid_outage_islanding.rs`: EV at its reserve floor under a latched negative setpoint asserts the bus stays dead.

## MAJOR

**M1. Islanded EV charging bypasses the surplus clamp - phantom energy credited to the pack.**
- `ev/mod.rs:1802-1808` gates EV charging only on `!bus_energized()`; islanded (bus energized) charging proceeds at full EVSE rate. The stationary battery clamps its charging target to the visible Stage-1 surplus during an outage (`battery/mod.rs:1238-1240`).
- Islanded on a 5 kW battery, an 11.5 kW EVSE charges at full rate; the 6.5 kW deficit lands in `island_unserved_kw`, yet `apply_soc_and_thermal` (`:1608-1622`) still credits the full charge energy to the SOC - the pack gains energy no source produced until the island collapses.
- Fix: apply the same surplus clamp to the EV charge leg when `env.grid.grid_outage() && bus_energized()` (cap `charge_leg_ac_kw` at visible surplus; away charging unaffected).

**M2. Tariff import price silently falls back to $0/kWh on schedule/coverage gaps.**
- `crates/hares-tariff/src/evaluator.rs:199-225` - a TOU hour matching no period (schedule gap) yields `import_price = 0.0`; a matched period with no `energy_rates` entry for that season also `.unwrap_or(0.0)`. Same for NetBilling export credits (`:248-262`). `ElectricTariff::validate` (`types.rs:356-448`) checks that rates reference known periods but never the converse (period coverage) nor full-hour coverage.
- A hand-built tariff (Python bindings allow arbitrary `tou_schedule`/`energy_rates`) with a season gap silently bills free electricity - directly corrupts every DR/RL reward signal built on the bill.
- Fix: at `validate()`, require every TOU period (per season) to have an energy rate and the schedule to cover 24x7 (or error loudly at precompute when a step resolves to no rate).

**M3. The EV TOU rate is dead code at the dwelling integration - a configured EV rate silently never applies.**
- The only dwelling call is `evaluator.step(net_kw, 0.0, dt_secs, current_tz)` (`dwelling/mod.rs:7991`) - `ev_power_kw` hardcoded `0.0`. The EV-rate machinery (`ev_tou_period_name` validation `types.rs:440-447`, per-step EV price `evaluator.rs:227-240`, blended pricing `evaluator.rs:497-515`) is reachable only from unit tests (`checkpoint.rs:1003`, `snapshot.rs:562`).
- A user configuring an EV-TOU rider gets EV kWh silently billed at the house rate.
- Fix: pass the summed EV end-use signed flow (already computed at `dwelling/mod.rs:8007-8008`) into the tariff step, or reject `ev_tou_period_name` at attach time with "EV submetering not wired".

**M4. No voltage- or frequency-dependent grid failure physics: brownouts are invisible to every DER.**
- `GridState.frequency_hz` (`hares-types/src/environment.rs:377`) is written and exposed to Python but consumed by no equipment or solver. `voltage_pu` consumers are exclusively ZIP scaling and `bus_energized()` gating; `grid_outage()` is exact `== 0.0` (`environment.rs:401-403`). PV/battery/EV inverters never trip on under/over-voltage: at 0.6 pu the PV still exports full ZIP-unscaled power (`pv/mod.rs:1107` gates only on `bus_energized()`).
- The outage doc and HELICS filter ([0.5, 1.5] pu) admit sustained brownouts/sags, but IEEE 1547 trip/ride-through windows, UF/OV load shedding, frequency-Watt droop don't exist. A co-simulation feeding a realistic sag profile gets physically wrong DER behavior with no error.
- Fix: IEEE 1547-2018 Category mappings (V/HERT, f/WRT) evaluated from `bus_voltage_pu()`/`frequency_hz` in the three inverter families, with an explicit "no trip tables configured" warning rather than silence.

**M5. `Dwelling::set_grid_voltage` accepts NaN/inf/negative voltage - NaN silently un-gates every load.**
- `dwelling/mod.rs:4991-4998` -> `environment.rs:1149-1151` stores the override raw. NaN propagates: `bus_voltage_pu()` returns NaN (`:410-416`), `bus_energized()` = `NaN != 0.0` = true (`:424-426`), `grid_outage()` = false - an intended outage signaled with a NaN override runs the house at full load, islands nothing, no error.
- This is THE documented outage setter (`outage-behavior.md` names it). A gym/co-sim caller computing voltage from a model that produces one NaN poisons the entire outage machinery silently.
- Fix: validate in `set_grid_voltage` - exactly `0.0` or a finite value in a plausible pu band ([0.5, 1.5] to match the HELICS contract) - `HaresError` otherwise. Also make `bus_energized()` NaN-robust (`bus_voltage_pu() > 0.0`).

**M6. Price-driven V2G/V2H/TOU votes use the import price and silently treat a missing price signal as $0.**
- `ev_driver/v2g.rs:13` `price = ...electricity_price.unwrap_or(0.0)` (missing price => never exports); `ev_driver/price.rs:76` same fallback (missing price => always "charge" at full score). Export decisions price against the import retail rate, while the dwelling publishes a separate `export_price` (`dwelling/mod.rs:6677-6680`) and NetBilling credits can be far below retail.
- Under net-billing, V2G export dispatch is systematically mistimed/overvalued; a missing price signal silently flips strategic behavior.
- Fix: export votes read `price_signal.export_price`; a `None` price marks the actor unhealthy (the `estimate_error` pattern) rather than substituting 0.0.

## MINOR

- **m1.** PV default inverter priority `Var` curtails real power to hold vars, and the three modes disagree at P=0: `pv/mod.rs:325` defaults `InverterPriority::Var`; battery/EV kVA clamps are active-power-priority by contract (`battery/mod.rs:1413-1418`, `ev/mod.rs:1228-1231`). At P=0, Watt and Var force Q->0 (`enforce_min_pf` `:626-628`; `:601`) while Cpf yields full-Q-at-P=0. Default `Watt`; document the P=0 behavior per mode.
- **m2.** Driver actor sizes discharge against the charge rating: `ev_driver/mod.rs:1093` `max_discharge_kw: self.max_charge_kw`. Equipment clamps so physics is safe, but V2H/V2G votes overstate capability - dispatch-vs-delivered mismatch is what an RL consumer regresses on. Carry a real discharge ceiling.
- **m3.** Battery efficiency defaults disagree pre- vs post-init: `battery/mod.rs:483-484` charge = discharge = sqrt(0.97) (round-trip 0.97), but `init_typed` (`:1027-1038`) applies `inverter_efficiency` 0.97 each way (round-trip 0.9409). `docs/equipment/battery.md` matches post-init. Share `fn default_one_way_eta()`.
- **m4.** Self-discharge at the SOC floor is refunded by the bound clamp: `battery/mod.rs:1290-1291` subtracts self-discharge, `:1311` clamps to `[eff_min_soc, eff_max_soc]` - an idle battery at min_soc gets its self-discharged energy restored free every step. Apply self-discharge after the clamp or skip at the floor.
- **m5.** `bms_ready_by_power` is bang-bang with a +-1 h past-deadline grace window (`ev/mod.rs:1064-1079`): full rated power only when `hours_needed >= hours_until_deadline`, else 0; `(current_hour - ready_by_hour).abs() < 1.0` grace keeps charging at full rate up to an hour past a latched deadline whose departure never fired. Proportional pacing; clear `ready_by_hour` when the driver cancels the day's trip.
- **m6.** Island clamp timing hole: the Stage-1 surplus excludes later-stepping Electrical/Electrical-stage loads (EV, HPWH elements). Documented (`outage-behavior.md` "accounted, not faked"), but combined with M1 the clamp is applied against a surplus that later loads will themselves consume - consider ordering the clamp against the post-step meter.

## Top 5 improvements (with what a DR/RL gym consumer needs)
| # | Opportunity | Effort | Payoff |
|---|---|---|---|
| 1 | Marginal-price + demand-charge observability in `price_signal`: current marginal tier price, distance to next tier (kWh), running demand-window average, period peak, peak-so-far (fields already exist in `BillingState`) | S/M | A bill-based RL agent needs the shadow price of its next kW and next kWh |
| 2 | IEEE 1547 voltage/frequency response layer (M4): trip/ride-through + frequency-droop for the three inverter families; expose `bus_voltage_pu`, `frequency_hz`, per-DER trip state in env/telemetry | M | Realistic disturbance channel (sags/frequency events) with observable inverter states |
| 3 | Hard island power balance by priority class (C1+M1): extend the surplus clamp to EV charging, optional load-shed priority per equipment, `island_unserved_kw` per equipment (currently aggregate only) | M | Islanded microgrid episodes trainable without phantom-energy reward hacking |
| 4 | Dispatch-vs-delivered honesty: per actor-dispatched setpoint, delivered P/Q and the clamp reason (floor, derate, kVA, DR) - one telemetry channel from `clamp_power`/`compute_discharge`/`apply_inverter_limits`; fix m2 | S/M | RL off-policy methods need the saturating-constraint identity |
| 5 | Service-entrance electrical model: per-phase (split-phase) currents, panel/breaker kVA limit with apparent-power curtailment, meter-level S^2=P^2+Q^2 consistency check (today one load scale and net P/Q only, `hares-envelope/src/electrical_solver.rs:194-219`) | L | Congestion/transformer/panel-upgrade studies; line current and breaker-headroom observations |

## Missing tests / validation
1. Zero-capability EV island formation (C1): no dwelling test holds an EV at its discharge floor under a latched negative setpoint during an outage and asserts bus collapse (`grid_outage_islanding.rs` covers only battery cases, `:225`, `:265`, `:309`).
2. Islanded EV charging conservation (M1): no test equates islanded SOC gain against island generation (`dE_pack <= E_sources`); a property test over a battery+EV+PV island would have caught it.
3. Tariff coverage gaps (M2): no test for a TOU schedule hour with no matching period, or a matched period with no seasonal rate. No test that a configured `ev_tou_period_name` changes a dwelling-level bill (M3).
4. Brownout/frequency vectors: nothing exercises 0.5-0.88 pu or 59.3/60.5 Hz; no asserted contract at all until M4 lands.
5. `set_grid_voltage` input validation (M5): no test that NaN/negative overrides are rejected.
6. Ready-by guarantee under taper: no end-to-end test that a LUT-tapered cold pack reaches `ready_by_soc` by departure; cancelled trip clears the deadline grace (m5).
7. PV LUT nearest-neighbor fallback accuracy: counted (`pv/mod.rs:1244-1250`) but never validated against reference points outside the grid.
8. V2G export economics: no test that export votes respond to `export_price` rather than `import_price` (M6); existing V2G tests vary only `electricity_price`.
