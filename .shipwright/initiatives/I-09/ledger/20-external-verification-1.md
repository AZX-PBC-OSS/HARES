# 20: the first external-verification sweep (criterion 6)

Charter criterion 6 (00-charter.md): every physics constant, curve and model
form carries its authoritative lineage, verified against the UPSTREAM source by
web fetch or the vendored tree, never against HARES's own comment. This file is
the first sweep's record: 12 checklist items, the fetched citations, and the
row proposals.

Agent: the external-verification subagent, 2026-10-17, read-only. The agent
fetched every upstream file named below on 2026-10-17 from GitHub raw at the
named tag or branch. No HARES code was touched.

## Sources fetched (with the exact revision)

- `NREL/EnergyPlus` `develop` (post v26.2.0-RC3; fetched 2026-10-17):
  `src/EnergyPlus/DXCoils.cc`, `src/EnergyPlus/SolarShading.cc`,
  `src/EnergyPlus/WeatherManager.cc`, `src/EnergyPlus/HeatBalanceManager.cc`,
  `idd/Energy+.idd.in`,
  `doc/engineering-reference/src/air-heat-balance-manager-processes/infiltration-ventilation.tex`
- `NREL/EnergyPlus` tag `v26.1.0`: `src/EnergyPlus/DXCoils.cc` (the line-drift
  check; HARES pins ERM 26.1)
- `NREL/OCHRE` `master` head `0bf0503` ("Update version in initialization",
  v0.9.2 prep): `ochre/Equipment/HVAC.py`, `ochre/utils/schedule.py`
- `NatLabRockies/OpenStudio-HPXML` (the current home of `NREL/OpenStudio-HPXML`,
  API-confirmed rename) tag `v1.12.0` = commit `502811cd`:
  `HPXMLtoOpenStudio/resources/{hvac,defaults,schedules,waterheater,airflow,hpxml}.rb`,
  `resources/data/unavailable_periods.csv`, `docs/source/workflow_inputs.rst`,
  `tests/test_defaults.rb`. `v1.12.0` is the newest tag in that repository
  (`git ls-remote --tags`: the series ends at `v1.12.0`), so nothing newer
  exists to absorb.
- `NatLabRockies/ssc` (SAM's SSC, branch `patch`, the default branch):
  `shared/lib_battery_lifetime_nmc.{h,cpp}`, `shared/lib_cec6par.{h,cpp}`
  (`noct_celltemp_t`), `ssc/cmod_pvwattsv8.cpp`, `shared/lib_pv_io_manager.{h,cpp}`
- Web (non-GitHub): pvlib-python `soiling.kimber` reference page (the published
  reproduction of the Kimber 2006 rate table), the E+ issue #8489 thread (the
  15.555 C defrost-curve floor), OSTI record 392453 and the ASHRAE Transactions
  102 index (the Henderson-Rengarajan citation), all fetched 2026-10-17.

Verdict vocabulary: EXACT (value and lineage true), DIVERGENT-DOCUMENTED (a
real divergence from the named upstream, recorded in HARES's comments),
DIVERGENT (a real divergence, NOT recorded: needs a row), MISATTRIBUTED (the
citation names the wrong origin), WRONG (the value does not match the named
upstream), UNVERIFIABLE-WEB (the paywalled paper's own numbers need library
access).

## The verification table

| # | Item | HARES claim (file:line) | Upstream value (the fetched citation) | Verdict |
|---|------|------------------------|----------------------------------------|---------|
| 1a | Defrost enable temp 4.4445 C | `heat_pump/constants.rs:3`; the doc at `defrost.rs:87-88` says "OCHRE/HARES default 4.4445 (40 F); EnergyPlus default 5 C" | OCHRE `HVAC.py:1141` vendored (upstream master :1053): `self.defrost = t_ext_db < 4.4445`. E+ IDD `Coil:Heating:DX:SingleSpeed` default 5 C (`Energy+.idd.in:31878-31886` region) | EXACT (the OCHRE choice, documented as such) |
| 1b | Coil temp `0.82*OAT - 8.589` | `constants.rs:4-5` | E+ `DXCoils.cc:11283` (develop; v26.1.0 :11312): `OutdoorCoilT = 0.82 * OutdoorDryBulb - 8.589`; OCHRE `HVAC.py:1144` | EXACT |
| 1c | Min delta-omega 1e-6 | `constants.rs:6` | E+ `DXCoils.cc:11284`: `max(1.0e-6, ...)`; OCHRE `HVAC.py:1146`: `max(0.000001, ...)` | EXACT |
| 1d | OnDemand tf numerator 0.01446 | `constants.rs:7`; the tf form `defrost.rs:480-481` | E+ `DXCoils.cc:11315`: `FractionalDefrostTime = 1.0 / (1.0 + 0.01446 / OutdoorCoildw)`; OCHRE `HVAC.py:1148` | EXACT (see item 2) |
| 1e | Capacity base 0.875, form 0.875*(1-tf) | `constants.rs:8`; `defrost.rs:482-483` | E+ `DXCoils.cc:11320`: `HeatingCapacityMultiplier = 0.875 * (1.0 - FractionalDefrostTime)`; OCHRE `HVAC.py:1149` | EXACT (item 12's form too) |
| 1f | Power multiplier 0.954, applied as the constant ratio 0.954/0.875 | `constants.rs:9`; `defrost.rs:484-488` ("fixed ratio ~1.09 ... OCHRE-derived ... not clamped") | E+ `DXCoils.cc:11321`: `InputPowerMultiplier = 0.954 * (1.0 - FractionalDefrostTime)` (a capacity-scaled multiplier, not a constant); OCHRE `HVAC.py:1150`: `self.defrost_power_mult = 0.954 / 0.875` | DIVERGENT-DOCUMENTED (the divergence from E+'s form is real and recorded as OCHRE-derived at the use site; the constants.rs comment itself does not say so: row EV-2) |
| 1g | Q multiplier 0.01, reference 7.222 C, unit factor 1.01667 | `constants.rs:10-12`; `defrost.rs:490-493` | E+ `DXCoils.cc:11335`: `LoadDueToDefrost = (0.01 * FractionalDefrostTime) * (7.222 - OutdoorDryBulb) * (RatedTotCap / 1.01667)`; OCHRE `HVAC.py:1151` | EXACT (see item 2) |
| 1h | Defrost EIR modifier 0.1528 | `constants.rs:13-17`: "Sourced from EnergyPlus OnDemand defrost formula" (plus the correct "# in kW" rebuttal) | E+ has NO such constant: `DXCoils.cc:11336` evaluates the USER-SUPPLIED `DefrostEIRFT` curve, `CurveValue(state, DefrostEIRFT, max(15.555, WB), max(15.555, ODB))`. The 0.1528 is OCHRE's hardcode: `defrost_eir_temp_mod_frac = 0.1528` at `HVAC.py:1161` vendored (upstream :1073, still with the stale "# in kW" comment) | MISATTRIBUTED (the charter's own example; row EV-1) |
| 1i | Timed multipliers 0.909/107.33 and 0.90/36.45 | `constants.rs:23-27` ("EnergyPlus / DOE-2") | E+ `DXCoils.cc:11302-11303`: `HeatingCapacityMultiplier = 0.909 - 107.33 * OutdoorCoildw; InputPowerMultiplier = 0.90 - 36.45 * OutdoorCoildw` | EXACT (the [0,1] clamp is a HARES addition, documented at `defrost.rs:522-530`) |
| 1j | Defrost EIR curve floor 15.555 C | `constants.rs:29`; `defrost.rs:541-542` | E+ `DXCoils.cc:11336` (both arguments through `max(15.555, ...)`); the quirk is documented upstream in E+ issue #8489 | EXACT |
| 1k | Timed fraction 0.058 (E+ default 0.058333) | `defrost.rs:80-83`, `constants.rs:21` | E+ IDD `Energy+.idd.in:31882-31886`: `\default 0.058333` | DIVERGENT-DOCUMENTED (0.058 vs 0.058333, a 0.6% truncation, recorded in the doc comment) |
| 1l | Defrost defaults to OnDemand; E+ defaults Timed | `defrost.rs:70-71` | E+ IDD `:29960`: `\default Timed` | EXACT (the OCHRE-following choice is stated) |
| 2 | tf formula 1/(1+0.01446/dw); LoadDueToDefrost/(Q_rated/1.01667) | `defrost.rs:480-493` | Both EXACT vs E+ `DXCoils.cc:11315` and `:11335` (develop) / `:11374` (v26.1.0). One form note: E+ folds LoadDueToDefrost into the PLR (`DXCoils.cc:11420`); HARES subtracts it from capacity, matching OCHRE `HVAC.py:1154/1158` | EXACT (the OCHRE capacity-subtraction form, documented) |
| 3 | Henderson-Rengarajan latent degradation | `coil_physics.rs:35-208` (cites Henderson & Rengarajan 1996, ASHRAE Transactions, "validated against EnergyPlus Coils.cc") | The FORM is EXACT vs E+ `DXCoils.cc:12608-12690` (develop): Twet*ratio capped 9999 (:12608), Gamma with the (26.7-19.4) depression (:12609), Ton/Toff 3600/(4*Nmax*(1-RTF))/(4*Nmax*RTF) (:12612-12613), Toffa cap 2*Twet/Gamma (:12617), aa (:12641), the To fixed-point (:12643-12651), LHRmult (:12656-12658), SHReff and the clamps (:12661-12668). The suggested coefficients 1000/1.5/3/45 match the IDD notes (`Energy+.idd.in:54226-54255`). The paper is confirmed: Henderson, H.I. and Rengarajan, K. (1996), ASHRAE Transactions 102(1):266-274 (OSTI 392453; the 102 index on archive.org). Three nits: HARES's rated depression is 7.2222 K (exact 80/67 F) vs E+'s literal 7.3 K; HARES's companion-heating Toff reduction (`coil_physics.rs:176-180`, toff*rtf/heating_rtf) is NOT E+'s form (`DXCoils.cc:12628-12637`, Toffa = min(Toffa, Ton_heating - Ton)); the citation names "Coils.cc" where E+ implements it in DXCoils.cc | EXACT (form + coefficients) with two UNDOCUMENTED divergences (rows EV-3) |
| 4 | AIM-2 / Sherman-Grimsrud | `infiltration.rs:163-175` (the runtime form), `:645-742` (ACH50 to coefficients, "Walker & Wilson (1998) Eqs 9-25", OCHRE `envelope.py:488-633`), `:758-855` (the ELA shape factors, OS-HPXML `airflow.rb`) | The runtime form EXACT vs the E+ Engineering Reference (`infiltration-ventilation.tex:47-49`): Infiltration = sqrt((c*Cs*dT^n)^2 + (c*Cw*(s*v)^(2n))^2), and the Basic ELA form (:37-39) = (AL/1000)*sqrt(Cs*dT + Cw*v^2). The coefficients are line-for-line EXACT vs OCHRE `envelope.py:488-633` (shelter 0.5/0.9/0.3, flue y_i=0.2 and the 1.5 shelter, leakage splits 0.15/0.35/0.50 and 0.25/0.50/0.25, m_o/m_i/x_c/f_i/f_s, the crawl and non-crawl f_w, `inf_sft = f_t*(shelter*(1-y_i)+1.5*y_i)`) and vs OS-HPXML `airflow.rb:2749-2771` (f_s_SG, f_w_SG with s_g = shelter/3 at `airflow.rb:238-239`, c_s = fs^2*g*H/T) and `airflow.rb:203-216` (terrain 0.85/0.20/270 rural, 0.67/0.25/370 suburban, 0.47/0.35 urban; station 1.0/0.15/270). One undocumented numeric divergence: HARES's `RHO = 1.2041` kg/m3 (`infiltration.rs:647`) vs OCHRE's `outside_air_density = 0.0765 lb/ft3` = 1.2251 kg/m3 (`envelope.py:515`): about 0.9% on c_s and c_w. The Walker-Wilson 1998 paper itself (HVAC&R Research 4(2)) is paywalled: the paper's own tables are UNVERIFIABLE-WEB, but the two independent ports (OCHRE and OS-HPXML) agree with HARES | EXACT (both ports + E+ ER) with one minor UNDOCUMENTED density divergence (row EV-4) |
| 5 | Burch-Christensen mains water | `water_mains.rs:1-50, 96-161` (0.986 deg/day, offset 6 F, T_ref 44 F, ratio 0.4+0.01*(T-44), lag 35-1.0*(T-44), day-15, phase -90 North, 32 F floor) | E+ `WeatherManager.cc:7190-7212` (develop): `Ratio = 0.4 + 0.01*(Tavg-44)`, `Lag = 35.0 - 1.0*(Tavg-44)`, `Offset = 6.0`, `sin((0.986*(DayOfYear-15-Lag)-90)*DegToRad)` with the amplitude scaled by the latitude sign, and the `if < 32.0 then 32.0` floor. E+'s `Tdiff` is the full monthly max-diff (HARES's `dt_annual_range_c` is declared the same and halved in the formula: EXACT). E+ credits the correlation to Burch and Christensen at NREL via Hendron et al. 2004 (:7190-7195); HARES cites the 2007 ASES paper as primary and Hendron 2004 secondarily: consistent. HARES's [0,1] ratio clamp is its own extension, documented as such (`water_mains.rs:111-112`) | EXACT |
| 6a | Perez sky diffuse | `solar.rs:28-64` (the 48 coefficients, the epsilon bins, kappa 1.041), `:399-439` (the airmass), `:613-645` (the composition) | ALL 48 coefficients EXACT vs E+ `SolarShading.cc:2678-2684` (F11R..F23R); epsilon bins EXACT (`:2767-2768`, 1.065/1.23/1.5/1.95/2.8/4.5/6.2); kappa 1.041 EXACT (`:2723`, zenith cubed in radians); the airmass piecewise EXACT (`:2717-2721`: AirMassH = 1 - 0.1*elev/1000, cos z for z <= 75 deg, else the 0.15*(93.9-z)^-1.253 form); F1's max(0,...) EXACT (`:2740`); the a/b floors EXACT (`:2775`, 0.0871557). One undocumented divergence: E+ computes `Delta = DHI * AirMass / 1353.0` with a FIXED 1353 W/m2 (`:2725`); HARES divides by the day-varying `extraterrestrial_irradiance(day_of_year)` (`solar.rs:620, 629`): up to about 2.7% on Delta, a tiny shift in F1/F2. E+'s own comment (:2674-2675) says the coefficients came from R. Perez privately at higher precision than the 1990 paper's Table 1, so "Table 1" is the right lineage name | EXACT (coefficients + form) with one minor UNDOCUMENTED divergence (row EV-5) |
| 6b | SAM-NOCT cell temperature | `pv/mod.rs:93-126`: `T_cell = T_amb + (E_POA/800)*(NOCT-20)*9.5/(5.7+3.8*WS)`, "the SAM-NOCT model (used by PVWatts v8)", "at WS = 1.0 the factor is exactly 1.0" | The 9.5/(5.7+3.8*v) wind term is real and EXACT as a fragment: SSC `lib_cec6par.cpp:277` (`noct_celltemp_t::operator()`): `Tcell = Tamb + (Geff/I_noct*(Tnoct_adj - 20)*(1.0 - eff_ref/tau_al))*9.5/(5.7 + 3.8*W_spd)`, and pvwattsv8 passes the raw weather wind (`cmod_pvwattsv8.cpp:1247-1248`) with `ffv_wind = 0.51` (`cmod_pvwattsv8.cpp:453`), which multiplies W_spd INSIDE `noct_celltemp_t` (`lib_cec6par.cpp:272`). HARES omits BOTH the ffv_wind = 0.51 and the (1 - eff_ref/tau_al) cover-transmittance factor, and its "WS = 1.0 gives the plain NOCT" property is HARES's design (SAM at 1 m/s gives 9.5/(5.7+1.938) = 1.243 before the cover factor). The plain-NOCT baseline E/800*(NOCT-20) matches PVWatts v5/PVPMC | DIVERGENT (a real simplification of SAM's form, not recorded: row EV-6) |
| 7 | Kimber soiling | `pv/soiling.rs:43-84`: 6 mm cleaning threshold, 0.0015/day default, 14-day grace, 0.30 max, 24 h rain window, initial 0; the regional table at `:56-59` (0.0019 Central Valley suburban, 0.0030 Desert, 0.0010 Northern CA suburban) | pvlib's `soiling.kimber` reference page (the published reproduction of Kimber et al. 2006, IEEE WCPEC, DOI 10.1109/WCPEC.2006.279690): defaults cleaning_threshold 6, soiling_loss_rate 0.0015, grace_period 14, max_soiling 0.3, rain_accum_period 24, initial_soiling 0: ALL EXACT. The page's regional table: Central Valley suburban 0.0019, Desert 0.0030 (all environments), Northern CA suburban 0.0010: ALL EXACT. The rolling-window/grace/reset algorithm matches pvlib's semantics. The 2006 paper itself is IEEE-paywalled; the pvlib table settles the numbers | EXACT (the "suburban temperate" label on the 0.0015 default is loose: 0.0015 is pvlib's default, not a table row; cosmetic) |
| 8 | Smith 2017 battery degradation | `battery/degradation.rs:163-246` (`deg_const`), the module header (:6-14) claiming verification against SSC `lib_battery_lifetime_nmc.{h,cpp}` | SSC (NatLabRockies/ssc `patch`) `lib_battery_lifetime_nmc.h:64-67, 108-140`: Rug 8.314, F 96485, T_ref 298.15, V_ref 3.7, Uneg_ref 0.08, b0 1.07, b1_ref 0.0035, Ea_b1 35392, alpha_a_b1 = -1, beta_b1 2.157, gamma 2.472, b2_ref 1.541e-5, Ea_b2 -42800, b3_ref 0.02805, Ea_b3 42800, alpha_a_b3 0.0066, tau_b3 5, theta 0.135, c0_ref 75.675, c2_ref 5.226e-5, Ea_c2 -48260, beta_c2 4.54, Ah_ref 75, d0_ref 75.075: every HARES constant matches. `alpha_a_b1 = -1` confirmed (`nmc.h:126`), settling the module header's sign claim. The header also cites the right paper (Smith et al., 2017 ACC, IEEE 7963578). One nuance: HARES's B1_REF = 3.503e-3 vs SSC's rounded 0.0035 (the paper's unrounded value; a 0.09% difference, so "match it exactly" is 0.09% off for this one constant) | EXACT |
| 9 | E+ warm-up convergence | `hares-io/src/config.rs:667-703`: temperature tolerance default 0.4 delta C allowed (0, 0.5], loads tolerance 0.04 (a fraction, not watts), min warm-up days 1, max 25, the 100 W floor and the 1e-4 W pass-through | E+ IDD `Energy+.idd.in` Building N2-N5: Loads Convergence `\default .04` `\maximum .5` (:536-542), Temperature Convergence `\units deltaC` `\default .4` `\maximum .5` (:543-548), Maximum Warmup Days `\default 25` with the "should NOT be set less than 25" note (:562-568), Minimum Warmup Days `\default 1` and the max-resets-to-min note (:569-575): ALL EXACT, and HARES's IDD line citations are exact. `HeatBalanceManager.cc` `CheckWarmupConvergence` (:3308-3430): `MinLoad(100.0)` at :3320, the 1e-4 W gates at :3349 and :3366, and the loads check IS a relative difference against LoadsConvergTol (:3353-3356), so HARES's "a fraction, not watts" correction of the IDD's misleading `\units W` is itself verified. NOTE: the sweep's brief framed the defaults as "0.5 C / 25 iterations"; upstream says 0.4 delta C (0.5 is the maximum) and 25 max warm-up DAYS: HARES matches upstream, the brief did not. HARES errors where E+ silently resets max to min: documented at `config.rs:774-776` | EXACT |
| 10 | OS-HPXML row 64 (setpoints, IN-5) | `00-schedule.md` row 64: "hvac.rb:1641-1741; the hourly arrays or the constant element with its setback at :1768-1829, :1785-1798; the 68/78 F manual-thermostat defaults `defaults.rb:2827-2848`; the overlap warning OS-HPXML hvac.rb:1669-1675" | v1.12.0 `hvac.rb`: `apply_setpoints` at :1652 (the cited region should be 1652-1766), the schedule-file overlap warnings at :1680 and :1686 (cited 1669-1675), `get_hvac_setpoints` at :1768-1829 EXACT (the setback hours :1785-1790, the on/off-thermostat deadband offset at :1826-1829). `defaults.rb:2827-2848` EXACT as the defaulting site; the 68 F/78 F values live in `get_heating_setpoint`/`get_cooling_setpoint` (`defaults.rb:6746, 6770`: '68,...' and '78,...') | DIVERGENT (the RULES verify; two line citations drifted about 11 lines: row EV-7) |
| 10b | OS-HPXML row 94f (infiltration height, flue, shielding defaults; OD-10's numeric commit) | `11-other-defects.md` OD-10: the flue rule "non-electric ... AFUE/Percent < 0.89 ... water heater ... EnergyFactor less than 0.59", the shielding rule normal/well-shielded by facility type, the infiltration-height estimate, the 9.75 ft test | v1.12.0 `defaults.rb`: shielding EXACT (`:822-830`: apartment/SFA well-shielded, else normal, with the ACCA MJ8 class notes); height EXACT (`get_infiltration_height_and_volume` :5789-5853, rim-joist 6 in and 9 in allowances, the within-infiltration-volume attics and foundations); the 9.75 ft expectation EXACT (`test_defaults.rb:807-816`, `assert_in_epsilon(9.75, ..., 0.01)`). The FLUE threshold: upstream code and doc say 0.63 (`defaults.rb:5096-5136`: `next if energy_factor >= 0.63`; `workflow_inputs.rst:1155`: "less than 0.63"), not 0.59: the ledger's quoted rule is STALE (the readthedocs "latest" snapshot the row quoted predates the 0.63 rule) | MISATTRIBUTED (the quoted EF threshold 0.59 vs the v1.12.0 value 0.63: row EV-8; must be fixed BEFORE implementing 94f) |
| 10c | OS-HPXML row 94j (unavailable periods, B-15) | `09-numeric-fixes.md:3945+`: the type table `data/unavailable_periods.csv:1-40`, the water-heater 2 C freeze protection `schedules.rb:673-676`, the hour defaults `defaults.rb:229-241`, the wrap and timestep rules `schedules.rb:702-724, 1375-1408` | v1.12.0: `unavailable_periods.csv` is EXACTLY 40 lines and its zeroing table matches the row's summary line for line (Vacancy vs Power Outage on occupants/refrigerator/space conditioning/water heater/general_water_use/battery columns all consistent; PV and generators absent). `schedules.rb:673-676` EXACT (`ObjectTypeWaterHeaterSetpoint` -> `value = 2.0`, "Temperature of tank < 2C indicates of possibility of freeze"). `defaults.rb:229-241` EXACT (begin_hour 0, end_hour 24, natvent "regular"). `airflow.rb:337, 570-597` and `hpxml.rb` period class verified present | EXACT |
| 11 | The 2 C thermostat pair reconcile | `hares-io/src/hpxml/resolve_hvac.rs:3935-3945`: "matching OCHRE's reconciliation (see `vendors/OCHRE/ochre/utils/schedule.py:617-625`)"; OCHRE enforces 1 C, HARES uses 2 C because the thermostat invariant needs 2*hysteresis | Vendored OCHRE `schedule.py:621-628`: `setpoint_diff.min() < 1` -> clip cooling to at least avg+0.5 and heating to at most avg-0.5 (a 1 C gap, a per-hour CLIP, not a symmetric replacement). Upstream master has the same block at :585-593 (the fork's ruff pass shifted the file). HARES's 2 C gap and its avg +/- gap/2 form are a real, but DOCUMENTED, divergence (`:3939-3944`). The line citation is drifted against BOTH trees (617-625 vs vendored 621-628 and upstream 585-593) | DIVERGENT-DOCUMENTED (gap) + citation drift (row EV-9) |
| 12 | 0.875 frost derate and the (1-tf) capacity form | `constants.rs:8`, `defrost.rs:482-483` | E+ `DXCoils.cc:11320` (develop) / `:11349` (v26.1.0): `0.875 * (1.0 - FractionalDefrostTime)`; OCHRE `defrost_capacity_mult` `HVAC.py:1149`: identical. E+ also applies `InputPowerMultiplier = 0.954*(1-tf)` (:11321) where OCHRE/HARES use the constant ratio (item 1f) | EXACT |

## Row proposals

- **EV-1 (MISATTRIBUTED): correct `DEFROST_EIR_TEMP_MODIFIER`'s lineage comment.**
  `crates/hares-equipment/src/hvac/heat_pump/constants.rs:13-17`. The first
  sentence "Sourced from EnergyPlus OnDemand defrost formula" over-claims: E+
  supplies the FORMULA SHAPE only (`DefrostPower = DefrostEIRTempModFac *
  (RatedTotCap/1.01667) * FractionalDefrostTime`, DXCoils.cc:11337 develop /
  :11366 v26.1.0) with `DefrostEIRTempModFac` coming from a user-supplied
  curve; the VALUE 0.1528 is OCHRE's `defrost_eir_temp_mod_frac`
  (HVAC.py:1161 vendored, :1073 upstream). The comment's second half (the "# in
  kW" rebuttal, dimensionless in watts) is verified correct and stays. The
  ultimate origin of OCHRE's 0.1528 (plausibly the evaluated value of some
  upstream tool's defrost EIR curve at the 15.555 C evaluation point, per E+
  issue #8489's "simulated as a constant value" observation) is
  UNVERIFIABLE-WEB: say so in the comment rather than crediting E+.
  Acceptance: comment-only change; goldens bitwise; the parity defrost
  records unchanged.
- **EV-2 (DIVERGENT-DOCUMENTED): state the 0.954 form divergence at the
  constants site.** `constants.rs:9` gains: E+'s OnDemand form is
  `0.954*(1-tf)` (DXCoils.cc:11321); HARES carries OCHRE's constant ratio
  0.954/0.875 (HVAC.py:1150), as documented at the use site (defrost.rs:484-488).
  While there, record the 0.058-vs-0.058333 truncation next to
  DEFAULT_DEFROST_TIME_FRACTION (it is documented in defrost.rs:80-83 only).
  Acceptance: comment-only; goldens bitwise.
- **EV-3 (DIVERGENT): record the three Henderson-Rengarajan nits.**
  `coil_physics.rs`: (a) the rated depression constant is 7.2222 K (exact
  80/67 F) where E+ uses the literal 7.3 K (DXCoils.cc:12609): about 1% on
  gamma's depression scaling at off-rated conditions; (b) the companion-heating
  Toff reduction (:176-180) is HARES's own scaling `toff*rtf/heating_rtf`,
  not E+'s `Toffa = min(Toffa, Ton_heating - Ton)` (DXCoils.cc:12628-12637);
  (c) the reference says "Coils.cc" where the E+ implementation is in
  DXCoils.cc (CalcSHRseff region, :12608-12690 develop). Acceptance: comments
  only, or a follow-up row that adopts E+'s exact Toff form with goldens
  re-captured (choose one; the divergence is bounded by the min()).
- **EV-4 (DIVERGENT): record the AIM-2 density divergence.**
  `infiltration.rs:646-651`: HARES's RHO = 1.2041 kg/m3 vs OCHRE's
  `outside_air_density = 0.0765 lb/ft3` = 1.2251 kg/m3 (envelope.py:515):
  about 0.9% on c_s and c_w. Document it (and note OCHRE's TODO on the same
  line, "calculate based on average outdoor air temperature and altitude"),
  or add an OCHRE-parity density for the parity harness.
  Acceptance: comment-only, or a parity-harness row with a recorded delta.
- **EV-5 (DIVERGENT): record the Perez Delta denominator divergence.**
  `solar.rs:617-629`: HARES divides Delta by the day-varying extraterrestrial
  irradiance; E+ uses the fixed 1353 W/m2 (SolarShading.cc:2725). Document the
  choice (it follows modern practice, pvlib-style) next to the E+ citation.
  Acceptance: comment-only; goldens bitwise.
- **EV-6 (DIVERGENT): record the SAM-NOCT simplification.**
  `pv/mod.rs:93-126`: the 9.5/(5.7+3.8*WS) fragment is SAM's
  (`lib_cec6par.cpp:277`), but SAM's PVWatts v8 also applies ffv_wind = 0.51
  inside the wind term (`cmod_pvwattsv8.cpp:453`, `lib_cec6par.cpp:272`) and
  the (1 - eff_ref/tau_al) cover factor (:277), both dropped in HARES; the
  "WS = 1.0 degrades to the plain NOCT" property is HARES's design. Document
  the simplification (and its direction at typical wind speeds), or add SAM's
  full form behind a config flag as its own numeric row.
  Acceptance: comment-only, or the full-form row with a PV parity delta.
- **EV-7 (citation drift): refresh row 64's hvac.rb line numbers.**
  In `00-schedule.md` row 64's record and wherever the code comments copy it:
  `apply_setpoints` is `hvac.rb:1652-1766` (cited 1641-1741) and the overlap
  warnings are at :1680 and :1686 (cited 1669-1675); `get_hvac_setpoints`
  :1768-1829 and `defaults.rb:2827-2848` are already exact. Acceptance: doc
  edits; the 68/78 F values' home (defaults.rb:6746, 6770) added.
- **EV-8 (MISATTRIBUTED, blocking for 94f): fix the flue-rule EF threshold
  quote.** `11-other-defects.md` OD-10 (and any code comment derived from it):
  the water-heater threshold is "energy factor (or equivalent calculated from
  uniform energy factor) less than 0.63" in v1.12.0
  (`defaults.rb:5096-5136`, `workflow_inputs.rst:1155`), not 0.59. An
  implementer following the ledger's quote would mis-default the flue for
  water heaters with EF in [0.59, 0.63). Acceptance: ledger + comment edits
  BEFORE row 94f's numeric commit; the row 94f acceptance test gains a case
  with a gas water heater EF 0.61 in conditioned space (flue true).
- **EV-9 (citation drift): refresh the OCHRE schedule.py citation.**
  `resolve_hvac.rs:3937` (and the row 11 record): the reconciliation block is
  at `schedule.py:621-628` in the vendored fork and at :585-593 in upstream
  NREL/OCHRE master. State both (the fork's ruff pass moved it). The adjacent
  OCHRE line references that checked out clean: HVAC.py:1141-1172 (defrost),
  :1482 (pan heater 0.150 kW), :1209 (the -17.78 lockout, cited as 1208 in
  constants.rs:39: one line off, fix in passing). Acceptance: comment edits
  only; goldens bitwise.

## Upstream changes HARES should absorb (as of 2026-10-17)

- **EnergyPlus v26.2.0 is released; HARES pins ERM 26.1.** Substance check
  done: the defrost region, the Henderson-Rengarajan region, the warm-up
  checks, the water-mains correlation and the Perez tables are unchanged
  between v26.1.0 and develop in every constant this sweep verifies
  (the v26.1.0 DXCoils.cc carries the same 0.01446/0.875/0.954/0.1528-form
  region; its line numbers sit about 29-43 lines lower than develop's after a
  new `OATType::WetBulb` capacity-modifier block, develop :11270-11298).
  Proposed row: a citation-refresh row that re-pins the E+ line references to
  the v26.1.0 tag (or states the pin and the observed drift), and a watch item
  reviewing v26.2's outdoor-wet-bulb coil rating capability for future ASHP
  ports.
- **OCHRE upstream is at v0.9.2-prep** (pint 0.23 pin; "Broken Equipment column
  in schedule files" #198). The vendored fork (head 58f39d8) is functionally
  identical in the defrost and setpoint-reconcile regions this sweep checks
  (the diff shows numba/ruff and fork-only fixes). Proposed watch item: track
  upstream v0.9.2's release notes for HVAC.py and schedule.py changes and
  absorb them as rows if the defrost or setpoint regions move.
- **OpenStudio-HPXML v1.12.0 is the newest tag** in the repository's history:
  nothing to absorb; the v1.12.0 tag remains the correct citation target.

## Not settled from the web (the operator's call)

- The Walker-Wilson 1998 paper's own shape-factor tables (HVAC&R Research
  4(2), paywalled) and the Kimber 2006 paper's full table (IEEE, paywalled):
  both are settled here through two independent open implementations that
  agree with HARES (OCHRE + OS-HPXML; pvlib), but the paper-level provenance
  needs library access to close.
- OCHRE's 0.1528 ultimate origin (see EV-1): no public upstream carries it as
  a published constant.
