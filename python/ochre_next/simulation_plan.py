"""Multi-segment dwelling simulation with equipment swapping between time windows."""

from __future__ import annotations

import warnings
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime

import polars as pl

from ochre_next import Dwelling, DwellingBlueprint


def _noop(_bp: DwellingBlueprint) -> None:
    """No-op setup callback for SimulationSegment default."""
    pass


@dataclass
class SimulationSegment:
    start: datetime
    end: datetime
    setup: Callable[[DwellingBlueprint], None] = field(
        default=_noop, repr=False, compare=False
    )
    post_build: Callable[[Dwelling], None] | None = field(
        default=None, repr=False, compare=False
    )

    @property
    def duration_s(self) -> int:
        return int((self.end - self.start).total_seconds())


class SimulationPlan:
    def __init__(
        self,
        hpxml: str,
        schedule: str,
        weather: str,
        *,
        defaults_path: str | None = None,
        bldg_id: int = 0,
        time_res_s: int = 300,
        output_verbosity: int = 0,
        master_seed: int = 42,
        initialization_duration: int = 604800,  # 7 days
        **extra_kwargs,
    ):
        self.hpxml = hpxml
        self.schedule = schedule
        self.weather = weather
        self.base_kwargs = {
            "defaults_path": defaults_path,
            "bldg_id": bldg_id,
            "time_res_s": time_res_s,
            "output_verbosity": output_verbosity,
            "master_seed": master_seed,
            "initialization_duration": initialization_duration,
            **extra_kwargs,
        }
        self.segments: list[SimulationSegment] = []

        DwellingBlueprint.from_hpxml(
            hpxml, schedule, weather,
            start_time="2000-01-01T00:00:00",
            duration_s=time_res_s,
            **self.base_kwargs,
        )

    def add_segment(
        self,
        start: datetime,
        end: datetime,
        setup: Callable[[DwellingBlueprint], None] | None = None,
        post_build: Callable[[Dwelling], None] | None = None,
    ) -> None:
        self.segments.append(
            SimulationSegment(start, end, setup or _noop, post_build=post_build)
        )

    def run(self) -> pl.DataFrame:
        if not self.segments:
            raise ValueError("SimulationPlan has no segments")

        all_frames: list[pl.DataFrame] = []
        checkpoint = None

        for i, seg in enumerate(self.segments):
            kwargs = dict(self.base_kwargs)
            if i > 0:
                # Zero initialization_duration for segments 1+.
                # After segment 0's warmup restore_building_state transfers
                # the thermal state forward, so re-initialization is unnecessary
                # and would reset the dwelling state.
                kwargs["initialization_duration"] = 0

            bp = DwellingBlueprint.from_hpxml(
                self.hpxml,
                self.schedule,
                self.weather,
                start_time=seg.start.isoformat(),
                duration_s=seg.duration_s,
                **kwargs,
            )
            seg.setup(bp)
            dw = bp.build()

            if checkpoint is not None:
                dw.restore_building_state(checkpoint)

            if seg.post_build is not None:
                seg.post_build(dw)

            # initialize() snapshots the post-restore state as the "initial"
            # state for this segment, ensuring the first step() call sees
            # correct equipment core outputs from the transferred state.
            try:
                dw.initialize()
                for _ in dw.timesteps():
                    dw.step()
            except Exception as e:
                warnings.warn(
                    f"Segment {i} failed: {e} — re-raising so caller can "
                    "decide how to handle the failure",
                    stacklevel=2,
                )
                raise

            df: pl.DataFrame = dw.results()
            if df is not None and not df.is_empty():
                df = df.with_columns(pl.lit(i).alias("segment"))
                all_frames.append(df)
            else:
                warnings.warn(
                    f"Segment {i} produced no results — "
                    "this may indicate a simulation error",
                    stacklevel=2,
                )

            checkpoint = dw.save_checkpoint()

        if not all_frames:
            raise RuntimeError("No simulation results produced")
        return pl.concat(all_frames)
