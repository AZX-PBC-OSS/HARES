#!/usr/bin/env python3
"""Extract OCHRE RC network reference data for HARES parity testing.

Loads BEopt_example.xml through OCHRE, computes effective UA per boundary
(with TARP/DOE-2 film resistances and same-zone halving), and dumps to JSON.

Run from the OCHRE vendor directory:
    cd vendors/OCHRE && uv run ../../tests/fixtures/parity/extract_ochre_rc.py
"""

import json
import math
import os
import sys

# OCHRE is in vendors/OCHRE relative to repo root.
script_dir = os.path.dirname(os.path.abspath(__file__))
repo_root = os.path.abspath(os.path.join(script_dir, "..", "..", ".."))
ochre_dir = os.path.join(repo_root, "vendors", "OCHRE")
sys.path.insert(0, ochre_dir)

from ochre.utils import hpxml, envelope  # noqa: E402

HPXML_FILE = os.path.join(
    ochre_dir, "ochre", "defaults", "Input Files", "BEopt_example.xml"
)
OUTPUT_FILE = os.path.join(script_dir, "ochre_rc_reference.json")

# Match HARES defaults for film resistance computation.
LOCATION = {
    "Average Wind Speed (m/s)": 2,
    "Average Ambient Temperature (C)": 10,
    "Average Ground Temperature (C)": 10,
}


def main():
    props, _ = hpxml.load_hpxml(hpxml_file=HPXML_FILE)
    bd_props = envelope.get_boundary_rc_values(
        props["boundaries"], time_res_mins=60, location=LOCATION
    )
    zones = props.get("zones", {})

    boundaries = []
    total_ua = 0.0

    for name, bd in bd_props.items():
        area = sum(bd.get("Area (m^2)", [0]))
        res = bd.get("Resistances", [])
        caps = bd.get("Capacitances", [])
        r_layer = sum(res)
        n_nodes = len(caps)
        cap_total_kj = sum(c * area for c in caps)  # kJ/m²·K → kJ/K

        int_label = bd.get("Interior Zone Label", "")
        ext_label = bd.get("Exterior Zone Label", "")
        same_zone = int_label == ext_label

        # Window: EnergyPlus Simple Window Model decomposition.
        if name == "Window":
            u = bd.get("U Factor (W/m^2-K)", 0)
            if u and u > 0:
                if u < 5.85:
                    r_int = 1.0 / (0.359073 * math.log(u) + 6.949915)
                else:
                    r_int = 1.0 / (1.788041 * u - 2.886625)
                r_glass = 1.0 / u - r_int
                r_eff = r_glass + r_int  # = 1/u
                r_film_int = r_int
                r_film_ext = 0.0
            else:
                r_eff = 0
                r_film_int = 0
                r_film_ext = 0
        else:
            film = envelope.calculate_film_resistances(name, bd, LOCATION)
            r_film_int = film.get("Interior Film Resistance (m^2-K/W)", 0)
            r_film_ext = film.get("Exterior Film Resistance (m^2-K/W)", 0)
            if same_zone:
                r_eff = r_layer / 2.0 + r_film_int
            else:
                r_eff = r_layer + r_film_int + r_film_ext

        ua = area / r_eff if r_eff > 0 else 0
        total_ua += ua

        boundaries.append(
            {
                "name": name,
                "area_m2": round(area, 4),
                "r_layer_m2_k_w": round(r_layer, 6),
                "r_film_int_m2_k_w": round(r_film_int, 6),
                "r_film_ext_m2_k_w": round(r_film_ext, 6),
                "r_total_m2_k_w": round(r_eff, 6),
                "ua_w_k": round(ua, 2),
                "capacitance_kj_k": round(cap_total_kj, 2),
                "n_nodes": n_nodes,
                "interior_zone": int_label,
                "exterior_zone": ext_label,
                "same_zone": same_zone,
            }
        )

    zone_data = []
    for name, z in zones.items():
        vol = z.get("Volume (m^3)")
        zone_data.append(
            {
                "name": name,
                "volume_m3": round(vol, 4) if vol else None,
            }
        )

    result = {
        "hpxml_file": "BEopt_example.xml",
        "location": LOCATION,
        "boundaries": boundaries,
        "zones": zone_data,
        "total_ua_w_k": round(total_ua, 2),
    }

    with open(OUTPUT_FILE, "w") as f:
        json.dump(result, f, indent=2)

    print(f"Wrote {OUTPUT_FILE}")
    print(f"Total effective UA: {total_ua:.2f} W/K")
    print(f"Boundaries: {len(boundaries)}")
    for b in boundaries:
        print(f"  {b['name']:<25} area={b['area_m2']:>8.2f}  R={b['r_total_m2_k_w']:>8.4f}  UA={b['ua_w_k']:>8.2f}")


if __name__ == "__main__":
    main()
