"""Register the pint units pint 0.25 removed that the vendored OCHRE needs.

OCHRE's infiltration model converts with ``inch_H2O_39F`` (see
vendors/OCHRE/ochre/utils/envelope.py), a name pint 0.25 dropped from the
default registry in favor of the equal-quantity ``inch_H2O_4C``. Call
``register_removed_units`` on ``ochre.utils.units.ureg`` right after
``ochre`` is imported and before any OCHRE dwelling is built.

The guard is the point: pint's registry only warns and replaces on a
redefinition, so without the check a pint release that defines the name
again would silently take the alias. Raising instead fails the OCHRE
modules loudly, which is the signal to delete this helper. Registering the
same registry twice is a no-op: the test modules share one pytest process
and one ``ochre.utils.units`` registry, and only a definition from outside
this helper is the failure signal.
"""

from __future__ import annotations

import pint

REMOVED_UNIT = "inch_H2O_39F"

_registered: list[pint.UnitRegistry] = []


def register_removed_units(ureg: pint.UnitRegistry) -> None:
    """Define ``inch_H2O_39F`` on ``ureg`` unless the registry already has it."""
    if any(ureg is registered for registered in _registered):
        return
    if REMOVED_UNIT in ureg:
        msg = (
            f"{REMOVED_UNIT} is already defined in the registry "
            f"(pint {pint.__version__}): delete tests/python/ochre_units.py, "
            "the pint default registry defines the unit again"
        )
        raise RuntimeError(msg)
    ureg.define(f"{REMOVED_UNIT} = inch_H2O_4C")
    _registered.append(ureg)
