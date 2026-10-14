"""The OCHRE weather adapter's hold: the moved zero-order weather hold.

The engine carries no per-field resampling overrides; OCHRE comparisons read
weather this adapter has already held. These tests pin the hold's shape and
keep the committed held-weather fixtures byte-identical to what the adapter
generates, so the committed data cannot drift from the adapter.
"""

from __future__ import annotations

from pathlib import Path

from hares.adapters.ochre import hold_weather

ROOT = Path(__file__).resolve().parents[2]
PARITY_ROOT = ROOT / "tests" / "fixtures" / "parity"
HOLD_ROOT = ROOT / "tests" / "fixtures" / "ochre_hold"
DENVER_EPW = ROOT / "data" / "examples" / "USA_CO_Denver.epw"

# Every committed held file: the file, the hourly source it is held from,
# and the held step. Discovery, not a list, so a file committed without the
# adapter generating it fails the byte-equality check.
def _committed_held_files() -> list[tuple[Path, Path, int]]:
    files: list[tuple[Path, Path, int]] = []
    for case in sorted(PARITY_ROOT.iterdir()):
        source = case / "weather.epw"
        held = case / "weather_ochre_hold_60s.epw"
        if source.is_file() and held.is_file():
            files.append((held, source, 60))
    for held in sorted(HOLD_ROOT.glob("USA_CO_Denver_*s.epw")):
        time_res_s = int(held.stem.rsplit("_", 1)[1].removesuffix("s"))
        files.append((held, DENVER_EPW, time_res_s))
    return files


def test_hold_weather_holds_each_hour(tmp_path: Path) -> None:
    """cz4a's weather held at 900 s: header lines 1 to 7 byte-equal to the
    source, four records per hour, and minutes 15, 30, 45, 60. The hour's
    value occupies the records stamped from coverage start plus 30 minutes
    to the next hour's coverage start plus 30 minutes: the block's second
    half carries the hour's own record in every field but the minute, and
    the block's first half carries the previous hour's record (the file's
    first half hour carries the first record's value, OCHRE's own
    backfill of the first row)."""
    source = PARITY_ROOT / "cz4a_ashp_hpwh" / "weather.epw"
    source_lines = source.read_text(encoding="utf-8").splitlines()

    held_path = tmp_path / "weather_ochre_hold_900s.epw"
    held_path.write_text(hold_weather(source, 900), encoding="utf-8")
    held_lines = held_path.read_text(encoding="utf-8").splitlines()

    # Header lines 1 to 7 byte-equal.
    assert held_lines[:7] == source_lines[:7]

    # Line 8 states four records per hour; the rest of the line is the
    # source's.
    held_data_periods = held_lines[7].split(",")
    source_data_periods = source_lines[7].split(",")
    assert held_data_periods[0] == "DATA PERIODS"
    assert held_data_periods[2] == "4"
    assert held_data_periods[:2] == source_data_periods[:2]
    assert held_data_periods[3:] == source_data_periods[3:]

    # 8760 hourly records held four times.
    assert len(held_lines) == 8 + len(source_lines[8:]) * 4

    # The block's second half (minutes 30 and 45) carries the hour's own
    # record in every field but the minute; the first half (minutes 15
    # and 30... the block's earlier halves) carries the previous hour's
    # record, the file's first half hour the first record's value.
    for hour, source_line in enumerate(source_lines[8:]):
        source_fields = source_line.split(",")
        previous_fields = (
            source_lines[8 + hour - 1].split(",") if hour > 0 else source_fields
        )
        for sub in range(4):
            held_fields = held_lines[8 + hour * 4 + sub].split(",")
            assert held_fields[:4] == source_fields[:4]
            assert held_fields[4] == str((sub + 1) * 15)
            minute = (sub + 1) * 15
            value_source = source_fields if minute > 30 else previous_fields
            assert held_fields[5:] == value_source[5:]


def test_hold_weather_rejects_a_non_hourly_source(tmp_path: Path) -> None:
    """A source that is not hourly (here: one already held at 900 s) is an
    error, not a second hold compounding the first."""
    source = PARITY_ROOT / "cz4a_ashp_hpwh" / "weather.epw"
    once = tmp_path / "once.epw"
    once.write_text(hold_weather(source, 900), encoding="utf-8")
    try:
        hold_weather(once, 900)
    except ValueError as err:
        assert "must be hourly" in str(err)
    else:
        msg = "holding an already-held file must fail"
        raise AssertionError(msg)


def test_committed_held_weather_matches_the_adapter() -> None:
    """Every committed held file is byte-identical to what the adapter
    generates from its hourly source, so the committed data cannot drift."""
    missing = [str(held) for held, _, _ in _committed_held_files() if not held.exists()]
    assert not missing, f"committed held files absent: {missing}"
    for held, source, time_res_s in _committed_held_files():
        expected = hold_weather(source, time_res_s)
        actual = held.read_text(encoding="utf-8")
        assert actual == expected, f"{held} does not match hold_weather({source}, {time_res_s})"
