"""The pvlib solar-override generator must not touch committed fixtures.

The generator used to write straight into tests/fixtures/freefloat/ with no
argument, so any run of it, by hand or from a verification loop, rewrote the
committed pvlib_solar_override.csv files with values from whatever pvlib was
installed, and the freefloat oracle chain depends on those exact bytes. The
contract now: a required --out directory the generator writes only into,
parsed before any third-party import, so a run without it fails before pvlib
is even looked up, and no generation run ever writes the fixtures.
"""

from __future__ import annotations

import hashlib
import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import offline_guard
import pytest
_REPO_ROOT = Path(__file__).resolve().parents[2]
_GENERATOR = _REPO_ROOT / "tests" / "python" / "generate_pvlib_solar_override.py"
_FIXTURES = _REPO_ROOT / "tests" / "fixtures" / "freefloat"


def _fixture_digests() -> dict[Path, bytes]:
    return {
        path: hashlib.sha256(path.read_bytes()).digest()
        for path in sorted(_FIXTURES.rglob("*"))
        if path.is_file()
    }


def _child_environment(first_on_path: Path) -> dict[str, str]:
    """This process's environment with ``first_on_path`` ahead of the offline guard's site directory."""
    environment = {**os.environ, **offline_guard.child_environment()}
    environment["PYTHONPATH"] = os.pathsep.join([str(first_on_path), environment["PYTHONPATH"]])
    return environment


def test_pvlib_generator_requires_an_output_directory(tmp_path: Path) -> None:
    """A no-argument run fails naming --out before pvlib, fixtures unchanged."""
    blocker = tmp_path / "no_pvlib_here"
    blocker.mkdir()
    (blocker / "pvlib.py").write_text(
        "raise RuntimeError('pvlib must not be imported by the no-argument run')\n"
    )

    before = _fixture_digests()
    assert before, "no fixture files found under tests/fixtures/freefloat/"

    result = subprocess.run(
        [sys.executable, str(_GENERATOR)],
        capture_output=True,
        text=True,
        env=_child_environment(blocker),
        check=False,
    )

    assert result.returncode != 0, f"expected failure, got 0; stderr:\n{result.stderr}"
    assert "--out" in result.stderr, f"error must name --out; stderr:\n{result.stderr}"
    assert _fixture_digests() == before, "the run touched tests/fixtures/freefloat/"


@pytest.mark.ochre
def test_pvlib_generator_fails_loudly_when_pvlib_call_fails(tmp_path: Path) -> None:
    """A failing pvlib call must exit non-zero, never fall back to zeros.

    The generator used to wrap its pvlib calls in a blind except Exception
    that wrote all-zero irradiance rows on failure, which is how an entire
    fixture could be silently replaced with zeros when the pvlib API moved
    under it. The rule now: a pvlib failure propagates, the run exits
    non-zero, and no CSV is written.

    The shim patches pvlib itself, so the test needs the ochre dependency
    group: the marker keeps it out of the dev-only default run, and a run
    that selects it without pvlib fails naming the group, never skips.
    """
    assert importlib.util.find_spec("pvlib") is not None, (
        "this test needs pvlib from the ochre dependency group (uv sync --group ochre)"
    )
    blocker = tmp_path / "shim"
    blocker.mkdir()
    # This shim is the child's sitecustomize, so it runs the offline guard's
    # first, which it would otherwise shadow.
    (blocker / "sitecustomize.py").write_text(
        "import runpy\n"
        f"runpy.run_path({str(offline_guard.SITE_DIR / 'sitecustomize.py')!r})\n"
        "import os\n"
        "import sys\n"
        "sys.path.remove(os.path.dirname(os.path.abspath(__file__)))\n"
        "import pvlib.irradiance\n"
        "def _boom(*args, **kwargs):\n"
        "    raise RuntimeError('simulated pvlib call-path failure')\n"
        "pvlib.irradiance.get_total_irradiance = _boom\n"
    )

    out_dir = tmp_path / "out"
    result = subprocess.run(
        [sys.executable, str(_GENERATOR), "--out", str(out_dir)],
        capture_output=True,
        text=True,
        env=_child_environment(blocker),
        check=False,
    )

    assert result.returncode != 0, f"expected failure, got 0; stderr:\n{result.stderr}"
    assert "simulated pvlib call-path failure" in result.stderr, (
        f"failure must propagate, not be swallowed; stderr:\n{result.stderr}"
    )
    assert not out_dir.exists() or not any(out_dir.rglob("*.csv")), (
        "no CSV may be written when the pvlib call path fails"
    )
