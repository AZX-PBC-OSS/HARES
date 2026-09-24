# ASHRAE Handbook of Fundamentals 2021 — Chapter Index

The 2021 edition is the sole authoritative HOF edition for HARES psychrometric
constants, molecular weights, film coefficient tables, and all other
authoritative chapter citations (see T-0306).

Source: <https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals>

## Verified Chapter ↔ Topic Mappings

| Ch. | Topic |
|-----|-------|
| 1 | Psychrometrics |
| 4 | Heat Transfer |
| 14 | Climatic Design Information |
| 15 | Fenestration |
| 16 | Ventilation and Infiltration |
| 17 | Residential Cooling and Heating Load Calculations |
| 18 | Nonresidential Cooling and Heating Load Calculations |
| 21 | Duct Design |
| 22 | Pipe Sizing |
| 25 | Heat, Air, and Moisture Control in Building Assemblies |
| 26 | Heat, Air, and Moisture Control in Building Assemblies — Material Properties |
| 27 | Heat, Air, and Moisture Control in Building Assemblies — Examples |
| 33 | Physical Properties of Materials |
| 34 | Geothermal Energy |
| 51 | Heat Exchangers and Water Heating |

## Topic → Correct Chapter (for Citation Verification)

These mappings are used by `scripts/check-ashrae-chapters.sh` to flag
chapter citations that are known to be wrong for the claimed topic.

| Topic / Keyword | Correct Chapter | Known Wrong Chapters |
|---|---|---|
| Psychrometrics, saturation pressure, humidity ratio, h_fg, ρ_da | 1 | — |
| Heat transfer, TARP, natural convection, radiation, film coefficient (interior) | 4 | 25 |
| Climatic design, design-day, solar, clear-sky | 14 | — |
| Fenestration, window, glass, U-factor, NFRC | 15 | — |
| Ventilation, infiltration, AIM-2 | 16 | — |
| Residential load, below-grade residential, slab, basement, F-factor perimeter | 17 | 18, 18.31 |
| Nonresidential load | 18 | — |
| Duct design | 21 | — |
| Pipe sizing | 22 | — |
| HAM fundamentals | 25 | — |
| HAM material properties | 26 | — |
| HAM examples, zone method, framing factor | 27 | — |
| Material physical properties, concrete, gypsum | 33 | — |
| Geothermal, borehole, ground-source | 34 | — |
| Water heating, WH tank, gas WH | 51 | — |

## Known Citation Errors (Fixed in T-0311)

1. **TARP natural convection → Ch. 25**: Corrected to Ch. 4. Ch. 25 (2021) is
   "Heat, Air, and Moisture Control in Building Assemblies," not heat transfer
   (see `docs/validation.md:161`).

2. **Below-grade residential → Ch. 18.31**: Corrected to Ch. 17. Ch. 18 is
   "Nonresidential Cooling and Heating Load Calculations"; residential
   below-grade content belongs in Ch. 17 (see 042 and 027 ticket docs;
   `crates/hares-core/src/dwelling/conversions.rs:170,307`;
   `crates/hares-io/src/hpxml/building.rs:755`).

3. **Coil bypass factor → ASHRAE 2017 Ch. 18 Eq. 63**: The specific equation
   number could not be verified against the 2017 edition. The citation has been
   updated to a descriptive reference (see
   `docs/reviews/equipment-hvac/equip-hvac-04-ac-shr-latent-degradation.md:87`).
