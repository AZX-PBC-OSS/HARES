"""Adversarial boundary probe on the solar-override Python surface.

`Dwelling.set_solar_override` (py_dwelling.rs:1560) parses per-surface
irradiance tables through `parse_solar_override` — dict/list/ndarray/
dataframe arms — whose `extract::<f64>` sites carry no `is_finite`
guard, and `EnvironmentState::set_solar_override`
(crates/hares-core/src/environment.rs:333) stores the data verbatim.
This is the documented PySAM/pvlib injection channel
(tests/python/generate_pvlib_solar_override.py) — exactly the kind of
pipeline where a NaN rides in from missing satellite data. The dwelling's
finiteness screens downstream are `#[cfg(any(debug_assertions,
feature = "check_invariants"))]` gated, so a NaN irradiance that crosses
here surfaces mid-simulation at best (a loud panic far from the cause in
debug builds) and silently poisons solar gains in release builds — the
same failure shape the actor-param boundary was fixed for, on the
boundary whose entire purpose is accepting external pipeline data.

The contract asserted: a non-finite irradiance value is rejected loudly
at the boundary that received it — never which layer raises it (the
boundary-test convention: the loud-never-silent contract is the
contract, the raising layer is implementation detail).
"""

import pytest

from conftest import make_dwelling


def _override_with(direct: float) -> dict:
    """A minimal one-surface, one-timestep override table."""
    return {0: {"direct": [direct], "diffuse": [0.0], "reflected": [0.0], "aoi": [0.0]}}


@pytest.mark.parametrize("bad", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_solar_override_fails_loudly_at_the_boundary(bad: float) -> None:
    dwelling = make_dwelling()
    with pytest.raises((TypeError, ValueError)):
        dwelling.set_solar_override(_override_with(bad))


def test_finite_solar_override_attaches() -> None:
    # Control: the rejection (if any) must be value-triggered, not a
    # blanket failure of the override path — a finite table attaches.
    dwelling = make_dwelling()
    dwelling.set_solar_override(_override_with(500.0))
    assert dwelling.has_solar_override()
