---
entry_path: natural_language
entry_created_at: 2026-09-11T19:40:36.974Z
---
Impact: blocks the envelope half of building efficiency functionality. Retrofit targeting needs per-component heat flows; these can't be read as a budget.

Reproduce: one run, then read two things that should agree with each other and with physics:import ochre_next as on

dw = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)   # output_verbosity=6 required
dw.initialize()
df = dw.simulate()

env = dw.metrics().envelope_loads_kwh          # metrics is CALLABLE, not a property
total = 0.0
for k in ("opaque_solar_lwr_kwh","window_solar_kwh","internal_gains_kwh","hvac_cooling_kwh",
          "hvac_heating_kwh","infiltration_kwh","ventilation_kwh","interior_lwr_kwh",
          "duct_loss_kwh"):
    v = getattr(env, k, 0.0); total += v
    print(f"  {k:24s} {v:>+14,.2f}")
print(f"  {'SUM':24s} {total:>+14,.2f} kWh/yr = {total/8760:+.2f} kW mean")

print("annual consumption :", df["Total Electric Power (kW)"].sum()*0.25, "kWh")
print("indoor temp range  :", df["Temperature - Zone_1 (C)"].min(),
                              df["Temperature - Zone_1 (C)"].max())


Expected: sum +141,142 kWh/yr ≈ +16.11 kW mean against a home consuming 15,352 kWh/yr (1.75 kW mean). opaque_solar_lwr_kwh alone is +121,278. That's about eight times the building's entire electrical consumption. Indoor temperature nonetheless stays within 17.4–27.2 °C all year, which a sustained +16 kW net gain is not compatible with.

You can confirm it the same way: integrate the per-interval … Heat Gain … (W) columns from df and compare against the envelope_loads_kwh totals. Eight of nine agree to about 1 part in 10¹⁵. (The ninth, ventilation, disagreed because we summed only forced and not natural ventilation — our defect, fixed.) So the per-interval columns and the run totals tell the same story; this is not a reading error.

What we need: confirm the intended scope of the wall/roof/floor term, and whether the nine components are meant to balance. Either answer is actionable — the second lets us restrict F13 honestly rather than report unusable magnitudes.

---

## Resolution (added at close, 2026-09-11)

Answer: the nine were **not** all meant to balance — `opaque_solar_lwr_kwh`
was the gross radiant flux absorbed at the exterior skin (an outside-face
boundary condition, EnergyPlus ERM 26.1 — "Outside Surface Heat Balance";
OCHRE's "{boundary} Ext. Solar/LWR Gain"), never a zone load. The budget
now carries only net, summable quantities — `opaque_conduction_kwh`
(renamed), `window_conduction_kwh`, `internal_mass_kwh` — and the +141 MWh
phantom-gain class is dead, pinned by tests. F13 can read the budget
honestly.

The fix round then swept the same defect class codebase-wide (seven
name-vs-meaning defects), and grew into a joint hardening program with a
concurrent agent — one shared skin-coupling constructor for every
eliminated-skin path, construction-time validation, a runtime skin-balance
invariant, the exact-matrix-exchange zone-balance residual, closure tests
across resistance ratios 0.02–100, and the divergence/flake ledgers.
Measured: the freefloat-oracle opaque exterior gap fell from +30.9 % to
+7.3 % (like-for-like for the first time); the full suite (78 binaries,
fmt, clippy, release, Python) is green. Full record:
`initiative.md` Implementation Notes, `rca.md` closure addendum, and
`docs/physics-hardening-program.md`.