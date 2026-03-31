from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import sys
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import pandas as pd


ROOT = Path(__file__).resolve().parents[2]
VENDOR_OCHRE = ROOT / 'vendors' / 'OCHRE'
DEFAULTS = ROOT / 'defaults'


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description='HARES vs OCHRE parity diagnostics with plots')
    p.add_argument('--fixture', default='tests/fixtures/parity/cz4a_ashp_hpwh', help='Path to parity fixture dir')
    p.add_argument('--verbosity', type=int, default=9, help='Output verbosity for both engines')
    p.add_argument('--out', default='artifacts/parity_diagnostics', help='Output directory root')
    return p.parse_args()


def load_fixture_config(fixture_dir: Path) -> dict:
    import tomllib

    with (fixture_dir / 'config.toml').open('rb') as f:
        cfg = tomllib.load(f)
    sim = cfg['simulation']
    return {
        'start_time': sim['start_time'],
        'duration_s': int(sim['duration']),
        'time_res_s': int(sim['time_res']),
        'master_seed': int(sim.get('master_seed', 0)),
    }


def to_pandas(df_obj):
    if hasattr(df_obj, 'to_pandas'):
        return df_obj.to_pandas()
    if isinstance(df_obj, pd.DataFrame):
        return df_obj
    return pd.DataFrame(df_obj)


def run_hares(fixture_dir: Path, cfg: dict, verbosity: int) -> pd.DataFrame:
    from ochre_next import Dwelling as HaresDwelling

    dw = HaresDwelling.from_hpxml(
        str(fixture_dir / 'building.xml'),
        str(fixture_dir / 'schedule.csv'),
        str(fixture_dir / 'weather.epw'),
        start_time=cfg['start_time'],
        time_res_s=cfg['time_res_s'],
        duration_s=cfg['duration_s'],
        output_verbosity=verbosity,
        defaults_path=str(DEFAULTS),
        master_seed=cfg['master_seed'],
    )
    df = to_pandas(dw.simulate())
    return df


def run_ochre(fixture_dir: Path, cfg: dict, verbosity: int) -> pd.DataFrame:
    if str(VENDOR_OCHRE) not in sys.path:
        sys.path.insert(0, str(VENDOR_OCHRE))

    from ochre import Dwelling as OchreDwelling

    start_local = dt.datetime.fromisoformat(cfg['start_time']).replace(tzinfo=None)
    dw = OchreDwelling(
        name='parity_diag',
        start_time=start_local,
        time_res=dt.timedelta(seconds=cfg['time_res_s']),
        duration=dt.timedelta(seconds=cfg['duration_s']),
        hpxml_file=str(fixture_dir / 'building.xml'),
        hpxml_schedule_file=str(fixture_dir / 'schedule.csv'),
        weather_file=str(fixture_dir / 'weather.epw'),
        verbosity=verbosity,
        save_results=False,
    )
    df, _metrics, _hourly = dw.simulate()
    return to_pandas(df)


def ensure_time_col(df: pd.DataFrame) -> pd.DataFrame:
    out = df.copy()
    if 'Time' in out.columns:
        out['Time'] = pd.to_datetime(out['Time'])
    else:
        out['Time'] = pd.RangeIndex(len(out))
    return out


def pick_col(df: pd.DataFrame, candidates: list[str]) -> str | None:
    for c in candidates:
        if c in df.columns:
            return c
    return None


def sum_cols(df: pd.DataFrame, candidates: list[str]) -> pd.Series:
    existing = [c for c in candidates if c in df.columns]
    if not existing:
        return pd.Series([0.0] * len(df), index=df.index)
    return df[existing].sum(axis=1)


def aligned_frames(h: pd.DataFrame, o: pd.DataFrame) -> tuple[pd.DataFrame, pd.DataFrame]:
    h2 = ensure_time_col(h)
    o2 = ensure_time_col(o)
    n = min(len(h2), len(o2))
    h2 = h2.iloc[:n].reset_index(drop=True)
    o2 = o2.iloc[:n].reset_index(drop=True)
    return h2, o2


def metrics_summary(h: pd.Series, o: pd.Series) -> dict[str, float]:
    d = h - o
    mae = float(d.abs().mean())
    rmse = float(math.sqrt((d * d).mean()))
    max_abs = float(d.abs().max())
    mean = float(d.mean())
    o_peak = float(o.max())
    h_peak = float(h.max())
    peak_rel_pct = float((h_peak - o_peak) / o_peak * 100.0) if abs(o_peak) > 1e-9 else float('nan')
    return {
        'mae': mae,
        'rmse': rmse,
        'max_abs': max_abs,
        'mean_bias': mean,
        'ochre_peak': o_peak,
        'hares_peak': h_peak,
        'peak_rel_pct': peak_rel_pct,
    }


def plot_core(out_dir: Path, h: pd.DataFrame, o: pd.DataFrame, channels: dict[str, tuple[pd.Series, pd.Series]]):
    fig, axes = plt.subplots(4, 1, figsize=(14, 14), sharex=True)

    for ax, (name, (hs, os_)) in zip(axes, channels.items()):
        ax.plot(os_.values, label='OCHRE', linewidth=1.4)
        ax.plot(hs.values, label='HARES', linewidth=1.2)
        ax.set_ylabel(name)
        ax.grid(alpha=0.25)
        ax.legend(loc='upper right')

    axes[-1].set_xlabel('Timestep')
    fig.suptitle('HARES vs OCHRE Core Time Series')
    fig.tight_layout()
    fig.savefig(out_dir / 'core_timeseries.png', dpi=150)
    plt.close(fig)


def plot_gains(out_dir: Path, h: pd.DataFrame, o: pd.DataFrame):
    gain_cols = [
        'Net Sensible Heat Gain - Indoor (W)',
        'Infiltration Heat Gain - Indoor (W)',
        'Forced Ventilation Heat Gain - Indoor (W)',
        'Natural Ventilation Heat Gain - Indoor (W)',
        'Internal Heat Gain - Indoor (W)',
        'Window Transmitted Solar Gain (W)',
        'Radiation Heat Gain - Indoor (W)',
        'Roof Heat Gain - Indoor (W)',
        'Wall Heat Gain - Indoor (W)',
        'Floor Heat Gain - Indoor (W)',
        'Window Heat Gain - Indoor (W)',
    ]

    shared = [c for c in gain_cols if c in h.columns and c in o.columns]
    if not shared:
        return

    n = min(6, len(shared))
    fig, axes = plt.subplots(n, 1, figsize=(14, 2.6 * n), sharex=True)
    if n == 1:
        axes = [axes]

    for ax, c in zip(axes, shared[:n]):
        ax.plot(o[c].values, label='OCHRE', linewidth=1.3)
        ax.plot(h[c].values, label='HARES', linewidth=1.2)
        ax.set_ylabel(c.replace(' (W)', ''))
        ax.grid(alpha=0.25)
        ax.legend(loc='upper right')

    axes[-1].set_xlabel('Timestep')
    fig.suptitle('Envelope/Indoor Gain Drivers (W)')
    fig.tight_layout()
    fig.savefig(out_dir / 'gain_drivers_timeseries.png', dpi=150)
    plt.close(fig)


def plot_equipment_breakdown(out_dir: Path, h: pd.DataFrame, o: pd.DataFrame):
    # End-use comparable channels
    h_hvac_heat = sum_cols(h, ['ASHP Heater Electric Power (kW)', 'MSHP Heater Electric Power (kW)', 'Gas Furnace Electric Power (kW)', 'Electric Furnace Electric Power (kW)'])
    o_hvac_heat = sum_cols(o, ['HVAC Heating Electric Power (kW)'])

    h_hvac_cool = sum_cols(h, ['ASHP Cooler Electric Power (kW)', 'MSHP Cooler Electric Power (kW)', 'Air Conditioner Electric Power (kW)', 'Room AC Electric Power (kW)'])
    o_hvac_cool = sum_cols(o, ['HVAC Cooling Electric Power (kW)'])

    h_wh = sum_cols(h, ['Heat Pump Water Heater Electric Power (kW)', 'Water Heating Electric Power (kW)', 'Resistance Water Heater Electric Power (kW)', 'Gas Water Heater Electric Power (kW)'])
    o_wh = sum_cols(o, ['Water Heating Electric Power (kW)'])

    h_light = sum_cols(h, ['Indoor Lighting Electric Power (kW)', 'Exterior Lighting Electric Power (kW)'])
    o_light = sum_cols(o, ['Lighting Electric Power (kW)', 'Indoor Lighting Electric Power (kW)', 'Exterior Lighting Electric Power (kW)'])

    h_other = sum_cols(h, ['Other Electric Power (kW)', 'MELs Electric Power (kW)', 'TV Electric Power (kW)', 'Refrigerator Electric Power (kW)', 'Ventilation Fan Electric Power (kW)'])
    o_other = sum_cols(o, ['Other Electric Power (kW)'])

    fig, axes = plt.subplots(2, 1, figsize=(14, 8), sharex=True)

    axes[0].plot(o_hvac_heat.values, label='OCHRE HVAC Heat', linewidth=1.4)
    axes[0].plot(h_hvac_heat.values, label='HARES HVAC Heat', linewidth=1.2)
    axes[0].plot(o_hvac_cool.values, label='OCHRE HVAC Cool', linewidth=1.4)
    axes[0].plot(h_hvac_cool.values, label='HARES HVAC Cool', linewidth=1.2)
    axes[0].plot(o_wh.values, label='OCHRE WH', linewidth=1.2)
    axes[0].plot(h_wh.values, label='HARES WH', linewidth=1.2)
    axes[0].set_ylabel('kW')
    axes[0].set_title('Major End-Use Power Channels')
    axes[0].grid(alpha=0.25)
    axes[0].legend(loc='upper right', ncol=3)

    axes[1].plot((h_hvac_heat - o_hvac_heat).values, label='HVAC Heat Δ (H-O)')
    axes[1].plot((h_hvac_cool - o_hvac_cool).values, label='HVAC Cool Δ (H-O)')
    axes[1].plot((h_wh - o_wh).values, label='WH Δ (H-O)')
    axes[1].plot((h_light - o_light).values, label='Lighting Δ (H-O)')
    axes[1].plot((h_other - o_other).values, label='Other Δ (H-O)')
    axes[1].axhline(0.0, color='black', linewidth=0.8)
    axes[1].set_ylabel('kW')
    axes[1].set_xlabel('Timestep')
    axes[1].set_title('Channel Difference Signals')
    axes[1].grid(alpha=0.25)
    axes[1].legend(loc='upper right', ncol=3)

    fig.tight_layout()
    fig.savefig(out_dir / 'equipment_breakdown.png', dpi=150)
    plt.close(fig)


def main() -> None:
    args = parse_args()
    fixture_dir = (ROOT / args.fixture).resolve() if not Path(args.fixture).is_absolute() else Path(args.fixture)
    fixture_name = fixture_dir.name
    out_dir = (ROOT / args.out / fixture_name).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    cfg = load_fixture_config(fixture_dir)

    print(f'[diag] fixture={fixture_name} verbosity={args.verbosity}')
    h_df = run_hares(fixture_dir, cfg, args.verbosity)
    o_df = run_ochre(fixture_dir, cfg, args.verbosity)
    h, o = aligned_frames(h_df, o_df)

    # Core comparable channels
    channels: dict[str, tuple[pd.Series, pd.Series]] = {}
    t_col_h = pick_col(h, ['Temperature - Indoor (C)', 'Indoor Temperature (C)'])
    t_col_o = pick_col(o, ['Temperature - Indoor (C)', 'Indoor Temperature (C)'])
    if t_col_h and t_col_o:
        channels['Indoor Temp (C)'] = (h[t_col_h], o[t_col_o])

    h_heat = sum_cols(h, ['ASHP Heater Electric Power (kW)', 'MSHP Heater Electric Power (kW)', 'HVAC Heating Electric Power (kW)', 'Gas Furnace Electric Power (kW)', 'Electric Furnace Electric Power (kW)'])
    o_heat = sum_cols(o, ['HVAC Heating Electric Power (kW)'])
    channels['HVAC Heat Elec (kW)'] = (h_heat, o_heat)

    h_cool = sum_cols(h, ['ASHP Cooler Electric Power (kW)', 'MSHP Cooler Electric Power (kW)', 'HVAC Cooling Electric Power (kW)', 'Air Conditioner Electric Power (kW)', 'Room AC Electric Power (kW)'])
    o_cool = sum_cols(o, ['HVAC Cooling Electric Power (kW)'])
    channels['HVAC Cool Elec (kW)'] = (h_cool, o_cool)

    if 'Total Electric Power (kW)' in h.columns and 'Total Electric Power (kW)' in o.columns:
        channels['Total Electric (kW)'] = (h['Total Electric Power (kW)'], o['Total Electric Power (kW)'])

    # Summaries
    summary = {
        name: metrics_summary(hs, os_)
        for name, (hs, os_) in channels.items()
    }

    # Top timestep diagnostics for HVAC heat difference
    delta_heat = (h_heat - o_heat).abs()
    top_idx = delta_heat.sort_values(ascending=False).head(15).index.tolist()
    top_rows = []
    for i in top_idx:
        row = {
            'step': int(i),
            'hares_hvac_heat_kw': float(h_heat.iloc[i]),
            'ochre_hvac_heat_kw': float(o_heat.iloc[i]),
            'delta_kw': float(h_heat.iloc[i] - o_heat.iloc[i]),
        }
        if t_col_h and t_col_o:
            row['hares_indoor_c'] = float(h[t_col_h].iloc[i])
            row['ochre_indoor_c'] = float(o[t_col_o].iloc[i])
            row['delta_indoor_c'] = float(h[t_col_h].iloc[i] - o[t_col_o].iloc[i])
        top_rows.append(row)

    pd.DataFrame(top_rows).to_csv(out_dir / 'top_hvac_heat_delta_steps.csv', index=False)

    # Full aligned channels dump for ad-hoc analysis
    dump = pd.DataFrame({'step': range(len(h))})
    for name, (hs, os_) in channels.items():
        key = name.lower().replace(' ', '_').replace('(', '').replace(')', '').replace('/', '_')
        dump[f'{key}_hares'] = hs.values
        dump[f'{key}_ochre'] = os_.values
        dump[f'{key}_delta'] = hs.values - os_.values
    dump.to_csv(out_dir / 'aligned_core_channels.csv', index=False)

    with (out_dir / 'summary.json').open('w', encoding='utf-8') as f:
        json.dump({'fixture': fixture_name, 'verbosity': args.verbosity, 'metrics': summary}, f, indent=2)

    plot_core(out_dir, h, o, channels)
    plot_gains(out_dir, h, o)
    plot_equipment_breakdown(out_dir, h, o)

    print(f'[diag] wrote diagnostics to {out_dir}')
    print('[diag] files: summary.json, aligned_core_channels.csv, top_hvac_heat_delta_steps.csv, core_timeseries.png, gain_drivers_timeseries.png, equipment_breakdown.png')


if __name__ == '__main__':
    main()
