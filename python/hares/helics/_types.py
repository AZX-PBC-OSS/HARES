"""Shared structural protocols and type guards for HELICS Python bindings."""

import math
from typing import Any, Protocol
import weakref

from typing_extensions import TypeIs  # typing.TypeIs is Python 3.13+


def validate_timeout(value: float, name: str) -> float:
    """Validate that ``value`` is a positive, finite timeout in seconds.

    The one guard every blocking HELICS call routes its timeout through:
    finiteness first (an infinite timeout never expires and a NaN one
    fails every comparison, so the raw ``<= 0`` guard both admits ``inf``
    and rejects ``nan`` as the wrong error class), then positivity. The
    error names the value.
    """
    try:
        result = float(value)
    except (TypeError, ValueError) as exc:
        raise TypeError(f"{name} must be a number of seconds, got {value!r}") from exc
    if not math.isfinite(result) or result <= 0.0:
        raise ValueError(f"{name} must be a positive, finite number of seconds, got {value!r}")
    return result


def is_json_object(value: object) -> TypeIs[dict[str, Any]]:
    """Narrow a decoded control payload to a JSON-object mapping."""
    return isinstance(value, dict)


class HelicsFederateInfoLike(Protocol):
    core_type: str
    core_init: str
    core_init_string: str
    property: dict[int, float]


class HelicsPublicationLike(Protocol):
    def publish(self, value: float | str) -> None: ...
    def set_info(self, info: str) -> None: ...


class HelicsSubscriptionLike(Protocol):
    double: float
    string: str

    def is_updated(self) -> bool: ...


class HelicsFederateLike(Protocol):
    def register_publication(self, key: str, type_: str) -> HelicsPublicationLike: ...
    def register_global_publication(self, key: str, type_: str) -> HelicsPublicationLike: ...
    def register_subscription(self, key: str, type_: str) -> HelicsSubscriptionLike: ...
    def set_flag_option(self, flag: int, enabled: bool) -> None: ...
    def disconnect(self) -> None: ...


class HelicsCoreLike(Protocol):
    # The wrapper's weakref.finalize that frees the core's handle; calling it
    # frees the core now and leaves nothing to free on collection.
    _finalizer: "weakref.finalize[..., object]"

    def is_connected(self) -> bool: ...


class HelicsBrokerLike(Protocol):
    def disconnect(self) -> None: ...
    def is_connected(self) -> bool: ...
    def get_address(self) -> str: ...
