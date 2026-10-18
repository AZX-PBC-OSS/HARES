---
entry_path: natural_language
entry_created_at: 2026-09-13T04:33:07.487Z
---
Summary. The driver's own state channels do not track charging, on both construction routes including the working archetype one. 

Evidence. 60 days, DailyCommuterL1, vehicle drew 449.9 kWh across 1037 live intervals: charge_kw identically 0.000; needed_charge_hours identically 0.000; soc monotone non-increasing, rises on 0% of charging intervals. plugged_in and phase work. On the HPXML route the pack reaches 0.000 on day 8 and the car draws a further 234 of its 318 kWh reading empty. 

What we need. Are charge_kw and needed_charge_hours meant to be populated, and should soc reflect charging as well as driving? 

Impact. We read charging from the EV End Use Electric Power (kW) column instead, so we're unblocked — but a caller trusting the driver's own state would conclude the car never charges.

Check out other PRs, either build this on a worktree off of main, or maybe a PR  of the HPXML EV fix?  whatever makes most sense