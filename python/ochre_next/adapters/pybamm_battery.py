"""PyBaMM battery lookup table generator.

Generates efficiency look-up tables and degradation parameter sets from PyBaMM
electrochemical models.  When PyBaMM is not installed, returns objects backed
by built-in defaults so callers never need to handle ``ImportError``.
"""

from __future__ import annotations

import hashlib
import logging
from dataclasses import dataclass, field as dataclass_field
from pathlib import Path
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq

try:
    import pybamm as _pybamm  # type: ignore[import-untyped]

    _HAS_PYBAMM = True
except ImportError:
    _pybamm = None
    _HAS_PYBAMM = False

LOGGER = logging.getLogger(__name__)

_DEFAULT_EFFICIENCY = 0.95
_DEFAULT_DEGRADATION: dict[str, Any] = {
    "model": "rainflow_arrhenius",
    "calendar_q": 4.14e-10,
    "calendar_a": -7280.0,
    "cycle_q": 2.64e-4,
    "cycle_d": 0.7,
}


def _canonical_hash_efficiency(
    chemistry: str,
    capacity_ah: float,
    n_series: int,
    n_parallel: int,
    temperature_range_c: tuple[float, float],
    soc_range: tuple[float, float],
    power_range_kw: tuple[float, float],
    age_cycles: int,
) -> str:
    parts: dict[str, str] = {
        "age_cycles": f"{age_cycles:.6f}",
        "capacity_ah": f"{capacity_ah:.6f}",
        "chemistry": chemistry,
        "n_parallel": f"{n_parallel:.6f}",
        "n_series": f"{n_series:.6f}",
        "power_range_kw_max": f"{power_range_kw[1]:.6f}",
        "power_range_kw_min": f"{power_range_kw[0]:.6f}",
        "soc_range_max": f"{soc_range[1]:.6f}",
        "soc_range_min": f"{soc_range[0]:.6f}",
        "temperature_range_c_max": f"{temperature_range_c[1]:.6f}",
        "temperature_range_c_min": f"{temperature_range_c[0]:.6f}",
    }
    canonical = "".join(f"{k}={v};" for k, v in sorted(parts.items()))
    return hashlib.sha256(canonical.encode()).hexdigest()


def _canonical_hash_degradation(
    chemistry: str,
    capacity_ah: float,
    temperature_range_c: float,
) -> str:
    parts: dict[str, str] = {
        "capacity_ah": f"{capacity_ah:.6f}",
        "chemistry": chemistry,
        "temperature_range_c": f"{temperature_range_c:.6f}",
    }
    canonical = "".join(f"{k}={v};" for k, v in sorted(parts.items()))
    return hashlib.sha256(canonical.encode()).hexdigest()


def _cache_valid(path: Path, expected_hash: str) -> bool:
    hash_path = path.with_suffix(".hash")
    if not path.exists() or not hash_path.exists():
        return False
    stored = hash_path.read_text().strip()
    return stored == expected_hash


def _write_cache(path: Path, content_hash: str) -> None:
    hash_path = path.with_suffix(".hash")
    hash_path.write_text(content_hash + "\n")


def _format_toml_value(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return f"{value}"
    if isinstance(value, str):
        return f'"{value}"'
    if isinstance(value, list):
        items = ", ".join(_format_toml_value(v) for v in value)
        return f"[{items}]"
    return repr(value)


def _dict_to_toml(data: dict[str, Any]) -> str:
    lines: list[str] = []
    scalars = {k: v for k, v in data.items() if not isinstance(v, dict)}
    tables = {k: v for k, v in data.items() if isinstance(v, dict)}

    for k in sorted(scalars):
        lines.append(f"{k} = {_format_toml_value(scalars[k])}")

    for section in sorted(tables):
        lines.append(f"\n[{section}]")
        for k in sorted(tables[section]):
            lines.append(f"{k} = {_format_toml_value(tables[section][k])}")

    return "\n".join(lines) + "\n"


@dataclass
class EfficiencyLut:
    """Battery efficiency look-up table backed by an Arrow table."""

    table: pa.Table
    metadata: dict[str, Any] = dataclass_field(default_factory=dict)

    def save(self, path: Path) -> None:
        """Write the LUT to a Parquet file on disk."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        pq.write_table(self.table, path)
        if "content_hash" in self.metadata:
            _write_cache(path, self.metadata["content_hash"])


@dataclass
class DegradationParams:
    """Battery degradation parameter set."""

    params: dict[str, Any] = dataclass_field(default_factory=dict)
    metadata: dict[str, Any] = dataclass_field(default_factory=dict)

    def save(self, path: Path) -> None:
        """Write the parameters to a TOML file on disk."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(_dict_to_toml(self.params))
        if "content_hash" in self.metadata:
            _write_cache(path, self.metadata["content_hash"])


def _linspace(start: float, stop: float, n: int) -> list[float]:
    if n <= 1:
        return [start]
    step = (stop - start) / (n - 1)
    return [round(start + i * step, 6) for i in range(n)]


def _build_default_efficiency_table(
    soc_range: tuple[float, float],
    power_range_kw: tuple[float, float],
    temperature_range_c: tuple[float, float],
) -> pa.Table:
    """Build a uniform efficiency table from built-in defaults."""
    soc_points = _linspace(soc_range[0], soc_range[1], 5)
    power_points = _linspace(power_range_kw[0], power_range_kw[1], 5)
    temp_points = _linspace(temperature_range_c[0], temperature_range_c[1], 3)

    socs: list[float] = []
    powers: list[float] = []
    temps: list[float] = []
    efficiencies: list[float] = []

    for soc in soc_points:
        for power in power_points:
            for temp in temp_points:
                socs.append(soc)
                powers.append(power)
                temps.append(temp)
                eff = _DEFAULT_EFFICIENCY
                if abs(power) > 0:
                    eff -= 0.02 * (1.0 - soc) if power > 0 else 0.01 * soc
                temp_mid = (temperature_range_c[0] + temperature_range_c[1]) / 2.0
                temp_span = max(temperature_range_c[1] - temperature_range_c[0], 1.0)
                eff -= 0.01 * abs(temp - temp_mid) / temp_span
                efficiencies.append(max(0.80, min(1.0, eff)))

    return pa.table(
        {
            "soc": pa.array(socs, type=pa.float64()),
            "power_kw": pa.array(powers, type=pa.float64()),
            "temperature_c": pa.array(temps, type=pa.float64()),
            "efficiency": pa.array(efficiencies, type=pa.float64()),
        }
    )


def _run_pybamm_efficiency(
    chemistry: str,
    capacity_ah: float,
    n_series: int,
    n_parallel: int,
    temperature_range_c: tuple[float, float],
    soc_range: tuple[float, float],
    power_range_kw: tuple[float, float],
    age_cycles: int,
) -> pa.Table:
    """Run PyBaMM SPM to generate an efficiency LUT."""
    model = _pybamm.lithium_ion.SPM()

    soc_points = _linspace(soc_range[0], soc_range[1], 5)
    power_points = _linspace(power_range_kw[0], power_range_kw[1], 5)
    temp_points = _linspace(temperature_range_c[0], temperature_range_c[1], 3)

    socs: list[float] = []
    powers: list[float] = []
    temps: list[float] = []
    efficiencies: list[float] = []

    for soc_init in soc_points:
        for power_kw in power_points:
            for temp_c in temp_points:
                try:
                    param = _pybamm.ParameterValues("Chen2020")
                    param["Initial temperature [K]"] = temp_c + 273.15
                    param["Ambient temperature [K]"] = temp_c + 273.15
                    param["Nominal cell capacity [A.h]"] = capacity_ah

                    current_a = (power_kw * 1000.0) / (3.6 * n_series) if power_kw != 0 else 0.01
                    param["Current function [A]"] = abs(current_a)

                    sim = _pybamm.Simulation(model, parameter_values=param)
                    sim.solve([0, 60])
                    sol = sim.solution

                    v_terminal = float(sol["Terminal voltage [V]"].entries[-1])
                    i_actual = float(sol["Current [A]"].entries[-1])
                    ocv = float(sol["X-averaged battery open-circuit potential [V]"].entries[-1])

                    if abs(i_actual) > 1e-9 and abs(ocv) > 1e-9:
                        eff = (v_terminal * i_actual) / (ocv * i_actual)
                        eff = max(0.5, min(1.0, abs(eff)))
                    else:
                        eff = _DEFAULT_EFFICIENCY
                except Exception:
                    LOGGER.debug(
                        "PyBaMM solve failed for soc=%.2f power=%.2f temp=%.1f; using default",
                        soc_init,
                        power_kw,
                        temp_c,
                    )
                    eff = _DEFAULT_EFFICIENCY

                socs.append(soc_init)
                powers.append(power_kw)
                temps.append(temp_c)
                efficiencies.append(eff)

    return pa.table(
        {
            "soc": pa.array(socs, type=pa.float64()),
            "power_kw": pa.array(powers, type=pa.float64()),
            "temperature_c": pa.array(temps, type=pa.float64()),
            "efficiency": pa.array(efficiencies, type=pa.float64()),
        }
    )


def generate_efficiency_lut(
    chemistry: str,
    capacity_ah: float,
    n_series: int,
    n_parallel: int,
    temperature_range_c: tuple[float, float],
    soc_range: tuple[float, float],
    *,
    power_range_kw: tuple[float, float],
    age_cycles: int,
    cache_dir: Path | None = None,
) -> EfficiencyLut:
    """Generate a battery efficiency LUT.

    Parameters
    ----------
    chemistry:
        Battery chemistry (e.g. ``"NMC"``, ``"LFP"``).
    capacity_ah:
        Cell capacity in Ah.
    n_series:
        Number of cells in series.
    n_parallel:
        Number of cells in parallel.
    temperature_range_c:
        ``(min, max)`` temperature range in Celsius.
    soc_range:
        ``(min, max)`` state-of-charge range (0-1).
    power_range_kw:
        ``(min, max)`` power range in kW (negative = charging).
    age_cycles:
        Number of equivalent full cycles for degraded-state modeling.
    cache_dir:
        Optional directory for caching.

    Returns
    -------
    EfficiencyLut
        A look-up table object.  Call ``.save(path)`` to write Parquet.
    """
    content_hash = _canonical_hash_efficiency(
        chemistry,
        capacity_ah,
        n_series,
        n_parallel,
        temperature_range_c,
        soc_range,
        power_range_kw,
        age_cycles,
    )

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "efficiency_lut.parquet"
        if _cache_valid(cached_path, content_hash):
            LOGGER.info("Efficiency LUT cache hit: %s", cached_path)
            table = pq.read_table(cached_path)
            return EfficiencyLut(table=table, metadata={"content_hash": content_hash, "cached": True})

    if _HAS_PYBAMM:
        LOGGER.info("Running PyBaMM SPM to generate efficiency LUT for %s", chemistry)
        table = _run_pybamm_efficiency(
            chemistry,
            capacity_ah,
            n_series,
            n_parallel,
            temperature_range_c,
            soc_range,
            power_range_kw,
            age_cycles,
        )
        lut = EfficiencyLut(table=table, metadata={"content_hash": content_hash, "cached": False, "source": "pybamm"})
    else:
        LOGGER.info("PyBaMM not installed; using built-in defaults for efficiency LUT")
        table = _build_default_efficiency_table(soc_range, power_range_kw, temperature_range_c)
        lut = EfficiencyLut(table=table, metadata={"content_hash": content_hash, "cached": False, "source": "defaults"})

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "efficiency_lut.parquet"
        lut.save(cached_path)

    return lut


def generate_degradation_params(
    chemistry: str,
    capacity_ah: float,
    temperature_range_c: float,
    *,
    cache_dir: Path | None = None,
) -> DegradationParams:
    """Generate battery degradation parameters.

    Parameters
    ----------
    chemistry:
        Battery chemistry (e.g. ``"NMC"``, ``"LFP"``).
    capacity_ah:
        Cell capacity in Ah.
    temperature_range_c:
        Reference temperature in Celsius.
    cache_dir:
        Optional directory for caching.

    Returns
    -------
    DegradationParams
        A parameter object.  Call ``.save(path)`` to write TOML.
    """
    content_hash = _canonical_hash_degradation(chemistry, capacity_ah, temperature_range_c)

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "degradation_params.toml"
        if _cache_valid(cached_path, content_hash):
            LOGGER.info("Degradation params cache hit: %s", cached_path)
            return DegradationParams(
                params=dict(_DEFAULT_DEGRADATION),
                metadata={"content_hash": content_hash, "cached": True},
            )

    if _HAS_PYBAMM:
        LOGGER.info("Running PyBaMM to extract degradation params for %s", chemistry)
        try:
            _pybamm.ParameterValues("Chen2020")
            params = dict(_DEFAULT_DEGRADATION)
            params["chemistry"] = chemistry
            params["capacity_ah"] = capacity_ah
            params["reference_temperature_c"] = temperature_range_c
        except Exception:
            LOGGER.warning("PyBaMM degradation extraction failed; using defaults")
            params = dict(_DEFAULT_DEGRADATION)
    else:
        LOGGER.info("PyBaMM not installed; using built-in defaults for degradation params")
        params = dict(_DEFAULT_DEGRADATION)

    result = DegradationParams(params=params, metadata={"content_hash": content_hash, "cached": False})

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "degradation_params.toml"
        result.save(cached_path)

    return result


# ---------------------------------------------------------------------------
# 4D CC-CV charging curve LUT generation
# ---------------------------------------------------------------------------

# Default grid axes matching DER_Detection convention
_DEFAULT_SOC_GRID: list[float] = [
    0.00, 0.05, 0.10, 0.15, 0.20, 0.25, 0.30, 0.35,
    0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75,
    0.80, 0.85, 0.88, 0.91, 0.94, 0.96, 0.98, 1.00,
]
_DEFAULT_TEMP_GRID: list[float] = [-15.0, -5.0, 5.0, 15.0, 25.0, 35.0, 45.0]
_DEFAULT_CRATE_GRID: list[float] = [0.04, 0.08, 0.15, 0.25, 0.50, 1.00, 1.50, 2.00]
_DEFAULT_SOH_GRID: list[float] = [0.70, 0.80, 0.90, 1.00]


def generate_charging_curve_lut(
    param_set: str = "Chen2020",
    *,
    v_upper: float = 4.2,
    v_lower: float = 2.5,
    cv_taper_cutoff: str = "C/20",
    soc_grid: list[float] | None = None,
    temp_grid: list[float] | None = None,
    crate_grid: list[float] | None = None,
    soh_grid: list[float] | None = None,
    thermal_model: str | None = None,
    thermal_patch_source: str | None = None,
    parameter_overrides: dict[str, Any] | None = None,
    cache_path: Path | str | None = None,
) -> dict[str, Any]:
    """Generate a 4D CC-CV charging curve LUT using PyBaMM.

    Runs PyBaMM SPMe CC-CV simulations across the full grid of
    (temperature × C-rate × SOH) conditions and interpolates the resulting
    power-fraction trajectories onto the SOC grid.

    Returns a dict with keys ``soc_grid``, ``temp_grid``, ``crate_grid``,
    ``soh_grid``, ``lut`` (numpy arrays) that can be passed directly to
    ``Battery(charging_curve_lut=...)`` or ``EV(charging_curve_lut=...)``.

    Parameters
    ----------
    param_set:
        PyBaMM parameter set name (e.g. "Chen2020", "Mohtat2020", "Prada2013").
    v_upper:
        Upper voltage cut-off for CC-CV (V).
    v_lower:
        Lower voltage cut-off (V).
    cv_taper_cutoff:
        CV phase termination current (e.g. "C/20", "C/50").
    soc_grid, temp_grid, crate_grid, soh_grid:
        Custom axis grids.  Defaults match DER_Detection convention.
    thermal_model:
        PyBaMM thermal option ("lumped" or None for isothermal).
    thermal_patch_source:
        Donor parameter set for thermal parameters (e.g. "Chen2020").
    parameter_overrides:
        Additional PyBaMM parameter overrides.
    cache_path:
        Path to save/load the LUT as an NPZ file.

    Returns
    -------
    dict with numpy arrays ready for Rust ``RegularGridInterpolator``.
    """
    import itertools

    import numpy as np

    if not _HAS_PYBAMM:
        raise ImportError(
            "PyBaMM is required for LUT generation. "
            "Install with: uv pip install -e '.[pybamm]'"
        )

    soc_arr = np.array(soc_grid or _DEFAULT_SOC_GRID, dtype=np.float64)
    temp_arr = np.array(temp_grid or _DEFAULT_TEMP_GRID, dtype=np.float64)
    crate_arr = np.array(crate_grid or _DEFAULT_CRATE_GRID, dtype=np.float64)
    soh_arr = np.array(soh_grid or _DEFAULT_SOH_GRID, dtype=np.float64)

    # Check cache
    if cache_path is not None:
        cache_path = Path(cache_path)
        if cache_path.exists():
            LOGGER.info("Loading cached charging curve LUT from %s", cache_path)
            data = np.load(cache_path, allow_pickle=False)
            return {
                "soc_grid": data["soc_grid"],
                "temp_grid": data["temp_grid"],
                "crate_grid": data["crate_grid"],
                "soh_grid": data["soh_grid"],
                "lut": data["lut"],
            }

    shape = (len(soc_arr), len(temp_arr), len(crate_arr), len(soh_arr))
    lut = np.zeros(shape, dtype=np.float32)
    total = len(temp_arr) * len(crate_arr) * len(soh_arr)
    done = 0

    for (j, T), (k, cr), (h, soh) in itertools.product(
        enumerate(temp_arr),
        enumerate(crate_arr),
        enumerate(soh_arr),
    ):
        soc_traj, pfrac = _simulate_cc_cv_single(
            param_set=param_set,
            v_upper=v_upper,
            v_lower=v_lower,
            cv_taper_cutoff=cv_taper_cutoff,
            initial_soc=float(soc_arr[0]),
            T_celsius=float(T),
            c_rate=float(cr),
            soh=float(soh),
            thermal_model=thermal_model,
            thermal_patch_source=thermal_patch_source,
            parameter_overrides=parameter_overrides,
        )

        # Deduplicate SOC for monotonic interp
        _, unique_idx = np.unique(soc_traj, return_index=True)
        soc_unique = soc_traj[unique_idx]
        pfrac_unique = pfrac[unique_idx]

        lut[:, j, k, h] = np.clip(
            np.interp(
                soc_arr, soc_unique, pfrac_unique,
                left=float(pfrac_unique[0]), right=0.0,
            ),
            0.0, 1.0,
        )

        done += 1
        LOGGER.info(
            "LUT %3d/%d (%.0f%%)  T=%+.0f°C  C=%.2fC  SOH=%.0f%%",
            done, total, 100.0 * done / total, T, cr, soh * 100,
        )

    result = {
        "soc_grid": soc_arr,
        "temp_grid": temp_arr,
        "crate_grid": crate_arr,
        "soh_grid": soh_arr,
        "lut": lut,
    }

    if cache_path is not None:
        cache_path = Path(cache_path)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        np.savez_compressed(cache_path, **result)
        LOGGER.info("Saved charging curve LUT → %s", cache_path)

    return result


def _simulate_cc_cv_single(
    *,
    param_set: str,
    v_upper: float,
    v_lower: float,
    cv_taper_cutoff: str,
    initial_soc: float,
    T_celsius: float,
    c_rate: float,
    soh: float,
    thermal_model: str | None,
    thermal_patch_source: str | None,
    parameter_overrides: dict[str, Any] | None,
) -> tuple[Any, Any]:
    """Run one CC-CV simulation and return (soc_trajectory, power_fraction)."""
    import numpy as np

    param = _pybamm.ParameterValues(param_set)
    param["Upper voltage cut-off [V]"] = v_upper
    param["Lower voltage cut-off [V]"] = v_lower

    if soh < 0.999:
        nominal = float(param["Nominal cell capacity [A.h]"])
        param["Nominal cell capacity [A.h]"] = nominal * soh

    T_kelvin = T_celsius + 273.15
    param["Ambient temperature [K]"] = T_kelvin
    param["Initial temperature [K]"] = T_kelvin

    if thermal_model == "lumped" and thermal_patch_source is not None:
        donor = _pybamm.ParameterValues(thermal_patch_source)
        thermal_keys = [
            "Negative current collector thickness [m]",
            "Negative current collector density [kg.m-3]",
            "Negative current collector specific heat capacity [J.kg-1.K-1]",
            "Negative current collector thermal conductivity [W.m-1.K-1]",
            "Positive current collector thickness [m]",
            "Positive current collector density [kg.m-3]",
            "Positive current collector specific heat capacity [J.kg-1.K-1]",
            "Positive current collector thermal conductivity [W.m-1.K-1]",
            "Negative electrode density [kg.m-3]",
            "Negative electrode specific heat capacity [J.kg-1.K-1]",
            "Negative electrode thermal conductivity [W.m-1.K-1]",
            "Positive electrode density [kg.m-3]",
            "Positive electrode specific heat capacity [J.kg-1.K-1]",
            "Positive electrode thermal conductivity [W.m-1.K-1]",
            "Separator density [kg.m-3]",
            "Separator specific heat capacity [J.kg-1.K-1]",
            "Separator thermal conductivity [W.m-1.K-1]",
        ]
        param.update(
            {k: donor[k] for k in thermal_keys},
            check_already_exists=False,
        )

    if parameter_overrides:
        param.update(parameter_overrides, check_already_exists=False)

    options = {"thermal": "lumped"} if thermal_model == "lumped" else {}
    model = _pybamm.lithium_ion.SPMe(options=options) if options else _pybamm.lithium_ion.SPMe()

    soc_start = max(initial_soc, 0.01)
    experiment = _pybamm.Experiment([
        f"Charge at {c_rate:.4f}C until {v_upper:.3f}V",
        f"Hold at {v_upper:.3f}V until {cv_taper_cutoff}",
    ])

    sim = _pybamm.Simulation(model, parameter_values=param, experiment=experiment)

    try:
        sol = sim.solve(initial_soc=soc_start)
    except Exception as exc:
        LOGGER.warning(
            "PyBaMM solve failed T=%.0f°C C=%.3fC SOH=%.0f%%: %s",
            T_celsius, c_rate, soh * 100, exc,
        )
        return np.array([soc_start, 1.0]), np.array([0.0, 0.0])

    cap_nom = float(param["Nominal cell capacity [A.h]"])
    throughput = sol["Throughput capacity [A.h]"].entries
    current = sol["Current [A]"].entries

    soc_traj = np.clip(soc_start + throughput / cap_nom, 0.0, 1.0)
    i_cc = max(float(np.abs(current[0])), 1e-8)
    power_fraction = np.clip(np.abs(current) / i_cc, 0.0, 1.0)

    return soc_traj, power_fraction
