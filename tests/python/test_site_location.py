"""Tests for explicit site-location specification via the Python bindings.

The site-location resolver (override → HPXML → weather → coordinate lookup)
determines the latitude/longitude/timezone that drive solar position. These
tests verify that the Python API exposes the explicit-override escape hatch
correctly through both `Dwelling.from_hpxml(...)` kwargs and
`SimulationConfig(...)`.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from conftest import make_dwelling

ROOT = Path(__file__).resolve().parents[2]


class TestFromHpxmlSiteKwargs:
    """`Dwelling.from_hpxml` accepts explicit site-location kwargs."""

    def test_accepts_all_site_location_kwargs(self):
        dw = make_dwelling(
            latitude=33.52,
            longitude=-86.81,
            elevation_m=180.0,
            utc_offset_h=-6.0,
        )
        dw.initialize()
        # A dwelling that initializes successfully with the override accepted
        # is the contract; the resolved values drive solar geometry internally.
        assert dw is not None

    def test_partial_override_accepted(self):
        # Only the UTC offset overridden; lat/lon come from HPXML/weather.
        dw = make_dwelling(utc_offset_h=-7.0)
        dw.initialize()
        assert dw is not None

    @pytest.mark.parametrize(
        "kwargs",
        [
            {"latitude": 91.0},
            {"latitude": -91.0},
            {"longitude": 181.0},
            {"longitude": -181.0},
            {"elevation_m": -1000.0},
            {"elevation_m": 10000.0},
            {"utc_offset_h": 15.0},
            {"utc_offset_h": -15.0},
            {"latitude": float("nan")},
            {"utc_offset_h": float("inf")},
        ],
    )
    def test_out_of_range_values_raise(self, kwargs):
        with pytest.raises((ValueError, Exception)):
            make_dwelling(**kwargs)

    def test_unknown_kwarg_still_rejected(self):
        # Guard that adding the site kwargs did not loosen kwarg validation.
        with pytest.raises(Exception):
            make_dwelling(definitely_not_a_kwarg=1.0)


class TestSimulationConfigSiteFields:
    """`SimulationConfig` exposes the four site-location fields."""

    def test_construct_with_site_fields(self):
        from ochre_next import SimulationConfig

        cfg = SimulationConfig(
            duration_s=3600,
            time_res_s=60,
            latitude=33.52,
            longitude=-86.81,
            elevation_m=180.0,
            utc_offset_h=-6.0,
        )
        assert cfg.latitude == pytest.approx(33.52)
        assert cfg.longitude == pytest.approx(-86.81)
        assert cfg.elevation_m == pytest.approx(180.0)
        assert cfg.utc_offset_h == pytest.approx(-6.0)

    def test_site_fields_default_none(self):
        from ochre_next import SimulationConfig

        cfg = SimulationConfig(duration_s=3600, time_res_s=60)
        assert cfg.latitude is None
        assert cfg.longitude is None
        assert cfg.elevation_m is None
        assert cfg.utc_offset_h is None

    def test_setters_validate_range(self):
        from ochre_next import SimulationConfig

        cfg = SimulationConfig(duration_s=3600, time_res_s=60)
        cfg.latitude = 40.0
        assert cfg.latitude == pytest.approx(40.0)
        with pytest.raises((ValueError, Exception)):
            cfg.latitude = 200.0
        with pytest.raises((ValueError, Exception)):
            cfg.utc_offset_h = 99.0

    def test_constructor_rejects_out_of_range(self):
        from ochre_next import SimulationConfig

        with pytest.raises((ValueError, Exception)):
            SimulationConfig(duration_s=3600, time_res_s=60, longitude=999.0)
