//! The HPXML elements the parser reads, declared once: every
//! element-name literal in the parser is one of these statics, the
//! accessors take them by reference, and the choice dispatches match
//! the nodes against them, so the parser cannot read an element it has
//! not declared. The input schema publishes this list.

mod table;

pub use table::*;

#[cfg(test)]
pub(crate) use table::read_recorder;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Parsing every HPXML fixture under `tests/fixtures/` records only
    /// elements in `HPXML_ELEMENTS`, and every entry of `HPXML_ELEMENTS`
    /// is read by at least one fixture or listed here with the reason no
    /// checked-in fixture carries it.
    #[test]
    fn hpxml_elements_cover_every_parser_read() {
        let declared = super::table::declared_names();
        let rejected = super::table::rejected_names();
        let mut read: HashSet<&'static str> = HashSet::new();
        let mut unparsed = Vec::new();
        let fixtures = hpxml_fixtures();
        for fixture in fixtures.iter() {
            let xml = std::fs::read_to_string(fixture).expect("fixture reads");
            let _ = super::read_recorder::take();
            match crate::hpxml::parse_hpxml_str(&xml) {
                Ok(building) => {
                    // The equipment reads run at resolve time, not in the
                    // parse: resolve every fixture's equipment so the
                    // coverage sees those reads too. Homes that cannot
                    // resolve standalone (the ResStock ones, whose runs
                    // supply data patches) still read what they can.
                    let _ = crate::hpxml::equipment::resolve_equipment(
                        &building,
                        building.climate_zone_iecc.as_deref(),
                        None,
                        &crate::DefaultsStore::empty(),
                        None,
                        &mut Vec::new(),
                    );
                }
                Err(err) => {
                    // A fixture the parser rejects is still a fixture the
                    // parser reads: the schema-validation and domain
                    // checks run after the structural reads. Only a parse
                    // that failed before the tree was built reads
                    // nothing; the tail count below keeps the coverage
                    // meaningful.
                    unparsed.push((fixture.clone(), err.to_string()));
                }
            }
            for name in super::read_recorder::take() {
                assert!(
                    declared.contains(name) || rejected.contains(name),
                    "the parser reads <{name}> in {} but the element is not \
                     declared in HPXML_ELEMENTS",
                    fixture.display(),
                );
                read.insert(name);
            }
        }
        let exempted: &[(&str, &str)] = &[
            (
                "Usage",
                "the helper test's synthetic energy element: the child_energy_kwh \
                 unit test builds it in code, and no checked-in fixture carries it",
            ),
            (
                "text",
                "the XML-1.0 normalisation pin's synthetic document element",
            ),
        ];
        // The optional reads no checked-in fixture exercises: each is an
        // optional element the resolver reads only when a document
        // declares one, and no committed fixture does. A fixture added
        // later moves its elements out of this list.
        let optional_reads: &[(&str, &str)] = &[
            (
                "Aext",
                "the AIM-2 wind-side extension factor; no fixture declares one",
            ),
            (
                "AnnualConsumptionkBtu",
                "a generator's annual fuel consumption; no fixture declares a generator",
            ),
            (
                "AnnualOutputkWh",
                "a generator's annual output; no fixture declares a generator",
            ),
            (
                "ArrayOrientation",
                "the PV array's orientation enum; the fixtures use ArrayAzimuth",
            ),
            (
                "AttachedToFloor",
                "the skylight's floor link; no fixture declares a skylight",
            ),
            (
                "AttachedToPool",
                "a spa's or load's pool link; no fixture declares a spa",
            ),
            (
                "AttachedToRoof",
                "the skylight's roof link; no fixture declares a skylight",
            ),
            (
                "BackupType",
                "a heat pump's backup kind; the fixtures declare no backup",
            ),
            (
                "BatteryCapacity",
                "an EV's battery capacity; no fixture declares an EV in HPXML",
            ),
            (
                "BranchPipingLoopLength",
                "a recirculation loop's branch length; no fixture declares recirculation",
            ),
            (
                "ChargingLevel",
                "an EV's charging level; no fixture declares an EV in HPXML",
            ),
            (
                "ElectricalPowerOutput",
                "a generator's rated output; no fixture declares a generator",
            ),
            (
                "ElectricVehicle",
                "an EV element; no fixture declares one in HPXML",
            ),
            (
                "FractionCoolingLoadServed",
                "a cooling system's served fraction; every fixture's system serves all",
            ),
            (
                "FractionHeatingLoadServed",
                "a heating system's served fraction; every fixture's system serves all",
            ),
            (
                "FractionLoadServed",
                "a dehumidifier's served fraction; every fixture's unit serves all",
            ),
            (
                "FrameFloor",
                "a frame floor; no checked-in fixture carries one",
            ),
            ("Garage", "a garage zone; no checked-in fixture carries one"),
            ("Generator", "a generator element; no fixture declares one"),
            (
                "Generators",
                "the generator container; no fixture declares one",
            ),
            (
                "HotWaterDistribution",
                "a water heater's distribution; no fixture declares one",
            ),
            (
                "InsulationWidth",
                "a foundation wall's insulation width; no fixture declares one",
            ),
            (
                "Length",
                "a duct's or geometry element's length; the fixtures give areas and heights",
            ),
            (
                "LowFlow",
                "a fixture's low-flow flag; no fixture declares one",
            ),
            (
                "MaxCapacity",
                "an AutosizingLimits element's maximum; no fixture declares limits",
            ),
            (
                "MaxChargingPower",
                "an EV's maximum charging power; no fixture declares an EV in HPXML",
            ),
            (
                "MinCapacity",
                "an AutosizingLimits element's minimum; no fixture declares limits",
            ),
            (
                "MonthlyScheduleMultipliers",
                "the prefixed extension family's base name; the fixtures carry the prefixed forms",
            ),
            (
                "Name",
                "the weather station's name; the fixtures name no station",
            ),
            (
                "Perimeter",
                "a slab's perimeter; no fixture declares a slab perimeter",
            ),
            ("PermanentSpa", "a permanent spa; no fixture declares one"),
            (
                "PipeInsulation",
                "a hot-water pipe's insulation; no fixture declares one",
            ),
            (
                "PipeRValue",
                "a hot-water pipe's insulation R-value; no fixture declares one",
            ),
            (
                "PipingLength",
                "a hot-water pipe's length; no fixture declares one",
            ),
            (
                "PortableElectricSpa",
                "a portable spa; no fixture declares one",
            ),
            (
                "Recirculation",
                "a hot-water distribution's recirculation loop; no fixture declares one",
            ),
            (
                "SetbackStartHourHeating",
                "the heating setback's start hour; no fixture declares a setback",
            ),
            (
                "SetupStartHourCooling",
                "the cooling setup's start hour; no fixture declares a setup",
            ),
            ("Skylight", "a skylight; no checked-in fixture carries one"),
            (
                "Standard",
                "a water heater's efficiency standard; no fixture declares one",
            ),
            (
                "StandbyPower",
                "a spa's standby power; no fixture declares a spa",
            ),
            (
                "SystemType",
                "a cooling system's type wrapper (the ducted/unducted form); the fixtures use the simple CoolingSystemType",
            ),
            (
                "Temperature",
                "the tank temperature element beside HotWaterTemperature; every fixture's tank declares the other form",
            ),
            (
                "TotalAnnualPowerConsumptionInStandbyMode",
                "a spa's standby energy; no fixture declares a spa",
            ),
            (
                "TotalSetbackHoursperWeekHeating",
                "the heating setback's hours per week; no fixture declares a setback",
            ),
            (
                "TotalSetupHoursperWeekCooling",
                "the cooling setup's hours per week; no fixture declares a setup",
            ),
            (
                "TypeOfInfiltrationLeakage",
                "the leakage-type discriminator; the fixtures use AirLeakage only",
            ),
            (
                "UnitHeightAboveGrade",
                "the unit's height above grade; the fixtures use the site's elevation",
            ),
            (
                "Version",
                "the ERI calculation's version; no fixture declares one",
            ),
            (
                "WallHeight",
                "a wall's or foundation wall's height; the fixtures give the zone heights",
            ),
            ("WaterFixture", "a water fixture; no fixture declares one"),
            (
                "WaterFixturesUsageMultiplier",
                "the water fixtures' usage multiplier; no fixture declares one",
            ),
            (
                "WaterHeaterAutosizingFactor",
                "the water heater's autosizing factor (extension); no fixture declares one",
            ),
            (
                "WeatherStation",
                "the site's weather-station link; the fixtures name no station",
            ),
            (
                "WeekdayScheduleFractions",
                "the prefixed extension family's base name; the fixtures carry the prefixed forms",
            ),
            (
                "WeekendScheduleFractions",
                "the prefixed extension family's base name; the fixtures carry the prefixed forms",
            ),
            (
                "WithinInfiltrationVolume",
                "a space's within-infiltration-volume flag; every fixture's spaces default it",
            ),
        ];
        let mut exempted: Vec<&str> = exempted.iter().map(|(name, _)| *name).collect();
        exempted.extend(optional_reads.iter().map(|(name, _)| *name));
        exempted.sort_unstable();
        let unread: Vec<&'static str> = super::HPXML_ELEMENTS
            .iter()
            .map(|element| element.name)
            .filter(|name| !read.contains(*name))
            .filter(|name| !exempted.binary_search(name).is_ok())
            .collect();
        assert!(
            unread.is_empty(),
            "elements declared but no fixture reads them; add a fixture that \
             carries each, or exempt it with the reason no checked-in fixture \
             does: {unread:?}"
        );
        assert!(
            unparsed.len() * 10 < fixtures.len(),
            "most fixtures must parse for the coverage to be meaningful: \
             {} failed (first: {})",
            unparsed.len(),
            unparsed
                .first()
                .map_or(String::new(), |(p, e)| format!("{}: {e}", p.display())),
        );
    }

    fn hpxml_fixtures() -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures"),
            &mut out,
        );
        out.sort();
        assert!(!out.is_empty(), "the fixture corpus is not empty");
        out
    }

    fn collect(dir: &PathBuf, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "xml") {
                out.push(path);
            }
        }
    }

    /// The choice dispatch's census: the parser branches on an XML
    /// node's name only through the declared choices, so a node name
    /// read for a match (bound for one with as_str, or compared with a
    /// string literal) in the hpxml module fails the census. The
    /// equipment specs' name comparisons are not element reads (their
    /// receivers are the closure parameter `s`, `spec`, `tank_spec`, or
    /// an indexed `specs` element: the canonical equipment names, not
    /// XML elements).
    #[test]
    fn the_choice_dispatch_reads_no_raw_string_name() {
        let mut sources = Vec::new();
        collect_rust_sources(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/hpxml"),
            &mut sources,
        );
        assert!(!sources.is_empty(), "the hpxml sources are present");
        let hits: Vec<String> = sources
            .iter()
            .flat_map(|path| {
                raw_string_name_reads(&std::fs::read_to_string(path).expect("source reads"))
                    .into_iter()
                    .map(move |snippet| format!("{}: {snippet}", path.display()))
            })
            .collect();
        assert!(
            hits.is_empty(),
            "a choice dispatch must name declared choices, not raw \
             strings; found {hits:?}"
        );
    }

    /// The snippets around node-name reads that bind the name for a
    /// match (the as_str form) or compare it with a string literal.
    /// Comment lines are skipped, and the lines are joined so a read
    /// wrapped across lines is still seen.
    fn raw_string_name_reads(text: &str) -> Vec<String> {
        let flattened = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut hits = Vec::new();
        for (found, _) in flattened.match_indices(".name") {
            let after = &flattened[found + ".name".len()..];
            let tail = after.trim_start();
            let compared = (tail.starts_with("==") || tail.starts_with("!="))
                && tail[2..].trim_start().starts_with('"');
            let bound_for_a_match = tail.starts_with(".as_str");
            if !(compared || bound_for_a_match) {
                continue;
            }
            let (receiver, indexed) = receiver_of(&flattened[..found]);
            if indexed || receiver == "s" || receiver == "spec" || receiver == "tank_spec" {
                continue;
            }
            hits.push(flattened[found..].chars().take(72).collect());
        }
        hits
    }

    /// The receiver of a field access: the identifier run ending right
    /// before it, and whether the access is indexed (a `specs`
    /// element's field, where the name is an equipment's, not an XML
    /// element's).
    fn receiver_of(before: &str) -> (&str, bool) {
        match before
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
            .last()
        {
            Some((start, _)) => (&before[start..], false),
            None => ("", before.trim_end().ends_with(']')),
        }
    }

    fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
}
