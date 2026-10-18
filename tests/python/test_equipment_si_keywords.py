"""The equipment constructors' removed keywords name their replacements.

Every renamed parameter of `Battery`, `PV`, `EV`, `BmsMode.self_consumption`,
`ChargingStrategy.solar_surplus` and `ChargingStrategy.v2g` is a removed
key: a call carrying the old keyword fails naming the replacement's row,
and the parent's positional form raises `TypeError` (every renamed
parameter is keyword-only, so a positional call cannot silently build a
config in the wrong unit's magnitude).
"""

from typing import Any, Callable, cast

import pytest

from hares import BmsMode, Battery, ChargingStrategy, EV, PV

REMOVED_KEYWORDS: list[tuple[object, str, str]] = [
    (Battery, "capacity_kwh", "BatteryConfig.capacity_j (J)"),
    (Battery, "max_charge_kw", "BatteryConfig.max_charge_w (W)"),
    (Battery, "max_discharge_kw", "BatteryConfig.max_discharge_w (W)"),
    (Battery, "inverter_capacity_kva", "BatteryConfig.inverter_capacity_va (VA)"),
    (PV, "capacity_kw", "PvConfig.capacity_w (W)"),
    (EV, "capacity_kwh", "EvConfig.capacity_j (J)"),
    (EV, "max_charging_kw", "EvConfig.max_charging_power_w (W)"),
    (EV, "charger_capacity_kva", "EvConfig.charger_capacity_va (VA)"),
    (
        BmsMode.self_consumption,
        "surplus_deadband_kw",
        "BatteryConfig.bms_mode.SelfConsumption.surplus_deadband_w (W)",
    ),
    (
        ChargingStrategy.solar_surplus,
        "min_charge_rate_kw",
        "EvConfig.charging_strategy.SolarSurplus.min_charge_rate_w (W)",
    ),
    (
        ChargingStrategy.v2g,
        "max_export_kw",
        "EvConfig.charging_strategy.V2G.max_export_w (W)",
    ),
]


@pytest.mark.parametrize(("constructor", "keyword", "replacement"), REMOVED_KEYWORDS)
def test_removed_keyword_names_replacement(
    constructor: object, keyword: str, replacement: str
) -> None:
    """A removed keyword raises its removed-key error naming the
    replacement, never Python's bare unexpected-keyword refusal."""
    with pytest.raises(TypeError, match=replacement.replace("(", "\\(").replace(")", "\\)")):
        # The splat is the point (one parametrized builder for every row);
        # erased through a permissive alias so the checker sees one call.
        builder = cast("Callable[..., object]", _build)
        builder(constructor, {keyword: 1.0})


def _build(constructor: Any, kwargs: dict[str, Any]) -> object:
    """Calls the constructor with the removed keyword set to 1.0."""
    if constructor is Battery:
        return Battery("b", capacity_j=48600000.0, **kwargs)
    if constructor is PV:
        return PV("pv", capacity_w=5000.0, tilt=30.0, azimuth=180.0, **kwargs)
    if constructor is EV:
        return EV("ev", capacity_j=270000000.0, **kwargs)
    if constructor is BmsMode.self_consumption:
        return BmsMode.self_consumption(**kwargs)
    if constructor is ChargingStrategy.solar_surplus:
        return ChargingStrategy.solar_surplus(min_charge_rate_w=1400.0, **kwargs)
    if constructor is ChargingStrategy.v2g:
        return ChargingStrategy.v2g(min_soc=0.2, max_export_w=5000.0, price_threshold=0.25, **kwargs)
    raise AssertionError(f"no builder for {constructor}")


@pytest.mark.parametrize(
    "positional_call",
    [
        # The parent's positional forms, called through a permissive alias
        # (the call's badness is the test's subject).
        lambda: cast("Callable[..., object]", PV)("pv", 5.0, 25.0, 180.0),
        lambda: cast("Callable[..., object]", Battery)("b", 10.0),
        lambda: cast("Callable[..., object]", ChargingStrategy.v2g)(0.2, 5.0, 0.3),
        lambda: cast("Callable[..., object]", ChargingStrategy.solar_surplus)(1.0),
        lambda: cast("Callable[..., object]", BmsMode.self_consumption)(0.1, 1.0, False, 0.5),
        lambda: cast("Callable[..., object]", EV)("ev", 60.0),
    ],
)
def test_positional_calls_raise_type_error(positional_call) -> None:
    """The parent's positional form raises `TypeError`: every renamed
    parameter is keyword-only."""
    with pytest.raises(TypeError):
        positional_call()
