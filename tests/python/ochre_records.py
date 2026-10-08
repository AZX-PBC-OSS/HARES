"""The margin that holds a measured OCHRE divergence to its record.

A test that records a measured divergence checks the new measurement first:
the record holds while ``|measured - recorded| <= RECORD_REL_MARGIN *
|recorded|``, the edge included, and fails as soon as the difference exceeds
it. Only the floating-point rounding of a value exactly at the edge can tip it
either way. A zero record therefore holds only an exact zero, and a NaN or
infinite measurement or record never holds.
"""

from __future__ import annotations

import math

import pytest

RECORD_REL_MARGIN = 0.02


def record_holds(measured: float, recorded: float) -> bool:
    if not (math.isfinite(measured) and math.isfinite(recorded)):
        return False
    return abs(measured - recorded) <= RECORD_REL_MARGIN * abs(recorded)


def check_record(name: str, measured: float, recorded: float) -> None:
    """Fail, outside any xfail, when a measured divergence leaves its record.

    ``pytest.fail`` raises ``Failed``, not the ``AssertionError`` the xfail
    marks expect, so a stale record fails the test instead of passing as the
    expected failure.
    """
    if not record_holds(measured, recorded):
        pytest.fail(
            f"{name}: measured {measured:.4f}, recorded {recorded:.4f} "
            f"(margin {RECORD_REL_MARGIN:.0%}); update the record and its xfail reason",
            pytrace=False,
        )
