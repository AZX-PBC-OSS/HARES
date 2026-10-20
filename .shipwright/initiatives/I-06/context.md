---
entry_path: natural_language
entry_created_at: 2026-09-22T19:47:47.476Z
---
On a 45-day run, an EV under a Nightly charging strategy charges on the first few nights only, then never charges again for the rest of the run. The vehicle keeps driving daily (trips are then cancelled for range), stays plugged in, and the pack ends the run empty.

What I ran
Fixture tests/fixtures/resstock/2025.1/bldg0000007, EV added to the HPXML text (75 kWh pack, 11.5 kW charger, Level 2), Nightly strategy override, 45 days at 900 s:

nightly = {"Nightly": {"off_peak_start_hour": 22.0, "off_peak_end_hour": 6.0, "target_soc": 0.90}}
overrides = {"EV": {"charging_strategy": json.dumps(nightly)}}
kw = dict(start_time="2018-01-01T00:00:00", duration_s=45*24*3600, time_res_s=900,
          master_seed=0, output_verbosity=1, write_output=True, output_path=str(tmp/"run"),
          defaults_path=str(HARES/"defaults"),
          overrides=overrides)
dw = Dwelling.from_hpxml(hpxml, schedule, weather, **kw)
dw.initialize()
df = dw.simulate()
Same result on the debug and release builds:

2 charge-days in 45; 71.7 kWh charged total (a 75 kWh pack)
SOC 1.00 -> 0.072; actor:EvDriver:EV:drive_cancelled reaches 31 by day 45
sessions only around day 6, and not all inside the window (start hours 10, 16, 21, 23)
after that: plugged in, soc_gate_charging_allowed = 1, needed_charge_hours around 2, and no charging for the remaining ~38 days
A second home (injected via the calibrator's injection path, Tarrant County home 46363) shows the same shape: 5 charge-days (all sessions starting exactly 22:00), SOC pinned near 0.001 from day 8, drive_cancelled climbing 1 -> 11 by day 20, and needed_charge_hours still reporting ~2 h per night. So the nightly window itself works when charging happens; the driver just stops initiating sessions, and the range-anxiety backstop never fires despite an effectively empty pack.

Expected: a Nightly strategy keeps topping the pack to target on plugged-in nights for the whole horizon, or at minimum the range-anxiety backstop charges an empty pack.

Build: engine_build_id 50f0e3d060f6ed90, from 9f23024b (post-#25), Linux x86_64, Python 3.13.

Worth noting for a test: the existing EV tests run 120 one-minute steps, which is not enough horizon to see this - it needs a multi-week run to show up.