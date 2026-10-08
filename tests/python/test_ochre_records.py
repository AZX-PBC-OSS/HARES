"""The margin that holds a measured OCHRE divergence to its record."""

from __future__ import annotations

import math

import pytest
from ochre_records import RECORD_REL_MARGIN, record_holds


def test_margin_is_two_percent() -> None:
    assert RECORD_REL_MARGIN == 0.02


@pytest.mark.parametrize("recorded", [50.0, -50.0])
def test_a_value_exactly_at_the_margin_holds(recorded: float) -> None:
    assert record_holds(recorded * 1.02, recorded)
    assert record_holds(recorded * 0.98, recorded)


@pytest.mark.parametrize("recorded", [50.0, -50.0])
def test_a_value_just_past_the_margin_fails(recorded: float) -> None:
    edge = abs(recorded) * RECORD_REL_MARGIN
    assert not record_holds(recorded + edge * 1.0001, recorded)
    assert not record_holds(recorded - edge * 1.0001, recorded)


def test_the_recorded_value_itself_holds() -> None:
    assert record_holds(0.8776, 0.8776)


def test_a_zero_record_holds_only_an_exact_zero() -> None:
    assert record_holds(0.0, 0.0)
    assert record_holds(-0.0, 0.0)
    assert not record_holds(1e-300, 0.0)
    assert not record_holds(-1e-300, 0.0)


@pytest.mark.parametrize(
    ("measured", "recorded"),
    [
        (math.nan, 1.0),
        (1.0, math.nan),
        (math.nan, math.nan),
        (math.inf, 1.0),
        (-math.inf, 1.0),
        (1.0, math.inf),
        (math.inf, math.inf),
        (-math.inf, -math.inf),
    ],
)
def test_a_non_finite_value_never_holds(measured: float, recorded: float) -> None:
    assert not record_holds(measured, recorded)
