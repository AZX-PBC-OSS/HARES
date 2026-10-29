"""The IEEE 1547 DER response layer's Python surface: the ``der_1547``
config block on the PV constructor, the per-DER trip state end to end
(the telemetry's ``der1547_tripped`` channel), and the unconfigured
contract (the pre-1547 behavior, the excursion counted).

The tables' values are pinned at the Rust layer
(``crates/hares-equipment/src/der1547.rs``: the Category II defaults from
IEEE 1547-2018's voltage shall-trip tables via NERC Table 1.2 / MISO
Table 3, the frequency trip defaults via MISO Table 6, the frequency-droop
parameters via MISO Table 8, the enter-service criteria via MISO Table 9).
These tests pin the Python-facing configuration surface and the trip
state's end-to-end publication.
"""

from __future__ import annotations

from conftest import make_dwelling
from hares import PV


class TestDer1547Surface:
    def test_configured_pv_trips_on_a_06pu_sag(self):
        """A 0.6 pu sag sits inside UV1's 0.70 pu band; the Category II
        default clearing time (10 s) is reached within the first 60 s
        step, so the configured inverter is tripped for the step and the
        trip state reads on the Python telemetry surface."""
        dw = make_dwelling(duration_s=300, time_res_s=60)
        dw.add_pv(
            PV(
                "TestPV",
                capacity_w=5000,
                tilt=30.0,
                azimuth=180.0,
                der_1547={"category": "II"},
            )
        )
        dw.set_grid_voltage(0.6)
        dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("TestPV")
        assert tel["der1547_tripped"][idx] == 1.0, (
            "the 0.6 pu sag must trip the configured inverter on the first "
            "step, got der1547_tripped="
            f"{tel['der1547_tripped'][idx]}"
        )

    def test_unconfigured_pv_rides_the_sag_with_the_trip_state_offline(self):
        """Without a ``der_1547`` block the layer is unconfigured: the
        pre-1547 contract (the inverter rides the excursion as a pinned
        source) with the trip state 0 — the explicit-configuration
        surface, never a silent default."""
        dw = make_dwelling(duration_s=300, time_res_s=60)
        dw.add_pv(PV("TestPV", capacity_w=5000, tilt=30.0, azimuth=180.0))
        dw.set_grid_voltage(0.6)
        dw.step()
        tel = dw.telemetry().equipment()
        idx = tel["names"].index("TestPV")
        assert tel["der1547_tripped"][idx] == 0.0, (
            "unconfigured: the inverter must ride the sag as the pre-1547 "
            "model did, got der1547_tripped="
            f"{tel['der1547_tripped'][idx]}"
        )

    def test_der_1547_dict_rejects_unknown_keys(self):
        """An unknown key in the ``der_1547`` dict is a configuration
        error naming the parameter (the config discipline: unknown keys
        are errors), not a silent drop."""
        dw = make_dwelling(duration_s=300, time_res_s=60)
        try:
            dw.add_pv(
                PV(
                    "TestPV",
                    capacity_w=5000,
                    tilt=30.0,
                    azimuth=180.0,
                    der_1547={"category": "II", "uv3_s": 5.0},
                )
            )
        except Exception as err:
            assert "uv3_s" in str(err) or "unknown" in str(err).lower(), (
                f"the error must name the offending key, got: {err}"
            )
        else:
            raise AssertionError("an unknown der_1547 key must be rejected")

    def test_der_1547_rejects_an_unknown_category(self):
        dw = make_dwelling(duration_s=300, time_res_s=60)
        try:
            dw.add_pv(
                PV(
                    "TestPV",
                    capacity_w=5000,
                    tilt=30.0,
                    azimuth=180.0,
                    der_1547={"category": "IV"},
                )
            )
        except Exception as err:
            assert "IV" in str(err) or "unknown" in str(err).lower() or "variant" in str(err).lower(), (
                f"the error must name the category, got: {err}"
            )
        else:
            raise AssertionError("an unknown der_1547 category must be rejected")
