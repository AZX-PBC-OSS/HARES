"""The defaults surface: a caller without an explicit ``defaults`` path
gets the defaults tree shipped inside the package, never a
working-directory guess; an explicit path overrides it. The wheel's own
packaging pin is the Rust test ``wheel_carries_defaults_inside_the_package``
(``crates/hares-python/tests/packaged_defaults.rs``, ignored by default:
it runs a release wheel build)."""

from __future__ import annotations

from pathlib import Path

import pytest

from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def _build_dwelling(**kw):
    import hares

    config = hares.SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=86400,
        time_res=3600,
        warmup=None,
    )
    return hares.Dwelling.from_hpxml(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        config=config,
        **kw,
    )


def test_python_default_defaults_are_the_packaged_tree(tmp_path, monkeypatch):
    import hares

    # Run from a temporary working directory: the packaged resolution may
    # not depend on where the caller started (the entry's rule). The repo
    # tree's own `defaults` must not be resolvable from here.
    monkeypatch.chdir(tmp_path)
    assert not (Path.cwd() / "defaults").exists(), "the probe needs a clean working directory"

    packaged = hares.defaults_dir()
    assert Path(packaged).is_dir(), "the packaged defaults directory exists in the installed package"
    assert Path(packaged).is_absolute(), "the packaged defaults directory resolves absolutely"

    default_dwelling = _build_dwelling()
    explicit_dwelling = _build_dwelling(defaults=str(HARES_DEFAULTS))
    default_dwelling.initialize()
    explicit_dwelling.initialize()
    default_result = default_dwelling.simulate()
    explicit_result = explicit_dwelling.simulate()
    default_inputs = default_result.provenance.inputs
    explicit_inputs = explicit_result.provenance.inputs
    assert default_inputs is not None and explicit_inputs is not None
    assert (
        default_inputs["defaults_files_sha256"] == explicit_inputs["defaults_files_sha256"]
    ), "the packaged default's defaults digest equals the explicit path's"
    assert default_inputs["defaults_files_sha256"], "the defaults digest is recorded"


def test_an_explicit_defaults_path_overrides_the_packaged_tree(tmp_path):
    """An explicit path that does not exist is a construction error even
    though the packaged tree is available: the explicit path wins."""
    with pytest.raises(Exception, match="defaults load failed"):
        _build_dwelling(defaults=str(tmp_path / "no-such-defaults"))
