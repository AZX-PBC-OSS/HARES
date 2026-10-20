"""Acceptance tests for the package rename to hares (distribution hares-py)."""

from __future__ import annotations

import importlib.metadata
import importlib.util
from pathlib import Path

import pytest


def test_import_ochre_next_fails() -> None:
    """The old package name resolves to nothing: no module, no alias, no fallback."""
    assert importlib.util.find_spec("ochre_next") is None


def test_extension_module_is_hares_hares() -> None:
    """The Rust extension module lives at hares._hares."""
    import hares._hares

    assert hares._hares.__name__ == "hares._hares"


def test_distribution_name_is_hares_py() -> None:
    """The installed distribution resolves under its new name."""
    distribution = importlib.metadata.distribution("hares-py")
    assert distribution.metadata["Name"] == "hares-py"


def test_cache_dirs_are_under_hares(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The download caches sit under <cache base>/hares, not the old name."""
    from hares.data import resstock, weather

    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
    assert weather._default_weather_cache_dir() == tmp_path / "hares" / "weather"
    assert resstock._default_cache_dir() == tmp_path / "hares" / "resstock"
