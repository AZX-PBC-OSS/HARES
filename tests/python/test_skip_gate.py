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
        ("slow or ochre", True),
        ("not slow and not ochre", False),
        ("slow and not ochre", False),
        ("not ochre", False),
    ],
)
def test_ochre_modules_are_collected_only_when_the_selector_can_pick_them(
    markexpr: str, selected: bool
) -> None:
    assert selects_ochre(markexpr) is selected


def _run(tmp_path: Path, body: str, *args: str) -> subprocess.CompletedProcess[str]:
    (tmp_path / "pytest.ini").write_text("[pytest]\n")
    (tmp_path / "conftest.py").write_text(
        "from skip_gate import pytest_addoption, pytest_sessionfinish  # noqa: F401\n"
    )
    (tmp_path / "test_case.py").write_text(body)
    (tmp_path / "test_other.py").write_text(PASSING)
    env = {
        **os.environ,
        "PYTHONPATH": os.pathsep.join([str(HERE), os.environ.get("PYTHONPATH", "")]),
    }
    return subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-p",
            "no:cacheprovider",
            "-p",
            "no:xdist",
            str(tmp_path),
            *args,
        ],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


SKIPPING_MODULE = "import pytest\npytest.importorskip('a_module_that_does_not_exist')\n\ndef test_x():\n    pass\n"
SKIPPING_TEST = "import pytest\n\ndef test_x():\n    pytest.skip('absent')\n"
PASSING = "def test_x():\n    pass\n"


@pytest.mark.parametrize("body", [SKIPPING_MODULE, SKIPPING_TEST])
def test_fail_on_skip_fails_a_run_with_a_skip(tmp_path: Path, body: str) -> None:
    assert _run(tmp_path, body).returncode == 0
    gated = _run(tmp_path, body, "--fail-on-skip")
    assert gated.returncode == pytest.ExitCode.TESTS_FAILED, gated.stdout
    assert "--fail-on-skip: 1 skipped" in gated.stdout


def test_fail_on_skip_passes_a_run_without_a_skip(tmp_path: Path) -> None:
    assert _run(tmp_path, PASSING, "--fail-on-skip").returncode == 0


def test_fail_on_skip_fails_a_run_whose_only_module_skips(tmp_path: Path) -> None:
    gated = _run(tmp_path, SKIPPING_MODULE, "--fail-on-skip", "test_case.py")
    assert gated.returncode == pytest.ExitCode.TESTS_FAILED, gated.stdout
