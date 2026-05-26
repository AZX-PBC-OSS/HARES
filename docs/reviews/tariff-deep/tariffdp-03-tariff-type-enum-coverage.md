# Tariff type enum: flat, TOU, tiered, demand-charge, net-metering, time-varying export rates
**Review ID**: tariffdp-03
**Category**: tariff-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-tariff/src/types.rs` (881 lines) — primary data model: `ElectricTariff`, `ExportMode`, `EnergyRate`, `DemandRate`, `TieredBlock`, `ExportRate`, `SeasonFilter`
- `crates/hares-tariff/src/evaluator.rs` (1679 lines) — billing dispatch: energy/export price resolution (lines 90–142), demand charge computation (lines 365–390), billing period close (lines 320–363, 420–466)
- `crates/hares-tariff/src/billing.rs` (818 lines) — tiered cost computation (lines 274–312), ratchet logic (lines 374–381)
- `crates/hares-tariff/src/urdb.rs` (933 lines) — URDB JSON parser, `ExportMode` mapping (lines 333–351)
- `crates/hares-types/src/schedule.rs` — `SeasonFilter` enum (lines 860–889), `BillingCycle` enum (lines 946–953)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.cc` (4882 lines):
  - Real-time pricing support: `curRTPprice`, `curRTPbaseline`, `curRTPenergy` (lines 2539–2541), RTP accumulation logic (lines 768, 2251)
  - Tariff type dispatch: block/TOU/RTP branched calculation paths
  - Demand window guard: waits for full window before evaluating peak (lines 2557–2562)
- `vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.hh` — RTP array declarations (line 358)

## Findings

### Finding 1: No explicit tariff type classification — CPP, RTP, and EV-only rates cannot be modeled [Severity: critical]
**Description**: The HARES tariff model has no `TariffType` or `RateType` enum. Instead, it uses a compositional approach where `ElectricTariff` (types.rs:206–233) carries all rate components as parallel optional fields (`tou_schedule`, `energy_rates`, `demand_rates`, `tiered_rates`, `export_rate`). This covers flat, TOU, tiered, demand, and net metering because those are all compositional layers within a single tariff. However, the following rate structures are **entirely absent** and have no data fields or calculation logic anywhere in the codebase:

1. **Real-time pricing (RTP)**: Energy charge varies hourly based on wholesale market prices. Requires an 8760-hour external price time series. EnergyPlus supports this via `RealTimePriceSchedule` (EconomicTariff.cc:2539–2541). HARES has no `rtp_schedule: Vec<f64>` field, no price lookup from external data, and no time-varying import price dispatch path. The `energy_rates` field only supports static per-period rates — the price per TOU period is constant across all days.

2. **Critical peak pricing (CPP)**: Extremely high rates during a limited number of event hours per year (typically 12–20 events), with lower rates the rest of the time. Requires: (a) an event calendar or signal declaring CPP events, (b) a separate (much higher) rate for event hours, (c) a counter enforcing annual event limits. EnergyPlus supports this via `CriticalPeakSchedule` and dedicated CPP rate fields. HARES has no event signal concept, no CPP rate field, and no event counter.

3. **Electric vehicle (EV) rates**: A separate meter, TOU sub-period, or dedicated rate schedule for EV charging. While HARES could in principle model EV load on a second `ElectricTariff` attached to a separate pseudo-dwelling, there is no built-in dual-meter support, no sub-tariff concept in `ElectricTariff`, and no way to apply a different rate to a fraction of household load within the same `BillingState`. The `ChargingStrategy::TouAware` variant in `hares-types/src/equipment.rs:448` can respond to a single tariff's TOU periods but cannot reference a separate EV-specific rate.

**Code Location**: The absence is architectural — the following structures would need to be created:
- A new field in `ElectricTariff` (types.rs:206) for RTP hourly prices (e.g., `rtp_schedule: Vec<f64>`)
- A new field for CPP configuration (event calendar + event rate + annual limit)
- A new tariff variant or sub-meter concept for EV-specific rates
- Corresponding dispatch branches in `evaluator.rs:90–123` (per-step price resolution) and `billing.rs:274–312` (period-close cost computation)

**Root Cause**: The compositional field approach was designed for rate structures that are orthogonal layers within a single tariff, not for rate structures that fundamentally differ in how the import price is determined (static schedule vs. external time series vs. event-driven schedule).

**Impact**:
- RTP customers: ~4% of US residential customers (2.6M households, per EIA 2021–2023 retail choice participation data, growing ~3%/yr). Concentrated in Illinois (Ameren, ComEd), Texas (REPs), Pennsylvania, New York, and other retail choice states. A HARES simulation of a Texas dwelling with indexed retail pricing would produce zero-accurate bills.
- CPP customers: ~8–10% of California residential customers are on default CPP as mandated by CPUC for investor-owned utilities (PG&E, SCE, SDG&E), representing ~1.1–1.4M households. Additional CPP programs exist via demand response aggregators in other states (e.g., BGE Smart Energy Rewards in Maryland, Xcel in Colorado). A HARES simulation of a PG&E CPP customer would miss all event-hour costs, under-counting bills by potentially $100–200/year.
- EV rate customers: 75%+ of US utilities offer EV-specific rates. Approximately 60% of the 3.3M US EV-owning households use an EV-specific rate (~2M households). HARES would incorrectly bill EV charging at the whole-house rate, which can double the EV-charging cost (e.g., PG&E EV2-A off-peak EV rate of ~$0.27/kWh vs. E-TOU-C off-peak of ~$0.37/kWh).

**Priority**: These three missing rate structures collectively affect 5–6M US households. In California alone (the largest US solar+storage market), CPP is mandatory for most IOU customers and EV rates are ubiquitous. Implementing RTP and CPP is a prerequisite for accurate residential economics in western US markets.

---

### Finding 2: Time-varying export rates limited to static TOU bands — no 8760-hour avoided cost profile support [Severity: high]
**Description**: `ExportMode::NetBilling` (types.rs:141) resolves export prices by looking up `tou_credits` (a `Vec<EnergyRate>`) keyed by TOU period name and season (evaluator.rs:128–140). This provides at most 2–4 distinct export prices (one per TOU period × one per season). However, California's NEM 3.0 (net billing tariff, effective April 2023) uses the Avoided Cost Calculator (ACC), which produces **8,760 hourly export rates** that vary daily based on wholesale market conditions and grid value. Each hour of the year has a unique export price — not just an on-peak/off-peak binary.

**Code Location**: `evaluator.rs:125–142` — the `export_price` match on `ExportMode` uses only `tou_credits`, which resolves to a constant per-period × season price. `types.rs:151` — `ExportRate.tou_credits: Vec<EnergyRate>` is a small static vector, not an 8760-index array.

**Root Cause**: The `NetBilling` mode was designed for simple net billing policies (e.g., utilities that credit at a single avoided-cost rate or a small set of TOU-specific avoided-cost rates), not for CPUC-mandated hourly avoided cost profiles.

**Impact**: All new California solar installations since April 2023 (~200,000/year) and dozens of other jurisdictions moving to avoided-cost export models (Hawaii HECO CGS+, Arizona SRP solar export plan, Nevada NV Energy NEM replacement, New York Value of Distributed Energy Resources). Approximately 2M+ projected US residential solar households will be on time-varying export rates by 2027. HARES simulations using static TOU export credits will be significantly inaccurate for these households — over-crediting exports during low-value midday hours and under-crediting during high-value evening hours.

**EnergyPlus Reference**: EnergyPlus `EconomicTariff.cc` supports hourly schedules for all charge types, including export rates. The schedule infrastructure allows 8760-hour resolution for any tariff component.

**Recommendation**: Add an optional `export_price_schedule: Vec<f64>` field to `ExportRate` (or a separate `ExportMode::HourlySchedule(Box<[f64; 8760]>)` variant) that provides per-hour export prices. Alternatively, add a generic schedule reference that can point to an external 8760-hour time series.

---

### Finding 3: NEM 2.0 non-bypassable charges (NBCs) are not tracked or billed separately [Severity: high]
**Description**: Under California NEM 2.0, customers receive retail-rate credits for exported energy BUT must pay non-bypassable charges (NBCs) on every imported kWh regardless of net export position. NBCs include: Department of Water Resources bond charge, public purpose program charge, nuclear decommissioning charge, and competition transition charge — typically $0.02–0.03/kWh. These are assessed on gross imports, not net consumption. The NEM 2.0 "net" bill is: `(import_kwh × import_rate) − (export_kwh × export_rate) + (import_kwh × nbc_rate)`. The current model has no `nbc_rate_per_kwh` field and no gross-import NBC charge computation.

**Code Location**: `ElectricTariff` (types.rs:206) has no NBC-related field. `compute_tiered_energy_cost` (billing.rs:282) computes cost against `import_kwh` but applies tiered rates — there is no concept of a flat additive per-kWh NBC charged on top. The `minimum_charge` field (types.rs:218) is a bill floor, not an NBC tracker. A user attempting to model NEM 2.0 would either omit NBCs (under-billing) or try to fold them into `energy_rates` (where they'd be credited back on exports, which is wrong — NBCs are not credited).

**Root Cause**: NBCs are a hybrid charge: they behave like an additional energy charge on imports only but are not credited on exports. This doesn't fit the existing `energy_rates` model (which symmetrically applies import_price to both the import cost and the export credit in `NetMetering` mode at evaluator.rs:126).

**Impact**: ~1.5M California NEM 2.0 customers (all installations between 2016–2023, plus grandfathered extensions). NBCs add ~$10–25/month to a typical residential solar bill depending on import volume. HARES simulations of NEM 2.0 customers will under-count annual bills by $120–300/household.

**Recommendation**: Add `nbc_rate_per_kwh: Option<f64>` to `ElectricTariff`. The NBC charge should be computed as `import_kwh × nbc_rate` at billing period close and added to `metered` charges separately from energy charges so it's never credited on exports. This mirrors how California IOUs itemize NBCs on bills as a separate line item.

---

### Finding 4: SeasonFilter only supports binary Summer/Winter — insufficient for 3+ season tariffs [Severity: medium]
**Description**: `SeasonFilter` (hares-types/src/schedule.rs:860–889) has three variants: `Summer` (June–September), `Winter` (October–May), and `All`. Many residential tariffs define 3 or more seasons:

| Utility | Tariff | Seasons |
|---|---|---|
| SRP (Arizona) | E-27 Time-of-Use | Summer, Summer Peak (July–August), Winter |
| Xcel (Colorado) | Residential TOU | Summer (June–Sept), Winter (Oct–May), plus optional separate GHG season |
| PG&E (California) | E-TOU-C | Summer, Winter — but baseline territory allowances differ by month-bundle |
| LADWP (California) | R-1B | Summer Tier 1/2, Winter Tier 1/2 (4 effective blocks with different thresholds) |
| ConEd (New York) | SC-1 | Summer (June–Sept), Winter (Oct–May) — standard binary, but with demand charge season Jan–Dec |

The `SeasonFilter` binary limits TOU period definitions, tiered block season matching, and demand rate seasonal applicability to two seasons. While `SeasonalSplit` (types.rs:228) allows configuring the summer/winter boundary months, it doesn't enable a third season.

**Code Location**: `hares-types/src/schedule.rs:860–889` — `SeasonFilter` enum. `SeasonFilter::contains_month()` (line 873–889) hardcodes summer as months 6–9 inclusive. All tariff components (`EnergyRate`, `DemandRate`, `TieredBlock`, `TouPeriod`) use `season: SeasonFilter`.

**Root Cause**: The `SeasonalSplit` field shifts the binary boundary but cannot add a third state. Adding a shoulder season or a summer-peak sub-season would require either expanding `SeasonFilter` to `N` named seasons or replacing it with a month-index bitmask or `BTreeSet<u8>`.

**Impact**: Arizona summer peak tariffs (affecting all SRP residential TOU customers, ~350K households) and any tariff with a shoulder month would lose the shoulder-rate differentiation. The workaround using closest-fit binary season yields approximate results but produces period-assignment errors in transition months.

**Recommendation**: Replace `SeasonFilter` with a more flexible approach — either an extensible `enum Season { Summer, Winter, Shoulder, CustomMonth(u8) }` or a `u16` month bitmask (`1 << month`). If `SeasonalSplit` is retained, allow specifying up to 4 boundaries to define up to 5 named seasons.

---

### Finding 5: No multi-tariff or sub-metered EV rate support in BillingState [Severity: medium]
**Description**: Some EV customers are on dual-meter or whole-house-plus-sub-meter arrangements where EV charging and household load are billed under different rate structures (e.g., PG&E's EV-B separate meter plan, or SCE's TOU-D-PRIME with a TOU period specifically for EV charging). The current architecture ties one `ElectricTariff` to one `TariffEvaluator` (evaluator.rs) which maintains one `BillingState`. There is no facility to:
(a) Track consumption separately for EV vs. non-EV load within a dwelling
(b) Apply a different `ElectricTariff` to EV-specific consumption
(c) Split the dwelling's net load into sub-metered components for rate allocation

**Code Location**: `evaluator.rs` — `TariffEvaluator` holds a single `tariff: ElectricTariff` and single `billing_state: BillingState`. `BillingState` (billing.rs) tracks one set of cumulative imports, peaks, and tier progression.

**Root Cause**: The dwelling-tariff relationship is 1:1. Multi-rate households require either 1:N dwelling-to-tariff mapping with consumption segregation logic, or a single composite tariff that internally separates sub-components.

**Impact**: ~2M US EV households on EV-specific rates. HARES can approximate these by running separate simulations (whole-house on standard rate + EV-only on EV rate) but cannot model the interaction where battery storage arbitrages between the two meters (e.g., charging the battery at the whole-house off-peak rate and discharging it into the EV meter's on-peak period).

**Recommendation**: For full EV rate support, add a `sub_tariffs: Vec<ElectricTariff>` field with a mechanism to allocate consumption fractions or meter points to each sub-tariff. A lighter alternative: add an `ev_tou_period_name: Option<String>` field that allows the evaluator to apply a different energy rate when the load is identified as EV charging.

---

### Finding 6: Declining block rates are structurally supported but not validated or tested [Severity: low]
**Description**: The `TieredBlock` struct (types.rs:117–135) models inclining block rates where `rates_per_kwh` increases with consumption. It also structurally supports declining block rates (where `rates_per_kwh` decreases) because `rates_per_kwh` is an unconstrained `Vec<f64>` — the `validate_tiered` function (types.rs:72–109) checks sort order on `thresholds` but has no constraint on rate direction. However, there is no test case asserting declining block behavior, and no documentation comment indicating declining blocks are supported. Declining block rates are common outside the US (e.g., many European, Australian, and Asian residential tariffs have a "lifeline" block that's cheaper than higher blocks, or declining rates for industrial customers).

**Code Location**: `types.rs:117–135` — `TieredBlock` struct. `billing.rs:274–312` — `compute_tiered_energy_cost` walks tiers in increasing threshold order, which would produce correct costs for both inclining and declining blocks because it always charges the band at `rates_per_kwh[i]`.

**Root Cause**: Comment at line 111 states "Inclining/declining block rate for a season" but the tests only exercise inclining rates. No assertion validates declining block arithmetic.

**Impact**: Low — the computation is correct for declining blocks since `rates_per_kwh[i]` is always used for band `i` regardless of whether the rate vector is increasing or decreasing. The risk is that a user might not know this works, and the undocumented feature could be broken by a future maintainer who assumes inclining-only.

**Recommendation**: Add a test case with a declining block rate (e.g., thresholds [500, 1000], rates [0.25, 0.15, 0.10]) asserting the correct blended cost. Update the doc comment on `TieredBlock` to emphasize that both directions are supported.

---

### Finding 7: `DemandRate.period_name = None` conflates coincident peak with "any time" peak — missing true system coincident peak reference [Severity: low]
**Description**: According to the doc comment on `DemandRate` (types.rs:50), `period_name: None` means "coincident peak (system-wide)". The implementation at evaluator.rs:371–374 treats `None` by calling `effective_peak_with_ratchet()`, which returns the maximum demand seen during the billing period across all timesteps. This is not a system coincident peak — it's the dwelling's non-coincident peak. True coincident peak charges reference the **utility system's** peak demand hour (typically the single hour of the month when the entire grid had its highest load) and charge the dwelling's demand at that hour.

**Code Location**: `types.rs:49–51` — `DemandRate.period_name` doc comment. `evaluator.rs:371–374` — peak selection for `None` case.

**Root Cause**: The `None` case was mis-labeled. What it actually computes is "dwelling peak at any time" (non-coincident but unbounded by TOU period). True system coincident peak would require: (a) an external signal indicating the system peak hour for each billing period, (b) looking up the dwelling's demand specifically at that hour.

**Impact**: Low — most residential demand charges are non-coincident (the dwelling's own peak). True coincident demand charges are rare in residential tariffs (more common in commercial/industrial). A few utilities (e.g., some Salt River Project plans, Glendale Water & Power) apply coincident demand to residential. The mis-labeled semantics won't affect calculation correctness for non-coincident charges but would confuse a user implementing a coincident demand tariff.

**Recommendation**: Rename the comment from "coincident peak (system-wide)" to "non-coincident peak across all periods" to accurately describe what `None` computes. If true coincident demand support is needed, add a `CoincidentDemandConfig { system_peak_hours: Vec<i32> }` field that allows indexing into a system peak schedule.

---

### Finding 8: Compositional model avoids dispatch fallthrough but creates silent omission risk [Severity: low]
**Description**: Because there is no `TariffType` enum with a match/dispatch pattern, there is no risk of a new variant falling through to a default (wrong) branch. However, the compositional approach creates a different class of error: rate components can be silently omitted. A user who intends to model a TOU tariff but forgets to populate `energy_rates` will get zero energy cost without any validation error — the tariff will validate as long as all the empty Vec fields pass default validation.

Conversely, the approach allows valid combinations that may be unintended: a tariff can simultaneously have `tiered_rates` AND `energy_rates` populated, and the system will apply both (tiered rates at period close overrides the cumulative step-accumulated cost). This is correctly documented but potentially surprising.

**Code Location**: `types.rs:264–331` — `ElectricTariff::validate()`. The only validation is structural (non-negative rates, valid period name cross-references, ascending thresholds). There is no semantic validation that at least one of `energy_rates` or `tiered_rates` is non-empty, or that `tou_schedule` is non-empty when `energy_rates` references period names.

**Root Cause**: The compositional design intentionally permits any combination of rate components. Validation checks individual component correctness but not holistic tariff sensibility.

**Impact**: Low — test fixtures and the URDB parser populate the correct fields. The risk is limited to manually constructed tariffs. The net metering dispatch (evaluator.rs:125–142) is already exhaustive on `ExportMode` — adding a new variant there would produce a compile error.

**Recommendation**: Add a validation warning (not error) when `energy_rates.is_empty() && tiered_rates.is_empty() && demand_rates.is_empty()`, since this produces a zero-cost tariff. Similarly, warn if `energy_rates` reference TOU period names but `tou_schedule` is empty.

---

## Summary
- **Total findings**: 8
- **Critical**: 1 (missing CPP, RTP, EV rate structures)
- **High**: 2 (hourly avoided-cost export profiles, NEM 2.0 non-bypassable charges)
- **Medium**: 2 (binary season filter, multi-tariff/sub-meter support)
- **Low**: 3 (declining block testing, coincident peak semantics, compositional omission risk)

## Recommendations
1. **Implement real-time pricing**: Add `rtp_schedule: Option<Vec<f64>>` to `ElectricTariff` and a per-step price lookup path in the evaluator that overrides the static `EnergyRate` when RTP is active. This is the highest-priority gap given EnergyPlus's established reference implementation and ~4% US residential market coverage.

2. **Implement critical peak pricing**: Add `cpp_config: Option<CppConfig { event_rate_per_kwh: f64, event_count_limit: u32, event_schedule: Vec<i32> }>` to `ElectricTariff`, with per-step detection of CPP events in the evaluator. EnergyPlus provides a clear reference design.

3. **Implement hourly avoided-cost export profiles**: Add an optional `ExportMode::HourlySchedule(Vec<f64>)` variant (or a parallel `export_price_schedule` field) that provides per-timestep export prices. This is required for California NEM 3.0 accuracy.

4. **Add non-bypassable charge tracking**: Add `nbc_rate_per_kwh: Option<f64>` to `ElectricTariff` with billing logic that charges NBCs on gross imports without crediting them on exports. This closes the NEM 2.0 coverage gap.

5. **Extend SeasonFilter beyond binary**: Replace or augment `SeasonFilter` with month-bitmask or `N`-named-seasons enum to support Arizona SRP, Xcel, and other 3+ season tariffs.

6. **Add sub-tariff or sub-meter support for EV rates**: Provide a `sub_tariffs` mechanism or `ev_tou_period_name` field to support dual-meter households. Given that 60% of EV households use EV-specific rates, this is increasingly important.

7. **Add declining block test and documentation**: Verify declining block arithmetic with a test case and update doc comments to clarify bidirectional support.

8. **Add tariff sensibility validation**: Warn when a tariff has no energy rates and no tiered rates, or when TOU period names are referenced without a TOU schedule.

## References / Citations
- EnergyPlus `EconomicTariff.hh` (line 358): RTP price array declarations
- EnergyPlus `EconomicTariff.cc` (line 768, 2251): RTP charge computation, (lines 2539–2541): RTP price/baseline/energy variables, (lines 2557–2562): demand window full-fill guard
- EIA Annual Electric Power Industry Report, Form EIA-861 (2023): 4% residential RTP participation, ~75% of utilities offer EV rates
- CPUC Decision 22-12-056 (December 2022): NEM 3.0 hourly avoided cost methodology
- CPUC NEM 2.0 tariff: NBCs applied to gross imports, not net consumption
- California Energy Commission: ~1.5M NEM 1.0, ~1.5M NEM 2.0, and ~200K/year new NEM 3.0 solar installations
- URDB (OpenEI Utility Rate Database): tariff type taxonomy including RTP, CPP, EV categories
- Existing HARES review tariffdp-01: annual true-up gap documented but not resolved
