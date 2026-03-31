"""Generate HARES-vs-OCHRE parity diagnostics with metrics and charts.

Usage:
    UV_CACHE_DIR=/tmp/uvcache MPLCONFIGDIR=/tmp/mplconfig \
    uv run --no-sync --group ochre python tests/python/parity_diagnostics.py \
      --fixture tests/fixtures/parity/cz4a_ashp_hpwh \
      --out-dir /tmp/parity_diag_ashp
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import matplotlib.pyplot as plt
import pandas as pd


@dataclass(frozen=True)
class ChannelSpec:
    label: str
    ochre_primary: str
    ochre_components: tuple[str, ...]
    hares_primary: str
    hares_components: tuple[str, ...]
    native_unit: str = ""


CHANNEL_SPECS: tuple[ChannelSpec, ...] = (
    ChannelSpec(
        "total_electric_kw",
        "Total Electric Power (kW)",
        (),
        "Total Electric Power (kW)",
        (),
        native_unit="kW",
    ),
    ChannelSpec(
        "hvac_heat_kw",
        "HVAC Heating Electric Power (kW)",
        (),
        "HVAC Heating Electric Power (kW)",
        ("ASHP Heater Electric Power (kW)",),
        native_unit="kW",
    ),
    ChannelSpec(
        "hvac_cool_kw",
        "HVAC Cooling Electric Power (kW)",
        (),
        "HVAC Cooling Electric Power (kW)",
        ("ASHP Cooler Electric Power (kW)", "Room AC Electric Power (kW)"),
        native_unit="kW",
    ),
    ChannelSpec(
        "water_heat_kw",
        ochre_primary="Water Heating Electric Power (kW)",
        ochre_components=(),
        hares_primary="Water Heating Electric Power (kW)",
        hares_components=("Heat Pump Water Heater Electric Power (kW)",),
        native_unit="kW",
    ),
    ChannelSpec(
        "ventilation_kw",
        "Ventilation Fan Electric Power (kW)",
        (),
        "Ventilation Fan Electric Power (kW)",
        (),
        native_unit="kW",
    ),
    ChannelSpec(
        "lighting_kw",
        "Lighting Electric Power (kW)",
        (),
        "Lighting Electric Power (kW)",
        (
            "Indoor Lighting Electric Power (kW)",
            "Exterior Lighting Electric Power (kW)",
            "Garage Lighting Electric Power (kW)",
        ),
        native_unit="kW",
    ),
    ChannelSpec(
        "other_kw",
        "Other Electric Power (kW)",
        (),
        "Other Electric Power (kW)",
        ("Occupancy Electric Power (kW)",),
        native_unit="kW",
    ),
    ChannelSpec(
        "indoor_temp_c",
        "Temperature - Indoor (C)",
        (),
        "Temperature - Indoor (C)",
        (),
        native_unit="C",
    ),
    ChannelSpec(
        "outdoor_temp_c",
        "Temperature - Outdoor (C)",
        ("Outdoor Dry Bulb (C)",),
        "Temperature - Outdoor (C)",
        ("Outdoor Dry Bulb (C)",),
        native_unit="C",
    ),
)

HARES_ELECTRIC_COL_SUFFIX = "Electric Power (kW)"
ENVELOPE_GAIN_KEYS: tuple[str, ...] = (
    "window_solar_w",
    "opaque_solar_lwr_w",
    "interior_lwr_w",
    "infiltration_w",
    "ventilation_w",
    "natural_ventilation_w",
    "port_sensible_w",
    "internal_gain_w",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fixture",
        type=Path,
        default=Path("tests/fixtures/parity/cz4a_ashp_hpwh"),
        help="Parity fixture directory (contains building.xml/schedule.csv/weather.epw/config.toml).",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("/tmp/parity_diagnostics"),
        help="Output directory for report artifacts.",
    )
    parser.add_argument(
        "--actual-parquet",
        type=Path,
        default=None,
        help="Optional precomputed HARES parquet path; skips running ochre_next when provided.",
    )
    parser.add_argument(
        "--reference-parquet",
        type=Path,
        default=None,
        help="Optional OCHRE reference parquet path; defaults to fixture/reference_output.parquet.",
    )
    return parser.parse_args()


def load_sim_config(fixture_dir: Path) -> dict:
    config_path = fixture_dir / "config.toml"
    cfg = tomllib.loads(config_path.read_text())
    simulation = cfg["simulation"]
    start = dt.datetime.fromisoformat(simulation["start_time"])
    return {
        "start_time": start,
        "start_time_iso": simulation["start_time"],
        "duration_s": int(simulation["duration"]),
        "time_res_s": int(simulation["time_res"]),
        "output_verbosity": int(simulation["output_verbosity"]),
        "master_seed": int(simulation.get("master_seed", 0)),
    }


def _normalize_time_index(df: pd.DataFrame, start_time: dt.datetime) -> pd.DataFrame:
    offset = start_time.utcoffset()
    if offset is None:
        raise ValueError("fixture simulation.start_time must include a timezone offset")

    out = df.copy()
    if "Time" in out.columns:
        out["Time"] = pd.to_datetime(out["Time"])
        idx = pd.DatetimeIndex(out["Time"])
        out = out.drop(columns=["Time"])
    else:
        idx = pd.DatetimeIndex(pd.to_datetime(out.index))

    if idx.tz is None:
        idx = idx.tz_localize(dt.timezone(offset))
    out.index = idx
    out.index.name = "Time"
    return out


def load_reference_parquet(reference_parquet: Path, sim_cfg: dict) -> pd.DataFrame:
    if not reference_parquet.exists():
        raise FileNotFoundError(f"reference parquet not found: {reference_parquet}")
    df = pd.read_parquet(reference_parquet)
    return _normalize_time_index(df, sim_cfg["start_time"])


def run_ochre(fixture_dir: Path, sim_cfg: dict, reference_parquet: Path | None) -> pd.DataFrame:
    try:
        sys.path.insert(0, str(Path("vendors/OCHRE").resolve()))
        from ochre import Dwelling as OchreDwelling
    except Exception:
        parquet = reference_parquet or (fixture_dir / "reference_output.parquet")
        return load_reference_parquet(parquet, sim_cfg)

    start_local = sim_cfg["start_time"].replace(tzinfo=None)
    df, _metrics, _hourly = OchreDwelling(
        name="parity_diag_ochre",
        start_time=start_local,
        time_res=dt.timedelta(seconds=sim_cfg["time_res_s"]),
        duration=dt.timedelta(seconds=sim_cfg["duration_s"]),
        hpxml_file=str((fixture_dir / "building.xml").resolve()),
        hpxml_schedule_file=str((fixture_dir / "schedule.csv").resolve()),
        weather_file=str((fixture_dir / "weather.epw").resolve()),
        verbosity=sim_cfg["output_verbosity"],
        save_results=False,
    ).simulate()

    return _normalize_time_index(df, sim_cfg["start_time"])


def _extract_solver_gain_map(snapshot: dict[str, Any]) -> dict[str, float]:
    solver = snapshot.get("post_solvers") or {}
    if not isinstance(solver, dict):
        raise ValueError("observer snapshot post_solvers must be a dict")
    out: dict[str, float] = {}
    for key in ENVELOPE_GAIN_KEYS:
        if key not in solver:
            raise KeyError(f"observer post_solvers missing required key: {key}")
        value = solver[key]
        out[key] = float(value) if value is not None else 0.0
    return out


def _extract_equipment_thermal(snapshot: dict[str, Any]) -> dict[str, float]:
    totals: dict[str, float] = {
        "hvac_thermal_equipment_sensible_w": 0.0,
        "hvac_thermal_equipment_latent_w": 0.0,
        "nonthermal_equipment_sensible_w": 0.0,
        "nonthermal_equipment_latent_w": 0.0,
    }
    for phase_name, sensible_key, latent_key in (
        ("post_thermal_equipment", "hvac_thermal_equipment_sensible_w", "hvac_thermal_equipment_latent_w"),
        ("post_nonthermal_equipment", "nonthermal_equipment_sensible_w", "nonthermal_equipment_latent_w"),
    ):
        phase = snapshot.get(phase_name) or {}
        equipment = phase.get("equipment", []) if isinstance(phase, dict) else []
        for obs in equipment:
            telemetry = obs.get("telemetry", {}) if isinstance(obs, dict) else {}
            totals[sensible_key] += float(telemetry.get("sensible_gain_w", 0.0) or 0.0)
            totals[latent_key] += float(telemetry.get("latent_gain_w", 0.0) or 0.0)
    return totals


def run_hares(
    fixture_dir: Path,
    sim_cfg: dict,
    actual_parquet: Path | None,
) -> tuple[pd.DataFrame, pd.DataFrame]:
    if actual_parquet is not None:
        pdf = pd.read_parquet(actual_parquet)
        return _normalize_time_index(pdf, sim_cfg["start_time"]), pd.DataFrame()

    import ochre_next

    dwelling_sim = ochre_next.Dwelling.from_hpxml(
        str((fixture_dir / "building.xml").resolve()),
        str((fixture_dir / "schedule.csv").resolve()),
        str((fixture_dir / "weather.epw").resolve()),
        start_time=sim_cfg["start_time_iso"],
        time_res_s=sim_cfg["time_res_s"],
        duration_s=sim_cfg["duration_s"],
        output_verbosity=sim_cfg["output_verbosity"],
        defaults_path=str(Path("defaults").resolve()),
        master_seed=sim_cfg["master_seed"],
    )
    pdf = dwelling_sim.simulate().to_pandas()
    pdf = _normalize_time_index(pdf, sim_cfg["start_time"])

    dwelling_obs = ochre_next.Dwelling.from_hpxml(
        str((fixture_dir / "building.xml").resolve()),
        str((fixture_dir / "schedule.csv").resolve()),
        str((fixture_dir / "weather.epw").resolve()),
        start_time=sim_cfg["start_time_iso"],
        time_res_s=sim_cfg["time_res_s"],
        duration_s=sim_cfg["duration_s"],
        output_verbosity=sim_cfg["output_verbosity"],
        defaults_path=str(Path("defaults").resolve()),
        master_seed=sim_cfg["master_seed"],
    )

    obs_rows: list[dict[str, Any]] = []
    steps = sim_cfg["duration_s"] // sim_cfg["time_res_s"]
    has_observer = hasattr(dwelling_obs, "enable_observer") and hasattr(dwelling_obs, "drain_observations")
    if has_observer:
        dwelling_obs.enable_observer(int(steps) + 2)

    for _ in range(int(steps)):
        step_row = dwelling_obs.step()
        if has_observer:
            snapshots = dwelling_obs.drain_observations()
            if snapshots:
                snap = snapshots[-1]
                obs = {"Time": pd.to_datetime(step_row["timestamp"])}
                obs.update(_extract_solver_gain_map(snap))
                obs.update(_extract_equipment_thermal(snap))
                obs_rows.append(obs)

    obs_df = pd.DataFrame(obs_rows).set_index("Time") if obs_rows else pd.DataFrame(index=pdf.index)
    return pdf, obs_df


def _from_columns_or_components(df: pd.DataFrame, primary: str, components: tuple[str, ...]) -> pd.Series:
    if primary in df.columns:
        return df[primary].astype(float)

    present = [col for col in components if col in df.columns]
    if not present:
        return pd.Series(index=df.index, dtype=float).fillna(0.0)
    return df[present].astype(float).sum(axis=1)


def align_channels(ochre_df: pd.DataFrame, hares_df: pd.DataFrame) -> tuple[pd.DataFrame, pd.DataFrame]:
    index = ochre_df.index.intersection(hares_df.index)
    if index.empty:
        raise ValueError("No overlapping timestamps between OCHRE and HARES results")

    metrics_rows: list[dict[str, float | str]] = []
    series = pd.DataFrame(index=index)
    for spec in CHANNEL_SPECS:
        ochre_series = _from_columns_or_components(ochre_df, spec.ochre_primary, spec.ochre_components)
        hares_series = _from_columns_or_components(hares_df, spec.hares_primary, spec.hares_components)
        ochre_series = ochre_series.reindex(index)
        hares_series = hares_series.reindex(index)
        diff = hares_series - ochre_series
        mean_abs_ref = max(float(ochre_series.abs().mean()), 1e-9)
        metrics_rows.append(
            {
                "channel": spec.label,
                "unit": spec.native_unit,
                "ochre_primary": spec.ochre_primary,
                "ochre_components": ", ".join(spec.ochre_components),
                "hares_primary": spec.hares_primary,
                "hares_components": ", ".join(spec.hares_components),
                "mae": float(diff.abs().mean()),
                "rmse": float((diff.pow(2).mean()) ** 0.5),
                "bias": float(diff.mean()),
                "peak_abs_diff": float(diff.abs().max()),
                "mae_rel_pct_of_ref_mean": float((diff.abs().mean() / mean_abs_ref) * 100.0),
            }
        )
        series[f"{spec.label}_ochre"] = ochre_series
        series[f"{spec.label}_hares"] = hares_series
        series[f"{spec.label}_diff"] = diff

    metrics = pd.DataFrame(metrics_rows).sort_values(by="mae", ascending=False)
    return metrics, series


def equipment_power_frame(hares_df: pd.DataFrame) -> pd.DataFrame:
    cols = [
        col
        for col in hares_df.columns
        if col.endswith(HARES_ELECTRIC_COL_SUFFIX) and col != "Total Electric Power (kW)"
    ]
    if not cols:
        return pd.DataFrame(index=hares_df.index)
    return hares_df[cols].copy()


def write_top_contributors(series: pd.DataFrame, equip_df: pd.DataFrame, out_dir: Path) -> None:
    total_diff = series["total_electric_kw_diff"]
    peak_ts = total_diff.abs().idxmax()
    rows: list[dict[str, float | str]] = []
    if not equip_df.empty and peak_ts in equip_df.index:
        row = equip_df.loc[peak_ts]
        for col, value in row.items():
            rows.append({"timestamp": str(peak_ts), "column": col, "value_kw": float(value)})
    if rows:
        pd.DataFrame(rows).sort_values("value_kw", ascending=False).to_csv(
            out_dir / "hares_equipment_at_peak_diff.csv", index=False
        )

    summary = {
        "peak_abs_total_diff_timestamp": str(peak_ts),
        "peak_abs_total_diff_kw": float(total_diff.abs().max()),
        "peak_signed_total_diff_kw": float(total_diff.loc[peak_ts]),
    }
    (out_dir / "summary.json").write_text(json.dumps(summary, indent=2))


def plot_key_channels(series: pd.DataFrame, out_dir: Path) -> None:
    plot_specs = [
        ("Total Electric (kW)", "total_electric_kw"),
        ("HVAC Heating Electric (kW)", "hvac_heat_kw"),
        ("HVAC Cooling Electric (kW)", "hvac_cool_kw"),
        ("Indoor Temperature (C)", "indoor_temp_c"),
        ("Outdoor Temperature (C)", "outdoor_temp_c"),
    ]
    fig, axes = plt.subplots(len(plot_specs), 1, figsize=(14, 16), sharex=True)
    for ax, (title, prefix) in zip(axes, plot_specs):
        ax.plot(series.index, series[f"{prefix}_ochre"], label="OCHRE")
        ax.plot(series.index, series[f"{prefix}_hares"], label="HARES")
        ax.set_title(title)
        ax.grid(True, alpha=0.3)
        ax.legend()
    fig.tight_layout()
    fig.savefig(out_dir / "timeseries_overlay.png", dpi=160)
    plt.close(fig)


def plot_group_diff(series: pd.DataFrame, out_dir: Path) -> None:
    diff_cols = [col for col in series.columns if col.endswith("_diff")]
    fig, ax = plt.subplots(figsize=(14, 6))
    for col in diff_cols:
        ax.plot(series.index, series[col], label=col.removesuffix("_diff"))
    ax.axhline(0.0, color="black", linewidth=1.0, alpha=0.6)
    ax.set_title("HARES - OCHRE Channel Differences")
    ax.set_ylabel("Difference (native units)")
    ax.grid(True, alpha=0.3)
    ax.legend(ncol=3, fontsize=8)
    fig.tight_layout()
    fig.savefig(out_dir / "channel_differences.png", dpi=160)
    plt.close(fig)


def plot_observer_envelope(obs_df: pd.DataFrame, out_dir: Path) -> None:
    if obs_df.empty:
        return
    fig, axes = plt.subplots(2, 1, figsize=(14, 10), sharex=True)
    for key in ENVELOPE_GAIN_KEYS:
        if key in obs_df.columns:
            axes[0].plot(obs_df.index, obs_df[key], label=key)
    axes[0].set_title("HARES Envelope Gain Components")
    axes[0].set_ylabel("W")
    axes[0].grid(True, alpha=0.3)
    axes[0].legend(ncol=2, fontsize=8)

    for key in (
        "hvac_thermal_equipment_sensible_w",
        "nonthermal_equipment_sensible_w",
        "hvac_thermal_equipment_latent_w",
        "nonthermal_equipment_latent_w",
    ):
        if key in obs_df.columns:
            axes[1].plot(obs_df.index, obs_df[key], label=key)
    axes[1].set_title("HARES Equipment Thermal Contributions")
    axes[1].set_ylabel("W")
    axes[1].grid(True, alpha=0.3)
    axes[1].legend(ncol=2, fontsize=8)
    fig.tight_layout()
    fig.savefig(out_dir / "hares_observer_envelope_and_equipment.png", dpi=160)
    plt.close(fig)


def main() -> None:
    args = parse_args()
    fixture_dir = args.fixture.resolve()
    out_dir = args.out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    sim_cfg = load_sim_config(fixture_dir)
    reference_parquet = args.reference_parquet.resolve() if args.reference_parquet else None
    actual_parquet = args.actual_parquet.resolve() if args.actual_parquet else None

    ochre_df = run_ochre(fixture_dir, sim_cfg, reference_parquet)
    hares_df, obs_df = run_hares(fixture_dir, sim_cfg, actual_parquet)

    metrics, series = align_channels(ochre_df, hares_df)
    metrics.to_csv(out_dir / "channel_metrics.csv", index=False)
    series.to_csv(out_dir / "aligned_series.csv")

    equip_df = equipment_power_frame(hares_df).reindex(series.index)
    if not equip_df.empty:
        equip_df.to_csv(out_dir / "hares_equipment_power_series.csv")
    if not obs_df.empty:
        obs_df.to_csv(out_dir / "hares_observer_components.csv")
    write_top_contributors(series, equip_df, out_dir)
    plot_key_channels(series, out_dir)
    plot_group_diff(series, out_dir)
    plot_observer_envelope(obs_df, out_dir)

    print(f"wrote diagnostics to: {out_dir}")
    print("top channel errors:")
    print(metrics[["channel", "mae", "rmse", "bias", "mae_rel_pct_of_ref_mean"]].head(10).to_string(index=False))


if __name__ == "__main__":
    main()
