"""Acceptance tests for the pint units pint 0.25 removed from the default registry.

The vendored OCHRE reference converts infiltration pressures with
``inch_H2O_39F``, a name pint 0.25 dropped (the same quantity is still
defined as ``inch_H2O_4C``). tests/python/ochre_units.py re-registers the
name on the registry OCHRE builds; these tests pin the alias's value and
the guard that fails loudly when a pint release defines the name again,
which is the signal to delete the helper.
"""

from __future__ import annotations

import re

import pint
import pytest
from ochre_units import register_removed_units

# 0.0254 m * 999.972 kg/m^3 * 9.80665 m/s^2: the value pint 0.24.4 gives
# for both inch_H2O_39F and inch_H2O_4C.
EXPECTED_PA = 0.0254 * 999.972 * 9.80665


def test_inch_h2o_39f_matches_the_pint_0_24_definition() -> None:
    ureg = pint.UnitRegistry()
    register_removed_units(ureg)
    actual = ureg.Quantity(1, "inch_H2O_39F").to("Pa").magnitude
    assert actual == pytest.approx(EXPECTED_PA, rel=1e-12)


def test_redefinition_guard_keeps_the_existing_definition() -> None:
    ureg = pint.UnitRegistry()
    ureg.define("inch_H2O_39F = pascal")
    existing = ureg.Quantity(1, "inch_H2O_39F").to("Pa").magnitude
    assert existing == pytest.approx(1.0)

    with pytest.raises(
        RuntimeError, match=rf"inch_H2O_39F.*{re.escape(pint.__version__)}"
    ):
        register_removed_units(ureg)

    assert ureg.Quantity(1, "inch_H2O_39F").to("Pa").magnitude == existing
