"""Multi-segment dwelling simulation with equipment swapping between time windows."""

from __future__ import annotations
from dataclasses import dataclass, field
from datetime import datetime
from typing import Callable

import polars as pl

from ochre_next._hares import Dwelling, DwellingBlueprint


@dataclass
class SimulationSegment:
    start: datetime
    end: datetime
    setup: Callable[[DwellingBlueprint], None] = field(default=lambda _: None)

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
            **extra_kwargs,
        }
        self.segments: list[SimulationSegment] = []

    def add_segment(
        self,
        start: datetime,
        end: datetime,
        setup: Callable[[DwellingBlueprint], None] | None = None,
    ) -> None:
        self.segments.append(SimulationSegment(start, end, setup or (lambda _: None)))

    def run(self) -> pl.DataFrame:
        if not self.segments:
            raise ValueError("SimulationPlan has no segments")

        all_frames: list[pl.DataFrame] = []
        checkpoint = None

        for i, seg in enumerate(self.segments):
            bp = DwellingBlueprint.from_hpxml(
                self.hpxml,
                self.schedule,
                self.weather,
                start_time=seg.start.isoformat(),
                duration_s=seg.duration_s,
                **self.base_kwargs,
            )
            seg.setup(bp)
            dw = bp.build()

            if checkpoint is not None:
                dw.restore_building_state(checkpoint)

            dw.initialize()
            for _ in dw.timesteps():
                dw.step()

            df: pl.DataFrame = dw.results()
            if df is not None and not df.is_empty():
                df = df.with_columns(pl.lit(i).alias("segment"))
                all_frames.append(df)

            checkpoint = dw.save_checkpoint()

        if not all_frames:
            raise RuntimeError("No simulation results produced")
        return pl.concat(all_frames)
