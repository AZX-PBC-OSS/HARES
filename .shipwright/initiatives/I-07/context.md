---
entry_path: natural_language
entry_created_at: 2026-09-24T16:40:09.298Z
---
https://github.com/AZX-PBC-OSS/HARES/issues/32

Verifying the fix from 2ee83a8 (cross-wired EV observations) on a 45-day horizon turned up a residual that the calibration suite's own premise does not show, so this may be fixture- or injection-path-specific — recording both here.

What works on the fixed build
The pack no longer dies: nightly top-ups recover to target (45-day repro, end SOC 0.90; pre-fix it ended at 0.07 with the driver dead after ~a week).
On our study premise (Tarrant County home, EV injected through our HPXML-rewrite path), charging is near-nightly: 34 sessions in 45 days, and the sessions now TAPER (per-session mean-power/rating: median 0.87, min 0.69 — CC-CV, which the pre-fix build did not model; its sessions were flat rectangles).
What looks wrong
Same repro recipe as #26, but on fixture tests/fixtures/resstock/2025.1/bldg0000007 (EV added as raw HPXML text, <SystemIdentifier id='ReproEV1'/>, 75 kWh / 11.5 kW / Level 2, Nightly override, 45 days, 900 s):

Five consecutive nights show: plugged in, soc_gate_charging_allowed = 1, needed_charge_hours rising 0.75 -> 2.75 -> 5.0 -> 6.75 -> 9.0, SOC falling 0.83 -> 0.07 — and zero charging in the 22:00-06:00 window. (EV Electric Power (kW) is exactly 0.00 on those nights.)
Charging only starts at ~7% SOC, days 6-7 (38 kWh, then 49 kWh) — the range-anxiety backstop, not the window.
Then it settles: end SOC 0.90, but only 2 charge-days in 45 and drive_cancelled = 30 by day 45, i.e. after the backstop rescue the vehicle mostly stops driving (trips cancelled) rather than cycling normally.
The same 45-day recipe on our study premise charges 34 times. The difference between the two setups is the home and how the EV reaches the engine (raw HPXML text with a fresh SystemIdentifier vs our rewritten HPXML) — so either the identity assignment leaves a raw-injected EV driver unbound to something (weather/occupancy/miles sampling?) until the backstop bypasses it, or fixture 0000007 trips an interaction worth knowing about.

Engine build 129e676c76886682 from 2ee83a86 (+ three additive commits on our branch), Linux x86_64, Python 3.13. The day-by-day telemetry table is ready if useful; the repro script is the one in #26 with the fixture path swapped.

## Fix outcome (appended by the fix agent, 2026-09-24)

Root cause confirmed as reported-weather-triggered EV pack thermal defects,
not an identity/binding residual: the pack's 20 kJ/K thermal mass
flash-froze to ambient every sub-freezing night (~1.4 h time constant),
charger conversion losses were misattributed to the pack as heat (165-281 C
packs, negative capacity fade), and the heater's heat was gated on charge
power so preconditioning was impossible by any configuration. All three
fixed by aligning the EV with the stationary Battery's architecture (one
shared pack-electrical home for the I2R solve; capacity-derived thermal
mass ~480 kJ/K at 75 kWh; a 5 kW pack-duty heater gated on pack temperature
alone). The 45-day -7 C cold snap now charges on 39 of 45 days with zero
cancelled trips. Three further findings were fixed in the same pass: the
degradation model's constants aligned to NREL's own Smith 2017
implementation (b3_ref/theta/b0 corrected, the negative-electrode branch
added, usable capacity = min(QLi, Qneg)); the driver's needed-hours
estimate now keys on the equipment's published charge derate (+infinity
while charging is physically blocked); and the parity corpus gained a
charging-EV fixture (site-energy residual 1.36 %). The fix's own final
audit found and fixed one further class-member the plan had enumerated
only in the EV: the stationary Battery's clamp_power applied a charging-
curve LUT's temperature axis on top of its capacity derate (the same
one-effect-applied-twice shape), now fixed with its own regression test.
Full record: initiative.md Implementation Notes; decision-log.md
dl-i07-fix-01..03.