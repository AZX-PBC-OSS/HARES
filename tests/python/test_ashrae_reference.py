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
from gen_ashrae_reference import ashrae_simple_interior_h_conv  # type: ignore[import-not-found]


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
