#   Thermal Loss & Cooling Phenomena: OCHRE vs HARES

  Envelope Conduction

  ┌─────────────────────────────────┬────────────────────────────────────┬────────────────────────────┐
  │           Phenomenon            │               OCHRE                │           HARES            │
  ├─────────────────────────────────┼────────────────────────────────────┼────────────────────────────┤
  │ RC network (walls/roof/floor)   │ Yes - state-space model            │ Yes - state-space model    │
  ├─────────────────────────────────┼────────────────────────────────────┼────────────────────────────┤
  │ Multi-layer material properties │ Yes                                │ Yes                        │
  ├─────────────────────────────────┼────────────────────────────────────┼────────────────────────────┤
  │ Exterior film resistance        │ Yes (wind/tilt-dependent via TARP) │ Yes (constant 0.03 m²·K/W) │
  ├─────────────────────────────────┼────────────────────────────────────┼────────────────────────────┤
  │ Interior film resistance        │ Yes (tilt-dependent)               │ Yes (constant 0.12 m²·K/W) │
  └─────────────────────────────────┴────────────────────────────────────┴────────────────────────────┘

  Gap: OCHRE uses dynamic film coefficients (TARP algorithm, wind-dependent exterior). HARES uses fixed constants.

  Radiation

  ┌─────────────────────────────────────┬────────────────────────────────┬───────────────────────────────────────┐
  │             Phenomenon              │             OCHRE              │                 HARES                 │
  ├─────────────────────────────────────┼────────────────────────────────┼───────────────────────────────────────┤
  │ Exterior LWR to sky                 │ Yes - ε×σ×A×(T⁴_sky - T⁴_surf) │ Yes - same model with sky view factor │
  ├─────────────────────────────────────┼────────────────────────────────┼───────────────────────────────────────┤
  │ Exterior LWR to ground surroundings │ Yes                            │ Yes - (1-F_sky) ground fraction       │
  ├─────────────────────────────────────┼────────────────────────────────┼───────────────────────────────────────┤
  │ Interior surface-to-surface LWR     │ Yes - iterative view factors   │ Yes - linearised MRT approach         │
  ├─────────────────────────────────────┼────────────────────────────────┼───────────────────────────────────────┤
  │ Linearized radiation option         │ Yes (for reduced models)       │ Yes (4×ε×σ×T³)                        │
  ├─────────────────────────────────────┼────────────────────────────────┼───────────────────────────────────────┤
  │ Radiant barrier (attic)             │ Yes                            │ Yes (ε=0.05)                          │
  └─────────────────────────────────────┴────────────────────────────────┴───────────────────────────────────────┘

  Status: Well matched.

  Infiltration & Ventilation

  ┌────────────────────────────────────────┬──────────────────────────────────┬─────────────────────────────────┐
  │               Phenomenon               │              OCHRE               │              HARES              │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ ASHRAE AIM-2 (wind+stack)              │ Yes                              │ Yes                             │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ ELA method                             │ Yes                              │ Yes                             │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ ACH method                             │ Yes                              │ Yes                             │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ Natural ventilation (operable windows) │ Yes - stack+wind driven          │ Yes - stack+wind driven         │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ Mechanical ventilation                 │ Yes                              │ Yes                             │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ ERV/HRV recovery (sensible+latent)     │ Yes                              │ Yes                             │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ Balanced vs unbalanced combination     │ Yes (quadrature for unbalanced)  │ Yes (quadrature for unbalanced) │
  ├────────────────────────────────────────┼──────────────────────────────────┼─────────────────────────────────┤
  │ Humidity gating for nat vent           │ Yes (max outdoor humidity ratio) │ Yes (0.0115 kg/kg threshold)    │
  └────────────────────────────────────────┴──────────────────────────────────┴─────────────────────────────────┘

  Status: Well matched.

  HVAC Cooling

  ┌─────────────────────────────────────────┬──────────────────┬────────────────────────┐
  │               Phenomenon                │      OCHRE       │         HARES          │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ Central AC (biquadratic curves)         │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ Room AC                                 │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ ASHP cooler                             │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ Minisplit ASHP cooler                   │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ SHR / latent cooling                    │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ Henderson-Rengarajan latent degradation │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ Crankcase heater                        │ Yes              │ Yes                    │
  ├─────────────────────────────────────────┼──────────────────┼────────────────────────┤
  │ Duct DSE losses                         │ Yes (ASHRAE 152) │ Yes (multiplier-based) │
  └─────────────────────────────────────────┴──────────────────┴────────────────────────┘

  Gap: OCHRE computes DSE dynamically from duct parameters via ASHRAE 152. HARES uses a pre-computed DSE multiplier.

  Ground Coupling

  ┌────────────────────────────────┬─────────────────────────┬───────────────────────────────────┐
  │           Phenomenon           │          OCHRE          │               HARES               │
  ├────────────────────────────────┼─────────────────────────┼───────────────────────────────────┤
  │ Foundation/slab to ground temp │ Yes - ground node in RC │ Yes - GROUND_NODE_ID driving node │
  ├────────────────────────────────┼─────────────────────────┼───────────────────────────────────┤
  │ Ground temperature schedule    │ Yes                     │ Yes (from weather)                │
  └────────────────────────────────┴─────────────────────────┴───────────────────────────────────┘

  Status: Matched.

  Solar

  ┌─────────────────────────────────────┬───────┬─────────────────────────────────┐
  │             Phenomenon              │ OCHRE │              HARES              │
  ├─────────────────────────────────────┼───────┼─────────────────────────────────┤
  │ Window SHGC / transmittance         │ Yes   │ Yes                             │
  ├─────────────────────────────────────┼───────┼─────────────────────────────────┤
  │ IAM correction (angle of incidence) │ Yes   │ Yes (EnergyPlus glazing curves) │
  ├─────────────────────────────────────┼───────┼─────────────────────────────────┤
  │ Opaque surface solar absorptance    │ Yes   │ Yes (default 0.60)              │
  ├─────────────────────────────────────┼───────┼─────────────────────────────────┤
  │ Perez anisotropic diffuse model     │ Yes   │ Yes (8 clearness bins)          │
  └─────────────────────────────────────┴───────┴─────────────────────────────────┘

  Status: Well matched.

  Water Heater Losses

  ┌──────────────────────────────┬───────┬───────────────────┐
  │          Phenomenon          │ OCHRE │       HARES       │
  ├──────────────────────────────┼───────┼───────────────────┤
  │ Stratified tank standby UA   │ Yes   │ Yes               │
  ├──────────────────────────────┼───────┼───────────────────┤
  │ Inter-node conduction        │ Yes   │ Yes               │
  ├──────────────────────────────┼───────┼───────────────────┤
  │ Water draw heat removal      │ Yes   │ Yes               │
  ├──────────────────────────────┼───────┼───────────────────┤
  │ Inversion mixing             │ Yes   │ ? (not confirmed) │
  ├──────────────────────────────┼───────┼───────────────────┤
  │ HPWH evaporator zone cooling │ Yes   │ Yes (SHR=0.88)    │
  └──────────────────────────────┴───────┴───────────────────┘

  Gap: Inversion mixing (preventing unphysical temperature inversions in tank) — present in OCHRE, not confirmed in HARES.

  Humidity / Latent

  ┌─────────────────────────────────┬───────┬──────────────────────────┐
  │           Phenomenon            │ OCHRE │          HARES           │
  ├─────────────────────────────────┼───────┼──────────────────────────┤
  │ Zone moisture balance           │ Yes   │ Yes                      │
  ├─────────────────────────────────┼───────┼──────────────────────────┤
  │ Moisture buffering multiplier   │ Yes   │ Yes (15×)                │
  ├─────────────────────────────────┼───────┼──────────────────────────┤
  │ Dehumidifier                    │ Yes   │ Yes (biquadratic curves) │
  ├─────────────────────────────────┼───────┼──────────────────────────┤
  │ Latent ventilation/infiltration │ Yes   │ Yes (ṁ × H_fg × Δw)      │
  └─────────────────────────────────┴───────┴──────────────────────────┘

  Status: Matched.

  Key Gaps Summary

  1. Dynamic film coefficients — OCHRE uses TARP (wind/tilt-dependent convection); HARES uses fixed values
  2. Dynamic duct DSE — OCHRE computes from duct properties per ASHRAE 152; HARES uses a static multiplier
  3. Tank inversion mixing — confirmed in OCHRE, needs verification in HARES

