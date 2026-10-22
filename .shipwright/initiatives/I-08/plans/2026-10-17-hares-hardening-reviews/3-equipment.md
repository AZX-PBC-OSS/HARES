# Review 3/6: Equipment - HVAC, hot water, lights/TVs/appliances (crates/hares-equipment)

Read-only deep review, post-I-08. Docs read first (`docs/equipment/{hvac,water-heater,loads,power-factor}.md`, `docs/thermal_gain.md`), then verified against code and vendored OCHRE (`vendors/OCHRE/ochre/Equipment/{HVAC,WaterHeater}.py`). The crate is materially better than the slop era: typed configs, typed errors, conservation invariants, OCHRE line-cited constants.

## CRITICAL

**C1. Defrost penalty is double-counted: the continuous model and the discrete FSM both apply, compounding each other.**
- `hvac/heat_pump/heater.rs:1927-2036` - when the tracker is Accumulating and `hp_on`, the continuous `evaluate_defrost` still multiplies capacity by `0.875*(1-tf)` and adds `extra_power_w` (`:2029-2035`). When Defrosting, capacity -> 0 and power -> `defrost_eir*rated/1.01667` at full intensity. The tracker is unconditionally active (`heater.rs:664, 1360`) with interval `= cycle_duration_s / ewma_tf` (`defrost.rs:300-302`), i.e. discrete duty ~= tf.
- `defrost.rs:229-233` claims the discrete model "produces the same average time-in-defrost as the continuous model" - the discrete phases were meant to *replace* the continuous average, not stack on it. At tf ~= 0.24: continuous-only penalty is 33% of capacity; composed penalty `(1-0.24)*0.875*(1-0.24)` ~= 50%, and defrost electric is continuous extra-power *plus* full-intensity events 24% of the time. Systematic cold-climate under-delivery of ASHP/MSHP heating and over-metering of defrost energy vs both OCHRE and EnergyPlus ERM 26.1. The parity test only asserts "positive average output" (`tests/hvac_parity.rs:938-956`) - cannot catch this.
- Fix: pick one model - (a) when the discrete tracker is enabled, `evaluate_defrost` feeds only the frost accumulator and never applies its multipliers, or (b) discrete FSM opt-in via `DefrostConfig`. Add an accumulated-defrost-energy test: N hours at 0-2 C / RH 85% delivers the same average capacity (+-eps) as the continuous model alone.

## MAJOR

**M1. MSHP pan heater gated on compressor operation - diverges from OCHRE and under-counts winter standby.**
- `heater.rs:2117-2120` requires `&& hp_on`. OCHRE `HVAC.py:1495-1497` draws 150 W whenever `t_ext < 0 C` with no mode condition (adds no zone heat). The test `tests/hvac_tests.rs:1562` enshrines the HARES behavior.
- A cycling MSHP loses ~50-100 W average winter electric - hundreds of kWh/yr in cold climates, all meter-side. Outage behavior is handled separately (`outage_forces_off`), so `hp_on` is redundant. Undocumented in `docs/alignment/DIVERGENCES.md`.
- Fix: drop `&& hp_on` (keep the outage gate), keep the heat out of the zone, record the divergence (or re-pin the fixture if parity is the goal).

**M2. Gas boiler deposits the entire flue loss into the zone.**
- `boiler.rs:1009-1023` - `jacket_loss_w = (fuel_input + pump_electric - thermal_output).max(0)` routed 100% to the zone as JacketLoss. Non-condensing boiler (`eir_max = 1/AFUE ~= 1.2`, `boiler.rs:738`): ~17% of fuel becomes zone heat. The gas WH correctly splits flue vs jacket (`gas.rs:608-617`, `flue_loss_fraction` default 10%) and the furnace keeps losses out of the zone entirely.
- Fix: add `flue_loss_fraction` (as in `gas.rs`), route only `skin_loss_fraction x loss` to the zone; share one combustion-loss helper between `gas.rs` and `boiler.rs`.

**M3. Discrete resistive defrost delivers 100% of element heat to the zone; the continuous Timed+Resistive path delivers 0%.**
- Discrete path adds `resistive_defrost_capacity_w` to `er_capacity_w` -> zone (`heater.rs:2095-2115`); continuous path sets `q_defrost_w = 0.0` and only meters power (`defrost.rs:560-565`, applied `heater.rs:2035`). Same element, two destinations depending on which solver path fires. In reality the element heats the outdoor coil; the zone gets cabinet leakage only. The discrete path over-delivers zone heat during every defrost event exactly when the HP is capacity-constrained.
- Fix: route a configurable small fraction (or zero, matching continuous) to the zone; account the remainder as frost-melt loss; both paths share the destination logic.

**M4. ER soft-lockout timeout hardwired to 2x hard lockout; OCHRE default is soft = hard.**
- `heater.rs:2340-2342` - `soft_lockout_max_s = self.er_hard_lockout_time_s * 2.0`; no `er_soft_lockout_time` config key exists (`heater_config.rs`). OCHRE `HVAC.py:1203-1207` reads `Backup Soft Lockout Time (minutes)` defaulting to the hard value. With a user-configured hard lockout, HARES blocks ER during recovery ramps up to twice as long as OCHRE. Silent behavioral divergence.
- Fix: add `er_soft_lockout_time_s` (default = hard) or document the 2x as a divergence.

**M5. Per-step `std::env::var` read in the ASHP control hot path.**
- `heater.rs:2481-2500` - `std::env::var("HARES_DEBUG_ASHP_CONTROL")` inside `resolve_control`, every step for every ASHP/MSHP in release (environ lock + String alloc; stderr physics dumps if set). Cache with `OnceLock<bool>` or gate behind `observe`.

## MINOR

- **m1.** `space_fraction` asymmetry (meter scaled, zone gains not) is OCHRE-faithful but undocumented and applied inconsistently: electric x sf at `heater.rs:1378-1379`, `air_conditioner.rs:1314-1315`; zone gains unscaled (`heater.rs:1369-1377`, `air_conditioner.rs:1250-1255`) - exactly OCHRE `HVAC.py:558-565`. But the boiler scales its zone delivery by sf (`boiler.rs:905`) and OCHRE's boiler was a stub. Document in DIVERGENCES; pick one convention for the boiler.
- **m2.** Oversized tempered draws silently truncated: `tank.rs:469` `total_draw_m3.min(self.total_volume_m3)` no warning, no unmet-load accounting (raw-draw path errors instead, `tank.rs:336-341`; `unmet_load_w` covers only the temperature deficit, `:495-503`). Warn-once + count in health, or extend unmet load to the clamped volume.
- **m3.** COP telemetry inflates during ER backup: `heater.rs:1487-1495` - numerator total thermal (includes ER + fan), denominator compressor-only; during HeatingHPAndER the reported COP can exceed the compressor's physical COP; the clamp at 10.0 papers over the largest cases. Report HP-only heat or mark backup-influenced steps null.
- **m4.** Per-stage EIR count mismatch silently clamped for HP heaters: `heater.rs:876-880` accepts any `stage_heating_eirs` length; `staging.rs:413-418` clamps the index, so extra stages reuse the last EIR. The furnace errors on the same condition. Reuse the furnace-style validation.
- **m5.** Gas tankless uses the gas-storage UEF->EF conversion: `tankless.rs:235-240` `EF = 0.9066*UEF + 0.0711` (OCHRE applies it in the storage branch, `hpxml.py:1115`); RESNET defines a separate instantaneous formula. ~1-2% efficiency error on the fuel meter.
- **m6.** Companion-heating plumbing is dead code: `cooler.rs:110` `set_companion_heating_rtf`, `heater.rs:1419-1429` `last_heating_rtf` ("Exposed so the HP system coordinator can pass it...") called by nothing; `air_conditioner.rs:1854-1865` passes `None`, so the companion-aware Henderson-Rengarajan branch (`coil_physics.rs:167-169`) and the crankcase `max(heating_rtf, cooling_rtf)` refinement are inert. Wire it in the dwelling coordinator or delete it.
- **m7.** `PowerSetpoint` override zeroes reactive power: `scheduled_load.rs:433-435` returns `(override_kw, 0.0, 0.0)` - a nonzero-pf load overriding power for a step reports Q = 0 while P != 0. Compute Q from the override via `self.zip.reactive_kvar`.
- **m8.** Doc nit: `docs/equipment/hvac.md:100` lists crankcase in the *heater's* electrical port; the heater has no crankcase model (cooler only, matching OCHRE's class layout, `HVAC.py:1059`).

## Top 5 improvements
| # | Opportunity | Effort | Payoff |
|---|---|---|---|
| 1 | Defrost model consolidation (C1 + M3): one code path selected by config; shared destination logic for resistive-defrost heat; accumulated-energy invariant test vs the E+ ERM 26.1 continuous reference | M | High |
| 2 | Shared combustion-loss module for gas WH / furnace / boiler: flue fraction, skin/jacket fraction, pilot routing, one place to test the fuel -> zone + outdoors split (M2) | M | Medium-high |
| 3 | Wire or delete the companion-RTF plumbing (m6) - one-line coordinator pass makes crankcase and Henderson-Rengarajan companion corrections live | S | Medium |
| 4 | OCHRE-parity column expansion for cold-climate ASHP: assert Mode, ER Power, Defrost, COP, Runtime Fraction columns at 900 s/60 s on existing fixtures (`cz4a_ashp_hpwh`, `cz6b_*`), plus a multi-hour humid-near-freezing accumulated-energy check (would have caught C1/M1) | M | High |
| 5 | Stage/config validation unification: one helper validating (capacities, EIRs, cap-curves, EIR-curves) counts across furnace/HP-heater/cooler/AC (m4), expose `er_soft_lockout_time_s` (M4) | S | Medium |

## Missing tests / validation
1. Composed defrost energy test - only tracker-in-isolation tests exist (`defrost.rs:1456-1530`); nothing measures the heater's accumulated capacity/energy across Accumulating+Defrosting cycles vs the continuous equivalent (would catch C1).
2. Pan-heater off-cycle draw - `tests/hvac_tests.rs:1562` pins the divergent behavior; no OCHRE-parity fixture column for MSHP standby electric below 0 C.
3. Fuel-backup round-trip: gas/propane backup ER produces fuel-port energy = zone delivery / 0.80 and zero electric (`eir_from_backup_fuel`, `heater.rs:47-59`).
4. Boiler loss destination: no test distinguishes jacket vs flue heat for the boiler; an ASHRAE-derived reference like `ashrae_rc_reference.json` is the right pattern.
5. HPWH vs reference values: no COP/capacity curve comparison against OCHRE's HPWH at low ambient (7.2 C wet-bulb); no FHR-style draw-cycle energy test vs the DOE test procedure the autosizer implements.
6. Henderson-Rengarajan latent degradation vs lab/E+ data: no fixture reproduces the published twet/gamma case or an E+ DX-coil SHR comparison at PLR < 1.
7. TwoSpeedTime escalation transients: the OCHRE "temp moving away from setpoint" escalation semantics lack a multi-step behavioral test.
8. Parity-fixture scope: all 12 OCHRE fixtures complete and activated (good), but assertions skew to annual/aggregate columns; per-equipment end-use reconciliation (meter kW vs zone heat x sf) would make divergences like M1/C1 visible in every fixture.
