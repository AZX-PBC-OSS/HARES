"""pytest plugin running every test under ``offline_guard``.

A test marked ``network`` runs with the network open; every other test, with
the module- and session-scoped fixtures set up for it, runs refused, and fails
if it attempted anything. Code outside any test (collection, imports) runs
refused too, and an attempt there fails the session; under xdist each worker
hands its attempts to the controller, which fails the session.
"""

from __future__ import annotations

import os
from collections.abc import Generator
from contextlib import ExitStack
from typing import Any

import offline_guard
import pytest

_ATTEMPTS = pytest.StashKey[list[str]]()
_OUTSIDE_TESTS = pytest.StashKey[list[str]]()
_EXIT = pytest.StashKey[ExitStack]()
_WORKER_OUTPUT_KEY = "offline_guard_attempts_outside_tests"


def pytest_configure(config: pytest.Config) -> None:
    os.environ.update(offline_guard.child_environment())
    stack = ExitStack()
    config.stash[_OUTSIDE_TESTS] = stack.enter_context(offline_guard.scope(allow_network=False))
    config.stash[_EXIT] = stack


def pytest_unconfigure(config: pytest.Config) -> None:
    config.stash[_EXIT].close()


@pytest.hookimpl(wrapper=True)
def pytest_runtest_protocol(item: pytest.Item) -> Generator[None, object, object]:
    with offline_guard.scope(allow_network=item.get_closest_marker("network") is not None) as attempts:
        item.stash[_ATTEMPTS] = attempts
        return (yield)


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(
    item: pytest.Item, call: pytest.CallInfo[None]
) -> Generator[None, pytest.TestReport, pytest.TestReport]:
    report = yield
    attempts = item.stash.get(_ATTEMPTS, [])
    if call.when == "teardown" and attempts:
        report.outcome = "failed"
        report.longrepr = "test is not marked network but reached the network: " + "; ".join(attempts)
    return report


def pytest_sessionfinish(session: pytest.Session) -> None:
    attempts = session.config.stash[_OUTSIDE_TESTS]
    worker_output: dict[str, object] | None = getattr(session.config, "workeroutput", None)
    if worker_output is not None:
        worker_output[_WORKER_OUTPUT_KEY] = list(attempts)
    if attempts:
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


@pytest.hookimpl(optionalhook=True)
def pytest_testnodedown(node: Any, error: object) -> None:
    """On the xdist controller, take over the attempts a finished worker made outside tests."""
    worker_output: dict[str, object] = getattr(node, "workeroutput", {})
    attempts = worker_output.get(_WORKER_OUTPUT_KEY, [])
    if isinstance(attempts, list):
        node.config.stash[_OUTSIDE_TESTS].extend(str(attempt) for attempt in attempts)


def pytest_terminal_summary(terminalreporter: pytest.TerminalReporter, config: pytest.Config) -> None:
    attempts = config.stash[_OUTSIDE_TESTS]
    if attempts:
        terminalreporter.write_line(
            "code outside any test reached the network: " + "; ".join(attempts), red=True
        )
