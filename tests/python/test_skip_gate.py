"""The OCHRE-module collection gate and the ``--fail-on-skip`` option."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest
from conftest import selects_ochre

HERE = Path(__file__).resolve().parent


@pytest.mark.parametrize(
    ("markexpr", "selected"),
    [
        ("", True),
        ("ochre", True),
        ("slow", True),
        ("not slow", True),
        ("slow or ochre", True),
        ("slow and ochre", True),
        ("not slow and not ochre", False),
        ("not (slow or ochre)", False),
        ("slow and not ochre", False),
        ("not ochre", False),
        # Not plain boolean marker logic: collected, so a broken OCHRE install
        # errors instead of the modules being silently left out.
        ("ochre(kind=1)", True),
        ("ochre and", True),
    ],
)
def test_ochre_modules_are_collected_only_when_the_selector_can_pick_them(
    markexpr: str, selected: bool
) -> None:
    assert selects_ochre(markexpr) is selected


SKIPPING_MODULE = (
    "import pytest\npytest.importorskip('a_module_that_does_not_exist')\n\n"
    "def test_x():\n    pass\n"
)
SKIPPING_TEST = "import pytest\n\ndef test_x():\n    pytest.skip('absent')\n"
XFAILING_TEST = "import pytest\n\n@pytest.mark.xfail(strict=True)\ndef test_x():\n    assert False\n"
PASSING = "def test_x():\n    pass\n"

SERIAL = ("-p", "no:xdist")
XDIST = ("-n", "2")
NO_TERMINAL = ("-p", "no:xdist", "-p", "no:terminal")


def _run(
    tmp_path: Path, body: str, mode: tuple[str, ...], *args: str
) -> subprocess.CompletedProcess[str]:
    (tmp_path / "pytest.ini").write_text("[pytest]\n")
    (tmp_path / "conftest.py").write_text("from skip_gate import *  # noqa: F403\n")
    (tmp_path / "test_case.py").write_text(body)
    (tmp_path / "test_other.py").write_text(PASSING)
    env = {
        **os.environ,
        "PYTHONPATH": os.pathsep.join([str(HERE), os.environ.get("PYTHONPATH", "")]),
    }
    return subprocess.run(
        [sys.executable, "-m", "pytest", "-p", "no:cacheprovider", *mode, *args],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.parametrize(
    "mode", [SERIAL, XDIST, NO_TERMINAL], ids=["serial", "xdist", "no-terminal"]
)
@pytest.mark.parametrize(
    "body", [SKIPPING_MODULE, SKIPPING_TEST], ids=["module", "test"]
)
def test_fail_on_skip_fails_a_run_with_a_skip(
    tmp_path: Path, body: str, mode: tuple[str, ...]
) -> None:
    assert _run(tmp_path, body, mode).returncode == pytest.ExitCode.OK
    gated = _run(tmp_path, body, mode, "--fail-on-skip")
    assert gated.returncode == pytest.ExitCode.TESTS_FAILED, gated.stdout + gated.stderr
    assert "--fail-on-skip: 1 skipped" in gated.stdout + gated.stderr


@pytest.mark.parametrize(
    "mode", [SERIAL, XDIST, NO_TERMINAL], ids=["serial", "xdist", "no-terminal"]
)
@pytest.mark.parametrize("body", [PASSING, XFAILING_TEST], ids=["pass", "xfail"])
def test_fail_on_skip_passes_a_run_without_a_skip(
    tmp_path: Path, body: str, mode: tuple[str, ...]
) -> None:
    gated = _run(tmp_path, body, mode, "--fail-on-skip")
    assert gated.returncode == pytest.ExitCode.OK, gated.stdout + gated.stderr


def test_fail_on_skip_fails_a_run_whose_only_module_skips(tmp_path: Path) -> None:
    gated = _run(tmp_path, SKIPPING_MODULE, SERIAL, "--fail-on-skip", "test_case.py")
    assert gated.returncode == pytest.ExitCode.TESTS_FAILED, gated.stdout
