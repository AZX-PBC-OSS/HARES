# EnergyPlus Engineering Reference: Legacy § Number to Heading Mapping

The EnergyPlus Engineering Reference (all versions v8.0–v26.1) as hosted at
bigladdersoftware.com/epx/docs/ uses **prose headings** ("Sky Radiation Modeling",
"Inside Heat Balance") with HTML anchor IDs, **not numeric section designators**.
Section numbers like `§3.5`, `§15.4`, or `§14.8` never existed in the online
HTML format. They originate from either obsolete PDF editions distributed by
the DOE (prior to ~2015) or from contributor memory.

This document maps every known legacy `§X.Y` reference that appears in HARES
documentation to the correct heading-based citation in EnergyPlus 26.1
Engineering Reference. HARES has adopted **v26.1** as the algorithmic reference
version (see `docs/development.md` for the version policy).

## How to cite

**Correct format** (heading-based, verifiable):

> EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior Convection

**Incorrect format** (§-based, unverifiable):

> EnergyPlus Engineering Reference §3.5

## Legacy § references in historical documents

Historical HARES documents under `docs/tickets/`, `docs/reviews/`, and
`docs/findings/` may contain EnergyPlus § references that were used
before the version policy was established. These section numbers are
**preserved for provenance** in those documents but should not be used
as citations in new code, comments, or documentation.

---

## Mapping Table

| Legacy § Ref | Common Topic | Correct E+ 26.1 Citation |
|---|---|---|
| §1 | Site:Location / pressure | EnergyPlus ERM 26.1 — "Climate Calculations" |
| §1.2 | Warmup convergence | EnergyPlus ERM 26.1 — "Warmup Convergence" |
| §2.5.5 | Solar radiation interpolation | EnergyPlus ERM 26.1 — "Climate Calculations": Weather File Solar Interpolation |
| §3.1 | Ground heat transfer (general) | See specific ground models below |
| §3.1.1.3 | Site:WeatherStation, wind speed profile | EnergyPlus ERM 26.1 — "Outside Surface Heat Balance": Local Wind Speed Calculation |
| §3.1.3.2 | Building object, terrain field | EnergyPlus ERM 26.1 — "Outside Surface Heat Balance" |
| §3.2 | Simple Window Model (Arasteh et al.) | EnergyPlus ERM 26.1 — "Window Calculation Module": Simple Window Model |
| §3.2 | Outside Surface Heat Balance | EnergyPlus ERM 26.1 — "Outside Surface Heat Balance" |
| §3.2.4 | Exterior convection defaults | EnergyPlus ERM 26.1 — "Outside Surface Heat Balance": Outdoor/Exterior Convection |
| §3.5 | Inside Surface Heat Balance / interior convection | EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior Convection |
| §3.5.4 | Interior convection algorithms (TARP, Fohanno-Polidori) | EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior Convection (**Note:** MoWiTT is an exterior algorithm, not interior) |
| §3.5.6 | Sky emissivity calculations | EnergyPlus ERM 26.1 — "Climate Calculations": Sky Radiation Modeling |
| §3.5.10 | Network Solution (ScriptF, MRT, Star/Mesh) | EnergyPlus ERM 26.1 — "Inside Heat Balance": Internal Long-Wave Radiation Exchange |
| §3.6 | Inside Surface Heat Balance (alternate numbering) | EnergyPlus ERM 26.1 — "Inside Heat Balance" |
| §3.6.3 | ZoneHVAC:EquipmentList sensible/latent fractions | EnergyPlus ERM 26.1 — "Zone Equipment and Zone Forced Air Units" (**Note:** per-equipment fractions are on ElectricEquipment/OtherEquipment objects, not EquipmentList) |
| §3.6.4 | Interior convection (alternate numbering) | EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior Convection |
| §3.7.4 | Sky emissivity (alternate numbering) | EnergyPlus ERM 26.1 — "Climate Calculations": Sky Radiation Modeling |
| §3.3.10 | Conduction Finite Difference | EnergyPlus ERM 26.1 — "Conduction Finite Difference Solution" |
| §3.17 | Kusuda-Achenbach ground temperature | EnergyPlus ERM 26.1 — "Undisturbed Ground Temperature Model: Kusuda-Achenbach" |
| §6.3 | Ideal Loads Air System | EnergyPlus ERM 26.1 — "Ideal Loads Air System" |
| §9.4 | Interior convection (TARP / ASHRAE Simple) | EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior Convection (or "Convection from Surfaces") |
| §9.5 | Exterior convection (DOE-2) | EnergyPlus ERM 26.1 — "Outside Surface Heat Balance": Outdoor/Exterior Convection > DOE-2 Model |
| §9.5.1 | Exterior convection DOE-2 formula | EnergyPlus ERM 26.1 — "Outside Surface Heat Balance": Outdoor/Exterior Convection > DOE-2 Model |
| §13.2.2 | Zone Air Heat Balance / internal mass | EnergyPlus ERM 26.1 — "Inside Heat Balance": Thermal Mass and Furniture |
| §13.3 | Semi-implicit infiltration | EnergyPlus ERM 26.1 — "Infiltration/Ventilation" |
| §14.5 | Interior solar distribution | EnergyPlus ERM 26.1 — "Shading Module": Solar Distribution |
| §14.5 | Extraterrestrial radiation | EnergyPlus ERM 26.1 — "Climate Calculations": EnergyPlus Design Day Solar Radiation Calculations |
| §14.7 | Window Heat Balance / Inside Surface Heat Balance | EnergyPlus ERM 26.1 — "Window Heat Balance Calculation" |
| §14.8 | Stratified water tank / water heater | EnergyPlus ERM 26.1 — "Water Thermal Tanks (includes Water Heaters)": Stratified Water Thermal Tank |
| §15.2.11.4 | Defrost operation (heat pump) | EnergyPlus ERM 26.1 — within the heat pump/coil chapters (no standalone defrost page) |
| §15.4 | AIM-2 infiltration / Sherman-Grimsrud | EnergyPlus ERM 26.1 — "AirflowNetwork Model" (or "Infiltration/Ventilation" for simpler models) |
| §16.2 | Biquadratic performance curves | EnergyPlus ERM 26.1 — "Performance Curves and Lookup Tables" |
| §16.3.4 | Defrost defaults | EnergyPlus ERM 26.1 — within the coils/heat pump chapters |
| §16.5 | Residential DX Coil Model | EnergyPlus ERM 26.1 — "Coils" (residential DX content is within the coils chapter; the chapter covers cooling, heating, and heat pump coils) |
| §16.5.2 | Variable-speed DX coil | EnergyPlus ERM 26.1 — "Coils" (variable-speed content is within the coils chapter) |
| §17.5.5 | Zone and System Sizing | EnergyPlus ERM 26.1 — "Sizing Manager" |

## Notes

1. **Multiple topics per § number.** Some § numbers (e.g. §14.5, §3.2) refer to
   different topics in different documents. Always cross-reference the topic
   description in addition to the § number.

2. **§ numbers vary by source.** The PDF editions distributed by DOE prior to
   ~2015 used a different numbering scheme than the online HTML. The table above
   reflects the mapping to the current (v26.1) heading structure.

3. **No § numbers for EnergyPlus.** When citing EnergyPlus in new code or docs,
   always use the heading-based format:
   ```
   EnergyPlus ERM 26.1 — "<Page Title>": <Section Heading>
   ```
   Example: `EnergyPlus ERM 26.1 — "Inside Heat Balance": Interior Convection`

4. **ASHRAE, ISO, IEEE, and other standards** DO use section numbers in their
   published documents. The § prohibition applies specifically to EnergyPlus
   Engineering Reference citations, not to standards that publish with
   numbered sections.
