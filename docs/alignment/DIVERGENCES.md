# Deliberate Divergences from Reference Implementations

Every entry is a place where HARES knowingly differs from OCHRE (or another
reference) on physics. Each divergence exists because the first-principles
derivation or a more authoritative reference (EnergyPlus Engineering
Reference, ASHRAE) demands it. Anyone re-aligning with the reference
implementation will "fix" these back — this ledger is what stops that.

Entry format: quantity → reference behavior → HARES behavior →
justification → measured impact → pinning tests.

---

## D-001: Exterior radiative-split resistance (`rad_res_k_w`)

- **Reference behavior:** OCHRE `BoundarySurface.radiation_res =
  res_film / area` (Envelope.py:257). The exact parallel form
  `res_film * res_material / (res_film + res_material) / area` is present in
  OCHRE's source but **commented out** (line 258), with the note
  "radiation resistance assumes res_material >> res_film".
- **HARES behavior:** `skin_rad_coupling` (solver_builder.rs) emits the exact
  parallel (Thévenin) form `R_film·R_half/(R_film + R_half)/area`.
- **Justification:** Eliminating the skin node from the outside-face heat
  balance (outdoor —R_film— skin —R_half— mass node) gives the exact skin
  equation `T_skin = t_init + (solar + q_lwr)·R_parallel` with
  `R_parallel = R_film·R_half/(R_film+R_half)`; the divider injection
  `rad_frac = R_film/(R_film+R_half)` plus the series A-matrix edge is then
  exact at every state, for any resistance ratio. The bare-film form
  over-drives the skin by `(solar + q_lwr)·R_film²/(R_film+R_half)` whenever
  the outermost half-layer conducts comparably to the film (wood siding,
  stucco, metal skins; the ASHRAE 140 case-600 wall). EnergyPlus
  `CalcOutsideSurfTemp` solves the outside face with every path carrying its
  own parallel conductance — no film-split approximation at all.
- **Measured impact:** steady-state free-float error 0.392 K on an assembled
  siding wall pre-fix, exact post-fix; freefloat-oracle opaque exterior gap
  vs OCHRE 30.9 % → 7.3 % once this fix and the absorbed-flux diagnostic
  semantics (D-002) both landed; BESTEST/parity zone-temp MAEs moved only
  ~3×10⁻⁴ K (their skins are not thin-skin dominated).
- **Pinning tests:** `crates/hares-envelope/tests/exterior_skin_balance.rs`
  (closed-form steady state, insulated + conducting skins),
  `solver_builder::tests::skin_rad_coupling_uses_parallel_resistance`,
  `longwave::tests::iterative_skin_temperature_satisfies_exact_skin_balance`.

## D-002: Exterior skin diagnostics report absorbed flux, not the injected share

- **Reference behavior:** OCHRE reports `surface.solar_gain` (absorbed at the
  skin, W — `Irradiance (W) × absorptivity`, Envelope.py:1133) and
  `surface.lwr_gain` (net skin LWR); the rad_frac-scaled injection into the
  RC node is internal and unreported.
- **HARES behavior (since I-02):** `opaque_solar_w`, `exterior_lwr_w`, and
  `opaque_solar_lwr_w` report the skin-level quantities across BOTH
  application paths (iterative + linearized), accumulated where the physics
  computes them — not `u`-vector deltas at module boundaries, which mixed
  injected and absorbed semantics and read `opaque_solar_w` as 0.0 on any
  fully iterative-routed building.
- **Justification:** like-for-like parity with OCHRE's
  "{boundary} Ext. Solar/LWR Gain (W)"; a diagnostic that is a view of the
  mechanism cannot silently disagree with it.
- **Measured impact:** the freefloat oracle's opaque comparison became
  like-for-like (see D-001); the I-02 gross/net confusion becomes
  unprovable: the gross is the absorbed skin flux, the zone load is the
  inside-face convection (`wall/floor/roof_heat_gain_w`).
- **Pinning tests:** `exterior_skin_balance.rs`
  (`assert_exterior_diagnostics_split_honestly` — absorbed solar exact, net
  skin LWR at the converged skin temperature, sum identity).

## D-003: Per-step TARP interior convection (diagnostic and injection)

- **Reference behavior:** OCHRE's interior film conductance is frozen at
  init-time in the A-matrix.
- **HARES behavior:** the per-boundary net-convection accumulation uses the
  ΔT-dependent TARP model (Walton 1983, Eqs. 90–92 — the EnergyPlus default
  interior convection algorithm) per step; the A-matrix discretization
  remains frozen (tracked limitation, tickets T-0082/T-1922).
- **Justification:** EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior
  Convection; the reported zone loads are the physically correct convective
  flux, not the discretization's artifact.
- **Pinning tests:** `bestest_900ff_root_cause.rs`,
  `tests/envelope_opaque_loads.rs` (net conduction vs. per-boundary columns).

## D-004: Gross consumption = load behind the meter

- **Reference behavior:** none (OCHRE reports net-column positive parts);
  this was HARES's own semantic error, recorded here because the fix is a
  deliberate contract.
- **HARES behavior (since I-02):** `gross_consumption_kwh` = net total plus
  self-consumed PV generation (the load actually served), so
  `net == gross_consumption − gross_pv` holds exactly for PV-only export;
  `renewable_energy_fraction` = PV / gross load, unclamped (a net-positive
  home honestly reports > 1.0).
- **Justification:** an always-exporting PV home consumed real energy;
  reporting 0.0 consumption (the positive part of the net meter) is false at
  the moment it matters most. Battery-discharge-to-grid breaks the identity
  by design (documented on the field).
- **Pinning tests:** `engine.rs` metric identity assertions,
  `tests/resstock_smoke.rs` energy invariants (5/5, includes the PV
  exporters).

## D-005: Appliance and plug-load heat has a radiant part

- **Reference behavior:** OCHRE adds an equipment's radiative gain fraction
  to its convective fraction and puts the sum on the zone air node
  (`ochre/Equipment/Equipment.py:80-85`, injected at line 197). Its source
  notes the gap: "FUTURE: separate convection and radiation, move radiation
  gains to the surfaces around the zone".
- **HARES behavior:** appliance, plug-load and fuel-load heat is split into
  a convective part on the zone air and a long-wave radiant part of 0.6 of
  the sensible fraction, distributed to the zone's interior surfaces. The
  ceiling fan, all of whose power is sensible heat, is 0.558 radiant.
- **Justification:** OpenStudio-HPXML v1.12.0 gives appliances
  `frac_radiant: 0.6 *` their sensible fraction
  (`HPXMLtoOpenStudio/resources/hotwater_appliances.rb`: washer 80, dryer
  121 and 132, dishwasher 172, refrigerator 220, freezer 268, range 309 and
  320), plug loads and fuel loads the same (`misc_loads.rb:89`,
  `misc_loads.rb:186`) and the ceiling fan `frac_radiant: 0.558`
  (`hvac.rb:1635`). EnergyPlus v24.2.0 carries an equipment's radiant part
  separately from its convected part (`InternalHeatGains.cc:7666-7667`) and
  distributes the radiant part to the zone's surfaces, not the air. Radiant
  heat warms the room's mass first, so air-only injection overstates how
  fast the air temperature responds.
- **Measured impact:** conditioned-zone temperature MAE against the OCHRE
  1-hour fixtures, all-convective then with this split:
  cz2a_gas_furnace_ac_res_wh 0.690 to 0.677 °C, cz2a_pv_ev 0.908 to 0.899,
  cz4a_ashp_hpwh 0.888 to 0.887, cz4a_battery_only 0.883 to 0.867,
  cz4a_pv_battery 0.851 to 0.836, cz4a_pv_only 0.894 to 0.882,
  cz5a_ev_charging 0.884 to 0.867, cz5a_ev_only 0.882 to 0.866,
  cz5a_minisplit_gas_wh 1.711 to 1.622, cz6b_pv_battery_ev 0.875 to 0.860,
  cz6b_resistance_res_wh 0.746 to 0.696. One fixture moves away from OCHRE:
  resstock_bldg0112631_24h zone temperature 0.408 to 0.421 °C, HVAC energy
  20.1 % to 21.8 %, site energy 5.14 % to 5.26 %.
- **Pinning tests:** `crates/hares-core/tests/appliance_zone_gains.rs`
  (`hpxml_appliance_gains_reach_the_conditioned_zone`,
  `resstock_event_load_replay_delivers_its_gains`),
  `gain_fractions::tests::radiant_part_of_sensible_leaves_the_rest_convective`,
  `resolve_loads::tests::radiant_part_follows_the_sensible_fraction`. With
  the heat made all convective, the resolver test fails when the resolver
  gives no radiant share and the others fail when the load drops its radiant
  part.

## D-006: Lighting heat is 0.2 convective, 0.6 radiant and 0.2 visible

- **Reference behavior:** OCHRE gives every lighting load
  `Convective Gain Fraction (-)` 1 and `Radiative Gain Fraction (-)` 0
  (`ochre/utils/hpxml.py:1522-1527`, under "TODO: get default
  fractions/multipliers for lighting"), so all lighting heat reaches the
  zone air.
- **HARES behavior:** lights in a space give 0.6 of their power as long-wave
  radiant gain to the interior surfaces, 0.2 as visible short-wave (D-007)
  and the remaining 0.2 to the zone air. Exterior lighting has no zone and
  gives no zone heat.
- **Justification:** OpenStudio-HPXML v1.12.0 `Model.add_lights` sets
  `FractionRadiant` 0.6, `FractionVisible` 0.2 and `ReturnAirFraction` 0
  (`HPXMLtoOpenStudio/resources/model.rb:249-251`). EnergyPlus v24.2.0
  takes the convected part as the remainder of those fractions
  (`InternalHeatGains.cc:1393`) and the radiant part as
  `Q * FractionRadiant` (line 7639).
- **Measured impact:** conditioned-zone temperature MAE, appliance split only
  then with this lighting split: cz5a_minisplit_gas_wh 1.622 to 1.743 °C,
  cz6b_resistance_res_wh 0.696 to 0.708, resstock_bldg0112631_24h 0.421 to
  0.441 (all three away from OCHRE); cz2a_gas_furnace_ac_res_wh 0.677 to
  0.676, cz2a_pv_ev 0.899 to 0.898, cz4a_ashp_hpwh 0.887 to 0.884,
  cz4a_battery_only 0.867 to 0.863, cz4a_pv_battery 0.836 to 0.834,
  cz4a_pv_only 0.882 to 0.880, cz5a_ev_charging 0.867 to 0.865,
  cz5a_ev_only 0.866 to 0.862, cz6b_pv_battery_ev 0.860 to 0.858.
  cz5a_minisplit_gas_wh short-window HVAC energy 64.25 % to 64.15 % and site
  energy 17.15 % to 17.19 %.
- **Pinning tests:** `appliance_zone_gains.rs`
  (`hpxml_lighting_splits_convective_radiant_and_visible`),
  `gain_fractions::tests::visible_part_takes_the_short_wave_path`,
  `resolve_loads::tests::lighting_carries_a_visible_part`,
  `resolve_loads::tests::radiant_part_follows_the_sensible_fraction`. Each
  fails when lighting is made all convective, at the resolver or at the
  equipment.

## D-007: Visible light is absorbed like transmitted diffuse solar

- **Reference behavior:** OCHRE has no short-wave internal gain; the visible
  part of lighting is zone-air heat (D-006).
- **HARES behavior:** the visible part travels on its own short-wave port
  value and is spread over the zone's interior surfaces by the
  transmitted-solar distribution, as diffuse: in proportion to area times
  inside solar absorptance, each surface's share split between its node and
  the zone air by its radiation fraction. A zone with no interior surface
  list takes the gain at its air node.
- **Justification:** EnergyPlus v24.2.0 computes the lights' visible gain
  as `Q * FractionShortWave` and adds it to the space's `QLTSW`
  (`InternalHeatGains.cc:7640,7649`), then adds `QLTSW` to the enclosure's
  diffuse short-wave, `EnclSolQSWRad = EnclSolQD + sumSpaceQLTSW`
  (`HeatBalanceSurfaceManager.cc:3687-3693`), which the inside faces absorb
  by their solar absorptance.
- **Beyond EnergyPlus:** EnergyPlus also counts each window's diffuse
  transmittance and inside absorptance in the enclosure's absorption sum
  (`SUM1`, `HeatBalanceSurfaceManager.cc:4272`; `solVMULT = 1 / SUM1` at
  line 4315), so part of the visible light leaves through the glazing.
  HARES gives windows no share of the distribution, so all of the visible
  light stays in the zone, absorbed by the opaque surfaces. The window share
  is an open item for the transmitted-solar distribution as a whole.
- **Measured impact:** against sending the visible part to the zone air,
  the conditioned-zone temperature MAE moves by 0.0003 to 0.011 °C:
  cz5a_minisplit_gas_wh 1.749 to 1.743, cz6b_resistance_res_wh 0.719 to
  0.708, cz4a_battery_only 0.864 to 0.863, cz5a_ev_only 0.863 to 0.862,
  cz4a_ashp_hpwh 0.885 to 0.884, and the other OCHRE-paired fixtures by less
  than 0.001 °C toward OCHRE; resstock_bldg0112631_24h 0.436 to 0.441, away
  from OCHRE.
- **Pinning tests:**
  `thermal_solver::tests::shortwave_gains_are_absorbed_like_transmitted_diffuse_solar`
  (every short-wave watt is deposited, by area times absorptance). It fails
  when the visible part is sent to the zone air.
