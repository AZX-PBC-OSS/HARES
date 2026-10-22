"""Tests for the Python warm-up surface: the three `warmup` cases on
`SimulationConfig` (an omitted keyword is the Rust default, an explicit
`None` disables, a `WarmupConfig` sets the criteria), the removed
`initialization_duration` keyword naming its replacement, and the docs'
Warm-up section example running as written.

The Rust agreement pin (`python_and_rust_warmup_defaults_agree`) lives in
`crates/hares-python/src/py_dwelling.rs`'s tests, where the Rust
convenience constructor is callable directly.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, cast

import pytest

from conftest import HARES_DEFAULTS
from hares import SimulationConfig, WarmupConfig

START = "2023-01-01T00:00:00-07:00"

CZ4A = Path("tests/fixtures/parity/cz4a_ashp_hpwh")

_OMITTED = object()


def cz4a_kwargs(warmup: object = _OMITTED, **extra: Any) -> dict[str, Any]:
    """The cz4a_ashp_hpwh inputs from 2023-01-01 at 3600 s for 2 days. The
    `warmup` argument rides the `SimulationConfig`: a sentinel object
    omits the keyword entirely, `None` disables, a `WarmupConfig` sets
    the criteria."""
    config_kwargs: dict[str, Any] = {
        "start_time": START,
        "duration": 2 * 86_400,
        "time_res": 3600,
    }
    if warmup is not _OMITTED:
        config_kwargs["warmup"] = warmup
    return {
        "hpxml": str(CZ4A / "building.xml"),
        "schedule": str(CZ4A / "schedule.csv"),
        "weather": str(CZ4A / "weather.epw"),
        "defaults": str(HARES_DEFAULTS),
        "config": SimulationConfig(**config_kwargs),
        **extra,
    }


def test_explicit_none_disables_warmup() -> None:
    """`warmup=None` reports `Disabled` and no warm-up day runs."""
    from hares import Dwelling

    dwelling = Dwelling.from_hpxml(**cast("dict[str, Any]", cz4a_kwargs(warmup=None)))
    outcome = dwelling.health().warmup
    assert outcome.outcome == "disabled"
    assert outcome.days_run is None
    assert outcome.converged is None


def test_a_warmup_keyword_next_to_config_is_a_type_error() -> None:
    """The dwelling constructors take the config only as `config=`: a
    `warmup=` keyword next to it is a TypeError naming the config field,
    not a removed key."""
    from hares import Dwelling

    with pytest.raises(TypeError, match="config=SimulationConfig\\(warmup="):
        Dwelling.from_hpxml(**cast("dict[str, Any]", {**cz4a_kwargs(), "warmup": None}))


def test_omitted_warmup_converges_with_the_rust_defaults() -> None:
    """An omitted `warmup` keyword converges at the Rust defaults: the
    Python surface carries no copy of them."""
    from hares import Dwelling

    dwelling = Dwelling.from_hpxml(**cast("dict[str, Any]", cz4a_kwargs()))
    outcome = dwelling.health().warmup
    assert outcome.outcome == "ran"
    assert outcome.converged is True
    assert outcome.days_run is not None and outcome.days_run >= 1
    # The criteria the run used are the Rust table's defaults, read back
    # through the config's warmup getter.
    assert dwelling.config().warmup == WarmupConfig()


def test_warmup_config_sets_the_criteria() -> None:
    """A `WarmupConfig(max_days=2)` on a non-converging home stops at 2
    days with the outcome recorded."""
    from hares import Dwelling

    dwelling = Dwelling.from_hpxml(
        **cast(
            "dict[str, Any]",
            cz4a_kwargs(warmup=WarmupConfig(temperature_tolerance_c=1e-9, max_days=2)),
        )
    )
    outcome = dwelling.health().warmup
    assert outcome.outcome == "ran"
    assert outcome.converged is False
    assert outcome.days_run == 2


def test_initialization_duration_kwarg_names_warmup() -> None:
    """The removed `initialization_duration` keyword raises on
    `Dwelling.from_hpxml` and on `DwellingConfig` alike, naming
    `SimulationConfig.warmup`."""
    from hares import Dwelling, DwellingConfig

    for constructor in (Dwelling.from_hpxml, DwellingConfig):
        kwargs = cz4a_kwargs()
        kwargs["initialization_duration"] = 0
        with pytest.raises(TypeError, match="initialization_duration"):
            constructor(**cast("dict[str, Any]", kwargs))
        with pytest.raises(TypeError, match="warmup"):
            constructor(**cast("dict[str, Any]", kwargs))


def test_warmup_config_partial_construction_keeps_the_rust_defaults() -> None:
    """A partial `WarmupConfig` keeps the Rust defaults for the unset
    fields: the defaults come from the one Rust table, not a Python
    copy."""
    config = WarmupConfig(max_days=7)
    assert config.temperature_tolerance_c == 0.4
    assert config.loads_tolerance == 0.04
    assert config.min_days == 1
    assert config.max_days == 7
    assert config == WarmupConfig(
        temperature_tolerance_c=0.4,
        loads_tolerance=0.04,
        min_days=1,
        max_days=7,
    )


def test_warmup_config_rejects_out_of_range_tolerances() -> None:
    """The validation the TOML path runs runs here too."""
    with pytest.raises(ValueError, match="warmup"):
        WarmupConfig(temperature_tolerance_c=0.6)
    with pytest.raises(ValueError, match="warmup"):
        WarmupConfig(min_days=0)
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        WarmupConfig(no_such_field=1)


def test_docs_warmup_example() -> None:
    """The docs' "Warm-up" section example runs as written."""
    import re

    # The docs' example hands its frames through pandas, which ships only
    # with the ochre dependency group.
    pd = pytest.importorskip("pandas")

    docs = (Path("docs") / "python.md").read_text()
    section = re.search(r"^## Warm-up\n(.*?)(?=^## )", docs, re.S | re.M)
    assert section, 'docs/python.md carries a "Warm-up" section'
    block = re.search(r"```python\n(.*?)```", section.group(1), re.S)
    assert block, 'the "Warm-up" section carries a ```python example'
    code = block.group(1)
    code = code.replace("path/to/building.xml", str(CZ4A / "building.xml"))
    code = code.replace("path/to/schedules.csv", str(CZ4A / "schedule.csv"))
    code = code.replace("path/to/weather.epw", str(CZ4A / "weather.epw"))
    code = code.replace("path/to/HARES/defaults", str(HARES_DEFAULTS))
    env: dict[str, object] = {"__name__": "__main__", "pd": pd}
    import os

    cwd = os.getcwd()
    os.chdir(".")
    try:
        exec(compile(code, str(Path("docs") / "python.md"), "exec"), env)
    finally:
        os.chdir(cwd)
