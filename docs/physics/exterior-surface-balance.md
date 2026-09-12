# Exterior Surface Balance — the eliminated skin node

The authoritative derivation for the exterior solar + longwave coupling.
Code comments point here; if you change the physics, change this document
in the same commit.

## The physical problem

Each opaque exterior surface (wall, roof, floor) exchanges with the
environment through a **skin** — the outer face of the construction:

```
                absorbed solar S = α·A·POA            net LWR q_lwr(T_s)
                        │                                 │
                        ▼                                 ▼
 outdoor air ──[R_film]──● SKIN (T_s) ──[R_outer_half]──● outermost RC node
   T_air                                                T_node
```

- `R_film` — convection-only exterior film resistance [m²K/W]
  (TARP/DOE-2; the A-matrix deliberately carries **no** h_rad in the film —
  LWR is handled explicitly, so there is no double counting).
- `R_outer_half` — half of the outermost material sub-layer [m²K/W].

The skin has no capacitance. Its exact steady balance is:

```
(T_s − T_air)/R_f  +  (T_s − T_node)/R_h  =  S + q_lwr(T_s)        (1)
```

with `q_lwr = ε·σ·A·[(1−β·F_sky)·T_air⁴ + β·F_sky·T_sky⁴ − T_s⁴]`
(EnergyPlus 4-component exterior longwave model).

## The elimination

The RC network does not materialize the skin node. Define the no-flux
divider temperature

```
t_init = rad_frac·T_node + (1 − rad_frac)·T_air,
rad_frac = R_f / (R_f + R_h)
```

(the exact solution of (1) when S = q_lwr = 0). Expanding (1) around
`t_init`, every flux Q injected at the skin satisfies

```
T_s = t_init + Q·R_par,   R_par = (R_f·R_h)/(R_f + R_h)             (2)
```

— the **parallel** (Thévenin) combination, per area:
`R_par/A = (r_film·r_half)/(r_film + r_half)/area`.

And the flux share that reaches the RC node is the current-divider
fraction of Q:

```
Q_into_node = Q · rad_frac                                          (3)
```

(the complement `(1 − rad_frac)·Q` leaves via the film to outdoor air).
With (2) and (3) in the parallel form, the eliminated scheme is **exact at
every state**, not just steady state: the A-matrix series conductance
`1/(R_f + R_h)` plus the divider injection reproduce the skinned network
identically.

## The four quantities (do not conflate them)

| Quantity | Face | Meaning |
|---|---|---|
| `opaque_solar_w` | outside | gross absorbed solar `Σα·A·POA` at opaque skins (OCHRE "Ext. Solar Gain") |
| `exterior_lwr_w` | outside | net LWR exchange at opaque skins (OCHRE "Ext. LWR Gain"; typically negative) |
| `opaque_solar_lwr_w` | outside | their sum — a boundary-condition driver, **never** a zone load |
| `wall/floor/roof_heat_gain_w` | inside | net inside-face convection to zone air (E+ "Inside Heat Balance": Interior Convection) |

The zone-air heat balance closes over inside-face and direct-injection
terms only. Summing outside-face quantities into a zone budget was the
I-02 defect (+141 MWh/yr phantom gain with physical indoor temperatures).

## Implementation map

- `hares_envelope::boundary_rc::skin_rad_coupling(r_film, r_beyond, area)`
  → `Option<SkinCoupling { rad_frac, rad_res_k_w }>` — the **single**
  constructor for (2)/(3). `None` when there is no material half-layer.
- Exterior iterative path (rad_frac > 0):
  `thermal_solver/longwave.rs::apply_exterior_longwave_inputs_iterative` —
  solves (2) by damped fixed-point iteration with a cross-step warm start
  (`exterior_surface_temps`), injects (3) into the outer RC node.
- Exterior linearized path (rad_frac == 0, no half-layer): same file —
  exact T⁴ factorization `q_lw ≡ h_rad·(T_eff − T_surf)` routed through
  the semi-implicit coupling (`build_coupling`), unconditionally stable.
- Interior ScriptF path: same file, `apply_interior_longwave_inputs` —
  same skin algebra against the zone air node.
- Production wiring: `hares-core::dwelling::solver_builder` consumes
  `skin_rad_coupling` for exterior, interior-opaque, and window-interior
  skins.

## Deliberate divergences from OCHRE

See `docs/alignment/DIVERGENCES.md` (D-001). OCHRE uses the bare film
`R_f/A` for R_par with the parallel form commented out in its own source
(Envelope.py, "assumes res_material >> res_film"). The bare form
over-drives the skin by `Q·R_f²/(R_f + R_h)` whenever the outer layer
conducts comparably to the film (wood siding, stucco, metal — the ASHRAE
140 case 600 wall is exactly this regime). Measured delta of the fix:
600 cooling 5946→6071 kWh (toward band), 600FF peak +0.41 K.

## Pinning tests

- `thermal_solver/longwave.rs::tests::iterative_skin_temperature_satisfies_exact_skin_balance`
  — iteration-level closure vs. a Newton-solved exact (1).
- `tests/exterior_skin_balance.rs` — end-to-end steady state through the
  assembled RC network, conducting and insulated regimes, ratio sweep.
- `skin_rad_coupling_uses_parallel_resistance` — formula + identity
  `rad_res == rad_frac·R_half/A` + routing edge cases.
- Skin-closure invariant under `debug_assertions`/`check_invariants` in
  the iterative solve: converged skins must sit on their fixed
  point within 0.05 K.
