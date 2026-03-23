---
id: WEATHER-009
title: Additional sky emissivity models (Brunt, Idso, Berdahl-Martin)
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/weather.rs
references:
  - EnergyPlus Engineering Reference v9.3+, Climate Calculations — Sky Temperature
  - https://www.sciencedirect.com/science/article/abs/pii/S0038092X23005972
  - https://publications.ibpsa.org/proceedings/simbuild/2016/papers/simbuild2016_C056.pdf
verification:
  - cargo build --workspace
  - cargo test --workspace
---

## Background/Context

HARES currently uses two sky temperature models:
1. **Stefan-Boltzmann inversion** (primary): `T_sky = (IR / σ)^0.25 - 273.15`
   when horizontal infrared >= 50 W/m²
2. **Clark-Allen** (fallback): empirical correlation from dry bulb + dew point
   when infrared data is unavailable or < 50 W/m²

EnergyPlus 9.3+ added three additional models: **Brunt**, **Idso**, and
**Berdahl-Martin**. Research shows sky emissivity model choice can affect
heating/cooling loads by ±10-19% in some climates, though the impact is smaller
for US residential buildings.

### Model comparison (from literature)

| Model | Inputs | Best for | Accuracy |
|---|---|---|---|
| Stefan-Boltzmann | IR radiation | When IR data available | Best (direct measurement) |
| Clark-Allen (1978) | T_db, T_dp | Dry climates | Good (RMSE ~7-8%) |
| Brunt (1932) | T_db, vapor pressure | Humid climates | Good (RMSE ~6-7%) |
| Idso (1981) | T_db, vapor pressure | Clear sky | Moderate |
| Berdahl-Martin (1984) | T_db, T_dp, cloud cover | All conditions | Best empirical |

### What OCHRE does

OCHRE uses Stefan-Boltzmann only (same as HARES primary). No empirical fallbacks
beyond what EnergyPlus weather files provide.

### Recommendation

The Berdahl-Martin model is the most physically complete empirical model (uses
cloud cover data, which EPW provides as opaque sky cover). Adding it as an
alternative to Clark-Allen would improve accuracy for cases where IR data is
missing but cloud cover is available.

## Work to Do

- [ ] Add `SkyTemperatureModel` enum to `weather.rs`:
      ```rust
      pub enum SkyTemperatureModel {
          StefanBoltzmann,  // Primary: from horizontal IR
          ClarkAllen,       // Fallback: T_db + T_dp
          BerdahlMartin,    // Enhanced: T_db + T_dp + cloud cover
      }
      ```

- [ ] Implement Berdahl-Martin model in `epw.rs`:
      ```
      ε_sky = 0.741 + 0.0062 × T_dp_C
      ε_cloud = ε_sky + 0.84 × (N / 10) × (1 - ε_sky)
      T_sky = T_db_K × ε_cloud^0.25 - 273.15
      ```
      Where N = opaque sky cover [0, 10]
      Cite: Berdahl, P. and Martin, M. (1984), "Emissivity of Clear Skies",
      Solar Energy, 32(5), 663-664.

- [ ] Update EPW parser to optionally use Berdahl-Martin when:
      - IR < 50 W/m² (current Clark-Allen fallback condition)
      - Opaque sky cover data is available (not zero/missing)
      - Otherwise fall back to Clark-Allen (no cloud data)

- [ ] Add tests comparing all three models at known conditions
- [ ] Document model selection logic and cite all sources

## Measures of Success

- [ ] Berdahl-Martin produces more accurate sky temps than Clark-Allen when
      cloud cover data is available
- [ ] Default behavior unchanged (Stefan-Boltzmann still primary)
- [ ] All models documented with citations

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
