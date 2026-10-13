//! The sharing model's compile-time thread bounds: `Dwelling` is
//! `Send` (the fleet steps its dwellings with `par_iter_mut`, and the
//! Python wrapper hands a `Mutex<Dwelling>` across threads), and the
//! shared surface (the blueprint and the parsed input handles the
//! blueprint's `Arc`s carry, with the forcing table on the parsed
//! weather) is `Send + Sync` (many dwellings built from one blueprint
//! read it concurrently). `Sync` is not asserted for `Dwelling`: each
//! thread owns its dwelling.

use hares_core::{Dwelling, DwellingBlueprint};
use hares_io::{ForcingTable, ParsedDefaults, ParsedHpxml, ParsedSchedule, ParsedWeather};

const fn assert_send<T: Send>() {}
const fn assert_sync<T: Sync>() {}

#[test]
fn dwelling_and_blueprint_thread_bounds() {
    assert_send::<Dwelling>();

    assert_send::<DwellingBlueprint>();
    assert_sync::<DwellingBlueprint>();

    assert_send::<ParsedHpxml>();
    assert_sync::<ParsedHpxml>();
    assert_send::<ParsedWeather>();
    assert_sync::<ParsedWeather>();
    assert_send::<ParsedSchedule>();
    assert_sync::<ParsedSchedule>();
    assert_send::<ParsedDefaults>();
    assert_sync::<ParsedDefaults>();

    assert_send::<ForcingTable>();
    assert_sync::<ForcingTable>();
}
