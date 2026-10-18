# Attic volume under a gable roof: OS-HPXML v1.12.0 source check

Question: what attic air volume does OpenStudio-HPXML v1.12.0 give a vented or
unvented attic under a gable roof, and is `eb050372` (`fix-or18c`) right to
apply the square-hip rule to every attic?

Source: `git clone --depth 1 --branch v1.12.0 https://github.com/NREL/OpenStudio-HPXML`
(commit `502811cdfa5ccf82cd9afd00114a4d49eca85700`). Unless noted otherwise,
the paths below are under `HPXMLtoOpenStudio/`.

## 1. How OS-HPXML sets the attic volume

- `measure.rb:352` calls `Geometry.set_zone_volumes` after all surfaces are
  built.
- `resources/geometry.rb:1049-1077` (`set_zone_volumes`): for
  `attic - vented` and `attic - unvented` it calls
  `spaces[location].thermalZone.get.setVolume(volume)` and
  `spaces[location].setVolume(volume)`. The volume comes from
  `calculate_zone_volume`. The only exception is ASHRAE 140, which hardcodes
  3463 ft³ (`:1068-1069`).
- `resources/geometry.rb:1315-1330` (`calculate_zone_volume`): for an attic,
  `[footprint_area * height / 3.0, 0.01].max # Assume square hip roof`.
- `resources/geometry.rb:1373-1393` (`calculate_height_and_footprint_of_roofs`)
  works as follows:
  - Footprint = Σ roof area / √((pitch/12)² + 1).
  - Height = `0.5 * sin(atan(pitch/12)) * √footprint`. This is commented
    "Assume square hip roof", and it uses sin rather than the geometric tan.
  - A flat roof gets 2 ft.
- No other code path exists. A grep of `geometry.rb`, `defaults.rb`,
  `airflow.rb` and `hvac_sizing.rb` for gable, hip and attic volume turns up
  only these sites. Others use the same function:
  - `defaults.rb:911`: the "square hip" roof-average ceiling height.
  - `defaults.rb:5825`: infiltration volume for attics within it.
- Gable walls (HPXML `Wall` with `InteriorAdjacentTo` = attic) are built as
  heat-transfer surfaces only (`apply_walls`). They never enter the volume.

## 2. The volume in the EnergyPlus IDF

The volume is set explicitly. EnergyPlus does not autocalculate it.

- OpenStudio's forward translator writes `Zone, Volume` whenever the thermal
  zone carries one (`ForwardTranslateThermalZone.cpp:182-183`, OpenStudio
  `develop`). So the IDF attic `Zone` holds the hip value.
- Autocalculating from surfaces would not work anyway. OS-HPXML builds each
  roof as a free rectangle of width √A at `walls_top + 0.5·sin(atan p)·width`
  (`geometry.rb:13-42`). `explode_surfaces` (`geometry.rb:1085`) then pushes
  surfaces apart, so the zone has no closed enclosure.
- The zone air thermal capacitance is
  `TemperatureCapacityMultiplier = 7` × ρ·cp·V:
  - `simcontrols.rb:27-28` sets the multiplier.
  - `defaults.rb:219-221` defaults it to 7.0.
  - The object is not assigned to any zone (`model.rb:1068-1069`). With the
    zone name blank, it applies to every zone, the attic included.

## 3. Numbers

**`data/examples/BEopt_example.xml`:**
- Inputs: two 670.82 ft² roofs at 6:12, a 30×40 ft slab house and 8 ft walls.
  It is vented with SLA 0.00333 and has two 144.5 ft² gable walls.
- OS-HPXML footprint = 1341.64 / 1.1180 = 1200 ft².
- Height = 0.5 × sin(26.57°) × √1200 = 7.746 ft (2.361 m).
- V = 1200 × 7.746 / 3 = 3098 ft³ (**87.7 m³**).
- `eb050372` gives 87.7 m³ at 2.36 m, which matches exactly.
  `zone_geometry.rs:190` uses the same `0.5·sin(atan(slope))·√A`, and `:235`
  divides by 3.

**`workflow/sample_files/base.xml`:**
- Inputs: a 1509.3 ft² unvented roof at 6:12, a 1350 ft² attic floor and
  225 ft² of gable walls.
- OS-HPXML: footprint 1350 ft², height 8.216 ft (2.504 m),
  V = 3697 ft³ (**104.7 m³**).
- The true gable geometry (30×45 ft, ridge 7.5 ft) gives ½·1350·7.5 =
  5062 ft³ (143.4 m³). OS-HPXML uses 73 % of that.

**BEopt_example compared with the other rules:**

| Rule | h (m) | V (m³) | Air C (kJ/K) |
|---|---|---|---|
| OS-HPXML v1.12.0 (hip), ×7 | 2.361 | 87.7 | 743 |
| `eb050372` HARES (hip), ×1 | 2.361 | 87.7 | 106 |
| HARES before `eb050372` (OCHRE gable), ×1 | 2.591 | 144.4 | 175 |
| OCHRE (`hpxml.py:615-631`, gable from 144.5 ft² wall), ×7 | 2.591 | 144.4 | 1223 |
| True gable from the roof (span 30 ft, rise 7.5 ft) | 2.286 | 127.4 | (1079 at ×7) |

OCHRE's gable height comes out 8.5 ft, above the 7.5 ft that the roof and
walls give. BEopt's 144.5 ft² gable implies a 34 ft span, which looks like the
eaves are included. So OCHRE's volume overstates even the true gable volume by
13 %.

## 4. Verdict on the volume

`eb050372` reproduces OS-HPXML v1.12.0 exactly. OS-HPXML has one attic rule,
the square hip with `1/3·A·h` and `h = ½·sin(θ)·√A`, and applies it whatever
the roof shape. The IDF carries that number as an explicit zone volume. For
OS-HPXML parity the commit is correct, and the gable path that was removed was
an OCHRE rule, not an OS-HPXML one.

As physics, the hip rule underestimates a gable attic. On base.xml the true
gable volume is 143 m³ against 105 m³ (−27 %); on BEopt it is 127 m³ against
88 m³ (−31 %). The `sin` also makes the ridge `cos θ` (0.894 at 6:12) lower
than a real hip's.

If HARES targets better-than-OS-HPXML physics, the defensible gable rule is
the geometric one, ½·A·(span/2)·tan θ with span taken from the roof/footprint.
OCHRE's `√(A_gable·tan θ)` is not defensible, because it absorbs eave overhang
into the gable area.

Either way, the volume barely matters to attic temperature:
- It changes only the zone air capacitance.
- It changes the flow of an unvented attic at 0.1 ACH.
- A vented attic's ELA flow does not depend on volume: SLA × floor area
  (`airflow.rb:1684-1691`).
- The height it carries sets the stack coefficient and the wind height
  (`airflow.rb:2753-2771`).

The 2.93 → 3.09 °C move is therefore not mainly a volume effect. The bound
should not be decided on the volume rule.

## 5. The ~3 °C attic gap against OCHRE

I ran the six `conditioned_oracle` cases on the `fix-or18c` worktree
(`e903016e`) with
`cargo test --release -p hares-core --features observe --test conditioned_oracle`:

| Scenario | HARES attic mean | OCHRE attic mean | MAE |
|---|---|---|---|
| summer 48 h dynamic | 30.25 | 28.98 | 3.094 (FAIL > 3.0) |
| summer 48 h ideal | 30.25 | 28.96 | 3.088 (FAIL) |
| spring 72 h | 17.15 | 16.76 | 2.44 |
| winter 48 h | 3.47 | 4.77 | 1.81 |

In summer HARES runs +1.3 °C hotter, and in winter −1.3 °C colder. HARES
couples the attic more tightly to the roof deck, which is solar-hot by day and
sky-cold by night/winter, and less tightly to outdoor air. This sign pattern
points at the ventilation and capacitance terms. It does not point at roof
radiant properties, which both models take from the same HPXML (α 0.85,
ε 0.91).

The candidates, ranked:

1. **Attic air capacitance multiplier: HARES 1, OCHRE 7, OS-HPXML 7.**
   - HARES: `crates/hares-core/src/dwelling/conversions.rs` (`mass_multiplier_for_zone`)
     gives `Attic => 1.0`.
   - OCHRE: `Models/Envelope.py:443-446` sets `"Thermal Mass Multiplier", 7`
     for every zone.
   - OS-HPXML: a global `ZoneCapacitanceMultiplier:ResearchSpecial`
     temperature multiplier of 7 (`simcontrols.rb:27-28`, `defaults.rb:219-221`).
   - HARES attic air C is 106 kJ/K against OCHRE's 1223 kJ/K (11.5×) and
     OS-HPXML's 743 kJ/K (7×). The roof deck and shingles are about 1-2 MJ/K,
     so this term is a large fraction of the attic node's inertia. It controls
     peak amplitude and phase, which is what MAE measures.
   - It also explains why the MAE rose when the volume fell: 175 → 106 kJ/K.
   - The `conversions.rs` doc comment ("E+ default 1.0") is not what
     OS-HPXML runs. It sets 7 for every zone.
   - This departs from both references. It is the first fix to test.

2. **Attic ventilation wind coefficient (terrain model and height).**
   - HARES `hares-physics/src/infiltration.rs:777-820` uses the ASHRAE HoF
     Ch.16 power law: met 270 m/0.14, suburban 370 m/0.22.
   - OS-HPXML uses its Sherman-Grimsrud `f_t_SG` with suburban 0.67/0.25
     (`airflow.rb:216-217, 2766`).
   - OCHRE hardcodes rural 0.85/0.20 at the gable height above
     `building_height` (`utils/envelope.py:636-676`, `hpxml.py:659`).
   - On BEopt (walls top 2.44 m): f_t is 0.610 (HARES), 0.558 (OS-HPXML) and
     0.741 (OCHRE), so C_w ∝ f_t² is 0.372 / 0.311 / 0.549.
   - HARES attic wind ventilation is about 32 % below OCHRE's, and the stack
     coefficient is 9 % lower (2.36 against 2.59 m). That gives roughly 15-20 %
     less airflow. OCHRE's mean is 0.125 m³/s in summer, about 150 W/K.
   - This fits the summer-hot/winter-cold sign. It is also a parity defect
     against OS-HPXML: HARES claims OS-HPXML's attic leakage
     (`solver_builder.rs:1683-1688`) but uses a different f_t.

3. **Roof exterior convection roughness.**
   - HARES maps a roof with no `<Siding>` to MediumRough, r_f = 1.52, with a
     warning (`hares-physics/src/film_coefficients.rs:103-128`).
   - OCHRE uses r_f = 1.67 (`utils/envelope.py:390-394`). E+ DOE-2 in
     OS-HPXML (`simcontrols.rb:21-22`) takes roughness from the roofing
     material.
   - The lower forced h makes the deck hotter by day and colder at night.
   - This is small, a few percent of h_ext. Same sign as above.

4. **Interior roof film.**
   - OCHRE holds the roof's attic-side film at 0.536 m²K/W (h ≈ 1.87).
   - HARES runs per-step TARP but adds the correction only when TARP exceeds
     the frozen h (`hares-envelope/src/thermal_solver/stepping.rs:582-640`).
   - In a hot-deck summer afternoon, TARP "reduced" convection for a
     hot-above sloped surface stays near the frozen value. It probably
     contributes little, but this is unverified.

5. **Sky temperature and roof LWR: ruled out as a model difference.**
   - HARES uses F_sky·β = F^1.5 (`longwave_radiation.rs:15-21, 102-130`).
   - That is the same as OCHRE's `((1+cos)/2)^1.5` (`Envelope.py:268`).
   - Both derive T_sky from the EPW horizontal IR.

6. **Roof deck capacitance: low likelihood.**
   - HARES takes roof RC from the same OCHRE envelope lookup table
     (`hares-io/src/envelope_lut.rs:126, 814-818, 1024`).

## Recommendation

- Keep `eb050372`'s hip rule as the OS-HPXML-parity value. It is correct
  against the source.
- Do not move the bound because of volume.
- The attic gap belongs to the attic air capacitance multiplier (1 in HARES,
  7 in OS-HPXML and OCHRE), and secondly to the attic wind coefficient's
  terrain model, which is not OS-HPXML's `f_t_SG`.
- Fix both to OS-HPXML's model, then re-measure `conditioned_oracle`.
- Whether a gable attic should instead get its geometric gable volume
  (½·A·rise) as a better-than-OS-HPXML improvement is a separate physics
  decision. It is low impact, worth about a 25-30 % change in a capacitance
  that is already small at ×1.
