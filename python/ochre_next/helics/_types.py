"""Shared structural protocols and type guards for HELICS Python bindings."""

from typing import Any, Protocol, TypeIs


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


class HelicsBrokerLike(Protocol):
    def disconnect(self) -> None: ...
    def is_connected(self) -> bool: ...
    def get_address(self) -> str: ...
