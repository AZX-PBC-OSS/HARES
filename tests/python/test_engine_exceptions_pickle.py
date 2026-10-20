"""Engine exceptions survive pickling across a spawn worker boundary.

A worker process that raises an engine exception pickles it for the parent,
and the parent unpickles it by importing the exception's ``__module__`` and
looking the class up there: the module path the type carries must be the
importable extension module, or an exception raised in a worker cannot reach
its parent.
"""

from __future__ import annotations

import concurrent.futures
import multiprocessing
from typing import Any

import hares._hares as hares

_RAISED_ARGS: tuple[Any, ...] = ("boom", 42)


def _raise_engine_exception(type_name: str) -> None:
    cls = getattr(hares, type_name)
    raise cls(*_RAISED_ARGS)


def _engine_exception_types() -> list[type[BaseException]]:
    """Every exception class the extension module exports."""
    exported = [
        member
        for member in vars(hares).values()
        if isinstance(member, type) and issubclass(member, BaseException)
    ]
    assert exported, "the extension must export its engine exception types"
    return exported


def test_engine_exceptions_pickle_round_trip():
    """Each exported engine exception, raised in a spawn worker, reaches the
    parent as the very same class with the raised args."""
    ctx = multiprocessing.get_context("spawn")
    with concurrent.futures.ProcessPoolExecutor(max_workers=1, mp_context=ctx) as pool:
        for cls in _engine_exception_types():
            future = pool.submit(_raise_engine_exception, cls.__name__)
            received = future.exception(timeout=120)
            assert received is not None, f"the worker must raise {cls.__name__}"
            assert type(received) is cls, (
                f"{cls.__name__} did not survive the round trip: the parent "
                f"received {type(received).__module__}.{type(received).__qualname__}"
            )
            assert received.args == _RAISED_ARGS
