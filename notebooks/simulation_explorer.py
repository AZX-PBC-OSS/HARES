import marimo

__generated_with = "0.20.4"
app = marimo.App(width="medium")


@app.cell
def _imports():
    import datetime
    import sys
    from pathlib import Path

    import marimo as mo
    import plotly.graph_objects as go
    import polars as pl
    from plotly.subplots import make_subplots

    return datetime, mo, Path, pl, go, make_subplots, sys


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


# ---------------------------------------------------------------------------
# Building source
# ---------------------------------------------------------------------------


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


# ---------------------------------------------------------------------------
# DER configuration widgets
# ---------------------------------------------------------------------------


@app.cell
def _pv_widgets(mo):
    pv_mode = mo.ui.radio(
        options={
            "Off": "off",
            "Auto-size (roof-aware)": "auto",
            "Manual": "manual",
        },
        value="Auto-size (roof-aware)",
        label="PV mode",
    )
    pv_capacity = mo.ui.slider(
        start=0.5, stop=14.0, step=0.5, value=6.0, label="Capacity (kW)"
    )
    pv_tilt = mo.ui.slider(
        start=0, stop=45, step=1, value=26, label="Tilt (deg)"
    )
    pv_azimuth = mo.ui.slider(
        start=90, stop=270, step=1, value=180, label="Azimuth (deg)"
    )
    return pv_mode, pv_capacity, pv_tilt, pv_azimuth


@app.cell
def _battery_widgets(mo):
    bat_mode = mo.ui.radio(
        options={
            "Off": "off",
            "From catalog": "catalog",
            "Manual": "manual",
        },
        value="From catalog",
        label="Battery mode",
    )
    bat_product = mo.ui.dropdown(
        options={
            "Tesla Powerwall 3": "tesla_pw3",
            "Tesla Powerwall 2": "tesla_pw2",
            "Tesla Powerwall 3 x2": "tesla_pw3_x2",
            "Enphase IQ 5P": "enphase_iq5p",
            "Enphase IQ 5P x2": "enphase_iq5p_x2",
            "Enphase IQ 10C": "enphase_iq10c",
            "Franklin aPower": "franklin_apower",
            "Franklin aPower2": "franklin_apower2",
            "Franklin aPower2 x2": "franklin_apower2_x2",
            "SolarEdge Home": "solaredge_home",
            "LG RESU 10H": "lg_resu10h",
        },
        value="Tesla Powerwall 3",
        label="Product",
    )
    bat_capacity = mo.ui.slider(
        start=5.0, stop=30.0, step=0.5, value=13.5, label="Capacity (kWh)"
    )
    bat_max_power = mo.ui.slider(
        start=2.0, stop=12.0, step=0.5, value=5.0, label="Max power (kW)"
    )

    bat_control = mo.ui.dropdown(
        options={
            "Self-consumption": "self_consumption",
            "TOU optimization": "tou",
            "Backup reserve": "backup",
        },
        value="Self-consumption",
        label="Control mode",
    )
    bat_export = mo.ui.dropdown(
        options={
            "Unrestricted": "unrestricted",
            "Solar only": "solar_only",
            "Disabled": "disabled",
        },
        value="Unrestricted",
        label="Grid export",
    )
    return bat_mode, bat_product, bat_capacity, bat_max_power, bat_control, bat_export


@app.cell
def _ev_widgets(mo):
    ev_mode = mo.ui.radio(
        options={
            "Off": "off",
            "Vehicle + archetype": "archetype",
            "Manual": "manual",
        },
        value="Off",
        label="EV mode",
    )
    ev_vehicle = mo.ui.dropdown(
        options={
            "Tesla Model Y LR (77 kWh)": "tesla_model_y_lr",
            "Tesla Model Y SR (57 kWh)": "tesla_model_y_sr",
            "Tesla Model 3 LR (75 kWh)": "tesla_model_3_lr",
            "Chevy Bolt EV (65 kWh)": "chevy_bolt_ev",
            "Chevy Bolt EUV (65 kWh)": "chevy_bolt_euv",
            "Ford Mach-E SR (72 kWh)": "ford_mache_sr",
            "Ford Mach-E ER (91 kWh)": "ford_mache_er",
            "Ford Lightning ER (131 kWh)": "ford_lightning_er",
            "Hyundai Ioniq 5 LR (77 kWh)": "hyundai_ioniq5_lr",
            "Nissan Leaf (30 kWh)": "nissan_leaf30",
            "Jeep 4xe PHEV (17 kWh)": "jeep_4xe",
            "Toyota RAV4 Prime PHEV (18 kWh)": "toyota_rav4_prime",
            "Chevy Volt Gen1 PHEV (16 kWh)": "chevy_volt_gen1",
        },
        value="Tesla Model Y LR (77 kWh)",
        label="Vehicle",
    )
    ev_archetype = mo.ui.dropdown(
        options={
            "Daily commuter (L2)": "daily_commuter_l2",
            "Daily commuter (L1)": "daily_commuter_l1",
            "Long commuter (L2)": "long_commuter_l2",
            "WFH occasional": "wfh_occasional",
            "WFH L1 minimal": "wfh_l1_minimal",
            "Heavy-use SUV": "heavy_use_suv",
            "Shift worker": "shift_worker",
            "Weekend warrior": "weekend_warrior",
            "Workplace charger": "workplace_charger",
            "Retiree (L1)": "retiree_l1",
            "PHEV commuter": "phev_commuter",
            "TOU optimizer (CA)": "tou_optimizer_ca",
        },
        value="Daily commuter (L2)",
        label="Archetype",
    )
    ev_capacity = mo.ui.slider(
        start=15, stop=140, step=5, value=75, label="Battery (kWh)"
    )
    ev_charger_kw = mo.ui.number(
        start=1.0, stop=20.0, step=0.1, value=7.68, label="Charger (kW)"
    )
    return ev_mode, ev_vehicle, ev_archetype, ev_capacity, ev_charger_kw


@app.cell
def _sim_widgets(mo):
    season = mo.ui.dropdown(
        options={
            "Winter (Jan 7-20)": "winter",
            "Spring (Apr 8-21)": "spring",
            "Summer (Jul 8-21)": "summer",
            "Fall (Oct 8-21)": "fall",
        },
        value="Summer (Jul 8-21)",
        label="Season",
    )
    sim_days = mo.ui.slider(
        start=1, stop=30, step=1, value=14, label="Duration (days)"
    )
    time_res = mo.ui.dropdown(
        options={
            "1 min": 60,
            "5 min": 300,
            "15 min": 900,
            "1 hour": 3600,
        },
        value="15 min",
        label="Timestep",
    )
    seed = mo.ui.number(start=0, stop=99999, step=1, value=42, label="Seed")
    ochre_compare = mo.ui.checkbox(label="Compare with OCHRE", value=False)
    run_btn = mo.ui.run_button(label="Run simulation", kind="success", full_width=True)
    return season, sim_days, time_res, seed, ochre_compare, run_btn


# ---------------------------------------------------------------------------
# Controls layout
# ---------------------------------------------------------------------------


@app.cell
def _controls_layout(
    mo,
    pv_mode, pv_capacity, pv_tilt, pv_azimuth,
    bat_mode, bat_product, bat_capacity, bat_max_power, bat_control, bat_export,
    ev_mode, ev_vehicle, ev_archetype, ev_capacity, ev_charger_kw,
    season, sim_days, time_res, seed, ochre_compare, run_btn,
):
    pv_section = [mo.md("### PV"), pv_mode]
    if pv_mode.value == "manual":
        pv_section.extend([pv_capacity, pv_tilt, pv_azimuth])

    bat_section = [mo.md("### Battery"), bat_mode]
    if bat_mode.value == "catalog":
        bat_section.extend([bat_product, bat_control, bat_export])
    elif bat_mode.value == "manual":
        bat_section.extend([bat_capacity, bat_max_power, bat_control, bat_export])

    ev_section = [mo.md("### EV"), ev_mode]
    if ev_mode.value == "archetype":
        ev_section.extend([ev_vehicle, ev_archetype])
    elif ev_mode.value == "manual":
        ev_section.extend([ev_capacity, ev_charger_kw])

    controls_panel = mo.vstack(
        [
            mo.md("### Simulation"),
            mo.hstack([season, sim_days], gap="0.5rem"),
            mo.hstack([time_res, seed], gap="0.5rem"),
            *pv_section,
            *bat_section,
            *ev_section,
            mo.md("### Options"),
            ochre_compare,
            run_btn,
        ],
        gap="0.5rem",
    )
    return (controls_panel,)


# ---------------------------------------------------------------------------
# Run simulation
# ---------------------------------------------------------------------------


@app.cell
def _run_simulation(
    mo,
    bldg_hpxml, bldg_schedule, bldg_weather, bldg_label,
    pv_mode, pv_capacity, pv_tilt, pv_azimuth,
    bat_mode, bat_product, bat_capacity, bat_max_power, bat_control, bat_export,
    ev_mode, ev_vehicle, ev_archetype, ev_capacity, ev_charger_kw,
    season, sim_days, time_res, seed, ochre_compare, run_btn,
    HARES_DEFAULTS, VENDOR_OCHRE,
    pl, sys,
):
    mo.stop(
        not run_btn.value,
        mo.callout(mo.md("Configure the simulation and click **Run simulation**."), kind="neutral"),
    )

    from ochre_next import (
        Dwelling,
        PV,
        Battery,
        EV,
        ControlSignal,
        BmsMode,
        VehicleId,
        EvArchetypeId,
    )

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
        duration_days: int,
        time_res_s: int,
        pv_mode_val: str,
        pv_kw: float,
        pv_tilt_deg: float,
        pv_az_deg: float,
        bat_mode_val: str,
        bat_product_val: str,
        bat_kwh: float,
        bat_max_kw: float,
        bat_control_val: str,
        bat_export_val: str,
        ev_mode_val: str,
        ev_vehicle_val: str,
        ev_archetype_val: str,
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
                duration_s=duration_days * 86400,
                time_res_s=time_res_s,
                defaults_path=defaults,
                bldg_id=42,
                master_seed=seed_val,
                output_verbosity=3,
            )
            dw.initialize()

            # --- PV ---
            pv_info: dict[str, object] = {}
            if pv_mode_val == "auto":
                candidates = dw.pv_candidates()
                if candidates:
                    best = candidates[0]
                    pv_obj = PV("PV", best.max_capacity_kw, best.tilt_deg, best.azimuth_deg)
                    dw.add_pv(pv_obj)
                    pv_info = {
                        "enabled": True,
                        "kw": best.max_capacity_kw,
                        "tilt": best.tilt_deg,
                        "azimuth": best.azimuth_deg,
                        "source": "auto",
                    }
                else:
                    pv_info = {"enabled": False, "source": "auto", "reason": "no viable roof plane"}
            elif pv_mode_val == "manual":
                dw.add_pv(PV("PV", pv_kw, pv_tilt_deg, pv_az_deg))
                pv_info = {
                    "enabled": True,
                    "kw": pv_kw,
                    "tilt": pv_tilt_deg,
                    "azimuth": pv_az_deg,
                    "source": "manual",
                }

            # --- Battery ---
            bat_info: dict[str, object] = {}
            bat_name: str | None = None
            bat_charge_kw = bat_max_kw
            bat_discharge_kw = bat_max_kw
            if bat_mode_val == "catalog":
                bat_obj = Battery.by_product_id(bat_product_val)
                dw.add_battery(bat_obj)
                bat_charge_kw = float(bat_obj.max_charge_kw or 5.0)
                bat_discharge_kw = float(bat_obj.max_discharge_kw or 5.0)
                bat_info = {
                    "enabled": True,
                    "product": bat_product_val,
                    "kwh": bat_obj.capacity_kwh,
                    "max_kw": bat_charge_kw,
                    "chemistry": str(bat_obj.chemistry),
                    "control": bat_control_val,
                    "source": "catalog",
                }
                bat_name = bat_obj.name
            elif bat_mode_val == "manual":
                bat_obj = Battery("Battery", bat_kwh, max_charge_kw=bat_max_kw, max_discharge_kw=bat_max_kw)
                dw.add_battery(bat_obj)
                bat_info = {
                    "enabled": True,
                    "kwh": bat_kwh,
                    "max_kw": bat_max_kw,
                    "control": bat_control_val,
                    "source": "manual",
                }
                bat_name = "Battery"

            if bat_name is not None:
                dw.add_actor_by_name(
                    "BatteryManagement",
                    f"{bat_name}_bms",
                    {
                        "target": bat_name,
                        "mode": bat_control_val,
                        "grid_export_rule": bat_export_val,
                        "max_charge_kw": bat_charge_kw,
                        "max_discharge_kw": bat_discharge_kw,
                    },
                )

            # --- EV ---
            ev_info: dict[str, object] = {}
            if ev_mode_val == "archetype":
                vid = VehicleId.from_str(ev_vehicle_val)
                aid = EvArchetypeId.from_str(ev_archetype_val)
                dw.add_ev_with_driver(vid, aid, seed_val)
                ev_info = {
                    "enabled": True,
                    "vehicle": ev_vehicle_val,
                    "archetype": ev_archetype_val,
                    "source": "archetype",
                }
            elif ev_mode_val == "manual":
                ev_obj = EV("EV", capacity_kwh=ev_kwh, max_charging_kw=ev_kw)
                dw.add_ev(ev_obj)
                ev_info = {
                    "enabled": True,
                    "kwh": ev_kwh,
                    "charger_kw": ev_kw,
                    "source": "manual",
                }

            hares_df = dw.simulate()

        ochre_df = None
        if do_ochre:
            import datetime as _dt
            with mo.status.spinner(title="Running OCHRE comparison..."):
                if vendor_ochre not in sys.path:
                    sys.path.insert(0, vendor_ochre)
                from ochre import Dwelling as OchreDwelling  # type: ignore[import-not-found]

                _start = _dt.datetime.strptime(start_date, "%Y-%m-%d")
                _ochre_dw = OchreDwelling(
                    name="compare",
                    start_time=_start,
                    time_res=_dt.timedelta(seconds=time_res_s),
                    duration=_dt.timedelta(days=duration_days),
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
            "duration_days": duration_days,
            "time_res_s": time_res_s,
            "pv": pv_info,
            "battery": bat_info,
            "ev": ev_info,
        }
        return hares_df, ochre_df, sim_meta

    hares_df, ochre_df, sim_meta = _run(
        hpxml=str(bldg_hpxml),
        schedule=str(bldg_schedule),
        weather=str(bldg_weather),
        label=bldg_label,
        season_key=str(season.value),
        seed_val=int(seed.value),
        duration_days=int(sim_days.value),
        time_res_s=int(time_res.value),
        pv_mode_val=str(pv_mode.value),
        pv_kw=float(pv_capacity.value),
        pv_tilt_deg=float(pv_tilt.value),
        pv_az_deg=float(pv_azimuth.value),
        bat_mode_val=str(bat_mode.value),
        bat_product_val=str(bat_product.value),
        bat_kwh=float(bat_capacity.value),
        bat_max_kw=float(bat_max_power.value),
        bat_control_val=str(bat_control.value),
        bat_export_val=str(bat_export.value),
        ev_mode_val=str(ev_mode.value),
        ev_vehicle_val=str(ev_vehicle.value),
        ev_archetype_val=str(ev_archetype.value),
        ev_kwh=float(ev_capacity.value),
        ev_kw=float(ev_charger_kw.value),
        do_ochre=bool(ochre_compare.value),
        defaults=str(HARES_DEFAULTS),
        vendor_ochre=str(VENDOR_OCHRE),
    )

    return hares_df, ochre_df, sim_meta


# ---------------------------------------------------------------------------
# Visualizations
# ---------------------------------------------------------------------------


@app.cell
def _visualizations(
    mo,
    hares_df, ochre_df, sim_meta,
    bldg_weather,
    go, make_subplots, pl,
):
    from ochre_next import parse_epw

    _step_hours = sim_meta.get("time_res_s", 900) / 3600.0
    _duration_days = sim_meta.get("duration_days", 14)
    _time_col = "Time"
    _power_col = "Total Electric Power (kW)"
    _pv_col = "PV Electric Power (kW)"
    _bat_pow_col = "Battery Electric Power (kW)"
    _bat_soc_col = "Battery SOC (-)"
    _ev_pow_col = "EV Electric Power (kW)"
    _ev_soc_col = "EV SOC (-)"
    _outdoor_temp_col = "Outdoor Dry Bulb (C)"

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

        # Outdoor temperature from simulation output (verbosity >= 2)
        if _outdoor_temp_col in hares_df.columns:
            _temps = hares_df[_outdoor_temp_col].to_numpy()
            fig.add_trace(
                go.Scatter(
                    x=times, y=_temps,
                    name="Outdoor temp (C)",
                    fill="tozeroy",
                    fillcolor="rgba(255,152,0,0.15)",
                    line={"color": "darkorange", "width": 1},
                ),
                row=2, col=1,
            )
        else:
            # Fallback: parse weather file directly
            try:
                _wts = parse_epw(str(bldg_weather))
                _wdf = _wts.to_polars()
                if "dry_bulb_c" in _wdf.columns:
                    import datetime as _dt
                    _sim_start = _dt.datetime.fromisoformat(sim_meta["start_date"])
                    _epw_start = _sim_start.replace(month=1, day=1, hour=0, minute=0, second=0)
                    _hours = [_epw_start + _dt.timedelta(hours=i) for i in range(len(_wdf))]
                    _sim_end = _sim_start + _dt.timedelta(days=_duration_days)
                    _temps = _wdf["dry_bulb_c"].to_numpy()
                    _w_times = [h for h in _hours if _sim_start <= h < _sim_end]
                    _w_vals = [float(_temps[i]) for i, h in enumerate(_hours) if _sim_start <= h < _sim_end]
                    if _w_times:
                        fig.add_trace(
                            go.Scatter(
                                x=_w_times, y=_w_vals,
                                name="Outdoor temp (C)",
                                fill="tozeroy",
                                fillcolor="rgba(255,152,0,0.15)",
                                line={"color": "darkorange", "width": 1},
                            ),
                            row=2, col=1,
                        )
            except Exception:
                pass

        fig.update_yaxes(title_text="Power (kW)", row=1, col=1)
        fig.update_yaxes(title_text="Temp (C)", row=2, col=1)
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

        base_load = total.copy()

        if _pv_col in hares_df.columns:
            pv_gen = hares_df[_pv_col].to_numpy()
            base_load += pv_gen
            fig.add_trace(go.Scatter(
                x=times, y=pv_gen,
                name="PV generation (kW)",
                fill="tozeroy",
                fillcolor="rgba(255,210,0,0.4)",
                line={"color": "rgba(255,180,0,0.9)", "width": 1},
            ))

        if _bat_pow_col in hares_df.columns:
            bat_pow = hares_df[_bat_pow_col].to_numpy()
            base_load -= bat_pow
            fig.add_trace(go.Scatter(
                x=times, y=bat_pow,
                name="Battery (+ charge / - discharge)",
                line={"color": "rgba(40,160,80,0.9)", "width": 1.5},
            ))

        if _ev_pow_col in hares_df.columns:
            ev_pow = hares_df[_ev_pow_col].to_numpy()
            base_load -= ev_pow
            fig.add_trace(go.Scatter(
                x=times, y=ev_pow,
                name="EV charging",
                fill="tozeroy",
                fillcolor="rgba(120,60,200,0.3)",
                line={"color": "rgba(120,60,200,0.8)", "width": 1},
            ))
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
            .agg((pl.col(_power_col) * (_step_hours)).sum().alias("kWh"))
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
        total_kwh = float((hares_df[_power_col] * (_step_hours)).sum())
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

        # Build DER config table from structured metadata
        pv = sim_meta.get("pv", {})
        bat = sim_meta.get("battery", {})
        ev = sim_meta.get("ev", {})

        der_rows = [
            {"Parameter": "Scenario", "Value": str(sim_meta.get("scenario", ""))},
            {"Parameter": "Start date", "Value": str(sim_meta.get("start_date", ""))},
            {"Parameter": "Duration", "Value": f"{sim_meta.get('duration_days', 14)} days"},
            {"Parameter": "Timestep", "Value": f"{sim_meta.get('time_res_s', 900)} s"},
        ]
        if pv.get("enabled"):
            der_rows.extend([
                {"Parameter": "PV capacity (kW)", "Value": f"{pv.get('kw', 0):.1f}"},
                {"Parameter": "PV tilt / azimuth", "Value": f"{pv.get('tilt', 0):.0f} / {pv.get('azimuth', 0):.0f}"},
                {"Parameter": "PV source", "Value": str(pv.get("source", ""))},
            ])
        elif pv.get("reason") == "no viable roof plane":
            der_rows.append({"Parameter": "PV", "Value": "Auto-size found no viable roof plane"})
        else:
            der_rows.append({"Parameter": "PV", "Value": "Off"})

        if bat.get("enabled"):
            der_rows.extend([
                {"Parameter": "Battery capacity (kWh)", "Value": f"{bat.get('kwh', 0):.1f}"},
                {"Parameter": "Battery max power (kW)", "Value": f"{bat.get('max_kw', 0)}"},
            ])
            if bat.get("product"):
                der_rows.append({"Parameter": "Battery product", "Value": str(bat["product"])})
            if bat.get("chemistry"):
                der_rows.append({"Parameter": "Battery chemistry", "Value": str(bat["chemistry"])})
            if bat.get("control"):
                der_rows.append({"Parameter": "Battery control", "Value": str(bat["control"])})
            der_rows.append({"Parameter": "Battery source", "Value": str(bat.get("source", ""))})
        else:
            der_rows.append({"Parameter": "Battery", "Value": "Off"})

        if ev.get("enabled"):
            if ev.get("source") == "archetype":
                der_rows.extend([
                    {"Parameter": "EV vehicle", "Value": str(ev.get("vehicle", ""))},
                    {"Parameter": "EV archetype", "Value": str(ev.get("archetype", ""))},
                ])
            else:
                der_rows.extend([
                    {"Parameter": "EV capacity (kWh)", "Value": f"{ev.get('kwh', 0):.0f}"},
                    {"Parameter": "EV charger (kW)", "Value": f"{ev.get('charger_kw', 0):.1f}"},
                ])
            der_rows.append({"Parameter": "EV source", "Value": str(ev.get("source", ""))})
        else:
            der_rows.append({"Parameter": "EV", "Value": "Off"})

        der_table = mo.ui.table(der_rows, label="DER configuration")

        elements: list[object] = [perf_table, der_table]
        _any_der = pv.get("enabled") or bat.get("enabled") or ev.get("enabled")
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
