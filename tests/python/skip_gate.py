"""``--fail-on-skip``: a run that skips any selected test or module fails.

CI's OCHRE comparison job passes it, so an OCHRE install that cannot run
the comparison cannot pass as a run of skips. Skips are counted from the
test and collection reports themselves, so the gate does not depend on the
terminal reporter; under xdist the controller counts the reports its
workers forward, and the workers leave the decision to it.
tests/python/conftest.py re-exports the two hooks below.
"""

from __future__ import annotations

import sys

import pytest

__all__ = ["pytest_addoption", "pytest_configure"]


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--fail-on-skip",
        action="store_true",
        help="fail the run when any selected test or module is skipped",
    )


def pytest_configure(config: pytest.Config) -> None:
    is_xdist_worker = hasattr(config, "workerinput")
    if config.getoption("--fail-on-skip") and not is_xdist_worker:
        config.pluginmanager.register(SkipGate(config), "fail-on-skip")


class SkipGate:
    def __init__(self, config: pytest.Config) -> None:
        self.config = config
        self.skipped: list[str] = []

    def _record(self, report: pytest.CollectReport | pytest.TestReport) -> None:
        # An xfail is reported with outcome "skipped" and a wasxfail reason.
        if report.skipped and not hasattr(report, "wasxfail"):
            self.skipped.append(report.nodeid)

    def pytest_collectreport(self, report: pytest.CollectReport) -> None:
        self._record(report)

    def pytest_runtest_logreport(self, report: pytest.TestReport) -> None:
        self._record(report)

    @pytest.hookimpl(trylast=True)
    def pytest_sessionfinish(self, session: pytest.Session, exitstatus: int) -> None:
        if not self.skipped or exitstatus not in (
            pytest.ExitCode.OK,
            pytest.ExitCode.NO_TESTS_COLLECTED,
        ):
            return
        message = (
            f"--fail-on-skip: {len(self.skipped)} skipped: {', '.join(self.skipped)}"
        )
        reporter = self.config.pluginmanager.get_plugin("terminalreporter")
        if reporter is not None:
            reporter.write_line(message, red=True)
        else:
            print(message, file=sys.stderr)
        session.exitstatus = pytest.ExitCode.TESTS_FAILED
