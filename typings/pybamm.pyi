"""Typed surface of the optional ``pybamm`` package used by HARES.

The published ``pybamm`` wheel ships no py.typed marker. This stub models
the electrochemical-simulation surface the battery LUT generator uses:
parameter sets, SPMe/SPM models, experiments, and time-series solutions.
"""

from collections.abc import Iterable, Sequence
from typing import Protocol

import numpy as np
from numpy.typing import NDArray


__version__: str


class Model: ...


class _HasEntries(Protocol):
    entries: NDArray[np.float64]


class Solution:
    def __getitem__(self, key: str) -> _HasEntries: ...


class ParameterValues:
    def __init__(self, parameter_set: str | dict[str, float] | None = ...) -> None: ...
    def __getitem__(self, key: str) -> float: ...
    def __setitem__(self, key: str, value: float) -> None: ...
    def keys(self) -> Iterable[str]: ...
    def update(self, values: dict[str, float], *, check_already_exists: bool = ...) -> None: ...
    def evaluate(self, symbol: object, inputs: dict[str, float] | None = ...) -> float: ...


class Experiment:
    def __init__(self, operating_conditions: Sequence[str]) -> None: ...


class Simulation:
    def __init__(
        self,
        model: Model,
        parameter_values: ParameterValues | None = ...,
        experiment: Experiment | None = ...,
    ) -> None: ...
    def solve(
        self,
        t_eval: Sequence[float] | None = ...,
        initial_soc: float | None = ...,
    ) -> Solution: ...
    solution: Solution


class _ElectrodeSOHSolver:
    def __init__(
        self,
        parameter_values: ParameterValues,
        direction: object | None = ...,
        param: object | None = ...,
        known_value: str = ...,
        options: dict[str, str] | None = ...,
    ) -> None: ...
    def get_initial_stoichiometries(self, soc: float) -> tuple[float, float]: ...
    def get_min_max_stoichiometries(self) -> tuple[float, float, float, float]: ...


class lithium_ion:
    @staticmethod
    def SPM() -> Model: ...
    @staticmethod
    def SPMe(options: dict[str, str] | None = ..., name: str = ...) -> Model: ...

    ElectrodeSOHSolver: type[_ElectrodeSOHSolver]
