"""Canonical OCHRE to HARES output column-name mapping, shared by the parity
and thermal-trace suites.

HARES's output schema names per-end-use aggregates
"{DisplayName} End Use Electric Power (kW)" (columns.rs
END_USE_AGGREGATE_SUFFIX) to keep them distinct from per-equipment columns;
OCHRE aggregates its equipment under its own end-use names. HARES keeps
OCHRE's per-equipment column names verbatim, so an unmapped OCHRE name
resolves to itself.
"""

from __future__ import annotations

# A str value maps one OCHRE column to one HARES column; a list value maps one
# OCHRE aggregated column to the sum of those HARES columns.
OCHRE_TO_HARES: dict[str, str | list[str]] = {
    "HVAC Heating Electric Power (kW)": "HVAC Heating End Use Electric Power (kW)",
    "HVAC Cooling Electric Power (kW)": "HVAC Cooling End Use Electric Power (kW)",
    "Water Heating Electric Power (kW)": "Water Heating End Use Electric Power (kW)",
    # OCHRE's Lighting aggregate sums its lighting equipment (Indoor plus
    # Exterior for the BEopt fixture). HARES's Lighting End Use aggregate holds
    # only the equipment its columns.rs name map tags LIGHTING, which misses
    # "Exterior Lighting" (that name falls to the Other aggregate), so this
    # mapping compares against a narrower sum; the parity suite reports the
    # gap as a strict xfail.
    "Lighting Electric Power (kW)": "Lighting End Use Electric Power (kW)",
    # OCHRE's Other aggregate sums every equipment in its "Other" end-use
    # bucket (ochre/Dwelling.py combines equipment_by_end_use; the documented
    # member list is in ochre's defaults "Variable names and units.csv": mech
    # vent, refrigerator, freezer, dishwasher, clothes washer, clothes dryer,
    # range/oven, ceiling fan, pool pump, pool heater, hot tub pump, hot tub
    # heater, well pump, television, plug loads). HARES's same-named Other End
    # Use aggregate is instead the fallback for equipment its name map does
    # not recognise (for the BEopt fixture: TV, Exterior Lighting, Clothes
    # Washer, Clothes Dryer), so the definitionally exact match is the sum of
    # the per-equipment columns HARES keeps verbatim; members the dwelling
    # has no equipment for resolve to nothing on both sides.
    "Other Electric Power (kW)": [
        "Ventilation Fan Electric Power (kW)",
        "Refrigerator Electric Power (kW)",
        "Freezer Electric Power (kW)",
        "Dishwasher Electric Power (kW)",
        "Clothes Washer Electric Power (kW)",
        "Clothes Dryer Electric Power (kW)",
        "Cooking Range Electric Power (kW)",
        "MELs Electric Power (kW)",
        "TV Electric Power (kW)",
        "Well Pump Electric Power (kW)",
        "Pool Pump Electric Power (kW)",
        "Pool Heater Electric Power (kW)",
        "Spa Pump Electric Power (kW)",
        "Spa Heater Electric Power (kW)",
        "Ceiling Fan Electric Power (kW)",
    ],
    "Temperature - Outdoor (C)": "Outdoor Dry Bulb (C)",
}


def hares_columns_for(ochre_col: str) -> str | list[str]:
    """Return the HARES output column(s) carrying an OCHRE column's quantity.

    A str is a single HARES column; a list is the set of HARES per-equipment
    columns whose sum OCHRE reports under the one aggregated name. Unmapped
    names resolve to themselves.
    """
    mapped: str | list[str] | None = OCHRE_TO_HARES.get(ochre_col)
    return ochre_col if mapped is None else mapped


def resolve_hares_kwh(ochre_col: str, hares_kwh: dict[str, float]) -> float | None:
    """Look up an OCHRE column's kWh in HARES results through the shared map.

    The map wins over a verbatim column of the same name: a mapped OCHRE name
    is resolved by its declared HARES definition. A sum-valued mapping sums
    the members the results carry, and returns None only when no member is
    present: HARES emits per-equipment columns only for equipment the
    dwelling has, and a member both simulators lack contributes nothing to
    OCHRE's bucket. A member HARES drops while OCHRE has it is caught by
    that member's own parity case, which fails loudly on the missing column.
    """
    columns = hares_columns_for(ochre_col)
    if isinstance(columns, str):
        return hares_kwh.get(columns)
    total: float | None = None
    for column in columns:
        value = hares_kwh.get(column)
        if value is None:
            continue
        total = value if total is None else total + value
    return total
