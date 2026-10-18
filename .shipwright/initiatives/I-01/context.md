---
entry_path: natural_language
entry_created_at: 2026-09-10T20:40:10.872Z
---
Summary. Every typed equipment reports as constant power (zp=0, ip=0, pp=1), yet varying HARES service voltage measurably changes consumption. Anything computed from the published coefficients returns zero sensitivity. 

Reproduce:import ochre_next as on

HPXML, SCHEDULE, WEATHER = "<bldg0046363.xml>", "<schedule.csv>", "<G4804390 TMY3.epw>"
KW = dict(start_time="2018-01-01 00:00:00", duration_s=365*86400, time_res_s=900,
          bldg_id=46363, master_seed=0, output_verbosity=6,
          write_output=True, output_to_parquet=True,
          output_path="./out", defaults_path="<ABSOLUTE path to defaults>")

# --- Part A: what the published coefficients say -----------------------------
dw = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)
dw.initialize()
for name in dw.equipment_names():                 # or however equipment is enumerated
    print(name, dw.equipment_zip(name))
# Observed: every typed equipment returns zp=0, ip=0, pp=1 — constant power.
# A constant-power premise has, by definition, zero voltage sensitivity.

# --- Part B: what the engine actually does -----------------------------------
def annual_kwh(pu):
    d = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)
    d.initialize()
    d.set_grid_voltage(pu)      # AFTER initialize (ZIP models resolved), BEFORE simulate
    df = d.simulate()
    return df["Total Electric Power (kW)"].sum() * (900/3600)

lo, nom, hi = annual_kwh(0.95), annual_kwh(1.00), annual_kwh(1.05)
print(f"0.95 pu: {100*(lo-nom)/nom:+.2f}%   1.05 pu: {100*(hi-nom)/nom:+.2f}%")


Expected from this script: 0.95 pu: −1.18%, 1.05 pu: +1.38% — i.e. 0.236 and 0.276 %P per %V. Part A says zero. Same home, same seed, same build.

The engine's ~0.25 %P/%V is sensible and matches published residential figures. It is the coefficients that don't describe it. The response evidently comes from load whose ZIP equipment_zip doesn't expose, plausibly the scheduled/untyped base load rather than typed equipment.

What we need: is equipment_zip meant to describe whole-premise voltage response, or only the typed equipment it names? If the latter, what is the supported route to premise-level ZIP? The engine holds the information — it responds correctly when driven.