---
entry_path: natural_language
entry_created_at: 2026-09-14T17:08:49.155Z
---
What is already fixed, so it isn't re-investigated
Verified against HARES f1b9e1f8 on bldg0168875 / G4804390, 30 days, 15-minute:
An EV with Immediate or with no override now charges — 329.8 kWh over 35 of 35 days, where both previously delivered 0.0 kWh. That was the larger half of the original report.
The override now reaches the engine and validates. A mistyped field raises equipment error: equipment 'EV' init failed instead of being silently accepted. #[serde(deny_unknown_fields)] on ChargingStrategy is doing its job.
What still does not work
The strategy's content does not govern behaviour. Varying only charging_strategy, every variant produces the same charging:
override
charging energy
peak hour
nights
hourly profile (0→23)
Nightly 22:00–06:00
329.9 kWh
20
28
##+...............######
Nightly 02:00–06:00
329.8 kWh
20
28
#++...............######
LowSoc 0.15
329.8 kWh
20
28
##+...............######
Immediate
329.8 kWh
20
28
##+...............######
no override
329.8 kWh
20
28
##+...............######
Two things make this more than a totals coincidence. Moving the off-peak window from 22:00 to 02:00 — a four-hour shift — changes total energy by 0.1 kWh in 330 and leaves the peak hour and night count identical. And LowSoc is indistinguishable from Immediate, despite being a different policy class entirely (charge when SOC falls below a threshold, versus charge on plug-in).
All five profiles concentrate charging at 18:00–23:00 plus 00:00–02:00 — charge-on-plug-in from about 18:00, regardless of what was configured.
This is specific to the HPXML override route
Dwelling.add_ev_with_driver(VehicleId, EvArchetypeId, seed) differentiates properly — we verified that separately and it produces distinct energy, charger rating, night counts and start-time concentration per archetype. So the driver machinery works; it is the path from an HPXML-declared EV plus a charging_strategy override that does not carry the configuration through.
Reproduce
Two things will cost you an hour if missed, both of which cost us one:
The equipment is named EV, not the HPXML SystemIdentifier (CalibEV1 in our fixture). An override keyed on the identifier is silently ignored — every run then reads as "no override", which looks like all strategies agreeing.
Field names are exact and per-variant: Nightly { off_peak_start_hour, off_peak_end_hour, target_soc }, LowSoc { threshold, target_soc }, Immediate { target_soc }. Note threshold, not threshold_soc.

Compare the hourly profiles, not just the totals — two strategies can coincide on annual energy while differing in shape, and shape is what a utility cares about for managed charging.
Why it matters 
EV Identification is validated by injecting a known charging behavior and scoring detection against it. With the window and threshold discarded we can only inject one behavior — charge-on-plug-in — so we cannot test detection against off-peak or SOC-triggered charging, which is what a managed-charging program actually produces.
Complete when
Two Nightly overrides differing only in off_peak_start_hour produce measurably different hourly charging profiles, and LowSoc differs from Immediate.