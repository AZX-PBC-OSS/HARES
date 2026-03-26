import marimo

__generated_with = "0.20.4"
app = marimo.App(width="medium")


@app.cell
def _imports():
    import datetime
    import sys
    from pathlib import Path

    import marimo as mo
    import numpy as np
    import plotly.express as px
    import plotly.graph_objects as go
    import polars as pl
    from plotly.subplots import make_subplots

    return datetime, mo, np, Path, pl, px, go, make_subplots, sys


@app.cell
def _title(mo):
    mo.md("# HARES Simulation Explorer")


@app.cell
def _constants(Path):
    HARES_ROOT = Path(__file__).resolve().parents[1]
    HARES_DEFAULTS = HARES_ROOT / "defaults"
    DEFAULT_HPXML = HARES_ROOT / "data" / "examples" / "BEopt_example.xml"
    DEFAULT_SCHEDULE = HARES_ROOT / "data" / "examples" / "BEopt_example_schedule.csv"
    DEFAULT_WEATHER = HARES_ROOT / "data" / "examples" / "USA_CO_Denver.epw"
    VENDOR_OCHRE = HARES_ROOT / "vendors" / "OCHRE"

    return HARES_ROOT, HARES_DEFAULTS, DEFAULT_HPXML, DEFAULT_SCHEDULE, DEFAULT_WEATHER, VENDOR_OCHRE


@app.cell
def _building_source_selector(mo):
    building_source = mo.ui.radio(
        options={
            "BEopt Example (Denver CO)": "beopt_default",
            "ResStock (OEDI S3)": "resstock",
        },
        value="BEopt Example (Denver CO)",
        label="Building source",
    )
    building_source
    return (building_source,)


@app.cell
def _beopt_resolver(mo, building_source, DEFAULT_HPXML, DEFAULT_SCHEDULE, DEFAULT_WEATHER):
    mo.stop(building_source.value != "beopt_default")

    _missing = [
        p.name for p in [DEFAULT_HPXML, DEFAULT_SCHEDULE, DEFAULT_WEATHER]
        if not p.exists()
    ]
    if _missing:
        mo.stop(
            True,
            mo.callout(
                mo.md(f"Missing example files: {', '.join(_missing)}. Run `make data` from the repo root."),
                kind="danger",
            ),
        )

    bldg_hpxml = DEFAULT_HPXML
    bldg_schedule = DEFAULT_SCHEDULE
    bldg_weather = DEFAULT_WEATHER
    bldg_label = "BEopt Example (Denver CO)"

    mo.callout(
        mo.md(f"Using: `{DEFAULT_HPXML.name}` / `{DEFAULT_WEATHER.name}`"),
        kind="info",
    )
    return bldg_hpxml, bldg_schedule, bldg_weather, bldg_label


@app.cell
def _resstock_picker(mo, building_source):
    mo.stop(building_source.value != "resstock")

    rs_bldg_id = mo.ui.number(
        start=1,
        stop=550000,
        step=1,
        value=12345,
        label="Building ID",
    )
    rs_version = mo.ui.dropdown(
        options=["2024.1", "2024.2", "2025.1"],
        value="2024.2",
        label="ResStock version",
    )
    mo.hstack([rs_bldg_id, rs_version], gap="1rem")
    return rs_bldg_id, rs_version


@app.cell
def _resstock_fetch(mo, building_source, rs_bldg_id, rs_version):
    mo.stop(building_source.value != "resstock")

    from ochre_next.data.resstock import fetch_resstock_building

    with mo.status.spinner(title="Fetching ResStock building from OEDI S3..."):
        _bldg = fetch_resstock_building(
            bldg_id=int(rs_bldg_id.value),
            version=str(rs_version.value),
        )

    bldg_hpxml = _bldg.hpxml_path
    bldg_schedule = _bldg.schedule_path
    bldg_weather = _bldg.weather_path
    bldg_label = f"ResStock bldg{int(rs_bldg_id.value):07d} ({rs_version.value})"

    mo.callout(
        mo.md(f"Fetched building **{int(rs_bldg_id.value)}** (v{rs_version.value})"),
        kind="success",
    )
    return bldg_hpxml, bldg_schedule, bldg_weather, bldg_label


@app.cell
def _der_widgets(mo):
    pv_enable = mo.ui.checkbox(label="Enable PV", value=True)
    pv_capacity = mo.ui.slider(
        start=0.5, stop=14.0, step=0.5, value=6.0, label="Capacity (kW)"
    )
    pv_tilt = mo.ui.slider(
        start=0, stop=45, step=1, value=26, label="Tilt (°)"
    )
    pv_azimuth = mo.ui.slider(
        start=90, stop=270, step=1, value=180, label="Azimuth (°)"
    )

    bat_enable = mo.ui.checkbox(label="Enable Battery", value=True)
    bat_capacity = mo.ui.slider(
        start=5.0, stop=20.0, step=0.5, value=13.5, label="Capacity (kWh)"
    )
    bat_max_power = mo.ui.slider(
        start=2.0, stop=10.0, step=0.5, value=5.0, label="Max power (kW)"
    )

    ev_enable = mo.ui.checkbox(label="Enable EV", value=False)
    ev_capacity = mo.ui.slider(
        start=30, stop=100, step=5, value=75, label="Battery (kWh)"
    )
    ev_charger_kw = mo.ui.number(
        start=1.0, stop=20.0, step=0.1, value=7.68, label="Charger (kW)"
    )

    season = mo.ui.dropdown(
        options={
            "Winter (Jan 7–20)": "winter",
            "Spring (Apr 8–21)": "spring",
            "Summer (Jul 8–21)": "summer",
            "Fall (Oct 8–21)": "fall",
        },
        value="Summer (Jul 8–21)",
        label="Season",
    )
    seed = mo.ui.number(start=0, stop=99999, step=1, value=42, label="Seed")
    ochre_compare = mo.ui.checkbox(label="Compare with OCHRE", value=False)
    run_btn = mo.ui.run_button(label="Run simulation", kind="success", full_width=True)

    return (
        pv_enable, pv_capacity, pv_tilt, pv_azimuth,
        bat_enable, bat_capacity, bat_max_power,
        ev_enable, ev_capacity, ev_charger_kw,
        season, seed, ochre_compare, run_btn,
    )


@app.cell
def _controls_layout(
    mo,
    pv_enable, pv_capacity, pv_tilt, pv_azimuth,
    bat_enable, bat_capacity, bat_max_power,
    ev_enable, ev_capacity, ev_charger_kw,
    season, seed, ochre_compare, run_btn,
):
    controls_panel = mo.vstack(
        [
            mo.md("### Simulation"),
            season,
            seed,
            mo.md("### PV"),
            pv_enable,
            pv_capacity,
            pv_tilt,
            pv_azimuth,
            mo.md("### Battery"),
            bat_enable,
            bat_capacity,
            bat_max_power,
            mo.md("### EV"),
            ev_enable,
            ev_capacity,
            ev_charger_kw,
            mo.md("### Options"),
            ochre_compare,
            run_btn,
        ],
        gap="0.5rem",
    )
    return (controls_panel,)


@app.cell
def _sim_gate(mo, run_btn):
    mo.stop(
        not run_btn.value,
        mo.callout(mo.md("Configure the simulation and click **Run simulation**."), kind="neutral"),
    )


@app.cell
def _run_simulation(
    mo,
    bldg_hpxml, bldg_schedule, bldg_weather, bldg_label,
    pv_enable, pv_capacity, pv_tilt, pv_azimuth,
    bat_enable, bat_capacity, bat_max_power,
    ev_enable, ev_capacity, ev_charger_kw,
    season, seed, ochre_compare,
    HARES_DEFAULTS, VENDOR_OCHRE,
    pl, sys,
):
    from ochre_next import Dwelling, PV, Battery, EV, ControlSignal

    _SEASON_DATES: dict[str, str] = {
        "winter": "2019-01-07",
        "spring": "2019-04-08",
        "summer": "2019-07-08",
        "fall": "2019-10-08",
    }

    @mo.cache
    def _run(
        hpxml: str,
        schedule: str,
        weather: str,
        label: str,
        season_key: str,
        seed_val: int,
        pv_on: bool,
        pv_kw: float,
        pv_tilt_deg: float,
        pv_az_deg: float,
        bat_on: bool,
        bat_kwh: float,
        bat_max_kw: float,
        ev_on: bool,
        ev_kwh: float,
        ev_kw: float,
        do_ochre: bool,
        defaults: str,
        vendor_ochre: str,
    ):
        start_date = _SEASON_DATES[season_key]

        with mo.status.spinner(title="Running HARES simulation..."):
            dw = Dwelling.from_hpxml(
                hpxml,
                schedule,
                weather,
                start_time=f"{start_date}T00:00:00",
                duration_s=14 * 86400,
                time_res_s=900,
                defaults_path=defaults,
                bldg_id=42,
                master_seed=seed_val,
                output_verbosity=3,
            )
            dw.initialize()
            if pv_on:
                dw.add_pv(PV("PV", pv_kw, pv_tilt_deg, pv_az_deg))
            if bat_on:
                dw.add_battery(
                    Battery("Battery", bat_kwh, max_charge_kw=bat_max_kw, max_discharge_kw=bat_max_kw)
                )
                dw.apply_control("Battery", ControlSignal.self_consumption(True, False))
            if ev_on:
                dw.add_ev(EV("EV", capacity_kwh=ev_kwh, max_charging_kw=ev_kw))
            hares_df = dw.simulate()

        ochre_df = None
        if do_ochre:
            import datetime as _dt
            with mo.status.spinner(title="Running OCHRE comparison..."):
                sys.path.insert(0, vendor_ochre)
                from ochre import Dwelling as OchreDwelling  # type: ignore[import-not-found]

                _start = _dt.datetime.strptime(start_date, "%Y-%m-%d")
                _ochre_dw = OchreDwelling(
                    name="compare",
                    start_time=_start,
                    time_res=_dt.timedelta(minutes=15),
                    duration=_dt.timedelta(days=14),
                    hpxml_file=hpxml,
                    hpxml_schedule_file=schedule,
                    weather_file=weather,
                    verbosity=6,
                    save_results=False,
                )
                _pdf, _, _ = _ochre_dw.simulate()
                ochre_df = pl.from_pandas(_pdf.reset_index())

        sim_meta = {
            "scenario": label,
            "start_date": start_date,
            "season": season_key,
            "pv_enabled": pv_on,
            "pv_kw": pv_kw if pv_on else 0.0,
            "battery_enabled": bat_on,
            "battery_kwh": bat_kwh if bat_on else 0.0,
            "battery_max_kw": bat_max_kw if bat_on else 0.0,
            "ev_enabled": ev_on,
            "ev_kwh": ev_kwh if ev_on else 0.0,
            "ev_charger_kw": ev_kw if ev_on else 0.0,
        }
        return hares_df, ochre_df, sim_meta

    hares_df, ochre_df, sim_meta = _run(
        hpxml=str(bldg_hpxml),
        schedule=str(bldg_schedule),
        weather=str(bldg_weather),
        label=bldg_label,
        season_key=str(season.value),
        seed_val=int(seed.value),
        pv_on=bool(pv_enable.value),
        pv_kw=float(pv_capacity.value),
        pv_tilt_deg=float(pv_tilt.value),
        pv_az_deg=float(pv_azimuth.value),
        bat_on=bool(bat_enable.value),
        bat_kwh=float(bat_capacity.value),
        bat_max_kw=float(bat_max_power.value),
        ev_on=bool(ev_enable.value),
        ev_kwh=float(ev_capacity.value),
        ev_kw=float(ev_charger_kw.value),
        do_ochre=bool(ochre_compare.value),
        defaults=str(HARES_DEFAULTS),
        vendor_ochre=str(VENDOR_OCHRE),
    )

    return hares_df, ochre_df, sim_meta


@app.cell
def _visualizations(
    mo,
    hares_df, ochre_df, sim_meta,
    bldg_weather,
    go, make_subplots, np, pl,
):
    from ochre_next import parse_epw

    _time_col = "Time"
    _power_col = "Total Electric Power (kW)"
    _pv_col = "PV Electric Power (kW)"
    _bat_pow_col = "Battery Electric Power (kW)"
    _bat_soc_col = "Battery SOC (-)"
    _ev_pow_col = "EV Electric Power (kW)"
    _ev_soc_col = "EV SOC (-)"

    def _net_power_temp():
        times = hares_df[_time_col].to_list()
        power = hares_df[_power_col].to_numpy()

        consumption = [max(v, 0.0) for v in power]
        export = [min(v, 0.0) for v in power]

        fig = make_subplots(
            rows=2, cols=1,
            shared_xaxes=True,
            row_heights=[0.7, 0.3],
            vertical_spacing=0.05,
        )

        fig.add_trace(
            go.Scatter(
                x=times, y=consumption,
                name="Consumption",
                fill="tozeroy",
                fillcolor="rgba(220,80,60,0.3)",
                line={"color": "rgba(220,80,60,0.8)", "width": 1},
            ),
            row=1, col=1,
        )
        fig.add_trace(
            go.Scatter(
                x=times, y=export,
                name="Export",
                fill="tozeroy",
                fillcolor="rgba(30,120,200,0.3)",
                line={"color": "rgba(30,120,200,0.8)", "width": 1},
            ),
            row=1, col=1,
        )
        if ochre_df is not None and _power_col in ochre_df.columns:
            _ochre_times = ochre_df[_time_col].to_list()
            _ochre_power = ochre_df[_power_col].to_numpy()
            fig.add_trace(
                go.Scatter(
                    x=_ochre_times, y=_ochre_power,
                    name="OCHRE (base load)",
                    line={"color": "gray", "dash": "dash", "width": 1},
                ),
                row=1, col=1,
            )

        try:
            _wts = parse_epw(str(bldg_weather))
            _wdf = _wts.to_polars()
            if "dry_bulb_c" in _wdf.columns:
                import datetime as _dt
                _start = _dt.datetime.fromisoformat(sim_meta["start_date"])
                _hours = [_start + _dt.timedelta(hours=i) for i in range(len(_wdf))]
                _end = _start + _dt.timedelta(days=14)
                _mask = [_start <= h < _end for h in _hours]
                _w_times = [h for h, m in zip(_hours, _mask) if m]
                _w_temps = _wdf["dry_bulb_c"].to_numpy()
                _w_temps = [float(_w_temps[i]) for i, m in enumerate(_mask) if m]
                fig.add_trace(
                    go.Scatter(
                        x=_w_times, y=_w_temps,
                        name="Outdoor temp (°C)",
                        fill="tozeroy",
                        fillcolor="rgba(255,152,0,0.15)",
                        line={"color": "darkorange", "width": 1},
                    ),
                    row=2, col=1,
                )
        except Exception:
            pass

        fig.update_yaxes(title_text="Power (kW)", row=1, col=1)
        fig.update_yaxes(title_text="Temp (°C)", row=2, col=1)
        fig.update_layout(
            title="Net Electric Power & Outdoor Temperature",
            hovermode="x unified",
            margin={"t": 50},
        )
        return fig

    def _der_breakdown():
        times = hares_df[_time_col].to_list()
        total = hares_df[_power_col].to_numpy()

        fig = go.Figure()

        known_ders = np.zeros(len(total))

        if _pv_col in hares_df.columns:
            pv_raw = hares_df[_pv_col].to_numpy()
            pv_gen = -pv_raw
            known_ders += pv_raw
            fig.add_trace(go.Scatter(
                x=times, y=pv_gen,
                name="PV generation",
                fill="tozeroy",
                fillcolor="rgba(255,210,0,0.4)",
                line={"color": "rgba(255,180,0,0.9)", "width": 1},
            ))

        if _bat_pow_col in hares_df.columns:
            bat_pow = hares_df[_bat_pow_col].to_numpy()
            known_ders += bat_pow
            fig.add_trace(go.Scatter(
                x=times, y=bat_pow,
                name="Battery power",
                line={"color": "rgba(40,160,80,0.9)", "width": 1.5},
            ))

        if _ev_pow_col in hares_df.columns:
            ev_pow = hares_df[_ev_pow_col].to_numpy()
            known_ders += ev_pow
            fig.add_trace(go.Scatter(
                x=times, y=ev_pow,
                name="EV charging",
                fill="tozeroy",
                fillcolor="rgba(120,60,200,0.3)",
                line={"color": "rgba(120,60,200,0.8)", "width": 1},
            ))

        base_load = total - known_ders
        fig.add_trace(go.Scatter(
            x=times, y=base_load,
            name="Base load",
            fill="tozeroy",
            fillcolor="rgba(120,120,120,0.25)",
            line={"color": "rgba(80,80,80,0.7)", "width": 1},
        ))

        fig.update_layout(
            title="DER Breakdown",
            yaxis_title="Power (kW)",
            hovermode="x unified",
            margin={"t": 50},
        )
        return fig

    def _battery_soc():
        if _bat_soc_col not in hares_df.columns:
            return mo.callout(mo.md("No battery in this simulation."), kind="neutral")

        times = hares_df[_time_col].to_list()
        soc_pct = (hares_df[_bat_soc_col] * 100.0).to_numpy()

        fig = make_subplots(
            rows=2, cols=1,
            shared_xaxes=True,
            row_heights=[0.5, 0.5],
            vertical_spacing=0.05,
        )
        fig.add_trace(
            go.Scatter(
                x=times, y=soc_pct,
                name="SOC (%)",
                fill="tozeroy",
                fillcolor="rgba(40,160,80,0.3)",
                line={"color": "rgba(40,160,80,0.9)", "width": 1.5},
            ),
            row=1, col=1,
        )

        if _bat_pow_col in hares_df.columns:
            bat_pow = hares_df[_bat_pow_col].to_numpy()
            charge = [max(v, 0.0) for v in bat_pow]
            discharge = [min(v, 0.0) for v in bat_pow]
            fig.add_trace(
                go.Scatter(
                    x=times, y=charge,
                    name="Charging (kW)",
                    fill="tozeroy",
                    fillcolor="rgba(255,140,0,0.3)",
                    line={"color": "rgba(255,140,0,0.8)", "width": 1},
                ),
                row=2, col=1,
            )
            fig.add_trace(
                go.Scatter(
                    x=times, y=discharge,
                    name="Discharging (kW)",
                    fill="tozeroy",
                    fillcolor="rgba(30,100,200,0.3)",
                    line={"color": "rgba(30,100,200,0.8)", "width": 1},
                ),
                row=2, col=1,
            )

        fig.update_yaxes(title_text="SOC (%)", row=1, col=1)
        fig.update_yaxes(title_text="Power (kW)", row=2, col=1)
        fig.update_layout(
            title="Battery State of Charge",
            hovermode="x unified",
            margin={"t": 50},
        )
        return fig

    def _ev_charging():
        if _ev_pow_col not in hares_df.columns:
            return mo.callout(mo.md("No EV in this simulation."), kind="neutral")

        times = hares_df[_time_col].to_list()

        fig = make_subplots(
            rows=2, cols=1,
            shared_xaxes=True,
            row_heights=[0.5, 0.5],
            vertical_spacing=0.05,
        )
        ev_pow = hares_df[_ev_pow_col].to_numpy()
        fig.add_trace(
            go.Scatter(
                x=times, y=ev_pow,
                name="Charge power (kW)",
                fill="tozeroy",
                fillcolor="rgba(120,60,200,0.3)",
                line={"color": "rgba(120,60,200,0.8)", "width": 1.5},
            ),
            row=1, col=1,
        )

        if _ev_soc_col in hares_df.columns:
            ev_soc_pct = (hares_df[_ev_soc_col] * 100.0).to_numpy()
            fig.add_trace(
                go.Scatter(
                    x=times, y=ev_soc_pct,
                    name="SOC (%)",
                    fill="tozeroy",
                    fillcolor="rgba(30,100,200,0.3)",
                    line={"color": "rgba(30,100,200,0.8)", "width": 1.5},
                ),
                row=2, col=1,
            )

        fig.update_yaxes(title_text="Power (kW)", row=1, col=1)
        fig.update_yaxes(title_text="SOC (%)", row=2, col=1)
        fig.update_layout(
            title="EV Charging",
            hovermode="x unified",
            margin={"t": 50},
        )
        return fig

    def _daily_energy():
        daily = (
            hares_df
            .with_columns(pl.col(_time_col).dt.date().alias("date"))
            .group_by("date")
            .agg((pl.col(_power_col) * (15.0 / 60.0)).sum().alias("kWh"))
            .sort("date")
        )
        dates = daily["date"].to_list()
        kwh = daily["kWh"].to_list()
        colors = ["rgba(220,80,60,0.75)" if v >= 0 else "rgba(30,120,200,0.75)" for v in kwh]

        fig = go.Figure(go.Bar(
            x=dates, y=kwh,
            marker_color=colors,
            name="Daily energy (kWh)",
        ))
        fig.update_layout(
            title="Daily Net Energy (positive = consumption, negative = export)",
            yaxis_title="Energy (kWh)",
            margin={"t": 50},
        )
        return fig

    def _summary():
        power = hares_df[_power_col].to_numpy()
        total_kwh = float((hares_df[_power_col] * (15.0 / 60.0)).sum())
        peak_kw = float(power.max())
        min_kw = float(power.min())
        avg_kw = float(power.mean())

        perf_rows = [
            {"Metric": "Total net energy", "Value": f"{total_kwh:.1f} kWh"},
            {"Metric": "Peak demand", "Value": f"{peak_kw:.2f} kW"},
            {"Metric": "Min power (export peak)", "Value": f"{min_kw:.2f} kW"},
            {"Metric": "Average power", "Value": f"{avg_kw:.2f} kW"},
        ]
        perf_table = mo.ui.table(perf_rows, label="Performance metrics")

        der_rows = [
            {"Parameter": "Scenario", "Value": str(sim_meta.get("scenario", ""))},
            {"Parameter": "Start date", "Value": str(sim_meta.get("start_date", ""))},
            {"Parameter": "PV enabled", "Value": str(sim_meta.get("pv_enabled", False))},
            {"Parameter": "PV capacity (kW)", "Value": str(sim_meta.get("pv_kw", 0.0))},
            {"Parameter": "Battery enabled", "Value": str(sim_meta.get("battery_enabled", False))},
            {"Parameter": "Battery capacity (kWh)", "Value": str(sim_meta.get("battery_kwh", 0.0))},
            {"Parameter": "Battery max power (kW)", "Value": str(sim_meta.get("battery_max_kw", 0.0))},
            {"Parameter": "EV enabled", "Value": str(sim_meta.get("ev_enabled", False))},
            {"Parameter": "EV capacity (kWh)", "Value": str(sim_meta.get("ev_kwh", 0.0))},
            {"Parameter": "EV charger (kW)", "Value": str(sim_meta.get("ev_charger_kw", 0.0))},
        ]
        der_table = mo.ui.table(der_rows, label="DER configuration")

        elements: list[object] = [perf_table, der_table]
        _any_der = sim_meta.get("pv_enabled") or sim_meta.get("battery_enabled") or sim_meta.get("ev_enabled")
        if ochre_df is not None and _any_der:
            elements.insert(
                0,
                mo.callout(
                    mo.md("OCHRE comparison runs base-load only (no DERs). Power values will differ from HARES with DERs enabled."),
                    kind="warn",
                ),
            )
        return mo.vstack(elements, gap="1rem")

    results_tabs = mo.ui.tabs(
        {
            "Net Power & Temp": mo.lazy(_net_power_temp),
            "DER Breakdown": mo.lazy(_der_breakdown),
            "Battery SOC": mo.lazy(_battery_soc),
            "EV Charging": mo.lazy(_ev_charging),
            "Daily Energy": mo.lazy(_daily_energy),
            "Summary": mo.lazy(_summary),
        }
    )
    results_tabs
    return (results_tabs,)


@app.cell
def _main_layout(mo, controls_panel, results_tabs):
    mo.hstack(
        [controls_panel, results_tabs],
        widths=[1, 2],
        gap="2rem",
        align="start",
    )


if __name__ == "__main__":
    app.run()
