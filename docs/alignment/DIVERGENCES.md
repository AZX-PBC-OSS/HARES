# Deliberate Divergences from Reference Implementations

Every entry is a place where HARES knowingly differs from OCHRE (or another
reference) on physics. Each divergence exists because the first-principles
derivation or a more authoritative reference (EnergyPlus Engineering
Reference, ASHRAE) demands it. Anyone re-aligning with the reference
implementation will "fix" these back — this ledger is what stops that.

Entry format: quantity → reference behavior → HARES behavior →
justification → class → measured impact → pinning tests. The class is
the triage taxonomy's: (a) OCHRE wrong or simplified, (b) HARES wrong,
(c) architecture, (d) comparison.

---

## D-001: Exterior radiative-split resistance (`rad_res_k_w`)

- **Quantity:** the outside skin's radiative coupling: the film's parallel
  split against OCHRE's bare-film divider.
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
- **Class:** (a). OCHRE's bare-film divider is its own approximation (its
  comment's assumption `res_material >> res_film`); the exact parallel form is
  the reference physics.
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

- **Quantity:** what the exterior-skin diagnostics report: the skin-level
  absorbed flux against the rad_frac-scaled injection.
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
- **Class:** (b) in HARES (the mixed semantics were HARES's own error),
  recorded as the contract of record.
- **Measured impact:** the freefloat oracle's opaque comparison became
  like-for-like (see D-001); the I-02 gross/net confusion becomes
  unprovable: the gross is the absorbed skin flux, the zone load is the
  inside-face convection (`wall/floor/roof_heat_gain_w`).
- **Pinning tests:** `exterior_skin_balance.rs`
  (`assert_exterior_diagnostics_split_honestly` — absorbed solar exact, net
  skin LWR at the converged skin temperature, sum identity).

## D-003: Per-step TARP interior convection (diagnostic and injection)

- **Quantity:** the interior convection the reported zone loads carry:
  the per-step ΔT-dependent film against the frozen one.
- **Reference behavior:** OCHRE's interior film conductance is frozen at
  init-time in the A-matrix.
- **HARES behavior:** the per-boundary net-convection accumulation uses the
  ΔT-dependent TARP model (Walton 1983, Eqs. 90–92 — the EnergyPlus default
  interior convection algorithm) per step; the A-matrix discretization
  remains frozen (tracked limitation, tickets T-0082/T-1922).
- **Justification:** EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior
  Convection; the reported zone loads are the physically correct convective
  flux, not the discretization's artifact.
- **Class:** (a). The per-step TARP model is the EnergyPlus default interior
  convection algorithm; OCHRE's frozen film is its simplification. The frozen
  A-matrix discretization is a tracked limitation, not the divergence of
  record.
- **Measured impact:** none recorded separately: the per-boundary columns'
  like-for-like reading is exercised by the pinning tests and the BESTEST and
  parity bounds hold.
- **Pinning tests:** `bestest_900ff_root_cause.rs`,
  `tests/envelope_opaque_loads.rs` (net conduction vs. per-boundary columns).

## D-004: Gross consumption = load behind the meter

- **Quantity:** the gross-consumption metric's definition: the load behind
  the meter against the net column's positive part.
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
- **Class:** (b) in HARES (the positive-part reading was HARES's own
  semantic error), recorded as the contract of record.
- **Measured impact:** the identity holds exactly on the corpus's PV
  exporters (the resstock_smoke invariants, 5/5); the failure the fix removed
  is a home reporting 0.0 consumption while serving real load.
- **Pinning tests:** `crates/hares-io/src/output/metrics.rs`
  (`finish_separates_net_consumption_and_pv_generation`,
  `renewable_fraction_uses_abs_for_negative_pv`,
  `zero_net_with_high_gross_consumption_and_pv_is_correct`,
  `all_negative_power_gives_negative_net_positive_gross_pv`),
  `tests/resstock_smoke.rs` energy invariants (5/5, includes the PV
  exporters).

## D-005: Appliance and plug-load heat has a radiant part

- **Quantity:** the split of appliance, plug-load and fuel-load heat into a
  convective part on the air and a long-wave radiant part on the surfaces.
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
- **Class:** (a). OCHRE's air-only injection is its own simplification (its
  source's FUTURE note says the same); the radiant split is the reference's.
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
  `resstock_event_load_replay_delivers_its_gains`,
  `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares`,
  `every_gain_override_changes_the_split`),
  `gain_fractions::tests::radiant_part_of_sensible_leaves_the_rest_convective`,
  `resolve_loads::tests::resolver_carries_the_radiant_share`. With
  the heat made all convective, the resolver test fails when the resolver
  gives no radiant share and the others fail when the load drops its radiant
  part.

## D-006: Lighting heat is 0.2 convective, 0.6 radiant and 0.2 visible

- **Quantity:** lighting heat's split: 0.6 long-wave radiant, 0.2 visible
  short-wave, 0.2 convective, against OCHRE's all-convective.
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
- **Class:** (a). OCHRE's fractions are its own placeholder (its source's
  TODO note says the same); the split is the reference's.
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
  (`hpxml_lighting_splits_convective_radiant_and_visible`,
  `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares`,
  `every_gain_override_changes_the_split`),
  `gain_fractions::tests::visible_part_takes_the_short_wave_path`,
  `resolve_loads::tests::lighting_carries_a_visible_part`,
  `resolve_loads::tests::resolver_carries_the_radiant_share`. Each
  fails when lighting is made all convective, at the resolver or at the
  equipment.

## D-007: Visible light is absorbed like transmitted diffuse solar

- **Quantity:** where the lights' visible part is deposited: the zone's
  short-wave distribution against the zone air.
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
  (every short-wave watt is deposited, by area times absorptance) and its
  `_in_star_mesh_mode` twin for the default interior mode,
  `crates/hares-envelope/tests/thermal_pathway_physics.rs`
  (`zone_sensible_breakdown_debug_must_include_radiant_air_residual`,
  `star_mesh_breakdown_splits_shortwave_between_surface_and_air`). Each
  fails when the visible part is sent to the zone air in its mode.
  `production_step_injects_shortwave_gain` fails when the production step
  leaves the short-wave gain out.

## D-008: Wind terrain from the site, not an assumed rural one

- **Quantity:** the wind terrain (the Sherman-Grimsrud multiplier and
  exponent) driving the attic and garage leakage.
- **Reference behavior:** OCHRE drives attic and garage leakage with
  rural terrain, multiplier 0.85 and exponent 0.20, whatever the site
  ("assumed rural for now", `ochre/utils/envelope.py:648-649`). OS-HPXML
  v1.12.0 takes the terrain from the HPXML `SiteType`: (0.85, 0.20) rural,
  (0.67, 0.25) suburban, (0.47, 0.35) urban (`airflow.rb:200-221`), in
  its Sherman-Grimsrud factor `f_t_SG` (`airflow.rb:2766`).
- **HARES behavior:** the site's `SiteType` sets the terrain, and attic
  and garage leakage use OS-HPXML's `f_t_SG`
  (`sherman_grimsrud_terrain_factor`, `hares-physics/src/infiltration.rs`).
  A file without a `SiteType` is suburban, OS-HPXML's default
  (`defaults.rb:817-818`), with a warning in the dwelling's log; a value
  outside rural, suburban and urban is a parse error (`parse_site_type`,
  `hares-io/src/hpxml/building.rs`).
  This follows OS-HPXML and departs from OCHRE.
- **Justification:** the input states the terrain. A suburban house does
  not see open-country wind.
- **Class:** (a). OCHRE's rural assumption is its own placeholder ("assumed
  rural for now"); OS-HPXML reads the site's type, and so does HARES.
- **Measured impact:** `data/examples/BEopt_example.xml` is suburban. At
  its attic (2.36 m of hip above a 2.44 m walls top), f_t falls from 0.610
  (the ASHRAE power law HARES used before) to 0.558, against OCHRE's 0.741,
  and the wind coefficient by 16 %. Summer attic MAE against the OCHRE
  conditioned oracle rose from 2.14 to 2.16 °C. The move is away from
  OCHRE by design.
- **Pinning tests:** `infiltration::tests::ela_wind_terrain_factor_is_os_hpxml_sherman_grimsrud`
  and `conversions::tests::the_site_type_sets_the_terrain`. Both fail when
  suburban terrain takes OCHRE's rural (0.85, 0.20) or the site type is
  ignored for rural. The second also pins the suburban default.

## D-009: A gable attic's volume from its span and pitch

- **Quantity:** a gable attic's zone volume and its leakage height.
- **Reference behavior:** OS-HPXML v1.12.0 has one attic rule, a square
  hip under the roofs whatever their shape: volume one third of the roof
  footprint times a height of 0.5 sin(atan(slope)) sqrt(footprint)
  (`geometry.rb:1315-1330` and `:1373-1393`, "Assume square hip roof").
  OCHRE sizes a gable attic from its gable walls: rise
  sqrt(gable area × tan(pitch)), volume half the floor area times it
  (`ochre/utils/hpxml.py:617`, in `parse_hpxml_zones`).
- **HARES behavior:** a gable attic (walls to the outside under roofs of
  one pitch facing two opposite ways) takes its geometric volume, half the
  roof footprint times the ridge rise (span / 2) tan(pitch). The span is a
  side of the rectangle whose area is the roof footprint and whose
  perimeter is the conditioned wall area over the storey wall height. The
  ends' area comes from the outside gable walls: two or more are the ends
  themselves and must be of equal size, and a single one is read both as
  one end and as both ends together (a detached sample writes the two ends
  as one wall, an attached unit's single outside gable wall is one end
  whose twin is the party-side attic wall), the reading whose ends agree
  with a span's triangle winning. The end area must lie from 0.98 to
  (1 + 2 × 5 ft / span)² times that span's triangle: no smaller than the
  triangle beyond rounding, and no larger than the deepest eaves OS-HPXML's
  home builder offers (5 ft,
  `BuildResidentialHPXML/resources/options/geometry_eaves.tsv`) would make
  it. Across the committed ResStock and OS-HPXML sample homes, every gable
  whose span the walls fix has ends of 0.9993 to 1.0008 times its
  triangle, the attached units' single outside wall included;
  BEopt's, whose ends include 2 ft eaves, are 1.28. The span must
  also be well determined: a 0.25 % error in the walls' area (a heuristic,
  three times that largest disagreement) may move it by at most 10 %, which
  rejects a footprint near a square (`gable_rise_m`,
  `hares-io/src/hpxml/zone_geometry.rs`). Where the input does not
  determine the span (an attic over a garage, floors that do not match the
  footprint, a smaller upper storey, no floor counts, gable ends that
  disagree with the span or are of different sizes, a footprint near a
  square), the attic keeps
  OS-HPXML's hip, with a warning naming why.

  The ridge rise is also the attic's height for its leakage: the stack
  coefficient grows with it and the wind coefficient takes the
  attic's top above grade (`attic_infiltration_method`,
  `hares-core/src/dwelling/solver_builder.rs`). OS-HPXML feeds its
  leakage the hip height (`calc_wind_stack_coeffs`,
  `airflow.rb:2749-2771`, with `calculate_zone_height`,
  `geometry.rb:1362`), so a gable attic's leakage departs from OS-HPXML's
  too: on `base.xml` the rise is 7.5 ft against the hip's 8.216 ft, a 9 %
  lower stack coefficient.
- **Justification:** the hip rule undercounts a gable attic's air:
  OS-HPXML's `base.xml` gable holds 143.4 m³ and the hip gives 104.7 m³
  (−27 %). OCHRE's gable-wall formula inherits whatever the gable walls
  include. BEopt's include the eave overhang: 144.5 ft² per end on a
  30 ft span, whose 6:12 triangle is 112.5 ft². That raises the ridge
  from 7.5 to 8.5 ft and the volume by 13 %. The stack effect is driven by
  the height of the air column the attic actually holds, which is the
  ridge rise of the geometry whose volume it takes.
- **Class:** (a) against both references' rules: the hip undercounts the
  volume and OCHRE's gable-wall rise inherits the eaves; HARES reads the
  input's geometry.
- **Measured impact:** on `data/examples/BEopt_example.xml` the attic is
  127.4 m³ at a 2.29 m rise. OS-HPXML's hip gives 87.7 m³ and OCHRE 144.4
  m³. Attic MAE against the OCHRE conditioned oracle, from the hip to this
  volume (both at the then MediumRough roof roughness): summer 2.16 to
  1.77 °C (dynamic), spring 1.76 to 1.47 °C, winter 1.55 to 1.44 °C.
  OCHRE's eave-inflated volume would give 1.55, 1.30 and 1.39 °C; that
  closer agreement is not taken. Indoor MAE is unchanged within 0.01 °C.
  The leakage height alone moves little: with the hip height (2.36 m)
  in place of the rise (2.29 m) for the leakage only, at the current
  roughness, the attic MAE moves by at most 0.008 °C in any scenario
  (summer 0.894 to 0.886 °C, winter 1.275 to 1.279 °C).
- **Pinning tests:** `hpxml_parsing_tests::a_gable_attic_takes_its_geometric_volume`,
  `hpxml_parsing_tests::eaves_in_the_gable_walls_do_not_raise_the_attic`,
  `hpxml_parsing_tests::an_attached_unit_s_gable_end_is_its_full_outside_wall`,
  `hpxml_parsing_tests::a_gable_attic_of_unknown_span_is_a_square_hip_with_a_warning`,
  and `structural_envelope_oracle` (`beopt_building_structure`,
  `beopt_rc_network_topology`, `beopt_ua_parity`). With OS-HPXML's hip,
  the first two and all three structural tests fail. With OCHRE's
  gable-wall rise, the eaves test and the three structural tests fail;
  `a_gable_attic_takes_its_geometric_volume` does not discriminate it,
  since `base.xml`'s gable walls are exactly their triangles and both
  rules give 7.5 ft there.
  `solver_builder::tests::a_gable_attic_leaks_by_its_geometric_rise`
  pins the leakage coefficients to the rise and fails with the hip
  height.

## D-010: Outside convective roughness from the surface's material

- **Quantity:** the outside convective film's roughness class of each
  surface: the material's EnergyPlus record against one `Rough` for
  everything.
- **Reference behavior:** OS-HPXML v1.12.0 gives every surface the
  roughness `Rough` (multiplier 1.67): `model.rb:49` and `:95` default
  `roughness: 'Rough'` and no caller passes another. OCHRE hardcodes 1.67
  for every outside surface, its material lookup commented out
  (`ochre/utils/envelope.py:393`).
- **HARES behavior:** an outdoor-facing wall or rim joist takes the
  roughness class of its HPXML `Siding`, a roof of its `RoofType`, a
  foundation wall of its `Type` (`MATERIAL_ROUGHNESS` and
  `outside_layer_roughness`, `hares-physics/src/film_coefficients.rs`).
  One rule: a material takes the Roughness field of the record that names
  it in EnergyPlus v24.2.0's `datasets/ASHRAE_2005_HOF_Materials.idf`,
  searched family by family (the `Siding:` records for a siding, the
  `Shingles:` and roofing records for a roof, then the exterior finish
  layers F06-F15, the masonry M01-M09 and the concrete records); a
  material that is a kind of a named one takes that record, and a green
  roof, which has none, takes `Material:RoofVegetation`'s default
  roughness (`idd/Energy+.idd.in:5064-5072`). The table cites each
  record by name and line:

  | HPXML material | Dataset record (line) | Class |
  |---|---|---|
  | wood siding | Siding: Wood - bevel 13 by 200mm - lapped (2047) | Rough |
  | stucco | F07 25mm stucco (124) | Smooth |
  | synthetic stucco | F06 EIFS finish (116) | Smooth |
  | vinyl siding, aluminum siding | Siding: Hollow-backed (2062) | Smooth |
  | brick veneer | M01 100mm brick (332) | Medium Rough |
  | stone veneer | F10 25mm stone (148) | Medium Rough |
  | asbestos siding, fiber cement siding | Siding: Asbestos-cement 6.4mm (2022) | Very Rough |
  | composite shingle siding | Asphalt shingles (1897) | Very Rough |
  | masonite siding | Siding: Hardboard 11mm (2037) | Medium Smooth |
  | asphalt or fiberglass shingles, shingles | Asphalt shingles (1897) | Very Rough |
  | wood shingles or shakes | Shingles: Wood 400mm - 190-mm exposure (2007) | Medium Rough |
  | slate or tile shingles | Slate - 13mm (1907) | Very Rough |
  | concrete tiles | F14 Slate or tile (180) | Very Rough |
  | metal surfacing | F08 Metal surface (132) | Smooth |
  | plastic/rubber/synthetic sheeting | Built-up roofing - 10mm (1902) | Very Rough |
  | expanded polystyrene sheathing | EPS molded beads 16 kg/m3 (810) | Very Rough |
  | concrete (roof), solid concrete (foundation wall) | Concrete: sand and gravel 2400 kg/m3 (1018) | Medium Rough |
  | concrete block (all cores) | M05 200mm concrete block (364) | Medium Rough |
  | double brick | M01 100mm brick (332) | Medium Rough |
  | wood (foundation wall) | Siding: Wood - plywood 9.5mm - lapped (2057) | Rough |
  | green roof | Material:RoofVegetation default | Medium Rough |

  The Engineering Reference's Walton table gives stucco and brick as its
  Very Rough and Rough examples; the dataset's stucco and brick records
  are Smooth and Medium Rough, and the dataset is followed. Where two
  records in the searched families name the same material, the family
  order decides: wood shingles take `Shingles: Wood 400mm` (2007, Medium
  Rough) over the roofing family's `Wood shingles - plain and plastic
  film faced` (1912, Very Rough), and composite shingle siding takes
  `Asphalt shingles` (1897) over the asphalt siding records `Siding:
  Asphalt roll` and `Siding: Asphalt insulating` (2027, 2032), which name
  roll and board sidings, not shingles; all three are Very Rough. An
  absent element takes the material OS-HPXML defaults it to, with a
  warning in the dwelling's log: wood siding for a wall or rim joist
  (`defaults.rb:1338`, `:1379`), asphalt shingles for a roof (`:1268`),
  solid concrete for a foundation wall (`:1431-1433`). An explicit value
  that names no material ("other", "unknown", "not present", the pre-v4
  "none", "no one major type", "cool roof") is treated as absent, a HARES
  policy (OS-HPXML defaults only an absent element), with a warning that
  says so. A value outside the element's HPXML enumeration is an error
  naming the boundary. Doors, floors and slabs, which have no material
  element in HPXML, take `Rough` without a warning, as OS-HPXML gives
  them; glazing takes glass's Very Smooth. A synthetic (TOML) building
  names a boundary's material with `outside_material`. ASHRAE 140 states
  no outside roughness; its fixtures' walls and roof deck are the same
  wood (k 0.14 W/m·K, 530 kg/m³), named wood siding (Rough) on the walls
  and, on the roof, wood shingles (Medium Rough), the only wood material
  HPXML's `RoofType` has. The plywood record a wood deck would take
  (`Siding: Wood - plywood`, 2057, Rough) is not a roof material in
  HPXML, so Medium Rough is a modelling choice. It matters little: with
  the roof at Rough instead, case 600's annual cooling load is 6086.1
  against 6110.3 kWh (−0.4 %) and its heating load 3223.2 against 3227.5
  kWh, and case 900's heating load 952.9 against 952.0 kWh; every BESTEST
  pinned bound holds either way.
- **Justification:** EnergyPlus's DOE-2 and TARP outside convection scale
  forced convection by the roughness of the outside material layer.
  OS-HPXML builds that layer without passing the roughness its material
  has, so a shingle roof and a vinyl wall convect alike. The dataset
  record for the same material is the evidence for each class; every row
  departs from OS-HPXML's one `Rough`.
- **Class:** (a). EnergyPlus's roughness-by-material is the physics;
  OS-HPXML's single `Rough` is its simplification.
- **Measured impact:** on `data/examples/BEopt_example.xml` (asphalt
  shingle roof, vinyl siding), attic MAE against the OCHRE conditioned
  oracle in the dynamic runs:

  | Run | Summer (°C) | Spring (°C) | Winter (°C) |
  |---|---|---|---|
  | HARES's old MediumRough default | 1.77 | 1.47 | 1.44 |
  | OS-HPXML's and OCHRE's 1.67 | 1.48 | 1.24 | 1.37 |
  | Material roughness | 0.89 | 0.78 | 1.28 |

  Indoor MAE moves by at most 0.03 °C. The oracle is not evidence for any
  class: the closer summer and spring agreement comes from departing from
  OCHRE's own 1.67. The winter attic cold bias, about −1.2 °C, is not a
  roughness effect (roughness moves the winter attic by 0.05 °C); its
  cause is not yet established.
- **Pinning tests:** `film_coefficients::tests::every_hpxml_material_takes_its_energyplus_roughness`
  (each row against its record),
  `film_coefficients::tests::a_layer_without_a_material_takes_os_hpxml_default_material`,
  `film_coefficients::tests::a_material_outside_the_layer_enumeration_is_an_error`,
  `conversions::tests::only_material_bearing_boundaries_read_their_material`
  and `conversions::tests::finish_type_roughness_changes_exterior_film_resistance`.
  All but the third fail when every surface takes OS-HPXML's `Rough`.

## D-011: The equivalent battery holds the multiplied zone capacitance

- **Quantity:** the equivalent battery's capacitance basis: the multiplied
  zone capacitance against the air alone.
- **Reference behavior:** OS-HPXML v1.12.0 multiplies every zone's air
  capacitance by one temperature capacitance multiplier, 7 by default
  (`simcontrols.rb:27-28`, `defaults.rb:219-221`); it has no equivalent
  battery. OCHRE's equivalent battery reads its zone's capacitance, which
  carries the same 7 (`ochre/Equipment/HVAC.py:625-640`,
  `Models/Envelope.py:443-446`).
- **HARES behavior:** every zone's air node takes the building's
  multiplier, and the dwelling hands the conditioned zone's capacitance,
  multiplier included, to each HVAC unit's equivalent battery, whose
  energy and its minimum and maximum scale with it
  (`hares-equipment/src/hvac/equivalent_battery.rs`). This is a
  consequence of the multiplier, not a departure from either reference:
  HPXML homes with furniture boundaries previously took 1 on the
  conditioned zone.
- **Justification:** a design decision, not a reference's: neither
  OS-HPXML nor EnergyPlus defines an equivalent battery. HARES makes the
  battery describe the storage its thermal solver integrates, so with the
  multiplier on the zone, the same capacitance is in the battery, as in
  OCHRE's. A battery defined on the air alone would report a seventh of
  the storage the simulated zone shows.
- **Class:** none (a consequence of the capacitance multiplier, not a
  departure from either reference).
- **Measured impact:** the conditioned zone's capacitance on the golden
  `consumer_shape_900s` home (`bldg0176227`, 749.8 m³) is 1.7644 kWh/K
  against 0.2521 kWh/K before, and on `BEopt_example.xml` (271.8 m³)
  0.6403 against 0.0915 kWh/K. A calibration reading the equivalent
  battery's energy, minimum and maximum sees seven times the storage on
  such homes, and the zone recovers its thermostat band seven times more
  slowly.
- **Pinning tests:** `equipment_zone_resolution::the_equivalent_battery_holds_the_multiplied_zone_capacitance`
  (a unit's largest maximum energy at the default 7 is seven times its
  value at 1) and `boundary_rc::tests::zone_capacitance_requires_a_positive_multiplier`.

## D-012: The one-hour parity windows' delivered energy against OCHRE's envelope model

- **Reference behavior:** OS-HPXML v1.12.0 builds each surface from its
  HPXML construction and drives its zones with the weather; its zone air
  carries one temperature capacitance multiplier, 7 by default
  (`defaults.rb:219-221`, applied to EnergyPlus's
  `OS:ZoneCapacitanceMultiplier:ResearchSpecial`,
  `simcontrols.rb:27-28`). EnergyPlus v24.2.0's zone heat balance
  integrates that network, and its residential heating equipment cycles
  on and off at rated capacity under its thermostat
  (`EnergyPlus/HVACManager.cc`, the on/off system cycling).
- **OCHRE's model:** its envelope is a fitted RC network over the same
  multiplied capacitance (`ochre/Models/Envelope.py:442-446`), its HVAC
  heating column carries the blower power inside the end use
  (`Equipment/HVAC.py:514-519` and `:547-554`), and its water heater and
  other equipment differ in detail from the constructions above.
- **HARES behavior:** the constructions, film coefficients and
  infiltration rules of OS-HPXML v1.12.0 (the departures of record are
  D-008 to D-010), the blower's power carried outside the heating end
  use.
- **Measured impact:** on `cz6b_resistance_res_wh` (a resistance furnace
  at the -26 °C design temperature, one hour from the pinned start both
  sides hold): both models cycle the element at its rating (HARES
  10.551 kW, the HPXML's 36000 W; OCHRE 10.944 kW, the element plus its
  0.39 kW blower), but the duty differs, 24 on-minutes against 30: the
  two networks put the same total capacitance in different places, so
  the air node's swing and the cycling phase differ, and the window ends
  mid-cycle. The delivered energy is 4.40 kWh against 5.65 kWh: 22.1 %
  of HVAC energy and 20.9 % of total site; the difference is the duty
  (1.06 kWh) plus the blower's accounting (0.20 kWh). The indoor
  temperatures agree to 0.41 °C MAE, inside the corpus's 0.6 °C band.
  The corpus's other heated fixture, `cz2a_gas_furnace_ac_res_wh`, sits
  at 6.5 % on the same window shape; its unheated variants at 0.5 %. The
  residual is the one-hour window's phase sensitivity to the two
  models' zone dynamics, not a missing end use or a one-sided input.
- **Class:** (a). OCHRE's fitted RC network is its own simplification of
  the constructions; HARES keeps the reference physics. No HARES change
  is made toward OCHRE's numbers.
- **Documented bound:** the fixture's bands in `tests/parity/mod.rs` are
  the measured residuals plus margin: 23.1 % HVAC energy, 21.9 % total
  site.
- **Pinning tests:** the corpus's drift lock
  (`parity::parity_outputs_against_reference_corpus`, which fails when
  the residual leaves the documented bound),
  `equipment_zone_resolution::the_equivalent_battery_holds_the_multiplied_zone_capacitance`
  and the structural envelope oracles pin the capacitance and the
  constructions the bound stands on.

## D-013: A load's month multipliers scale its schedule

- **Quantity:** a scheduled load's month multipliers: a scale on the
  month's draw against OCHRE's zero-only reading.
- **Reference behavior:** OCHRE reads a scheduled load's
  `month_multipliers` only to zero the schedule in the months whose
  multiplier is exactly 0 (`ochre/Equipment/ScheduledLoad.py:36-40`, "for
  ceiling fans"); any other value is ignored. An HPXML load's monthly shape
  is already in the schedule OCHRE builds for it.
- **HARES behavior:** `month_multiplier_0` to `month_multiplier_11` scale
  the electric and gas draw of a scheduled load, and the event power of an
  event load or wet appliance, in their month, whatever the schedule's
  source; a daily profile's own month factors stay its shape and the
  multipliers apply on top. A multiplier of 0 turns the month off, as in
  OCHRE. A negative or non-finite multiplier is a parameter error.
- **Justification:** the parameter names a scale factor, and an input of
  0.5 that changes nothing is a silent substitution. Zeroing is the special
  case of scaling, so every input OCHRE gives meaning to means the same in
  HARES.
- **Class:** (a) on the parameter's meaning: OCHRE's zero-only reading is
  its own limitation (its source names it "for ceiling fans"); zeroing is
  the special case of scaling.
- **Measured impact:** none on the committed inputs: no resolver writes
  the per-month keys (an HPXML extension's `MonthlyScheduleMultipliers`
  feeds the schedule profile's own monthly shape, which OCHRE bakes into
  its generated schedule the same way), so they come only from an
  equipment's own parameters or an override, and no fixture or golden
  sets them.
- **Pinning tests:** `scheduled_load::tests::month_multipliers_scale_a_daily_profile`,
  `scheduled_load::tests::month_multiplier_zeroes_schedule_in_target_month`,
  `event_load::tests::month_multiplier_scales_event_output`,
  `event_load::tests::month_multiplier_zero_suppresses_event_output`,
  `schedule_helpers::tests::negative_and_non_finite_month_multipliers_are_parameter_errors`.

## D-014: ASHP single-speed heating defaults from the RESNET anchor model, not the OCHRE CSV

- **Quantity:** the default heating capacity and EIR temperature curves of
  a single-speed air-source heat pump (the curves HARES substitutes when
  the HPXML document supplies no biquadratic coefficients).
- **Reference behavior:** OpenStudio-HPXML v1.12.0 anchors the default
  heating model on RESNET HERS Addendum 82: capacity 1.0 at the 47 °F
  rating point and `qm17full = 0.626` at 17 °F
  (`HPXMLtoOpenStudio/resources/defaults.rb:8254`), EIR
  `eirm17full = 1.356` at 17 °F (`defaults.rb:8287`), both linear in
  outdoor temperature between and below the anchors (the defaulted
  datapoints all sit at the rated indoor temperature: `correct_ft_cap_eir`
  sets each heating datapoint's indoor temperature to the 70 °F
  `AirSourceHeatRatedIDB`, `hvac.rb:2956-2968`; the E+ coil reads them
  through a
  `Table:Lookup` with linear extrapolation and `output_min: 0.0`,
  `hvac.rb:3240-3252`; the no-detailed-data biquadratic fallback carries
  ±100 °C curve limits, `hvac.rb:3254-3266`). EnergyPlus evaluates the
  curve inputs clamped to the curve's limits (the CurveManager),
  resets a negative capacity modifier to 0 with a warning
  (`DXCoils.cc:10950-10965`), runs the compressor only above
  `MinOATCompressor` (`DXCoils.cc:10874-10876`), and applies the on-demand
  reverse-cycle defrost model
  (`DXCoils.cc:10995-10999`: `1/(1 + 0.01446/dw)`, capacity
  `0.875·(1-FDT)`, power `0.954·(1-FDT)`).
- **HARES behavior (since this fix):** the single-speed ASHP default
  capacity curve is the reference's capacity line in °C
  (`0.813 + 0.02244·To`, exact at the anchors and by extrapolation), and
  the default EIR curve is the quadratic through the reference's three
  points (1.0 at 8.333 °C, 1.356 at -8.333 °C, 1.785449 at -17 °C).
  Defrost keeps the E+ on-demand model (constants shared:
  `heat_pump/constants.rs:7-12`), and the curve machinery already
  mirrors the E+ CurveManager clamping (`hares-physics`
  `biquadratic.rs:23-30`).
- **Justification:** OCHRE 0.9.2's default curves
  (`ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv`,
  `Single_1`) are the old DOE-2 fits OS-HPXML itself shipped before
  v1.12.0, with ±100 °C bounds that clamp nothing
  (`min_Tdb`/`max_Tdb` rows); extrapolated to -17 °C they read capacity
  0.485 and EIR 1.545 where the reference's anchor model reads 0.432 and
  1.785: the COP came out 16% optimistic at the temperature where this
  class of unit runs just above its -17.8 °C lockout. Class
  (a): OCHRE's default curve set is outdated/simplified; HARES keeps the
  reference physics. OCHRE stays the bar for the rated-COP conversion
  (HSPF/3.412141633, `ochre/utils/hpxml.py:855-856`), which OS-HPXML
  v1.12.0 replaces with the Addendum 82 `cop47full` interpolation table
  (`defaults.rb:8288-8298`): HARES's absolute COP at a given outdoor
  temperature is therefore lower than OS-HPXML's read of the same unit
  (measured below); the curve shapes now agree.
- **Measured impact:** for the 12 kW, HSPF 9.5 ASHP of the pinning tests
  at -17 °C outdoor (dry January air, RH ≈ 80%): full-load compressor
  COP 1.80 → 1.56, capacity at -17 °C 5.82 kW → 5.18 kW. Between the anchors
  the quadratic EIR sits within ~3% of the reference's rational form
  (at 0 °C: 1.101 vs 1.137); above the rating point the quadratic turns
  EIR back up where the reference's linear power model keeps improving
  COP (at 20 °C: 1.118 vs 0.709): heating there is rare and modulated
  down, and the old curve understated the gain too (1.083 measured
  ratio). The two-speed and variable-speed ASHP defaults and the MSHP
  defaults keep the OCHRE CSV columns (no measured case; their
  extrapolation below the 17 °F anchor is a candidate for its own row).
- **Pinning tests:**
  `crates/hares-equipment/src/hvac/default_curves.rs`
  (`ashp_single_capacity_matches_resnet_anchor_model`,
  `ashp_single_eir_matches_resnet_anchor_model`,
  `ashp_single_compressor_cop_at_minus_17c_stays_above_one`),
  `crates/hares-equipment/src/hvac/heat_pump/heater.rs`
  (`ashp_default_curves_match_reference_at_minus_17c`),
  `crates/hares-equipment/src/hvac/hvac_core.rs`
  (`ashp_default_multi_speed_curves_match_ochre_csv_columns`). Each
  fails when the defaults revert to the OCHRE CSV's `Single_1` column.

---

## D-015: A conditioned basement merges into the conditioned space

- **Quantity:** the thermal zone a conditioned foundation (a finished
  basement, an explicitly conditioned crawlspace) occupies, and the HVAC
  capacity its air receives.
- **Reference behavior:** OS-HPXML v1.12.0 maps every conditioned
  location into the one conditioned space: `geometry.rb`
  `get_space_from_location` (1704-1716) rewrites
  `HPXML::conditioned_locations` (hpxml.rb:12311-12316: conditioned
  space, basement - conditioned, crawlspace - conditioned, other housing
  unit) to `LocationConditionedSpace`, and the HVAC serves that zone
  (`hvac.rb:97` and every `apply_*` entry point). Surfaces adjacent to a
  conditioned below-grade location get their adjacent surface created
  inside `LocationConditionedSpace` (geometry.rb:1495-1502), and the
  conditioned floor area and building volume count the basement
  (`geometry.rb` `apply_conditioned_floor_area`, 750-771).
- **OCHRE behavior:** the fork keeps a finished basement a separate
  `Foundation` zone with no setpoint and no HVAC serving it
  (`ochre/utils/hpxml.py:673-684` builds it for
  `total_floors > indoor_floors`; hpxml.py:688-689 leaves it unvented
  with no infiltration method), and routes 20% of a heater's DSE-adjusted
  capacity into it as a duct-style split (`ochre/Equipment/HVAC.py:181-184`).
  The zone floats between ground and indoor temperature instead of being
  conditioned space.
- **HARES behavior (since this fix):** a conditioned foundation parses as
  the conditioned zone (`parse_zone_label`), builds no Foundation zone
  (`build_zone_map`), and its surfaces, loads and ducts account to the
  conditioned zone; the conditioned zone's floor area is the full
  `ConditionedFloorArea`, the basement's included. The OCHRE 20%
  basement heat fraction is not reproduced: OS-HPXML has no such split
  because the space is one zone.
- **Justification:** class (a) against OCHRE, class (b) fix in HARES:
  HARES's `parse_zone_label` read any label containing "basement" as a
  Foundation zone before the conditioned check, so `base.xml`'s
  conditioned basement sat unheated at 7.6 °C on 15 January. The
  reference merges the space; OCHRE's floating zone and 20% heater split
  are its own workaround for the same zone it declines to condition.
- **Measured impact:** base.xml, 15 January, ideal HVAC at the 68 °F
  heating setpoint: the basement's zone went from 7.3 °C (a separate
  floating Foundation zone, the ledger's 7.6 °C) to tracking the
  setpoint within the deadband; the HVAC's delivered heating for the day
  changed by the basement's ground-coupled load (pre-fix the
  `<Conditioned>` flag moved nothing: the two models were bitwise
  identical). On the Denver July day the merged ground-coupled basement
  holds the zone under the defaulted 78 °F cooling setpoint where the
  separate-zone model called for cooling. The committed goldens and the
  twelve parity fixtures carry no conditioned basement and are unchanged.
- **Pinning tests:** `tests/conditioned_basement.rs`
  (`conditioned_basement_is_held_at_the_setpoint`,
  `conditioned_basement_loads_reach_the_hvac`),
  `crates/hares-io/tests/hpxml_parsing_tests.rs`
  (`conditioned_basement_merges_into_the_conditioned_zone`). Each fails
  when the label parse reverts to a separate unheated foundation zone.

---

## D-016: Diffuse short-wave leaves back out through the windows

- **Quantity:** the share of a zone's diffuse short-wave pool
  (transmitted diffuse solar plus the lights' visible part) that passes
  back out through the glazing instead of being absorbed indoors.
- **Reference behavior:** EnergyPlus v24.2.0 pools an enclosure's diffuse
  short-wave and distributes it over the opaque surfaces AND the windows:
  each opaque surface joins the denominator with `Area × AbsIntSurf`
  (`HeatBalanceSurfaceManager.cc:4206`), each window with
  `Area × (TransDiff + AbsDiffBack)` (`:4272`), `solVMULT = 1/SUM1`
  (`:4315`); the opaque surfaces absorb their share (`:3773-3774`), the
  windows' glass layers absorb their `AbsDiffBack` share (`:3837-3892`)
  and the transmittance-weighted share is deposited nowhere: it leaves
  back out through the glazing. The lights' visible gain joins the same
  pool (`EnclSolQSWRadLights`, `:3693`) and scales by the same
  `solVMULT` (`:3749-3753`).
- **OCHRE behavior:** the fork spreads ALL transmitted solar over the
  zone's surfaces by `Area × absorptivity` including the windows' own
  absorptivity (`ochre/Models/Envelope.py:548-558`); its comment claims
  "some heat is reflected back out of the windows and lost" but the
  window's own share is deposited (its `absorptivity` excludes the
  transmittance, and the whole pool is delivered: the view factors sum
  to 1 and `zone.radiation_heat` reaches the zone air, Envelope.py:1198).
  Nothing leaves.
- **HARES behavior (since this fix):** the diffuse distribution's
  denominator gains each zone's windows at
  `Area × (τ_diffuse + SHGC − τ)` (`solar.rs`
  `compute_solar_distribution_into_generic`, the same structure as the
  EnergyPlus `SUM1`); the transmittance-weighted share of the pool leaves
  (lost), the absorptance-weighted share splits by the window's
  inward-flowing fraction N_i (inward to zone air, outward conducted
  away), the same decomposition the window's beam-absorbed solar already
  applies (`apply_solar_inputs`). One path for transmitted solar and
  lighting: both flow through `distribute_transmitted_solar`.
- **Justification:** class (a) against OCHRE, class (b) fix in HARES:
  HARES distributed the diffuse pool over the opaque surfaces only (the
  windows carried `solar_absorptance = 0.0`), so every transmitted
  diffuse watt was absorbed indoors; the reference loses the window's
  transmittance share through the glazing. The N_i split follows
  HARES's own SHGC decomposition for consistency between the beam and
  diffuse paths; EnergyPlus resolves the glass-layer heat balance
  explicitly (a later refinement if the layer model arrives).
- **Measured impact:** on the seven committed goldens (every one has
  windows) the annual gross consumption moved ≤ 0.005 %, in the
  expected directions: cooling fell where cooling runs
  (consumer_shape hvac_cooling 1115.28 → 1114.72 kWh), heating rose
  slightly (95.12 → 95.18 kWh), and the free-float BESTEST peaks fell
  (  900FF peak 46.61 → 45.96 °C, min 3.44 → 3.18 °C); case 600's annual
  cooling fell 6134.0 → 5905.8 kWh. The committed goldens are the
  re-captured values; the pinning test closes the balance exactly
  (pool == opaque + out + inward).
- **Pinning tests:** `crates/hares-envelope/src/thermal_solver/solar.rs`
   (`diffuse_pool_leaves_back_out_through_the_windows`: the closed-form
   EnergyPlus shares and the energy closure;
   `solar_distribution_conserves_energy_randomized`: the conservation
   identity with windows in the denominator). Each fails when the
   distribution reverts to the opaque-only denominator.

---

## D-017: The fixtures' hot-water draw reaches the tank

- **Quantity:** the fixtures' share of a dwelling's domestic hot-water
  draw (sinks, showers, baths: the `hot_water_fixtures` schedule column),
  as a load on the storage tank.
- **Reference behavior:** the fixtures' draw is the dwelling's main
  hot-water load. OS-HPXML v1.12.0 sizes the water heater's load from the
  fixture demand (`WaterHeatingSystem`'s `FractionDHWLoadServed` applies
  to the fixture plus appliance draw, the ANSI/RESNET 301 reference of
  14.6 + 10 gal/day per adjusted bedroom plus the distribution waste);
  EnergyPlus `WaterHeater:Mixed` receives it as the plant hot-water
  demand. The draw schedule integrates to the normalized daily volume
  (here 256.1 L/day, the RESNET reference for 3.6 adjusted bedrooms and
  this fixture's standard distribution).
- **OCHRE behavior:** the fixture draw never reaches the tank. The
  schedule builder names the fixtures column "Water Fixtures (L/min)"
  (`ochre/utils/schedule.py:47` mapping, `convert_water_column` at
  `:350-368`) but the tank model reads "Water Heating (L/min)"
  (`ochre/Models/Water.py:179`, `:287`); no rename bridges the two, so
  `update_water_draw` sees the `.get` default 0 every step and only the
  clothes-washer and dishwasher columns flow (`Water.py:288-290`). The
  release tag v0.9.2 carries the mismatch (upstream commit b781132
  renamed the schedule side to "Water Fixtures" and the tank side with
  it, but the vendored tree's `Water.py:287` still reads the old name,
  as does the upstream v0.9.2 tag's own `Water.py`); the tank silently
  heats the appliances' draw and standby only. Measured on the parity
  window: the tank receives 15.6 L over 24 h (the dishwasher alone; the
  washer's draw is 0 that day) and the water heater draws 1.01 kWh.
- **HARES behavior:** the fixtures' normalized draw reaches the tank:
  `normalize_draw_profile` scales the schedule fractions to the RESNET
  daily volume (`crates/hares-io/src/draw_profile.rs`), and the storage
  water heaters pull it through the tempered-draw path
  (`crates/hares-equipment/src/water_heater/tank.rs` `step_tempered`,
  the TMV volume ratio of OCHRE Water.py:303-325). The tank draws
  259 L over the measured day and the element delivers the closed form:
  draw enthalpy 9.14 kWh + standby 1.16 kWh + storage 0.00 kWh =
  10.27 kWh, measured to 0.3 %.
- **Justification:** class (a): OCHRE is wrong (a dropped input, not a
  physics choice). EnergyPlus-grade physics is the target; the
  dwelling's water-heating energy follows the HPXML's declared fixture
  demand.
- **Measured impact:** the parity week's water heating is 78.56 kWh
  (HARES) against OCHRE's 11.37 kWh (6.9x), and the 24 h window's
  12.28 kWh against 1.01 kWh (12.2x). The reference scale: the
  fixtures' 256 L/day at the 40.6 C delivery temperature is
  8.1 kWh/day of delivered enthalpy, plus the tank's standby (0.9 to
  1.2 kWh/day at this UA) and the appliances' own hot draws (up to
  0.8 kWh/day): the flat-draw closed form measures 10.27 kWh/day at
  the equipment level (the balance identity to 0.3 %), the parity
  week's peaky draw 11.2 kWh/day averaged. OCHRE's 1.6 kWh/day average
  is the appliance draws and standby only; its fixtures draw nothing.
  The 24 h total-electric parity gap is this difference plus the D-012
  envelope class.
- **Pinning tests:** `crates/hares-equipment/src/water_heater/
  resistance.rs`
  (`element_energy_matches_the_draw_enthalpy_closed_form`: the 24 h
  element energy equals draw enthalpy + standby + storage and dominates
  the standby-only floor a lost draw would leave, so re-aligning with
  OCHRE's dropped column fails the test); `tests/python/
  test_ochre_parity.py` (the 24 h Water Heating case: a strict xfail
  whose reason quotes the measured pair and this register entry).

---

## D-018: Slab ground temperature from the EPW's shallow ground table

- **Quantity:** the undisturbed ground temperature driving a
  slab-on-grade floor's conduction.
- **Reference behavior:** OS-HPXML v1.12.0 sets EnergyPlus's
  `Site:GroundTemperature:Shallow` from the weather file's GROUND
  TEMPERATURES table (`HPXMLtoOpenStudio/resources/location.rb`
  `apply_ground_temps`: `weather.data.ShallowGroundMonthlyTemps`);
  the EPW carries the measured monthly shallow ground temperatures for
  the station (Denver Intl AP, 0.5 m: 0.36 C January to 16.66 C May).
- **OCHRE behavior:** the ground temperature is a DOE-2 correlation of
  the air temperature (`ochre/utils/schedule.py` `import_weather`, the
  `t_ground` block citing DOE-2 `src\WTH.f`): an annual sinusoid around
  the annual mean air temperature with a phase lag, never reading the
  EPW's ground table. Measured over the winter conditioned oracle:
  OCHRE's ground drives at 6.53 C mean where the EPW table's shallow
  temperature sits at 2.78 C.
- **HARES behavior:** the ground boundary reads the EPW's ground
  temperature table (the weather file's monthly shallow temperatures,
  carried through the weather state; the deep ground term uses the
  Kusuda-Achenbach correlation whose parameters derive from the same
  table).
- **Justification:** class (a): OCHRE substitutes an air-based
  correlation for the measured station data the reference toolchain
  uses. The EPW table is the OS-HPXML v1.12.0 source; the correlation
  is OCHRE's simplification.
- **Measured impact:** the slab's floor heat gain over the winter
  conditioned oracle: HARES -216.6 W mean against OCHRE -724.8 W
  (OCHRE's warmer correlated ground heats the slab; HARES's measured
  2.78 C January ground does not). On the free-float winter oracle
  this is the largest single component gap (Floor -26.7 W HARES
  against +183.3 W OCHRE) and it pushes the free-float indoor mean
  apart (HARES 11.43 C, OCHRE 13.38 C, MAE 1.94 C).
- **Pinning tests:** the free-float and conditioned oracles' floor
  heat-gain diagnostics (`tests/freefloat_oracle.rs`,
  `tests/conditioned_oracle.rs`, observe builds) carry the per-scenario
  means; the weather state's ground temperature binding is pinned by
  the EPW weather parity tests (`crates/hares-io/tests/
  weather_parity.rs`).

---

## D-019: A BEopt/OCHRE-format input's declared microwave is modelled once

- **Quantity:** the microwave oven's energy and gains on an input whose
  schedule is a BEopt/OCHRE event file (the `Occupancy (Persons)` family).
- **Placement note:** this row is placed and numbered by the register
  audit. The count-once fix and the pinning tests below land with their
  own in-flight block; at the audit's base a `microwave` schedule column
  still auto-creates the 100 kWh/yr Microwave load on an OS-HPXML-derived
  building. The in-flight block's own copy of this entry sits under
  D-015, a number the folded conditioned-basement merge already holds:
  that copy is dropped at the fold and this D-019 is the entry of
  record.
- **Reference behavior:** OS-HPXML v1.12.0 models no microwave appliance:
  its bundled HPXML v3 schema has no `Microwave` element, its appliance
  set (`hotwater_appliances.rb`) has no microwave, its schedule files
  carry no microwave column (`schedules.rb`), and the microwave's energy
  sits inside the residual "other" plug loads that `defaults.rb`
  `get_residual_mels_values` sizes from RECS 2020 (the residual is
  applied to `PlugLoadTypeOther`, `defaults.rb:4766`, values from
  `defaults.rb:7505-7530`). The vendored OCHRE agrees on the accounting
  and goes further: it has no Microwave equipment at all (the appliance
  set `ochre/utils/hpxml.py:1663-1683` reads ClothesWasher, ClothesDryer,
  Dishwasher, Refrigerator, Freezer and CookingRange only; `MEL_NAMES`
  maps only `TV other` and `other`, `ochre/utils/hpxml.py:38-40`), and a
  schedule CSV column it does not know raises
  (`ochre/utils/schedule.py:448` indexes `ALL_SCHEDULE_NAMES`, which has
  no microwave row, `ochre/utils/schedule.py:15-105`).
- **HARES behavior (since the count-once fix):** the microwave is modelled
  exactly once, and only when the input's own accounting separates it. On
  an OS-HPXML-derived building (the `occupants` schedule family) the
  microwave energy stays in the residual plug loads and a `microwave`
  schedule column shapes nothing additional: it is reported unread, like
  any other column no mapping applies to. On a BEopt/OCHRE-format input
  (the `Occupancy (Persons)` event family) a `microwave` column or an
  explicit `<Microwave>` element models the microwave once, with
  OS-HPXML's plug-load radiant split (`rad_frac = 0.6 * sens_frac`,
  `misc_loads.rb:89`) over the electric range's sensible split HARES
  already carries (0.72 sensible, 0.08 latent).
- **Justification:** class (b) at the root and class (a) on top. The
  double count was HARES's own bug: a `microwave` column auto-created a
  100 kWh/yr Microwave load on top of a residual already containing the
  microwave energy, which neither reference does; fixed at the root. The
  BEopt branch is the deliberate divergence: BEopt's own accounting
  models the microwave as an appliance, so an input of that family that
  declares one is honoured once instead of dropped the way OCHRE drops
  it; its gains take the reference's plug-load radiant split rather than
  OCHRE's radiative-gain-fraction-zero MEL form
  (`ochre/utils/hpxml.py:1553-1559`).
- **Measured impact:** no checked-in HPXML fixture or schedule file
  carries a microwave column or a `<Microwave>` element, so every golden
  compares bitwise, both profiles. The modelled change is on new inputs:
  an OS-HPXML building with a microwave column loses the 100 kWh/yr
  double count (its plug-load total is the residual alone), and a
  BEopt/OCHRE-format input's declared microwave moves 43.2% of its input
  power radiant (0.6 of the 0.72 sensible) instead of none of it.
- **Pinning tests:**
  `crates/hares-io/src/schedule_resolve.rs`
  (`an_os_hpxml_schedules_microwave_column_counts_only_the_residual_mels`,
  `the_microwave_column_is_unknown_only_outside_the_beopt_family`,
  `microwave_csv_column_creates_spec_with_event_schedule_and_nonzero_energy`
  for its radiant-split assertion),
  `crates/hares-core/tests/schedule_columns.rs`
  (`a_microwave_column_on_an_os_hpxml_building_counts_only_the_residual_mels`).

---

## D-020: The MSHP's default speed stages are evenly spaced at a flat EIR

- **Quantity:** the speed stages (per-stage capacity and EIR) a
  mini-split heat pump heater gets when its input declares no stage list.
- **Reference behavior:** OCHRE sizes an MSHP's stages from its
  multispeed equipment database (`ochre/defaults/HVAC Multispeed
  Parameters.csv`, the `MSHP Heater` rows): capacity ratios 0.40, 0.60,
  0.80 and 1.20 of rated with a per-stage COP declining with speed
  (5.14, 3.96, 3.77 and 3.42 at 9.5 HSPF). The rows are per-product
  calibrations (their "Received From" column names the calibrated
  building or BEopt), and the top stage exceeds rated.
- **HARES behavior:** with no explicit `stage_heating_capacities_w`, an
  MSHP generates four evenly spaced stages from
  `min_compressor_fraction` (default 0.25) to 1.0 of rated:
  0.25, 0.50, 0.75 and 1.00. The stage EIRs are flat at the rated EIR
  (`eir_part_load_benefit` defaults to 0) unless the input provides
  `stage_heating_eirs`, which are never overwritten.
- **Justification:** the class is not established. EnergyPlus's
  `Coil:Heating:DX:MultiSpeed` takes explicit stage capacities and has
  no default spacing rule, so neither reference is an authority for the
  no-input case: OCHRE's rows are product calibrations (one tops above
  rated), HARES's even spacing is its own disclosed default. The
  biquadratic temperature curves are a different mechanism and keep the
  OCHRE CSV columns (D-014). The gap is a candidate cause beside the
  mini-split fixture's open peak-power question in `tests/parity/mod.rs`
  (the reference's peak draw sits near its 0.40-rated first stage);
  the cause there is not yet established either.
- **Measured impact:** the mini-split parity fixture
  (`cz5a_minisplit_gas_wh`, a 36 kW MSHP with no stage list) runs with
  HARES's 9/18/27/36 kW stages against OCHRE's 14.4/21.6/28.8/43.2 kW
  ones, inside its parity bands; no golden fixture carries a mini-split
  at all.
- **Pinning tests:** `crates/hares-equipment/src/hvac/heat_pump/heater.rs`
  (`mshp_speed_stages_hardcoded_at_25_50_75_100pct`,
  `mshp_eir_identical_across_all_stages`,
  `mshp_stage_heating_eirs_preserved_when_is_mini_split`). The first two
  fail when the default stages revert to the OCHRE CSV's ratios or its
  per-stage COPs.

## D-021: The HPWH's low-power compressor class is a continuous UEF transition

- **Quantity:** the trigger and curve treatment of a heat pump water
  heater's low-power compressor class (the 120 V product family's
  distinct COP and capacity curves and widened ambient lockout).
- **Reference behavior:** OCHRE's HPXML import triggers the low-power
  coefficients by an exact-equality sentinel, `UEF == 4.9`
  (`ochre/utils/hpxml.py:1174-1181`), documented there as "a temporary
  flag for designating 120V HPWHs in [the] panels branch of ResStock",
  and applies its preset values (cop, setpoint, hp-only mode, tank
  temperature) as a hard switch for that one product class.
- **HARES behavior:** the parser never substitutes the product presets:
  the COP always derives from the UEF
  (`low_power_hpwh_auto_detected_from_uef_without_hardcoded_presets`),
  and the low-power transition is continuous, a linear cross-fade of the
  COP and capacity curve outputs over UEF in [4.8, 5.0]
  (`low_power_blend_factor`, `heat_pump_wh.rs`), with the widened
  ambient lockout bounds applied with the blend.
- **Justification:** class (a). OCHRE's sentinel is its own temporary
  flag for one SKU family, an exact-equality step discontinuity on a
  continuous parameter; HARES keeps the coefficients (verified against
  `WaterHeater.py:469-470`) and generalizes the trigger to the UEF band
  the flag intended.
- **Measured impact:** no committed fixture or golden carries a UEF in
  [4.8, 5.0] or a low-power HPWH, so no golden moves; the divergence is
  on new inputs, where a UEF of 4.95 gets 75% of the low-power curves
  instead of OCHRE's standard-coefficient hard no at 4.9 exactly.
- **Pinning tests:** `crates/hares-io/tests/hpxml_parity.rs`
  (`low_power_hpwh_auto_detected_from_uef_without_hardcoded_presets`),
  which fails when the parser reverts to the sentinel's preset
  substitution and COP.

## D-022: The ZIP model wires the real coefficients to P and the reactive to Q

- **Quantity:** which ZIP coefficient array drives real power and which
  drives reactive power at off-nominal bus voltage.
- **Reference behavior:** OCHRE cross-wires them:
  `ochre/Equipment/Equipment.py:200-218` (`run_zip`) computes
  `reactive_kvar = electric_kw * pf_mult * zip_p.dot(v_quadratic)` and
  `electric_kw = electric_kw * zip_q.dot(v_quadratic)`, so the real
  coefficients scale the vars and the reactive coefficients scale the
  watts.
- **HARES behavior:** `ZipLoad::apply` (`hares-types/src/zip.rs`) scales
  real power by the real coefficients `(zp, ip, pp)` and reactive power
  by the reactive coefficients `(zq, iq, pq)`, the physically correct
  wiring (the Z/I/P decomposition of each quantity's own voltage
  response).
- **Justification:** class (a). OCHRE's cross-wiring is a bug (its own
  arrays are documented per-quantity); no reference derives Q's voltage
  response from P's coefficients.
- **Measured impact:** none on the committed frames: at nominal voltage
  the two wirings coincide (both coefficient arrays sum to about one),
  and no committed frame carries a voltage event, so the cross-wiring's
  tens-of-percent error at off-nominal voltage touches no committed
  frame. The class table's rows (OCHRE's ZIP Parameters.csv values)
  carry strongly non-flat reactive coefficients, so the error is real
  on any input that drives the bus off 1.0 pu.
- **Pinning tests:** `crates/hares-types/src/zip.rs`
  (`real_power_follows_the_real_coefficients_and_reactive_the_reactive_off_nominal`,
  `apply_is_bit_identical_to_legacy_arithmetic_for_all_class_rows`).
  The first fails when the wiring reverts to OCHRE's cross-wired form.

## D-023: The window's U-factor decomposes into glass and both films

- **Quantity:** the resistance split of a window's U-factor into the
  glass-only resistance and the interior and exterior film resistances.
- **Reference behavior:** OCHRE sets the window's exterior film
  resistance to zero and lumps it into the glass:
  `create_rc_data` (`ochre/Models/Envelope.py:302`, used at :294-304)
  sets `res_ext_w = 0` and `r_window = 1/U - res_int_w`.
- **HARES behavior:** `window_u_factor_decomposition`
  (`hares-physics/src/solar.rs`) recovers the EnergyPlus Window
  Calculation Module's Step-1 decomposition: the interior film from the
  U-factor's embedded assumption, the exterior film from the standard
  winter coefficient, and the glass as the remainder, so solar
  parameters are computed against the true glass-only resistance.
- **Justification:** class (a). The EnergyPlus Window Calculation
  Module's Step 1 defines all three resistances; absorbing the exterior
  film into the glass overstates the glass resistance the absorbed
  solar acts on.
- **Measured impact:** the assembled resistance is exact to 1e-10 across
  the pinned U-factor range, so the decomposition changes no input that
  carried the same U-factor; the solar parameters the glass-only
  resistance feeds are the outputs the solar parity tests pin.
- **Pinning tests:** `crates/hares-physics/tests/solar_parity.rs`
  (`window_decomposition_low_e_double_pane` and the sibling
  decomposition tests at U 5.5, 6.5 and 1.0, plus the error cases).
  Each fails if the exterior film is absorbed into the glass again.

## D-024: A leap-year EPW keeps its February 29

- **Quantity:** the rows of an 8784-row (leap-year) EPW weather file
  that reach the simulation.
- **Reference behavior:** OCHRE strips February 29 from a leap-year
  EPW at import (`ochre/utils/schedule.py:172-175`, "leap year, remove
  Feb 29 data"), keeping 8760 rows.
- **HARES behavior:** the EPW parser keeps the full 8784 rows
  (`crates/hares-io/src/epw.rs`, the leap-year handling); annual
  simulations run 366 days and the schedule and weather indexing wrap
  modularly across the year boundary.
- **Justification:** class (a). Dropping a real day's measurements is a
  data loss the reference toolchain does not make (EnergyPlus runs
  leap-year weather natively); the modular wrap makes the extra day a
  handled case, not an index error.
- **Measured impact:** the leap-year golden
  (`tests/fixtures/golden/leap_year_february_900s`, a February window
  crossing the 29th) holds the preserved-day behaviour bitwise; no
  other committed input is a leap-year EPW.
- **Pinning tests:** the leap-year golden frame (bitwise, both
  profiles, via `frame_golden compare-all`) and
  `crates/hares-io/tests/weather_parity.rs`'s leap-year series-length
  checks.

## D-025: Zone capacitance density from the site's pressure, not a constant

- **Quantity:** the air density in a zone's air-node capacitance.
- **Reference behavior:** OCHRE uses the constant rho_air = 1.2041
  kg/m³ for every zone's capacitance
  (`ochre/Models/Envelope.py:11`, read at :442-446).
- **HARES behavior:** the capacitance's density comes from the ideal
  gas law at the site's standard atmospheric pressure for its
  elevation, at the RC network's 20 °C linearization temperature
  (`derive_zone_capacitances`, `hares-envelope/src/boundary_rc.rs`;
  sea-level pressure when the elevation is unknown).
- **Justification:** class (a). The capacitance scales the zone's
  thermal mass; a Denver site's air is about 17 percent thinner than
  the constant assumes. OCHRE's own infiltration code carries a TODO
  against the same constant (docs/findings/air_density.md records the
  investigation).
- **Measured impact:** a Denver site's capacitance is about 18 percent
  below the constant's (the standard pressure at 1.6 km elevation
  carries a density near 0.99 kg/m³ against 1.2041), so the golden
  homes' zones integrate correspondingly lighter air nodes; the goldens
  are the re-captured values. The infiltration module keeps the same
  constant OCHRE uses (its ELA-based form does not read the density).
- **Pinning tests:** `boundary_rc::tests::zone_capacitance_follows_the_zone_volume`
  (asserts the ideal-gas density, so reverting to the 1.2041 constant
  fails it) and the boundary capacitance test at the non-sea-level
  site (`boundary_rc.rs` tests, the ideal-gas assertion repeated).

## D-026: Windows take part in the exterior long-wave exchange

- **Quantity:** the exterior long-wave (sky, ground, air) exchange of
  window surfaces.
- **Reference behavior:** OCHRE computes no window exterior LWR at all:
  windows carry no thermal node (`t_idx=None`) and are skipped in its
  exterior radiation solve (`_solve_exterior_radiation`), their LWR
  implicit in the U-factor.
- **HARES behavior:** window exterior LWR is injected as
  `(U / h_out) · q_lwr_per_m2 · area` with the effective sky-air
  temperature form (`thermal_solver/longwave.rs`), reported as
  `window_exterior_lwr_w`, below the 1.0 W/(m²·K) natural-convection
  floor falling back to the ASHRAE conventional exterior coefficient
  (34 W/(m²·K)) with a warning.
- **Justification:** class (a). EnergyPlus computes the full exterior
  long-wave balance for windows by iterating on the window's exterior
  surface temperature inside the surface heat balance loop; the
  T_eff form is the same exchange on the RC network's terms
  (docs/findings/bestest_rca.md, Root Cause #3, records the
  investigation and its residual under-correction).
- **Measured impact:** clear-night attic and window cooling the
  free-float BESTEST cases see that OCHRE's windows cannot; the
  case-600/900 bands hold either way and the committed goldens are the
  re-captured values.
- **Pinning tests:** `thermal_solver/longwave.rs` tests
  (`window_exterior_lwr_clear_night_is_cooling`,
  `window_exterior_lwr_zero_when_sky_equals_air`,
  `window_exterior_lwr_teff_scaling_reduces_raw_delta`,
  `window_exterior_lwr_uses_actual_h_out_when_available`,
  `window_exterior_lwr_horizontal_sees_more_sky_than_vertical`). Each
  fails when windows are dropped from the exterior LWR solve again.

## D-027: A scheduled space's moisture blends by humidity ratio

- **Quantity:** the moisture (and wet-bulb) of the air an HPXML
  equipment sees in a location with no modelled thermal zone ("other
  heated space" and friends).
- **Reference behavior:** OS-HPXML v1.12.0 runs such equipment against
  a dry-bulb blend of the indoor and outdoor series
  (`geometry.rb` `get_temperature_scheduled_space_values`) and averages
  the two sources' RELATIVE HUMIDITIES for the ambient RH
  (`waterheater.rb` `apply_hpwh_loc_temp_rh_sensors`, :1124).
- **HARES behavior:** the dry-bulb follows the same blend and floors;
  the moisture blends the two sources' HUMIDITY RATIOS at the same
  weights (ASHRAE Handbook Fundamentals 2021 ch. 1, adiabatic mixing
  of two moist-air streams), holds the ratio through the location
  floor, and the wet-bulb is the thermodynamic wet-bulb of the blended
  state (`hares-core/src/ambient_air.rs`).
- **Justification:** class (a) against OS-HPXML. Averaging relative
  humidities does not conserve water vapour and applies at a dry-bulb
  neither source had; the ratio blend is the physically consistent
  mixing rule.
- **Measured impact:** no committed fixture places an HPWH or other
  equipment in a scheduled space, so no golden moves; on new inputs the
  blended placement's RH differs from OS-HPXML's average by the mix's
  nonlinearity (largest when the sources' temperatures differ most).
- **Pinning tests:** `hares-core/src/ambient_air.rs` tests
  (`dry_bulb_follows_the_scheduled_space_table`,
  `wet_bulb_comes_from_the_blended_humidity_ratio`,
  `single_source_placements_carry_their_source_moisture`). The second
  fails when the moisture reverts to an RH average.

## D-028: The ASHP compressor lockout default is 0 °F, EnergyPlus's coil default is -8 °C

- **Quantity:** the default minimum outdoor dry-bulb temperature for
  compressor operation of a single-speed air-source heat pump whose
  input declares no lockout.
- **Reference behavior:** EnergyPlus's `Coil:Heating:DX:SingleSpeed`
  defaults "Minimum Outdoor Dry-Bulb Temperature for Compressor
  Operation" to -8 °C. OCHRE defaults to 0 °F (-17.78 °C) with an
  explicit "0F default" annotation (`ochre/Equipment/HVAC.py:1208`,
  its HPXML parser at `hpxml.py:947-951`), and OS-HPXML defaults
  `CompressorLockoutTemperature` to 0 °F for single-speed air-to-air
  heat pumps.
- **HARES behavior:** the default lockout is -17.78 °C
  (`DEFAULT_HP_LOCKOUT_TEMP_C`, `heat_pump/constants.rs`), following
  OCHRE and OS-HPXML against EnergyPlus's coil default, with a 0.5 °C
  hysteresis band.
- **Justification:** the residential references' default reflects the
  practical minimum operating temperature for typical residential
  non-cold-climate ASHPs as manufactured and installed; the E+ object
  default serves a wider equipment class. HARES keeps the residential
  default. Deliberate on both sides; recorded because D-014's curve
  work anchors this unit's -17 °C analysis on the same lockout.
- **Measured impact:** none on the committed frames (no committed
  fixture relies on the default's edge; the lockout's behavioural
  effect is pinned at -20 °C outdoor in the defaults test).
- **Pinning tests:** `crates/hares-equipment/tests/hvac_tests.rs`
  (`ashp_defaults_match_reference`: the compressor delivers nothing at
  -20 °C outdoor with no declared lockout).

## D-029: A generating PV's power-factor setpoint uses an unsigned pf

- **Quantity:** the sign convention of a PV inverter's reactive power
  under a `PowerFactorSetpoint` control signal.
- **Reference behavior:** OCHRE encodes the generating-vars case in the
  pf's own sign: a negative signed power factor commands gen-P/consume-Q
  (`ochre/Equipment/PV.py:194-196`).
- **HARES behavior:** the power factor keeps its (0, 1] magnitude
  semantics and a generating PV at pf < 1 supplies vars, so bus
  Q = -|P|·tan(acos(pf)); var absorption is commanded by the separate
  `ReactiveSetpoint` signal, positive = absorbing
  (`hares-equipment/src/pv/mod.rs`, the setpoint handling).
- **Justification:** one convention across the control surface: the pf
  setpoint names a magnitude, the reactive setpoint names a signed var
  target; OCHRE's negative-pf encoding overloads a magnitude field with
  a sign. The same operating intent produces the same bus vars on both
  sides.
- **Class:** none (a control-interface convention, not a physics
  disagreement); recorded because re-aligning the control surface to
  OCHRE's encoding would flip the sign of every commanded-var case.
- **Measured impact:** none on the committed frames (no committed
  fixture drives a PV pf setpoint); the convention is pinned at the
  equipment level.
- **Pinning tests:** `crates/hares-equipment/src/pv/mod.rs`
  (`power_factor_setpoint_signal_computes_q`,
  `pv_baseline_pf_supplies_vars_and_passes_port_core_validator`,
  `pv_power_factor_setpoint_zeros_q_setpoint`),
  `crates/hares-equipment/tests/lifecycle.rs` (the `ReactiveSetpoint`
  absorption case).

## D-030: Defrost recovery leaves a small latent gain on the zone

- **Quantity:** the latent gain a reverse-cycle-defrost heat pump leaves
  on the zone while it recovers from the defrost.
- **Reference behavior:** EnergyPlus's DX heating coils produce no latent
  output in any mode (its upstream issue #7440 records the gap); OCHRE
  treats every heating step as all-sensible, SHR 1.0
  (`ochre/Equipment/HVAC.py:458-462`).
- **HARES behavior:** during an active reverse-cycle defrost of a unit
  whose declared heating SHR is below 1.0, the step carries a small
  positive latent gain, `capacity × (1 − SHR) × defrost_time_fraction`,
  from the indoor coil's surface moisture evaporating into the supply
  air; every non-defrost step and every SHR = 1.0 unit stays exactly
  all-sensible (`heat_pump/heater.rs`).
- **Justification:** HARES goes beyond both references: the E+ gap is
  recorded upstream, and the mechanism (condensate re-evaporating on the
  coil the defrost just warmed) is real where the declared SHR says the
  unit's heating is not all-sensible.
- **Class:** beyond the references (a deliberate extension, not a
  disagreement with a reference's physics).
- **Measured impact:** none on the committed frames (no committed
  fixture declares a sub-1.0 heating SHR, so the gain is zero everywhere
  committed); the gain appears only on inputs that declare the split.
- **Pinning tests:** `crates/hares-equipment/tests/hvac_tests.rs`
  (`heating_latent_nonzero_during_defrost_with_sub1_shr`,
  `heating_latent_zero_with_default_shr_during_defrost`, and the
  normal-heating zero above them).
