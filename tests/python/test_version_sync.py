"""The version sync check against the repository and a mismatched fixture.

scripts/check_version_sync.py requires the Python package's version
(pyproject.toml [project].version) and every workspace crate's
[package].version to agree (VERSIONING.md owns the bump rules). These
tests pin both outcomes: the check passes on this repository and fails a
deliberately mismatched fixture, naming the crate that missed the bump.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
from types import ModuleType

import pytest

_SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check_version_sync.py"
_SPEC = importlib.util.spec_from_file_location("check_version_sync", _SCRIPT)
assert _SPEC is not None and _SPEC.loader is not None
check_version_sync: ModuleType = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(check_version_sync)


def test_every_workspace_crate_version_agrees_with_pyproject(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert check_version_sync.check(Path(__file__).resolve().parents[2]) is True
    captured = capsys.readouterr()
    assert "OK: every workspace crate version agrees" in captured.out


def test_a_crate_that_missed_the_bump_fails_the_check(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = tmp_path / "mismatched"
    check_version_sync.write_workspace(
        root, crate_one="0.2.0", crate_two="0.1.0", pyproject="0.2.0"
    )

    assert check_version_sync.check(root) is False
    # The failure names the crate whose version missed the bump.
    captured = capsys.readouterr()
    assert "crates/two" in captured.err
