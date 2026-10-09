"""The OCHRE weather adapter: hold hourly EPW weather at the run's step.

OCHRE's weather handling zero-order-holds every field of an hourly weather
file onto the simulation grid (pandas ``resample().ffill()``). The engine's
weather resampling has its own methods (smooth interpolation for the
continuous fields, a hold for solar and wind), with no per-field overrides:
the OCHRE comparison instead reads weather this adapter has already held, so
the engine's resampling never enters the comparison.

Each hourly record is repeated ``3600 / time_res_s`` times with the minute
field set to the end of its sub-interval (EnergyPlus's sub-hourly EPW
convention). The engine's EPW reader reads such a file in record order at
its own interval; a file whose interval equals the run's step resamples at
factor 1 (a no-op).

Use from Python as ``hold_weather(weather, time_res_s)`` and from the shell
as ``python -m hares.adapters.ochre hold <weather> <time_res_s> [output]``.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

MINUTES_PER_HOUR = 60
SECONDS_PER_HOUR = 3600

# EPW line positions, zero-indexed against ``str.splitlines``.
_DATA_PERIODS_LINE = 7  # line 8: DATA PERIODS
_FIRST_DATA_LINE = 8  # line 9: the first data record
_MINUTE_FIELD = 4  # field 5 of a data record


def _parse_hourly_records_per_hour(data_periods_line: str) -> int:
    """The source file's records-per-hour field; must be 1 (an hourly file)."""
    fields = data_periods_line.split(",")
    if fields[0].strip() != "DATA PERIODS":
        msg = f"header line 8 must be DATA PERIODS, got `{fields[0]}`"
        raise ValueError(msg)
    records_per_hour = int(fields[2])
    if records_per_hour != 1:
        msg = (
            f"the source weather file must be hourly (one record per hour), "
            f"got {records_per_hour} records per hour"
        )
        raise ValueError(msg)
    return records_per_hour


def hold_weather(weather: str | Path, time_res_s: int) -> str:
    """Hold an hourly EPW file's every field at ``time_res_s`` resolution.

    Reads ``weather`` (a path to an hourly EPW file) and returns the held
    EPW content: header lines 1 to 7 copied verbatim, line 8 stating
    ``3600 / time_res_s`` records per hour, and each hourly record written
    once per sub-interval with its minute field at the sub-interval's end.
    Every other field of every record is unchanged.

    The step must divide the hour in whole minutes (for instance 3600, 900,
    300, 60), because the engine's EPW reader accepts a held file's record
    count only when the records-per-hour divides 60.
    """
    if time_res_s <= 0:
        msg = f"time_res_s must be positive, got {time_res_s}"
        raise ValueError(msg)
    records_per_hour, remainder = divmod(SECONDS_PER_HOUR, time_res_s)
    if remainder != 0 or MINUTES_PER_HOUR % records_per_hour != 0:
        msg = (
            f"time_res_s must divide the hour in whole minutes "
            f"(3600 % {time_res_s} != 0 or {records_per_hour} records "
            f"per hour do not divide {MINUTES_PER_HOUR})"
        )
        raise ValueError(msg)
    step_minutes = time_res_s // 60

    lines = Path(weather).read_text(encoding="utf-8").splitlines()
    if len(lines) <= _FIRST_DATA_LINE:
        msg = f"EPW file `{weather}` has no data records"
        raise ValueError(msg)

    _parse_hourly_records_per_hour(lines[_DATA_PERIODS_LINE])

    held: list[str] = []
    for line in lines[:_FIRST_DATA_LINE]:
        if held and len(held) == _DATA_PERIODS_LINE:
            fields = line.split(",")
            fields[2] = str(records_per_hour)
            held.append(",".join(fields))
        else:
            held.append(line)

    for line in lines[_FIRST_DATA_LINE:]:
        if not line.strip():
            continue
        fields = line.split(",")
        for sub in range(records_per_hour):
            fields[_MINUTE_FIELD] = str((sub + 1) * step_minutes)
            held.append(",".join(fields))

    return "\n".join(held) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m hares.adapters.ochre",
        description="Hold an hourly EPW weather file at a sub-hourly step.",
    )
    sub = parser.add_subparsers(required=True)
    hold = sub.add_parser(
        "hold", help="hold every field of an hourly EPW file at the step"
    )
    hold.add_argument("weather", help="the hourly EPW file to read")
    hold.add_argument("time_res_s", type=int, help="the held file's step, in seconds")
    hold.add_argument(
        "output",
        nargs="?",
        help="where to write the held file (default: beside the input, "
        "named <stem>_ochre_hold_<time_res>s.epw)",
    )
    args = parser.parse_args(argv)

    weather = Path(args.weather)
    output = (
        Path(args.output)
        if args.output is not None
        else weather.with_name(
            f"{weather.stem}_ochre_hold_{args.time_res_s}s{weather.suffix}"
        )
    )
    output.write_text(hold_weather(weather, args.time_res_s), encoding="utf-8")
    print(output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
