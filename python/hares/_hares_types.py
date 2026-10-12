"""Runtime TypedDicts for the dict shapes the ``_hares`` extension returns.

The compiled ``_hares`` module is a Rust extension and cannot carry
TypedDicts, so the shape declarations live here and ``_hares.pyi`` imports
them instead of re-declaring them (keeping the stub from drifting from the
runtime shapes). The gym layer's per-step ``info`` payload (``StepInfo``)
lives here too so ``hares.rl`` can re-export it.

``StepResult`` uses PEP 728's ``extra_items``, which needs
``typing_extensions.TypedDict`` on the supported Pythons (the stdlib
implementation only gained it in 3.14); the whole module uses
``typing_extensions`` for consistency. Annotations are evaluated eagerly
(no PEP 563 postpone) so ``NotRequired`` keys resolve correctly in the
runtime's ``__required_keys__``/``__optional_keys__``.
"""

import datetime
from typing import Any

import numpy as np
from typing_extensions import NotRequired, Protocol, TypedDict

class StepResult(TypedDict, extra_items=float):
    """Result from ``Dwelling.step()``.

    Zone temperatures are flattened as ``"Temperature - Indoor (C)"`` for zone 0,
    and ``"Temperature - Zone_N (C)"`` for other zones. The dynamic
    per-zone keys are covered by the ``extra_items`` entry, so every
    value of the mapping is a float.
    """

    timestamp: datetime.datetime
    net_electric_power_kw: float
    hvac_heating_w: float
    hvac_cooling_w: float
    gas_power_w: float

class SteppableStepResult(StepResult):
    reactive_power_kvar: NotRequired[float]

class FleetStepEntry(TypedDict):
    """Single per-dwelling entry returned by ``SteppableFleet.step()``."""

    ok: bool
    bldg_id: int
    result: NotRequired[SteppableStepResult]
    error: NotRequired[str]

class SteppableBuildError(TypedDict):
    bldg_id: int
    error: str

class ActorTimingSummary(TypedDict):
    """One registered actor's run-total scheduler-entry time.

    ``total_s`` is the actor's share of the summary's ``actors_s`` phase
    (the interest filter, ``decide()`` and the health check), ``calls`` the
    number of ``decide()`` invocations the interest filter made. Entries
    appear in registration order.
    """

    name: str
    total_s: float
    calls: int

class ProfilingSummary(TypedDict):
    """Per-phase wall-clock split returned by ``Dwelling.profiling_summary()``.

    Seconds per phase, plus the process memory high-water mark, the
    hot-path allocation counters (each ``None`` when no measurement is
    available: the extension does not install the counting allocator),
    ``finish_s`` (the run's finalisation time, ``None`` until the run's
    finalising call has run), and ``per_actor``: the run's per-actor
    totals, one entry per registered actor in registration order.
    """

    environment_s: float
    control_s: float
    ideal_capacity_s: float
    actors_s: float
    dispatch_s: float
    equipment_s: float
    envelope_s: float
    invariants_s: float
    state_snapshot_s: float
    output_s: float
    accounting_s: float
    step_total_s: float
    finish_s: float | None
    memory_high_water_kb: int | None
    hot_path_allocations: int | None
    hot_path_alloc_violations: int | None
    per_actor: list[ActorTimingSummary]

class StepInfo(TypedDict):
    """Per-step ``info`` payload for :class:`hares.rl.DwellingGymEnv`.

    ``warning_count`` and ``warnings`` follow the same contract as the Rust
    ``batch_step`` fast path: ``warning_count`` (float) is always present and
    counts the warnings drained from the dwelling during the step (e.g.
    control signals rejected at dispatch time); ``warnings`` (list[str]) is
    present only when the count is non-zero and holds the drained messages.
    Warnings are drained exactly once per step, so a subsequent
    ``Dwelling.take_warnings()`` will not return them again.
    """

    seed: int
    timestep_index: int
    step: dict[str, Any]
    observation_bounds: dict[str, tuple[float, float]]
    initial_observation_mask: np.ndarray | None
    warning_count: float
    warnings: NotRequired[list[str]]

class ArrowStreamExportable(Protocol):
    """Any object exporting its data as an Arrow stream.

    The Arrow PyCapsule interface: the engine's result tables (a report's
    ``arrow()`` export, a fleet table's) implement it, and every Arrow
    consumer (polars, pyarrow, pandas through pyarrow) imports the stream
    with the underlying buffers shared, zero-copy.
    """

    def __arrow_c_stream__(
        self, requested_schema: object | None = None
    ) -> object:
        """The object's data as a capsule holding an Arrow C stream."""
        ...


class ArrowArrayExportable(Protocol):
    """Any object exporting its data as a single Arrow record batch.

    The Arrow PyCapsule array interface: the engine's batch callback
    delivers each report batch as one (the ``Dwelling.attach_callback``
    callable's second argument), and every Arrow consumer (polars,
    pyarrow, pandas through pyarrow) imports it with the underlying
    buffers shared, zero-copy.
    """

    def __arrow_c_array__(
        self, requested_schema: object | None = None
    ) -> tuple[object, object]:
        """The object's schema and data as capsules holding the Arrow C
        array interface."""
        ...
