"""The machine-readable input schema's Python surface: `hares.input_schema()`
returns the schema as a dict, and the sections name every input the engine
reads."""

import json

from conftest import HARES_DEFAULTS


def test_input_schema_sections_are_present():
    import hares

    schema = hares.input_schema()
    for section in (
        "config_fields",
        "dwelling_fields",
        "settings_fields",
        "override_classes",
        "removed_keys",
        "schedule_columns",
        "unused_schedule_columns",
        "defaults_files",
        "hpxml_elements",
        "rejected_hpxml_elements",
    ):
        assert section in schema, f"the schema carries {section}"
    assert schema["config_fields"], "the config fields are listed"
    assert schema["hpxml_elements"], "the HPXML elements are listed"


def test_input_schema_config_fields_name_the_sim_config():
    import hares

    schema = hares.input_schema()
    names = {field["name"] for field in schema["config_fields"]}
    for key in ("start_time", "duration", "time_res", "reports", "master_seed"):
        assert key in names, f"the config key {key} is in the schema"


def test_input_schema_is_json_round_trip_deterministic():
    import hares

    first = json.dumps(hares.input_schema(), sort_keys=True)
    second = json.dumps(hares.input_schema(), sort_keys=True)
    assert first == second


def test_input_schema_defaults_files_equal_files_read():
    import hares
    from hares import ParsedDefaults

    parsed = ParsedDefaults(str(HARES_DEFAULTS))
    expected = parsed.read_files()
    assert expected, "the shipped defaults tree carries files"
    schema = hares.input_schema()
    assert schema["defaults_files"] == expected
