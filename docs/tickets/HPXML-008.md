---
id: HPXML-008
title: Parse heat pump HeatingCapacity17F from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: HeatPump/HeatingCapacity17F (Btu/hr at 17°F outdoor)"
  - "HPXML spec: extension/HeatingCapacityFraction17F"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Heat pump heating capacity at 17°F (-8.3°C) is a standard AHRI rating point critical for cold-climate heat pump modeling. The ratio of capacity at 17°F to rated capacity (at 47°F) anchors the heating capacity vs outdoor temperature curve. HPXML provides both `HeatingCapacity17F` (absolute) and `extension/HeatingCapacityFraction17F` (fractional). Neither is currently parsed.

## Work to Do

- [ ] In the heat pump resolution block of `resolve_hvac`, extract `HeatingCapacity17F` (Btu/hr)
- [ ] Convert to watts and insert as `"heating_capacity_17f_w"` param
- [ ] If `HeatingCapacity` is also available, compute and insert `"heating_capacity_fraction_17f"` = capacity17F / capacityRated
- [ ] Also check `extension/HeatingCapacityFraction17F` as a direct fractional value
- [ ] Add unit test: HP with 36,000 Btu/hr rated and 21,000 Btu/hr at 17°F → fraction ≈ 0.583

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend heat pump resolution

## Measures of Success

- [ ] Heat pump with `<HeatingCapacity17F>21000</HeatingCapacity17F>` and `<HeatingCapacity>36000</HeatingCapacity>` → `heating_capacity_fraction_17f ≈ 0.583`
- [ ] Extension fraction is used directly when present
- [ ] Missing 17F capacity has no effect on existing behavior

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
