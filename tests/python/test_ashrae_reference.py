"""Unit test: ASHRAE Simple interior film coefficient parity with HARES Rust.

Verifies that the Python ``ashrae_simple_interior_h_conv`` in
``scripts/gen_ashrae_reference.py`` returns the same fixed h_conv values
as the Rust implementation in
``crates/hares-physics/src/film_coefficients.rs:209-225``
for all five orientation cases.
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

import pytest

# Import the reference script as a module.
_SCRIPTS_DIR = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(_SCRIPTS_DIR))
from gen_ashrae_reference import (  # type: ignore[import-not-found]
    MIN_DELTA_T_TARP_NATURAL_K,
    FilmInputs,
    ashrae_simple_interior_h_conv,
    film_resistances,
    tarp_h_natural,
)


# ---------------------------------------------------------------------------
# Orientation boundary angles derived from:
#   cos(tilt) = 0.3827  →  tilt = arccos(0.3827) ≈ 67.5°
#   cos(tilt) = 0.9239  →  tilt = arccos(0.9239) ≈ 22.5°
# Source: EnergyPlus ConvectionCoefficients.cc CalcASHRAESimpleIntConvCoeff.
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "tilt_deg,above_hotter,expected",
    [
        # Vertical surfaces: 67.5° < tilt <= 112.5°, h_conv = 3.076.
        pytest.param(90.0, True, 3.076, id="vertical-wall"),
        pytest.param(90.0, False, 3.076, id="vertical-wall-cooler-above"),
        pytest.param(112.5, True, 3.076, id="vertical-boundary-upper"),
        pytest.param(67.5, True, 3.076, id="vertical-boundary-lower"),
        # Horizontal surfaces: 0° <= tilt < 22.5° or > 157.5°.
        # Enhanced (heat flow up): warm surface below, h_conv = 4.040.
        pytest.param(0.0, True, 4.040, id="horizontal-enhanced-ceiling"),
        pytest.param(22.4, True, 4.040, id="horizontal-enhanced-near-boundary"),
        pytest.param(180.0, True, 4.040, id="horizontal-enhanced-inverted"),
        # Reduced (heat flow down): warm surface above, h_conv = 0.948.
        pytest.param(0.0, False, 0.948, id="horizontal-reduced-floor"),
        pytest.param(22.4, False, 0.948, id="horizontal-reduced-near-boundary"),
        pytest.param(180.0, False, 0.948, id="horizontal-reduced-inverted"),
        # Tilted surfaces: 22.5° <= tilt <= 67.5°.
        # Enhanced, h_conv = 3.870.
        pytest.param(45.0, True, 3.870, id="tilted-enhanced"),
        pytest.param(22.5, True, 3.870, id="tilted-enhanced-lower-boundary"),
        pytest.param(67.4, True, 3.870, id="tilted-enhanced-upper-boundary"),
        # Reduced, h_conv = 2.281.
        pytest.param(45.0, False, 2.281, id="tilted-reduced"),
        pytest.param(22.5, False, 2.281, id="tilted-reduced-lower-boundary"),
        pytest.param(67.4, False, 2.281, id="tilted-reduced-upper-boundary"),
    ],
)
def test_ashrae_simple_interior_h_conv_returns_expected_value(
    tilt_deg: float, above_hotter: bool, expected: float
) -> None:
    """The five orientation categories map to the correct h_conv values."""
    result = ashrae_simple_interior_h_conv(tilt_deg, above_hotter)
    assert result == pytest.approx(expected), (
        f"tilt={tilt_deg}° above_hotter={above_hotter}: "
        f"expected {expected}, got {result}"
    )


@pytest.mark.parametrize("tilt_deg", [0.0, 45.0, 90.0, 135.0, 180.0])
@pytest.mark.parametrize("above_hotter", [True, False])
def test_ashrae_simple_interior_h_conv_positive_invariant(
    tilt_deg: float, above_hotter: bool,
) -> None:
    """h_conv must be positive for all valid orientations (invariant check)."""
    result = ashrae_simple_interior_h_conv(tilt_deg, above_hotter)
    assert result > 0.0, (
        f"invariant violated: h_conv={result} <= 0 for "
        f"tilt={tilt_deg}° above_hotter={above_hotter}"
    )


# ---------------------------------------------------------------------------
# Cross-check: the discrete jumps happen at exactly the expected cos(tilt)
# threshold angles (arccos boundary ≡ 67.5° and 22.5°).
# ---------------------------------------------------------------------------

_TRANSITION_ANGLES = [
    (67.5, 3.076, "vertical→tilted boundary (vertical side)"),
    (67.4, 3.870, "vertical→tilted boundary (tilted-enhanced side)"),
    (22.5, 3.870, "tilted→horizontal boundary (tilted-enhanced side)"),
    (22.4, 4.040, "tilted→horizontal boundary (horizontal-enhanced side)"),
]


@pytest.mark.parametrize("tilt_deg,expected,description", _TRANSITION_ANGLES)
def test_orientation_transitions(
    tilt_deg: float, expected: float, description: str
) -> None:
    """The boundary between orientation categories is at the correct angle."""
    result = ashrae_simple_interior_h_conv(tilt_deg, above_hotter=True)
    assert result == pytest.approx(expected), (
        f"{description}: expected {expected}, got {result} at tilt={tilt_deg}°"
    )


def test_solar_absorptance_above_0_9_does_not_affect_h_conv() -> None:
    """Solar absorptance is not an input to ASHRAE Simple; only tilt and
    above_hotter affect h_conv.  This guard document that h_conv is
    independent of surface radiative properties."""
    tilt_deg = 45.0
    above = True
    h_conv = ashrae_simple_interior_h_conv(tilt_deg, above)
    assert h_conv == 3.870, "h_conv should be independent of any radiative input"


def test_h_conv_matches_rust_implementation_all_cases() -> None:
    """Comprehensive parity check: every orientation × above_hotter combination
    produces the exact value from film_coefficients.rs."""

    # Rust cases from film_coefficients.rs:209-225
    # cos_tilt = cos(tilt).abs()
    # if cos_tilt < 0.3827       → 3.076  (vertical)
    # if cos_tilt >= 0.9239      → 4.040 (hotter) / 0.948 (reduced)  (horizontal)
    # else                       → 3.870 (hotter) / 2.281 (reduced)  (tilted)

    cases = [
        # (tilt_deg, above_hotter, expected_h_conv, expected_r_film)
        # Vertical: abs(cos(tilt)) < 0.3827 for tilt > 67.5° OR tilt < 112.5°
        (90.0, True, 3.076, 1.0 / 3.076),
        (90.0, False, 3.076, 1.0 / 3.076),
        (100.0, True, 3.076, 1.0 / 3.076),
        # Horizontal: abs(cos(tilt)) >= 0.9239 for tilt < 22.5° OR tilt > 157.5°
        (0.0, True, 4.040, 1.0 / 4.040),
        (0.0, False, 0.948, 1.0 / 0.948),
        (10.0, True, 4.040, 1.0 / 4.040),
        (10.0, False, 0.948, 1.0 / 0.948),
        # Tilted: all other angles
        (45.0, True, 3.870, 1.0 / 3.870),
        (45.0, False, 2.281, 1.0 / 2.281),
        (30.0, True, 3.870, 1.0 / 3.870),
        (30.0, False, 2.281, 1.0 / 2.281),
        (60.0, True, 3.870, 1.0 / 3.870),
        (60.0, False, 2.281, 1.0 / 2.281),
    ]

    for tilt_deg, above_hotter, expected_h_conv, expected_r_film in cases:
        h_conv = ashrae_simple_interior_h_conv(tilt_deg, above_hotter)
        r_film = 1.0 / h_conv
        assert h_conv == pytest.approx(expected_h_conv, abs=0.001), (
            f"tilt={tilt_deg}° above_hotter={above_hotter}: "
            f"h_conv mismatch: {h_conv} vs {expected_h_conv}"
        )
        assert r_film == pytest.approx(expected_r_film, abs=0.001), (
            f"tilt={tilt_deg}° above_hotter={above_hotter}: "
            f"r_film mismatch: {r_film} vs {expected_r_film}"
        )


# ---------------------------------------------------------------------------
# TARP at ΔT = 12.9 K accidental convergence with ASHRAE Simple (T-0346)
# ---------------------------------------------------------------------------
#
# The previous MIN_DELTA_T floor of 12.9 K (an OCHRE convention) made TARP
# produce h_conv values that were coincidentally close to ASHRAE Simple for
# *vertical* surfaces.  This match was accidental — a property of the TARP
# cube-root formula at one specific ΔT — and does not generalise to other
# surface orientations.  These tests document where the convergence exists
# and where it does not, demonstrating why the interior film calculation
# was changed to use ASHRAE Simple directly (T-0345) rather than relying on
# a TARP clamp to approximate it.

_TARP_12_9K: float = 12.9


def test_tarp_at_12_9k_vertical_matches_ashrae_simple_within_1_pct() -> None:
    """At ΔT = 12.9 K, vertical TARP h_natural ≈ 3.076, matching ASHRAE Simple
    vertical h_conv = 3.076 to within 0.12%.  This accidental match was the
    reason the old 12.9 K floor masked the interior convection model mismatch."""
    h_tarp = tarp_h_natural(90.0, _TARP_12_9K, above_hotter=True)
    h_ashrae = ashrae_simple_interior_h_conv(90.0, above_hotter=True)
    pct_diff = abs(h_tarp - h_ashrae) / h_ashrae * 100.0
    assert pct_diff < 1.0, (
        f"vertical TARP at ΔT={_TARP_12_9K}K: h={h_tarp:.4f} vs "
        f"ASHRAE Simple h={h_ashrae:.4f} ({pct_diff:.2f}% diff, expected < 1%)"
    )


def test_tarp_at_12_9k_does_not_match_ashrae_simple_horizontal_enhanced() -> None:
    """TARP at 12.9 K diverges from ASHRAE Simple for horizontal enhanced
    (heat flow up) — the clamp only accidentally converges on vertical."""
    h_tarp = tarp_h_natural(0.0, _TARP_12_9K, above_hotter=True)
    h_ashrae = ashrae_simple_interior_h_conv(0.0, above_hotter=True)  # 4.040
    pct_diff = abs(h_tarp - h_ashrae) / h_ashrae * 100.0
    # Expected: TARP ≈ 3.567, ASHRAE Simple = 4.040 → ~11.7% divergence
    assert pct_diff > 1.0, (
        f"horizontal enhanced TARP at ΔT={_TARP_12_9K}K: h={h_tarp:.4f} vs "
        f"ASHRAE Simple h={h_ashrae:.4f} ({pct_diff:.2f}% diff) — "
        f"this documents that the 12.9 K clamp does NOT converge for "
        f"horizontal surfaces"
    )


def test_tarp_at_12_9k_does_not_match_ashrae_simple_horizontal_reduced() -> None:
    """TARP at 12.9 K diverges from ASHRAE Simple for horizontal reduced
    (heat flow down) — the clamp only accidentally converges on vertical."""
    h_tarp = tarp_h_natural(0.0, _TARP_12_9K, above_hotter=False)
    h_ashrae = ashrae_simple_interior_h_conv(0.0, above_hotter=False)  # 0.948
    pct_diff = abs(h_tarp - h_ashrae) / h_ashrae * 100.0
    # Expected: TARP ≈ 1.783, ASHRAE Simple = 0.948 → ~88% divergence
    assert pct_diff > 1.0, (
        f"horizontal reduced TARP at ΔT={_TARP_12_9K}K: h={h_tarp:.4f} vs "
        f"ASHRAE Simple h={h_ashrae:.4f} ({pct_diff:.2f}% diff) — "
        f"this documents that the 12.9 K clamp does NOT converge for "
        f"horizontal surfaces"
    )


# ---------------------------------------------------------------------------
# MIN_DELTA_T_TARP_NATURAL_K floor behaviour (T-0346)
# ---------------------------------------------------------------------------


def test_min_delta_t_tarp_floor_is_applied() -> None:
    """``film_resistances()`` floors raw_dt to MIN_DELTA_T_TARP_NATURAL_K
    when the computed temperature difference is below the floor.

    Creates two FilmInputs with different ambient temperatures such that
    raw_dt differs (0.0 K vs 0.05 K) but both are below the 0.1 K floor.
    Without the floor, r_ext would differ because different raw_dt values
    produce different h_natural values.  With the floor, both get clamped
    to the same effective ΔT and produce identical r_ext.  If a future
    refactor accidentally drops the ``max(raw_dt, MIN_DELTA_T)`` line, this
    test fails because the two r_ext values would diverge.
    """
    # t_int for LIV = T_CONDITIONED_C = 20.0 °C (hardcoded).
    # t_ext for EXT = avg_ambient + T_OUTDOOR_OFFSET_C = avg_ambient + 5.0 °C.
    # Choosing avg_ambient = 15.0 → t_ext = 20.0 → raw_dt = 0.0 K.
    # Choosing avg_ambient = 15.05 → t_ext = 20.05 → raw_dt = 0.05 K.
    # Both are < MIN_DELTA_T_TARP_NATURAL_K (0.1 K).
    common_kwargs = dict(
        tilt_deg=90.0,
        interior_zone="LIV",
        exterior_zone="EXT",
        avg_wind_speed_m_s=2.0,
        avg_ground_temp_c=10.0,
    )
    inputs_raw_zero = FilmInputs(avg_ambient_temp_c=15.0, **common_kwargs)
    inputs_raw_half = FilmInputs(avg_ambient_temp_c=15.05, **common_kwargs)

    _, r_ext_zero = film_resistances(inputs_raw_zero)
    _, r_ext_half = film_resistances(inputs_raw_half)

    # Both raw_dt values are floored to MIN_DELTA_T_TARP_NATURAL_K (0.1 K),
    # so film_resistances must return the same r_ext in both cases.
    assert r_ext_zero == pytest.approx(r_ext_half, abs=1e-6), (
        f"r_ext for raw_ΔT=0.0K ({r_ext_zero:.6f}) must equal "
        f"r_ext for raw_ΔT=0.05K ({r_ext_half:.6f}) — "
        f"both must be floored to ΔT={MIN_DELTA_T_TARP_NATURAL_K}K. "
        f"If the max(raw_dt, MIN_DELTA_T) clamp is missing, these values diverge."
    )


def test_min_delta_t_floor_matches_hares_rust_value() -> None:
    """MIN_DELTA_T_TARP_NATURAL_K = 0.1 K matches the floor used in HARES's
    film_coefficients.rs (`(t_ext - t_int).abs().max(0.1)`)."""
    assert MIN_DELTA_T_TARP_NATURAL_K == 0.1, (
        f"MIN_DELTA_T_TARP_NATURAL_K = {MIN_DELTA_T_TARP_NATURAL_K} "
        f"must equal 0.1 K to match HARES film_coefficients.rs exterior "
        f"TARP+DOE-2 delta-T floor"
    )
