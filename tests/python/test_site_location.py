"""Tests for explicit site-location specification via the Python bindings.

The site-location resolver (override → HPXML → weather → coordinate lookup)
determines the latitude/longitude/timezone that drive solar position. These
tests verify that the Python API exposes the explicit-override escape hatch
correctly through `SimulationConfig(site_location=...)` — one frozen
`SiteLocationOverride` object whose constructor runs the same site-location
check `validate()` runs.
"""

from __future__ import annotations

from typing import Any, cast

import pytest

from conftest import make_dwelling
from hares import SimulationConfig, SiteLocationOverride

START = "2019-01-01T00:00:00"


class TestSiteLocationOverride:
    """`SiteLocationOverride` carries the four explicit fields."""

    def test_partial_override_accepted(self):
        # Only the UTC offset overridden; lat/lon come from HPXML/weather.
        site = SiteLocationOverride(utc_offset_h=-7.0)
        assert site.utc_offset_h == pytest.approx(-7.0)
        assert site.latitude_deg is None

    def test_all_fields_accepted(self):
        site = SiteLocationOverride(
            latitude_deg=33.52, longitude_deg=-86.81, elevation_m=180.0, utc_offset_h=-6.0
        )
        assert site.latitude_deg == pytest.approx(33.52)
        assert site.longitude_deg == pytest.approx(-86.81)
        assert site.elevation_m == pytest.approx(180.0)
        assert site.utc_offset_h == pytest.approx(-6.0)

    @pytest.mark.parametrize(
        "kwargs",
        [
            {"latitude_deg": 91.0},
            {"latitude_deg": -91.0},
            {"longitude_deg": 181.0},
            {"longitude_deg": -181.0},
            {"elevation_m": -1000.0},
            {"elevation_m": 10000.0},
            {"utc_offset_h": 15.0},
            {"utc_offset_h": -15.0},
            {"latitude_deg": float("nan")},
            {"utc_offset_h": float("inf")},
        ],
    )
    def test_out_of_range_values_raise(self, kwargs):
        with pytest.raises(ValueError, match="must be finite and within"):
            SiteLocationOverride(**kwargs)

    def test_unknown_keyword_is_type_error(self):
        with pytest.raises(TypeError, match="definitely_not_a_kwarg"):
            SiteLocationOverride(**cast(dict[str, Any], {"definitely_not_a_kwarg": 1.0}))


class TestFromHpxmlSiteLocation:
    """`Dwelling.from_hpxml` accepts the override through the config."""

    def test_accepts_site_location_through_the_config(self):
        sim_config = SimulationConfig(
            start_time=START,
            duration=300,
            site_location=SiteLocationOverride(
                latitude_deg=33.52, longitude_deg=-86.81, elevation_m=180.0, utc_offset_h=-6.0
            ),
        )
        dw = make_dwelling(sim_config=sim_config)
        dw.initialize()
        # A dwelling that initializes successfully with the override accepted
        # is the contract; the resolved values drive solar geometry internally.
        assert dw is not None

    def test_partial_override_accepted(self):
        sim_config = SimulationConfig(
            start_time=START,
            duration=300,
            site_location=SiteLocationOverride(utc_offset_h=-7.0),
        )
        dw = make_dwelling(sim_config=sim_config)
        dw.initialize()
        assert dw is not None

    @pytest.mark.parametrize(
        "kwargs",
        [
            {"latitude_deg": 91.0},
            {"longitude_deg": 181.0},
            {"elevation_m": -1000.0},
            {"utc_offset_h": 15.0},
            {"latitude_deg": float("nan")},
        ],
    )
    def test_out_of_range_values_raise_at_the_override(self, kwargs):
        # The check runs in the SiteLocationOverride constructor, before any
        # dwelling is built: one check, one message, on every surface.
        with pytest.raises(ValueError, match="must be finite and within"):
            SiteLocationOverride(**kwargs)

    def test_removed_site_kwargs_name_site_location(self):
        """The old `latitude=`/`longitude=`/`elevation_m=`/`utc_offset_h=`
        keywords on the config constructor name their replacement."""
        for key in ("latitude", "longitude", "elevation_m", "utc_offset_h"):
            with pytest.raises(TypeError, match="site_location"):
                # The constructor rejects the key before any value
                # extraction, so the value's type never matters.
                SimulationConfig(
                    start_time=START, duration=300, **cast(dict[str, Any], {key: 1.0})
                )


class TestSimulationConfigSiteFields:
    """`SimulationConfig` carries the site override as one field."""

    def test_construct_with_site_location(self):
        cfg = SimulationConfig(
            start_time=START,
            duration=3600,
            site_location=SiteLocationOverride(
                latitude_deg=33.52, longitude_deg=-86.81, elevation_m=180.0, utc_offset_h=-6.0
            ),
        )
        assert cfg.site_location.latitude_deg == pytest.approx(33.52)
        assert cfg.site_location.longitude_deg == pytest.approx(-86.81)
        assert cfg.site_location.elevation_m == pytest.approx(180.0)
        assert cfg.site_location.utc_offset_h == pytest.approx(-6.0)

    def test_site_location_defaults_to_the_empty_override(self):
        cfg = SimulationConfig(start_time=START, duration=3600)
        assert cfg.site_location.latitude_deg is None
        assert cfg.site_location.longitude_deg is None
        assert cfg.site_location.elevation_m is None
        assert cfg.site_location.utc_offset_h is None

    def test_setter_validates_range(self):
        cfg = SimulationConfig(start_time=START, duration=3600)
        cfg.site_location = SiteLocationOverride(latitude_deg=40.0)
        assert cfg.site_location.latitude_deg == pytest.approx(40.0)
        with pytest.raises(ValueError, match="must be finite and within"):
            SiteLocationOverride(latitude_deg=200.0)
        with pytest.raises(ValueError, match="must be finite and within"):
            SiteLocationOverride(utc_offset_h=99.0)

    def test_constructor_rejects_out_of_range(self):
        with pytest.raises(ValueError, match="longitude"):
            SimulationConfig(
                start_time=START,
                duration=3600,
                site_location=SiteLocationOverride(longitude_deg=999.0),
            )
