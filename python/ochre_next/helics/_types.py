"""Shared structural protocols for HELICS Python bindings."""

from __future__ import annotations

from typing import Protocol


class HelicsFederateInfoLike(Protocol):
    core_type: str
    core_init: str
    core_init_string: str
    property: dict[int, float]


class HelicsPublicationLike(Protocol):
    def publish(self, value: float) -> None: ...


class HelicsSubscriptionLike(Protocol):
    double: float
    string: str

    def is_updated(self) -> bool: ...
