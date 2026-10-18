---
entry_path: natural_language
entry_created_at: 2026-09-12T06:58:10.162Z
---
Summary. The charging_strategy override is accepted without error and its content is discarded; only its presence matters. An EV given Immediate or no strategy never charges at all while still appearing in the equipment list and emitting a column. 

Evidence. bldg0168875/G4804390, 30 days, varying only the override: Nightly 22:00–06:00 → 332.1 kWh; Nightly 02:00–06:00 → 332.1 kWh identical; LowSoc 0.15 → 332.1 kWh identical; Immediate → 0.0 kWh; no override → 0.0 kWh. What runs instead is charge-on-plug-in from ~18:00. ev/mod.rs:305-308 parses with .and_then(|s| from_str(s).ok()).unwrap_or(Immediate) — a mistyped field silently becomes Immediate, i.e. no charging, with no diagnostic. A Result would surface it at construction. 

What we need. Is the equipment-override path supported for this field? If not it should reject rather than accept-and-ignore. And should a strategy-less EV really never charge? 

Note. add_ev_with_driver(VehicleId, EvArchetypeId, seed) works and is our workaround.