use std::path::PathBuf;
use serde_json::json;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_defaults() -> hares_io::defaults::DefaultsStore {
    let defaults_path = project_root().join("defaults");
    hares_io::defaults::DefaultsStore::load(&defaults_path).unwrap_or_else(|_| {
        eprintln!("WARNING: Could not load defaults from {:?}, using empty", defaults_path);
        hares_io::defaults::DefaultsStore::empty()
    })
}

#[test]
fn trace_setpoints_162486_boiler() {
    let xml_path = project_root()
        .join("tests/fixtures/resstock/2025.1/bldg0162486/home.xml");
    let xml = std::fs::read_to_string(&xml_path).expect("read XML");

    let building = hares_io::hpxml::building::parse_building(&xml).expect("parse building");

    eprintln!("=== Building 162486 ===");
    eprintln!("Building heating_weekday[0] = {:?} C", building.heating_weekday_setpoints_c.as_ref().map(|v| v[0]));
    eprintln!("Building heating_weekend[0] = {:?} C", building.heating_weekend_setpoints_c.as_ref().map(|v| v[0]));
    eprintln!("Building cooling_weekday[0] = {:?} C", building.cooling_weekday_setpoints_c.as_ref().map(|v| v[0]));
    eprintln!("Building cooling_weekend[0] = {:?} C", building.cooling_weekend_setpoints_c.as_ref().map(|v| v[0]));

    // Compute gap if both set
    if let (Some(h), Some(c)) = (&building.heating_weekday_setpoints_c, &building.cooling_weekday_setpoints_c) {
        eprintln!("Building gap = {:.6} C (should be >= 2.0)", c[0] - h[0]);
    } else {
        eprintln!("MISSING setpoints in building struct");
    }

    let defaults = load_defaults();
    let specs = hares_io::hpxml::equipment::resolve_equipment(
        &building, &defaults, &json!(null), None,
    ).expect("resolve equipment");

    for spec in &specs {
        eprintln!("\n  Spec: {} (fuel: {:?})", spec.name, spec.fuel_type);
        if let Some(ref tc) = spec.typed_config {
            match &tc.payload {
                hares_equipment::ConfigPayload::Typed { data, .. } => {
                    if let Some(sc) = data.get("heating_setpoint_c") {
                        eprintln!("    typed heating_setpoint_c: {}", sc);
                    } else {
                        eprintln!("    typed heating_setpoint_c: NONE");
                    }
                    if let Some(sc) = data.get("cooling_setpoint_c") {
                        eprintln!("    typed cooling_setpoint_c: {}", sc);
                    } else {
                        eprintln!("    typed cooling_setpoint_c: NONE");
                    }
                }
                _ => {}
            }
        }
    }
}

#[test]
fn trace_setpoints_352714_gas_furnace() {
    let xml_path = project_root()
        .join("tests/fixtures/resstock/2025.1/bldg0352714/home.xml");
    let xml = std::fs::read_to_string(&xml_path).expect("read XML");

    let building = hares_io::hpxml::building::parse_building(&xml).expect("parse building");

    eprintln!("=== Building 352714 ===");
    eprintln!("Building heating_weekday[0] = {:?} C", building.heating_weekday_setpoints_c.as_ref().map(|v| v[0]));
    eprintln!("Building cooling_weekday[0] = {:?} C", building.cooling_weekday_setpoints_c.as_ref().map(|v| v[0]));

    if let (Some(h), Some(c)) = (&building.heating_weekday_setpoints_c, &building.cooling_weekday_setpoints_c) {
        eprintln!("Building gap = {:.6} C", c[0] - h[0]);
    } else {
        eprintln!("MISSING setpoints in building struct");
    }

    let defaults = load_defaults();
    let specs = hares_io::hpxml::equipment::resolve_equipment(
        &building, &defaults, &json!(null), None,
    ).expect("resolve equipment");

    for spec in &specs {
        eprintln!("\n  Spec: {} (fuel: {:?})", spec.name, spec.fuel_type);
        if let Some(ref tc) = spec.typed_config {
            match &tc.payload {
                hares_equipment::ConfigPayload::Typed { data, .. } => {
                    if let Some(sc) = data.get("heating_setpoint_c") {
                        eprintln!("    typed heating_setpoint_c: {}", sc);
                    } else {
                        eprintln!("    typed heating_setpoint_c: NONE");
                    }
                    if let Some(sc) = data.get("cooling_setpoint_c") {
                        eprintln!("    typed cooling_setpoint_c: {}", sc);
                    } else {
                        eprintln!("    typed cooling_setpoint_c: NONE");
                    }
                }
                _ => {}
            }
        }
    }
}

#[test]
fn trace_empty_heating_type_19694() {
    let xml_path = project_root()
        .join("tests/fixtures/resstock/2025.1/bldg0019694/home.xml");
    let xml = std::fs::read_to_string(&xml_path).expect("read XML");
    let building = hares_io::hpxml::building::parse_building(&xml).expect("parse building");
    let defaults = load_defaults();
    let result = hares_io::hpxml::equipment::resolve_equipment(
        &building, &defaults, &json!(null), None,
    );
    match result {
        Ok(specs) => {
            eprintln!("Parsed OK: {} specs", specs.len());
            for s in &specs {
                eprintln!("  {} (fuel: {:?})", s.name, s.fuel_type);
            }
        }
        Err(e) => {
            eprintln!("Parse FAILED: {:?}", e);
        }
    }
}

#[test]
fn trace_cooling_type_527060() {
    let xml_path = project_root()
        .join("tests/fixtures/resstock/2025.1/bldg0527060/home.xml");
    let xml = std::fs::read_to_string(&xml_path).expect("read XML");
    let building = hares_io::hpxml::building::parse_building(&xml).expect("parse building");
    let defaults = load_defaults();
    let result = hares_io::hpxml::equipment::resolve_equipment(
        &building, &defaults, &json!(null), None,
    );
    match result {
        Ok(specs) => {
            eprintln!("Parsed OK: {} specs", specs.len());
            for s in &specs {
                eprintln!("  {} (fuel: {:?})", s.name, s.fuel_type);
            }
        }
        Err(e) => {
            eprintln!("Parse FAILED: {:?}", e);
        }
    }
}
