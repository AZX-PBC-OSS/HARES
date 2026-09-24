# SimulationPlan: Multi-Segment Equipment Swapping

> **Depends on:** `2026-06-08-hpxml-equipment-swapping.md` (Tasks 1-3) and `2026-06-08-python-equipment-ergonomics.md` (Tasks E1-E7).

**Goal:** Simulate equipment changes mid-year while preserving building thermal state continuity — e.g., gas furnace January-June, ASHP July-December — and produce a single concatenated output.

**Architecture:** Build one `Dwelling` per time segment via `DwellingBlueprint`, transfer building shell state between segments using the existing `DwellingCheckpoint` format (skipping equipment/actor state), concatenate results. ~50 lines of Rust (`restore_building_state`), ~200 lines of Python (`SimulationPlan` orchestrator).

---

### Task S1: Add Dwelling::restore_building_state()

**Files:**
- Modify: `crates/hares-core/src/dwelling/mod.rs` (after `load_checkpoint`, around line 3833)

- [ ] **Step 1: Implement restore_building_state()**

The existing `load_checkpoint()` validates equipment and actor count match (lines 3766-3772, 3812-3817). This new method skips those checks — equipment has been freshly built and inited by `DwellingBlueprint::build()`, so only building envelope state needs transfer.

```rust
impl Dwelling {
    /// Restore building shell state from a prior-segment checkpoint.
    ///
    /// Transfers thermal envelope, humidity, fluid solver, clock, RNG, and
    /// electrical summary. Equipment and actor states are intentionally NOT
    /// restored — the current equipment set was freshly built by
    /// [`DwellingBlueprint::build()`] and already initialized via [`Equipment::init()`].
    ///
    /// This enables multi-segment simulation where equipment changes between
    /// time windows while preserving building thermal continuity.
    pub fn restore_building_state(&mut self, cp: &DwellingCheckpoint) -> Result<()> {
        if cp.format_version != CHECKPOINT_VERSION {
            return Err(HaresError::Io(format!(
                "restore_building_state: checkpoint version mismatch: file={}, expected={}",
                cp.format_version, CHECKPOINT_VERSION
            )));
        }

        // Clock
        self.clock.current_step = cp.timestep_index;

        // RNG
        let mut restored_rng = ChaCha8Rng::from_seed(cp.rng_state);
        restored_rng.set_stream(cp.rng_stream);
        restored_rng.set_word_pos(cp.rng_word_pos);
        self.rng = restored_rng;

        // Thermal solver — envelope temps, surface inputs, LWR state
        self.thermal_solver
            .restore_state(&cp.envelope_state, &cp.thermal_last_u, &cp.lwr_t_prev_c)
            .map_err(|err| HaresError::Envelope(format!(
                "restore_building_state: thermal solver restore failed: {err}"
            )))?;

        // Humidity solver
        let checkpoint_zones: HashMap<ZoneId, f64> =
            cp.humidity_states.iter().copied().collect();
        for zone in &mut self.latest_env.zones {
            let humidity = checkpoint_zones.get(&zone.id).ok_or_else(|| {
                HaresError::Io(format!(
                    "restore_building_state: checkpoint missing humidity for zone {:?}",
                    zone.id
                ))
            })?;
            self.humidity_solver.humidity_ratios.insert(zone.id, *humidity);
            zone.humidity_ratio = *humidity;
        }

        // Fluid solver — tank stratification, loop temps
        self.fluid_solver
            .restore_from_payload(&cp.fluid_states)
            .map_err(|err| HaresError::Envelope(format!(
                "restore_building_state: fluid solver restore failed: {err}"
            )))?;

        // Electrical summary
        self.prior_electrical_summary = cp.prior_electrical_summary.clone();

        // Populate latest_env from current equipment core outputs
        // so the first post-restore step sees correct initial conditions.
        self.snapshot_equipment_state();

        Ok(())
    }
}
```

- [ ] **Step 2: Add imports if needed**

`HashMap` and `ZoneId` should already be in-scope at the top of `mod.rs`. Verify:

```rust
// Already present (line ~15-20 of mod.rs):
use std::collections::HashMap;
use hares_types::ZoneId;
```

- [ ] **Step 3: Run existing checkpoint tests**

```bash
cargo test -p hares-core checkpoint --lib
cargo test -p hares-core save_checkpoint --lib
```

Existing tests must pass unchanged.

- [ ] **Step 4: Add unit test for restore_building_state**

In `crates/hares-core/src/dwelling/mod.rs` test module (or `tests/` integration test):

```rust
#[test]
fn restore_building_state_preserves_envelope_temps() {
    // 1. Build a dwelling, step it for N steps
    // 2. Save checkpoint
    // 3. Build a NEW dwelling with identical equipment
    // 4. restore_building_state(&checkpoint)
    // 5. Step both once — envelope temperatures should match within tolerance
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/hares-core/src/dwelling/mod.rs
git commit -m "feat: add Dwelling::restore_building_state for multi-segment simulation"
```

---

### Task S2: Create Python SimulationPlan orchestrator

**Files:**
- Create: `python/ochre_next/simulation_plan.py`
- Create: `tests/python/test_simulation_plan.py`

- [ ] **Step 1: Define SimulationSegment and SimulationPlan**

```python
"""Multi-segment dwelling simulation with equipment swapping between time windows.

Usage:
    plan = SimulationPlan(HPXML, SCHEDULE, WEATHER,
                          defaults_path=HARES_DEFAULTS,
                          bldg_id=42, time_res_s=300)

    plan.add_segment(
        start=datetime(2024, 1, 1), end=datetime(2024, 6, 30),
        setup=lambda bp: None,  # no changes
    )
    plan.add_segment(
        start=datetime(2024, 7, 1), end=datetime(2024, 12, 31),
        setup=lambda bp: (
            bp.remove_equipment_by_end_use(EndUse.HVAC_HEATING),
            bp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5)),
        ),
    )

    results = plan.run()  # single concatenated DataFrame
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

import polars as pl

from ochre_next import Dwelling, DwellingBlueprint, DwellingCheckpoint


@dataclass
class SimulationSegment:
    """A time window with optional equipment mutations.

    The `setup` callback receives a DwellingBlueprint after HPXML parsing
    but before build(). Mutate equipment spec list here.
    """
    start: datetime
    end: datetime
    setup: Callable[[DwellingBlueprint], None] = field(default=lambda _: None)

    @property
    def duration_s(self) -> int:
        return int((self.end - self.start).total_seconds())


class SimulationPlan:
    """Chain multiple simulation segments with building-state continuity.

    Equipment can change between segments while envelope thermal state,
    tank stratification, and humidity carry over via checkpoint transfer.
    """

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
        """Add a time window.

        Args:
            start: Segment start wall-clock time (UTC recommended).
            end: Segment end wall-clock time.
            setup: Called with a DwellingBlueprint before build().
                   Mutate equipment here (add, remove, swap).
        """
        self.segments.append(SimulationSegment(start, end, setup or (lambda _: None)))

    def run(self, *, write_output: bool = False) -> pl.DataFrame:
        """Execute all segments sequentially.

        Returns:
            Concatenated polars DataFrame of all segment results.
            Includes a 'segment' column identifying which segment each row
            belongs to.
        """
        if not self.segments:
            raise ValueError("SimulationPlan has no segments")

        all_frames: list[pl.DataFrame] = []
        checkpoint: DwellingCheckpoint | None = None

        for i, seg in enumerate(self.segments):
            # 1. Build blueprint for this segment
            bp = DwellingBlueprint.from_hpxml(
                self.hpxml, self.schedule, self.weather,
                start_time=seg.start.isoformat(),
                duration_s=seg.duration_s,
                **self.base_kwargs,
            )

            # 2. Apply segment-specific equipment mutations
            seg.setup(bp)

            # 3. Build dwelling
            dw = bp.build()

            # 4. If continuing from prior segment, restore building state
            if checkpoint is not None:
                dw.restore_building_state(checkpoint)
                # NOTE: RNG was restored, so master_seed continuity means
                # stochastic equipment (e.g. event loads) pick up where
                # the prior segment left off deterministically.

            # 5. Run
            dw.initialize()
            self._simulate_dwelling(dw, write_output=write_output)

            # 6. Collect results
            df = dw.results_df()
            if df is not None and not df.is_empty():
                df = df.with_columns(pl.lit(i).alias("segment"))
                all_frames.append(df)

            # 7. Save checkpoint for next segment
            checkpoint = dw.save_checkpoint()

        if not all_frames:
            raise RuntimeError("No simulation results produced")

        return pl.concat(all_frames)

    @staticmethod
    def _simulate_dwelling(dw: Dwelling, *, write_output: bool = False) -> None:
        """Step a dwelling to completion.
        
        HARES step() raises HaresConfigError at end-of-simulation — it never
        returns None. Use the timesteps() iterator to drive the loop.
        """
        for _ in dw.timesteps():
            dw.step()

    def save_checkpoint(self, path: str, segment_idx: int = -1) -> None:
        """Save the checkpoint from a segment for external use.

        Useful for inspection or for building custom orchestrators.
        """
        if segment_idx < 0:
            segment_idx = len(self.segments) + segment_idx
        # Checkpoint saving happens in run() — this is a placeholder
        # for the API surface. Full implementation would store checkpoints
        # during run().
        raise NotImplementedError("save_checkpoint during run() not yet implemented")
```

- [ ] **Step 2: Write SimulationPlan tests**

```python
"""Test multi-segment simulation with equipment swapping."""

from datetime import datetime
from ochre_next import DwellingBlueprint, EndUse, ASHPHeater, ASHPCooler
from ochre_next.simulation_plan import SimulationPlan

from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def test_simulation_plan_single_segment():
    """Single-segment plan == standard dwelling run."""
    plan = SimulationPlan(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS), bldg_id=42,
        time_res_s=300, duration_s=600,
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 10),
    )
    results = plan.run()
    assert results is not None
    assert len(results) > 0
    assert "segment" in results.columns


def test_simulation_plan_two_segments_same_equipment():
    """Two segments with no equipment changes should produce continuous output."""
    plan = SimulationPlan(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS), bldg_id=42,
        time_res_s=300,
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 5),
    )
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 5),
        end=datetime(2024, 1, 1, 0, 10),
    )
    results = plan.run()
    assert len(results) >= 2
    segments = results["segment"].unique().to_list()
    assert 0 in segments
    assert 1 in segments


def test_simulation_plan_swap_furnace_to_ashp():
    """Swap gas furnace for ASHP between segments."""
    plan = SimulationPlan(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS), bldg_id=42,
        time_res_s=300,
    )

    # Segment 1: original equipment
    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 0),
        end=datetime(2024, 1, 1, 0, 5),
    )

    # Segment 2: swap gas furnace -> ASHP
    def setup_segment2(bp: DwellingBlueprint) -> None:
        bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])
        bp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5))
        bp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))

    plan.add_segment(
        start=datetime(2024, 1, 1, 0, 5),
        end=datetime(2024, 1, 1, 0, 10),
        setup=setup_segment2,
    )

    results = plan.run()
    assert results is not None
    assert len(results) > 0
    # Both segments should have non-NaN power values
    power = results["net_electric_power_kw"]
    assert power.is_not_nan().all()


def test_simulation_plan_empty_segments_raises():
    """Plan with no segments raises ValueError."""
    plan = SimulationPlan(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS), bldg_id=42,
        time_res_s=300,
    )
    with pytest.raises(ValueError, match="no segments"):
        plan.run()
```

- [ ] **Step 3: Run tests**

```bash
uv run pytest tests/python/test_simulation_plan.py -v
```

- [ ] **Step 4: Commit**

```bash
git add python/ochre_next/simulation_plan.py tests/python/test_simulation_plan.py
git commit -m "feat: add SimulationPlan multi-segment orchestrator"
```

---

### Task S3: Update exports

**Files:**
- Modify: `python/ochre_next/__init__.py`

- [ ] **Step 1: Add export**

```python
from ochre_next._hares import DwellingCheckpoint
from ochre_next.simulation_plan import SimulationPlan, SimulationSegment
```

- [ ] **Step 2: Verify import**

```bash
uv run python -c "from ochre_next import SimulationPlan, DwellingCheckpoint; print('OK')"
```

- [ ] **Step 3: Commit**

```bash
git add python/ochre_next/__init__.py
git commit -m "feat: export SimulationPlan and DwellingCheckpoint"
```

---

## File Map (additions)

```
CREATED:
  python/ochre_next/simulation_plan.py     — SimulationPlan + SimulationSegment
  tests/python/test_simulation_plan.py     — Multi-segment tests

MODIFIED:
  crates/hares-core/src/dwelling/mod.rs    — +restore_building_state() (~50 lines)
  python/ochre_next/__init__.py            — +SimulationPlan, DwellingCheckpoint exports
```

---

## Key Design Decisions

| Decision | Rationale |
|---|---|
| `restore_building_state` takes `&DwellingCheckpoint`, not owned | Checkpoint is reusable across segments; caller retains ownership |
| Skip equipment state entirely | Equipment was freshly built + inited by `DwellingBlueprint::build()` |
| Skip actor state entirely | Actors change between segments; fresh init is correct |
| RNG state transfers | Ensures deterministic stochastic event timing across segment boundaries |
| `master_seed` remains constant | `derive_dwelling_rng` uses `master_seed + bldg_id`, so same seed produces same stream partitioning |
| `snapshot_equipment_state()` called after restore | Populates `latest_env` so the first post-restore `step()` reads correct equipment core outputs |
| Pure Python orchestrator | No Rust churn; `restore_building_state` is the only Rust addition |
| Polars `pl.concat()` for result merging | Already the project's data format; schema-compatible across segments |
| `segment` column in output | Identifies which time window each row belongs to |

---

## Self-Review

- [x] Building state (thermal, humidity, fluid, clock, RNG, electrical summary) carries over
- [x] Equipment NOT restored — freshly built and inited by `build()`
- [x] Actors NOT restored — actors change between segments
- [x] `snapshot_equipment_state()` called after restore to populate latest_env
- [x] Old `load_checkpoint()` unchanged — equipment count validation still applies for same-dwelling resume
- [x] `restore_building_state()` is additive, no existing API surface modified
- [x] RNG transfers deterministically so stochastic loads don't reset
- [x] Minimal Rust surface (~50 lines), majority Python

**~250 lines total, builds directly on Tasks 1-3 and E1-E7.**
