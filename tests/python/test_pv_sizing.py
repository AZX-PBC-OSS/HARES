"""Tests for PV sizing, roof plane introspection, and PV power production."""

import datetime as dt
from pathlib import Path

import ochre_next as hares
import pytest

REPO = Path(__file__).resolve().parents[2]
EXAMPLES = REPO / "data" / "examples"
HPXML = str(EXAMPLES / "BEopt_example.xml")
HPXML_PV = str(REPO / "tests" / "fixtures" / "hpxml" / "ochre_samples" / "base-pv.xml")
HPXML_ALL_NORTH = str(REPO / "tests" / "fixtures" / "hpxml" / "base-all-north.xml")
SCHEDULE = str(EXAMPLES / "BEopt_example_schedule.csv")
WEATHER = str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw")
HARES_DEFAULTS = REPO / "defaults"


@pytest.fixture
def dwelling_summer_24h():
    """Dwelling configured for a July day, 15-min steps, 24 hours."""
    from ochre_next import Dwelling

    return Dwelling.from_hpxml(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        start_time="2019-07-15T00:00:00Z",
        duration_s=86400,
        time_res_s=900,
    )


@pytest.fixture
def dwelling_winter_24h():
    """Dwelling configured for a January day, 15-min steps, 24 hours."""
    from ochre_next import Dwelling

    return Dwelling.from_hpxml(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        start_time="2019-01-15T00:00:00Z",
        duration_s=86400,
        time_res_s=900,
    )


@pytest.fixture
def dwelling_all_north():
    """Dwelling with both roof planes facing north (azimuth 0°)."""
    from ochre_next import Dwelling

    return Dwelling.from_hpxml(
        hpxml=HPXML_ALL_NORTH,
        schedule=SCHEDULE,
        weather=WEATHER,
        start_time="2019-07-15T00:00:00Z",
        duration_s=86400,
        time_res_s=900,
    )


# ---------------------------------------------------------------------------
# Roof plane introspection
# ---------------------------------------------------------------------------


class TestRoofPlanes:
    def test_roof_planes_returns_nonempty(self, dwelling_summer_24h):
        planes = dwelling_summer_24h.roof_planes()
        assert len(planes) >= 1

    def test_roof_planes_have_area(self, dwelling_summer_24h):
        for p in dwelling_summer_24h.roof_planes():
            assert p.area_m2 > 0

    def test_roof_planes_have_tilt(self, dwelling_summer_24h):
        for p in dwelling_summer_24h.roof_planes():
            assert 0 <= p.tilt_deg <= 90

    def test_roof_planes_have_boundary_index(self, dwelling_summer_24h):
        for p in dwelling_summer_24h.roof_planes():
            assert p.boundary_index is not None
            assert isinstance(p.boundary_index, int)

    def test_beopt_has_two_roof_planes(self, dwelling_summer_24h):
        planes = dwelling_summer_24h.roof_planes()
        assert len(planes) == 2
        azimuths = {p.azimuth_deg for p in planes}
        assert 0.0 in azimuths  # north
        assert 180.0 in azimuths  # south

    def test_repr_contains_area(self, dwelling_summer_24h):
        planes = dwelling_summer_24h.roof_planes()
        r = repr(planes[0])
        assert "area_m2" in r
        assert "RoofPlane" in r


# ---------------------------------------------------------------------------
# PV candidate enumeration
# ---------------------------------------------------------------------------


class TestPvCandidates:
    def test_candidates_exclude_north(self, dwelling_summer_24h):
        candidates = dwelling_summer_24h.pv_candidates()
        for c in candidates:
            # No north-facing candidates (az < 46 or az > 314).
            assert not (c.azimuth_deg >= 315 or c.azimuth_deg <= 45)

    def test_beopt_has_one_south_candidate(self, dwelling_summer_24h):
        candidates = dwelling_summer_24h.pv_candidates()
        assert len(candidates) == 1
        assert abs(candidates[0].azimuth_deg - 180.0) < 1.0

    def test_candidates_have_capacity(self, dwelling_summer_24h):
        for c in dwelling_summer_24h.pv_candidates():
            assert c.max_capacity_kw > 0
            assert c.max_panels > 0

    def test_candidates_sorted_by_score(self, dwelling_summer_24h):
        candidates = dwelling_summer_24h.pv_candidates()
        for i in range(len(candidates) - 1):
            assert candidates[i].solar_score >= candidates[i + 1].solar_score

    def test_candidates_carry_boundary_index(self, dwelling_summer_24h):
        candidates = dwelling_summer_24h.pv_candidates()
        assert len(candidates) >= 1
        # The south-facing candidate should have a boundary_index matching
        # the south roof plane.
        c = candidates[0]
        assert c.boundary_index is not None
        south_planes = [
            p for p in dwelling_summer_24h.roof_planes() if p.azimuth_deg == 180.0
        ]
        assert c.boundary_index == south_planes[0].boundary_index

    def test_repr_contains_capacity(self, dwelling_summer_24h):
        candidates = dwelling_summer_24h.pv_candidates()
        r = repr(candidates[0])
        assert "PvCandidate" in r
        assert "kW" in r

    def test_all_north_facing_raises_valueerror(self, dwelling_all_north):
        """All-north roofs should raise ValueError instead of returning empty list."""
        with pytest.raises(ValueError, match="north-facing"):
            dwelling_all_north.pv_candidates()


# ---------------------------------------------------------------------------
# PV sizing
# ---------------------------------------------------------------------------


class TestPvSizing:
    def test_sizing_returns_result(self, dwelling_summer_24h):
        result = dwelling_summer_24h.estimate_pv_capacity(6.0)
        assert result.capacity_kw > 0

    def test_sizing_respects_target(self, dwelling_summer_24h):
        result = dwelling_summer_24h.estimate_pv_capacity(4.0)
        # Should be close to 4 kW (rounded up to next panel).
        assert 3.5 <= result.capacity_kw <= 5.0

    def test_sizing_clamps_to_roof(self, dwelling_summer_24h):
        result = dwelling_summer_24h.estimate_pv_capacity(20.0)
        assert result.capacity_kw <= result.max_roof_capacity_kw + 0.1

    def test_sizing_errors_below_minimum(self, dwelling_summer_24h):
        with pytest.raises(ValueError, match="below minimum"):
            dwelling_summer_24h.estimate_pv_capacity(0.5, min_kw=100.0)

    def test_sizing_panel_count(self, dwelling_summer_24h):
        result = dwelling_summer_24h.estimate_pv_capacity(6.0)
        # 440 W panels: 6 kW ≈ 14 panels.
        expected_panels = round(result.capacity_kw * 1000 / result.panel_watts)
        assert result.num_panels == expected_panels

    def test_sizing_azimuth_matches_candidate(self, dwelling_summer_24h):
        result = dwelling_summer_24h.estimate_pv_capacity(6.0)
        candidates = dwelling_summer_24h.pv_candidates()
        assert abs(result.array_azimuth_deg - candidates[0].azimuth_deg) < 1.0

    def test_repr_contains_panels(self, dwelling_summer_24h):
        result = dwelling_summer_24h.estimate_pv_capacity(6.0)
        r = repr(result)
        assert "PvSizingResult" in r
        assert "panels=" in r

    def test_custom_panel_watts_changes_capacity(self, dwelling_summer_24h):
        default = dwelling_summer_24h.estimate_pv_capacity(6.0)
        custom = dwelling_summer_24h.estimate_pv_capacity(
            6.0, panel_watts=300, panel_area_m2=1.6
        )
        assert default.panel_watts != custom.panel_watts
        assert default.panel_watts == 440
        assert custom.panel_watts == 300


# ---------------------------------------------------------------------------
# PV power production (summer 24h simulation)
# ---------------------------------------------------------------------------


class TestPvPowerProduction:
    """Integration tests using base-pv.xml which has 4 kW south + 1.5 kW east PV."""

    @staticmethod
    def _make_pv_dwelling(start: str, duration_s: int = 86400, time_res_s: int = 900):
        from ochre_next import Dwelling

        return Dwelling.from_hpxml(
            hpxml=HPXML_PV,
            schedule=SCHEDULE,
            weather=WEATHER,
            start_time=start,
            duration_s=duration_s,
            time_res_s=time_res_s,
            defaults_path=str(HARES_DEFAULTS),
        )

    @pytest.mark.slow
    def test_pv_dwelling_initializes_without_error(self):
        """PV surface registration should allow the dwelling to init."""
        dw = self._make_pv_dwelling("2019-07-15T00:00:00Z")
        dw.initialize()
        # If we get here, PV surface registration worked.

    @pytest.mark.slow
    def test_pv_dwelling_steps_without_error(self):
        """PV equipment should step without surface_id mismatch errors."""
        dw = self._make_pv_dwelling("2019-07-15T00:00:00Z")
        dw.initialize()
        result = dw.step()
        assert "net_electric_power_kw" in result

    @pytest.mark.slow
    def test_summer_midday_produces_negative_net_power(self):
        """With 5.5 kW PV in Denver in July, net power should go negative
        during peak solar hours (roughly 10am-2pm local)."""
        dw = self._make_pv_dwelling("2019-07-15T00:00:00Z")
        dw.initialize()

        powers = []
        for _ in range(96):
            step = dw.step()
            powers.append(step["net_electric_power_kw"])

        # Steps 40-56 correspond roughly to 10:00-14:00 (peak solar in MDT).
        midday_powers = powers[40:56]
        min_midday = min(midday_powers)
        assert min_midday < 0, (
            f"Expected net negative power at midday with 5.5 kW PV, "
            f"but min midday power was {min_midday:.2f} kW. "
            f"All midday powers: {[f'{p:.2f}' for p in midday_powers]}"
        )

    @pytest.mark.slow
    def test_night_power_is_non_negative(self):
        """At night (first few hours), PV produces nothing, so net >= 0."""
        dw = self._make_pv_dwelling("2019-07-15T00:00:00Z")
        dw.initialize()

        # First 16 steps = midnight to 4am.
        night_powers = []
        for _ in range(16):
            step = dw.step()
            night_powers.append(step["net_electric_power_kw"])

        for p in night_powers:
            assert p >= -0.01, f"Expected non-negative power at night, got {p:.4f} kW"

    @pytest.mark.slow
    def test_winter_lower_pv_than_summer(self):
        """Winter PV production should be lower than summer."""
        dw_summer = self._make_pv_dwelling("2019-07-15T00:00:00Z")
        dw_summer.initialize()
        summer_powers = [dw_summer.step()["net_electric_power_kw"] for _ in range(96)]

        dw_winter = self._make_pv_dwelling("2019-01-15T00:00:00Z")
        dw_winter.initialize()
        winter_powers = [dw_winter.step()["net_electric_power_kw"] for _ in range(96)]

        # Summer should have more negative power (more PV export).
        assert min(summer_powers) < min(winter_powers), (
            f"Summer min ({min(summer_powers):.2f}) should be more negative "
            f"than winter min ({min(winter_powers):.2f})"
        )

    @pytest.mark.slow
    def test_no_pv_all_positive_power(self):
        """Without PV, net power should always be non-negative."""
        from ochre_next import Dwelling

        dw = Dwelling.from_hpxml(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            start_time="2019-07-15T00:00:00Z",
            duration_s=86400,
            time_res_s=900,
        )
        dw.initialize()

        powers = [dw.step()["net_electric_power_kw"] for _ in range(96)]
        min_power = min(powers)
        assert min_power >= -0.01, (
            f"Expected non-negative net power without PV, got {min_power:.4f} kW"
        )


# ---------------------------------------------------------------------------
# Standalone PV sizing functions — callable without a Dwelling object
# ---------------------------------------------------------------------------


class TestStandaloneInferRoofShape:
    """infer_roof_shape should work with RoofPlane objects directly."""

    def test_single_south_plane_gable(self):
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        shape = hares.infer_roof_shape([plane], latitude=40.0)
        assert shape in (hares.RoofShape.Gable, hares.RoofShape.Hip)

    def test_apartment_returns_flat(self):
        plane = hares.RoofPlane(area_m2=80.0, tilt_deg=26.0, azimuth_deg=180.0)
        shape = hares.infer_roof_shape(
            [plane], facility_type="apartment", latitude=40.0
        )
        assert shape == hares.RoofShape.Flat

    def test_flat_tilt_returns_flat(self):
        plane = hares.RoofPlane(area_m2=60.0, tilt_deg=0.5, azimuth_deg=180.0)
        shape = hares.infer_roof_shape([plane], latitude=40.0)
        assert shape == hares.RoofShape.Flat

    def test_two_opposing_planes_gable(self):
        """Two opposing planes at 40°N latitude, 26° tilt → Gable."""
        north = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=0.0)
        south = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        shape = hares.infer_roof_shape([north, south], latitude=40.0)
        assert shape == hares.RoofShape.Gable


class TestStandaloneComputeUsableArea:
    """compute_usable_area should work with RoofPlane objects directly."""

    def test_returns_usable_area(self):
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        result = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        assert result.usable_m2 > 0
        assert result.max_capacity_kw > 0
        assert result.max_panels > 0
        assert result.best_plane_idx == 0

    def test_empty_planes_raises_valueerror(self):
        with pytest.raises(ValueError):
            hares.compute_usable_area(
                [], roof_shape=hares.RoofShape.Gable, latitude=40.0
            )

    def test_custom_panel_changes_capacity(self):
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        default = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        custom = hares.compute_usable_area(
            [plane],
            roof_shape=hares.RoofShape.Gable,
            latitude=40.0,
            panel_watts=300,
            panel_area_m2=1.6,
        )
        assert default.max_capacity_kw != custom.max_capacity_kw


class TestStandaloneEnumeratePvCandidates:
    """enumerate_pv_candidates should work with RoofPlane objects directly."""

    def test_returns_candidates(self):
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        candidates = hares.enumerate_pv_candidates(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        assert len(candidates) == 1
        assert candidates[0].max_capacity_kw > 0
        assert candidates[0].roof_shape in (
            hares.RoofShape.Gable,
            hares.RoofShape.Hip,
        )

    def test_excludes_north_facing(self):
        north = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=0.0)
        south = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        candidates = hares.enumerate_pv_candidates(
            [north, south], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        # North-facing plane should be excluded.
        assert len(candidates) == 1
        assert abs(candidates[0].azimuth_deg - 180.0) < 1.0


class TestStandaloneSizePvSystem:
    """size_pv_system should chain with compute_usable_area."""

    def test_chains_with_usable_area(self):
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        result = hares.size_pv_system(usable, target_kw=6.0)
        assert result.capacity_kw > 0
        assert result.num_panels > 0
        assert result.collector_area_m2 > 0

    def test_insufficient_roof_raises_valueerror(self):
        plane = hares.RoofPlane(area_m2=5.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        with pytest.raises(ValueError):
            hares.size_pv_system(usable, target_kw=10.0, min_kw=10.0)

    def test_inverter_clamps_capacity(self):
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        no_inv = hares.size_pv_system(usable, target_kw=8.0)
        with_inv = hares.size_pv_system(
            usable, target_kw=8.0, inverter_kw_ac=5.0, max_dc_ac_ratio=1.2
        )
        assert with_inv.capacity_kw <= no_inv.capacity_kw

    def test_nec_100a_panel_clamps_to_backfeed_limit(self):
        """100 A main panel → ~4.8 kW AC backfeed limit → system clamped."""
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        # Without electrical limit, target 10 kW is achievable.
        no_limit = hares.size_pv_system(usable, target_kw=10.0)
        # With 100 A panel, backfeed = 100 × 1.2 − 100 = 20 A → 4.8 kW AC.
        limited = hares.size_pv_system(usable, target_kw=10.0, main_panel_ampacity=100)
        assert limited.capacity_kw < no_limit.capacity_kw, (
            f"100 A panel ({limited.capacity_kw:.2f} kW) must be less than "
            f"unconstrained ({no_limit.capacity_kw:.2f} kW)"
        )
        # ~4.8 kW AC → ~5.6 kW DC at 14% losses, 1.0 DC:AC ratio.
        assert limited.capacity_kw <= 6.0, (
            f"100 A panel must limit to ≤ 6 kW DC, got {limited.capacity_kw:.2f}"
        )

    def test_nec_200a_panel_allows_larger_system(self):
        """200 A main panel → ~9.6 kW AC backfeed → larger capacity than 100 A."""
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        result_100a = hares.size_pv_system(usable, target_kw=10.0, main_panel_ampacity=100)
        result_200a = hares.size_pv_system(usable, target_kw=10.0, main_panel_ampacity=200)
        assert result_200a.capacity_kw > result_100a.capacity_kw, (
            f"200 A panel ({result_200a.capacity_kw:.2f} kW) should allow more "
            f"than 100 A panel ({result_100a.capacity_kw:.2f} kW)"
        )

    def test_nec_result_exposes_electrical_constraint_fields(self):
        """PvSizingResult exposes max_backfeed_amps, max_ac_kw, and
        electrical_constraint_binding."""
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        result = hares.size_pv_system(
            usable, target_kw=10.0, main_panel_ampacity=100
        )
        assert result.max_backfeed_amps == 20.0
        assert result.max_ac_kw == 4.8
        assert result.electrical_constraint_binding is True

    def test_no_ampacity_fields_are_none(self):
        """Without main_panel_ampacity, electrical fields are None and
        constraint is not binding."""
        plane = hares.RoofPlane(area_m2=100.0, tilt_deg=26.0, azimuth_deg=180.0)
        usable = hares.compute_usable_area(
            [plane], roof_shape=hares.RoofShape.Gable, latitude=40.0
        )
        result = hares.size_pv_system(usable, target_kw=10.0)
        assert result.max_backfeed_amps is None
        assert result.max_ac_kw is None
        assert result.electrical_constraint_binding is False


class TestRequiredMainPanelAmpacity:
    """required_main_panel_ampacity — reverse NEC 120% service sizing."""

    def test_4_8kw_needs_100a(self):
        assert hares.required_main_panel_ampacity(4.8) == 100

    def test_9_6kw_needs_200a(self):
        assert hares.required_main_panel_ampacity(9.6) == 200

    def test_5kw_needs_125a(self):
        assert hares.required_main_panel_ampacity(5.0) == 125

    def test_explicit_breaker(self):
        assert hares.required_main_panel_ampacity(10.0, main_breaker_amps=150) == 200

    def test_rounds_up_to_nearest_standard(self):
        # 4.81 kW → 20.04 A backfeed → busbar ≥ 100.2 → 125
        assert hares.required_main_panel_ampacity(4.81) == 125
        # 9.61 kW → 40.04 A → busbar ≥ 200.2 → 225
        assert hares.required_main_panel_ampacity(9.61) == 225

    def test_saturates_at_400(self):
        assert hares.required_main_panel_ampacity(100.0) == 400
