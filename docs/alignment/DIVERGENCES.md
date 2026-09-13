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
