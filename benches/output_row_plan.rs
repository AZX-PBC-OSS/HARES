//! The per-step output column access: the hash lookups the step path
//! performed against the name-to-position map, against the frozen row
//! plan's index writes.
//!
//! Both sides do one step's worth of output work on the verbosity-2 schema
//! of a small single-family model: the before side fills the reused row
//! buffer the way `record_step` filled its roster-held `record_scratch`
//! and resolves every column name the step path touches (hits and misses
//! alike, the envelope gain names included); the after side begins the
//! frozen plan's row and writes the same hit slots by index. Both sides
//! observe the whole row, so no side's write work can be dead-store
//! eliminated.

use std::collections::HashMap;
use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use hares_io::{RowPlan, SchemaSurface, Selection, build_schema_for_specs};
use hares_types::ZoneId;

/// The column names the step path touches every step at verbosity 2: the
/// dwelling-level context columns plus every envelope gain name (at this
/// verbosity most resolve to nothing, and the miss is a lookup too).
const STEP_LOOKUPS: &[&str] = &[
    "Total Electric Power (kW)",
    "Total Gas Power (therms/hour)",
    "Total Reactive Power (kVAR)",
    "Indoor Temperature (C)",
    "Outdoor Dry Bulb (C)",
    "Window Transmitted Solar Gain (W)",
    "Infiltration Heat Gain - Indoor (W)",
    "Forced Ventilation Heat Gain - Indoor (W)",
    "Natural Ventilation Heat Gain - Indoor (W)",
    "Net Sensible Heat Gain - Indoor (W)",
    "Internal Heat Gain - Indoor (W)",
    "Interior LWR Exchange - Indoor (W)",
    "Opaque Surface Heat Gain - Indoor (W)",
    "Duct Loss Heat Gain - Indoor (W)",
    "Roof Heat Gain - Indoor (W)",
    "Floor Heat Gain - Indoor (W)",
    "Wall Heat Gain - Indoor (W)",
    "Window Heat Gain - Indoor (W)",
    "Internal Mass Heat Gain - Indoor (W)",
    "HVAC Heating Delivered (W)",
    "HVAC Cooling Delivered (W)",
    "Zone Air Heat Balance Residual (W)",
    "Temperature - Ground (C)",
    "Hot Water Mains Temperature (C)",
    "Scheduled Heating Setpoint (C)",
    "Scheduled Cooling Setpoint (C)",
    "Runtime Heating Setpoint (C)",
    "Runtime Cooling Setpoint (C)",
];

fn bench_output_row_plan(c: &mut Criterion) {
    let specs: Vec<hares_io::EquipmentSpec> = [
        ("Heat Pump Heater", hares_types::FuelType::Electric),
        ("Electric Water Heater", hares_types::FuelType::Electric),
        ("Battery", hares_types::FuelType::Electric),
        ("EV", hares_types::FuelType::Electric),
        ("PV", hares_types::FuelType::Electric),
        ("Clothes Washer", hares_types::FuelType::Electric),
    ]
    .into_iter()
    .map(|(name, fuel_type)| hares_io::EquipmentSpec {
        instance_name: None,
        name: name.to_string(),
        fuel_type,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_overrides: serde_json::Map::new(),
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    })
    .collect();
    let zones = vec![
        (ZoneId(1), "Indoor".to_string()),
        (ZoneId(2), "Attic".to_string()),
    ];
    let schema = build_schema_for_specs(&specs, 2, &zones);
    // The name-to-position map the step path looked up, as the freeze-time
    // builder derives it from the schema.
    let column_index: HashMap<String, usize> = schema
        .fields()
        .iter()
        .skip(1)
        .enumerate()
        .map(|(idx, field)| (field.name().clone(), idx))
        .collect();
    // The frozen plan's positions, resolved once from the same names.
    let frozen: Vec<usize> = STEP_LOOKUPS
        .iter()
        .filter_map(|name| column_index.get(*name).copied())
        .collect();

    let mut group = c.benchmark_group("output_row_plan");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    // One step's rows are consumed whole by the recorder's push, so both
    // sides observe the whole row: an xor over the value bits, every slot
    // read. Observing one slot lets the compiler drop the stores nothing
    // reads, including the fill each side's step begins with.
    fn observe(row: &[f64]) -> u64 {
        row.iter().map(|v| v.to_bits()).fold(0u64, |a, b| a ^ b)
    }

    group.bench_function("per_step_hash_lookup", |b| {
        // The old step path reused one roster-held buffer across steps
        // (`record_scratch.fill(0.0)`), so the fill is the only work the
        // before side pays for the row: no allocation.
        let mut row = vec![0.0; column_index.len()];
        b.iter(|| {
            row.fill(0.0);
            for name in STEP_LOOKUPS {
                if let Some(&idx) = column_index.get(black_box(*name)) {
                    row[idx] = 1.0;
                }
            }
            black_box(observe(&row));
        })
    });

    group.bench_function("frozen_row_plan_write", move |b| {
        let mut row_plan = RowPlan::build(&[Selection::Verbosity(2)], &SchemaSurface::new(&schema))
            .expect("row plan builds");
        b.iter(|| {
            row_plan.begin_row(0);
            let row = row_plan.row_mut();
            for &idx in &frozen {
                row[idx] = 1.0;
            }
            black_box(observe(row));
        })
    });

    group.finish();
}

criterion_group!(benches, bench_output_row_plan);
criterion_main!(benches);
