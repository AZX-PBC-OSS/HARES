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
  `resstock_event_load_replay_delivers_its_gains`,
  `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares`,
  `every_gain_override_changes_the_split`),
  `gain_fractions::tests::radiant_part_of_sensible_leaves_the_rest_convective`,
  `resolve_loads::tests::resolver_carries_the_radiant_share`. With
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
  (`hpxml_lighting_splits_convective_radiant_and_visible`,
  `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares`,
  `every_gain_override_changes_the_split`),
  `gain_fractions::tests::visible_part_takes_the_short_wave_path`,
  `resolve_loads::tests::lighting_carries_a_visible_part`,
  `resolve_loads::tests::resolver_carries_the_radiant_share`. Each
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
  (every short-wave watt is deposited, by area times absorptance) and its
  `_in_star_mesh_mode` twin for the default interior mode,
  `crates/hares-envelope/tests/thermal_pathway_physics.rs`
  (`zone_sensible_breakdown_debug_must_include_radiant_air_residual`,
  `star_mesh_breakdown_splits_shortwave_between_surface_and_air`). Each
  fails when the visible part is sent to the zone air in its mode.
  `production_step_injects_shortwave_gain` fails when the production step
  leaves the short-wave gain out.

## D-013: A load's month multipliers scale its schedule

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
- **Measured impact:** none on the committed inputs: no resolver writes the
  keys, so they come only from an equipment's own parameters or an
  override, and no fixture or golden sets them.
- **Pinning tests:** `scheduled_load::tests::month_multipliers_scale_a_daily_profile`,
  `scheduled_load::tests::month_multiplier_zeroes_schedule_in_target_month`,
  `event_load::tests::month_multiplier_scales_event_output`,
  `event_load::tests::month_multiplier_zero_suppresses_event_output`,
  `schedule_helpers::tests::negative_and_non_finite_month_multipliers_are_parameter_errors`.

## D-008: Wind terrain from the site, not an assumed rural one

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
  gable wall area picks which pair of sides carries the gables, and must
  lie from 0.98 to (1 + 2 × 5 ft / span)² times that span's triangle: no
  smaller than the triangle beyond rounding, and no larger than the
  deepest eaves OS-HPXML's home builder offers (5 ft,
  `BuildResidentialHPXML/resources/options/geometry_eaves.tsv`) would make
  it. Across the committed ResStock and OS-HPXML sample homes, every gable
  whose span the walls fix has ends of 0.9993 to 1.0008 times its
  triangle; BEopt's, whose ends include 2 ft eaves, are 1.28. The span must
  also be well determined: a 0.25 % error in the walls' area (a heuristic,
  three times that largest disagreement) may move it by at most 10 %, which
  rejects a footprint near a square (`gable_rise_m`,
  `hares-io/src/hpxml/zone_geometry.rs`). Where the input does not
  determine the span (an attic over a garage, floors that do not match the
  footprint, a smaller upper storey, no floor counts, gable ends that
  disagree with the span, a footprint near a square), the attic keeps
  OS-HPXML's hip, with a warning naming why. The ridge rise is also the attic's height for its leakage:
  the stack coefficient grows with it and the wind coefficient takes the
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
  HPXML; the BESTEST ratchets hold within 1 % with the roof at Medium
  Rough.
- **Justification:** EnergyPlus's DOE-2 and TARP outside convection scale
  forced convection by the roughness of the outside material layer.
  OS-HPXML builds that layer without passing the roughness its material
  has, so a shingle roof and a vinyl wall convect alike. The dataset
  record for the same material is the evidence for each class; every row
  departs from OS-HPXML's one `Rough`.
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
- **Justification:** the battery describes the storage the thermal
  solver integrates; with the multiplier on the zone, the same
  capacitance belongs in the battery.
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
