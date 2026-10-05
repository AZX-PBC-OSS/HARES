"""``--fail-on-skip``: a run that skips any selected test or module fails.

CI's OCHRE comparison job passes it, so an OCHRE install that cannot run
the comparison cannot pass as a run of skips. The hooks are re-exported by
tests/python/conftest.py.
"""

from __future__ import annotations

import pytest


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--fail-on-skip",
        action="store_true",
        help="fail the run when any selected test or module is skipped",
    )


def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    if not session.config.getoption("--fail-on-skip"):
        return
    reporter = session.config.pluginmanager.get_plugin("terminalreporter")
    if reporter is None:
        return
    skipped = reporter.stats.get("skipped", [])
    if skipped and exitstatus in (
        pytest.ExitCode.OK,
        pytest.ExitCode.NO_TESTS_COLLECTED,
    ):
        reporter.write_line(f"--fail-on-skip: {len(skipped)} skipped", red=True)
        session.exitstatus = pytest.ExitCode.TESTS_FAILED
