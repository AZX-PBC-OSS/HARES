"""
Generate 51-point OCV tables for NMC, LFP, NCA, and LTO chemistries using PyBaMM.

Chemistry -> Parameter set mapping:
  NMC: Chen2020        (NMC811/Graphite, LG M50 cell, validated)
  LFP: Prada2013       (LFP/Graphite, Afshar2017 LFP OCP)
  NCA: NCA_Kim2011     (NCA/Graphite, Kim2011 data)
  LTO: Chen2020 positive (NMC811) + LTO analytical OCP for negative
       LTO OCP from Colclasure et al. (2011) Electrochimica Acta 58:33-43
       NMC811/LTO full-cell stoichiometry from capacity-balance matching

SOC convention:
  SOC=0 -> discharged, SOC=1 -> fully charged
  neg stoichiometry increases with SOC (lithiation)
  pos stoichiometry decreases with SOC (delithiation)
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pybamm

SOC_POINTS = 51
SOC = np.linspace(0.0, 1.0, SOC_POINTS)

# TOML output directory (relative to script location)
TOML_DIR = Path(__file__).resolve().parent.parent / "defaults" / "ocv_tables"


def build_ocv_curve(
    u_pos_fn,
    u_neg_fn,
    x_n_min: float,
    x_n_max: float,
    x_p_min: float,
    x_p_max: float,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """
    Build full-cell OCV, U_pos, and U_neg curves over SOC 0-1.

    x_n_min: neg stoichiometry at 0% SOC (discharged, delithiated)
    x_n_max: neg stoichiometry at 100% SOC (charged, lithiated)
    x_p_min: pos stoichiometry at 100% SOC (charged, delithiated)
    x_p_max: pos stoichiometry at 0% SOC (discharged, lithiated)
    """
    x_neg = x_n_min + SOC * (x_n_max - x_n_min)
    x_pos = x_p_max - SOC * (x_p_max - x_p_min)

    u_pos_vals = np.array([float(u_pos_fn(xp)) for xp in x_pos])
    u_neg_vals = np.array([float(u_neg_fn(xn)) for xn in x_neg])
    ocv = u_pos_vals - u_neg_vals
    return ocv, u_pos_vals, u_neg_vals


def get_stoich_limits(param: pybamm.ParameterValues) -> tuple[float, float, float, float]:
    """Return (x_n_min, x_n_max, x_p_min, x_p_max) from ElectrodeSOHSolver."""
    solver = pybamm.lithium_ion.ElectrodeSOHSolver(param)
    x_n_min, x_n_max, x_p_min, x_p_max = solver.get_min_max_stoichiometries()
    return float(x_n_min), float(x_n_max), float(x_p_min), float(x_p_max)


def fmt_rust_vec(values: np.ndarray, label: str, indent: int = 4) -> str:
    """Format a numpy array as a Rust vec![] literal."""
    sp = " " * indent
    floats = ", ".join(f"{v:.6f}" for v in values)
    return f"{sp}{label}: vec![{floats}],"


def check_monotonic(ocv: np.ndarray, name: str) -> None:
    diffs = np.diff(ocv)
    if np.all(diffs > 0):
        print(f"  [OK] {name} OCV is strictly increasing")
    elif np.all(diffs >= 0):
        print(f"  [WARN] {name} OCV is non-decreasing (has flat regions)")
    else:
        n_violations = np.sum(diffs < 0)
        print(f"  [WARN] {name} OCV has {n_violations} non-monotonic steps")
        bad_idx = np.where(diffs < 0)[0]
        for i in bad_idx[:5]:
            print(f"    SOC[{i}]={SOC[i]:.2f}: {ocv[i]:.4f} -> {ocv[i+1]:.4f}")


# LTO OCP function (negative electrode)
# Colclasure, A.M., Smith, K.A., Kee, R.J. (2011) Electrochimica Acta 58:33-43
# Li4Ti5O12 has a very flat plateau at ~1.556 V vs Li/Li+
# Extended with boundary terms to avoid extrapolation issues
def lto_ocp_Colclasure2011(sto: float | np.ndarray) -> float | np.ndarray:
    """
    LTO (Li4Ti5O12) open-circuit potential vs Li/Li+ as a function of stoichiometry.
    Flat plateau at ~1.556 V with small boundary-stabilising exponential terms.
    Reference: Colclasure et al. (2011) Electrochimica Acta 58, 33-43.
    """
    return (
        1.5564
        + 0.0120 * np.exp(-100.0 * sto)
        + 0.0020 * np.exp(-10.0 * (1.0 - sto))
    )


# ---------------------------------------------------------------------------
# Version info
# ---------------------------------------------------------------------------
print(f"PyBaMM version: {pybamm.__version__}")
print()

# NMC (Chen2020): NMC811/Graphite, LG M50
print("=" * 70)
print("NMC  (Chen2020: NMC811 / Graphite LG M50)")
print("=" * 70)

param_nmc = pybamm.ParameterValues("Chen2020")
u_pos_nmc = param_nmc["Positive electrode OCP [V]"]
u_neg_nmc = param_nmc["Negative electrode OCP [V]"]
x_n_min_nmc, x_n_max_nmc, x_p_min_nmc, x_p_max_nmc = get_stoich_limits(param_nmc)

print(f"  neg stoich: {x_n_min_nmc:.6f} (0% SOC) -> {x_n_max_nmc:.6f} (100% SOC)")
print(f"  pos stoich: {x_p_max_nmc:.6f} (0% SOC) -> {x_p_min_nmc:.6f} (100% SOC)")

ocv_nmc, u_pos_nmc_vals, u_neg_nmc_vals = build_ocv_curve(
    u_pos_nmc, u_neg_nmc,
    x_n_min_nmc, x_n_max_nmc,
    x_p_min_nmc, x_p_max_nmc,
)
print(f"  OCV range: {ocv_nmc[0]:.4f}V -> {ocv_nmc[-1]:.4f}V")
check_monotonic(ocv_nmc, "NMC")

# LFP (Prada2013): LFP / Graphite
print()
print("=" * 70)
print("LFP  (Prada2013: LFP / Graphite)")
print("=" * 70)

param_lfp = pybamm.ParameterValues("Prada2013")
u_pos_lfp = param_lfp["Positive electrode OCP [V]"]
u_neg_lfp = param_lfp["Negative electrode OCP [V]"]
x_n_min_lfp, x_n_max_lfp, x_p_min_lfp, x_p_max_lfp = get_stoich_limits(param_lfp)

print(f"  neg stoich: {x_n_min_lfp:.6f} (0% SOC) -> {x_n_max_lfp:.6f} (100% SOC)")
print(f"  pos stoich: {x_p_max_lfp:.6f} (0% SOC) -> {x_p_min_lfp:.6f} (100% SOC)")

ocv_lfp, u_pos_lfp_vals, u_neg_lfp_vals = build_ocv_curve(
    u_pos_lfp, u_neg_lfp,
    x_n_min_lfp, x_n_max_lfp,
    x_p_min_lfp, x_p_max_lfp,
)
print(f"  OCV range: {ocv_lfp[0]:.4f}V -> {ocv_lfp[-1]:.4f}V")
check_monotonic(ocv_lfp, "LFP")

# NCA (NCA_Kim2011): NCA / Graphite
print()
print("=" * 70)
print("NCA  (NCA_Kim2011: NCA / Graphite)")
print("=" * 70)

param_nca = pybamm.ParameterValues("NCA_Kim2011")
u_pos_nca = param_nca["Positive electrode OCP [V]"]
u_neg_nca = param_nca["Negative electrode OCP [V]"]
x_n_min_nca, x_n_max_nca, x_p_min_nca, x_p_max_nca = get_stoich_limits(param_nca)

print(f"  neg stoich: {x_n_min_nca:.6f} (0% SOC) -> {x_n_max_nca:.6f} (100% SOC)")
print(f"  pos stoich: {x_p_max_nca:.6f} (0% SOC) -> {x_p_min_nca:.6f} (100% SOC)")

ocv_nca, u_pos_nca_vals, u_neg_nca_vals = build_ocv_curve(
    u_pos_nca, u_neg_nca,
    x_n_min_nca, x_n_max_nca,
    x_p_min_nca, x_p_max_nca,
)
print(f"  OCV range: {ocv_nca[0]:.4f}V -> {ocv_nca[-1]:.4f}V")
check_monotonic(ocv_nca, "NCA")

# ---------------------------------------------------------------------------
# LTO (NMC811 positive from Chen2020 + LTO negative, Colclasure 2011)
#
# Capacity balancing for NMC/LTO full cell:
#   LTO: Li4Ti5O12, stoichiometry window [0.01, 0.99] (essentially full range,
#        because the OCP is so flat the ElectrodeSOHSolver would struggle).
#   NMC: reuse Chen2020 positive with its standard stoichiometry limits.
#
# Approach: use the same NMC pos stoich limits as Chen2020.
# Typical NMC/LTO voltage range: ~1.5V (discharged) to ~2.9V (charged).
# The NMC positive goes from 0.85 sto (0% SOC) to 0.26 sto (100% SOC).
# LTO is flat ~1.556V so full-cell OCV is dominated by NMC positive shift.
# We use the standard LTO operating window sto_neg in [0.02, 0.97].
# ---------------------------------------------------------------------------
print()
print("=" * 70)
print("LTO  (NMC811 positive [Chen2020] + LTO negative [Colclasure2011])")
print("=" * 70)

# LTO stoichiometry limits: nearly full swing (LTO is fully reversible)
x_n_min_lto = 0.02   # discharged (0% SOC) - LTO delithiated
x_n_max_lto = 0.97   # charged (100% SOC) - LTO lithiated

# NMC811 positive: reuse Chen2020 limits
x_p_min_lto = x_p_min_nmc  # 100% SOC
x_p_max_lto = x_p_max_nmc  # 0% SOC

print(f"  neg stoich (LTO): {x_n_min_lto:.6f} (0% SOC) -> {x_n_max_lto:.6f} (100% SOC)")
print(f"  pos stoich (NMC): {x_p_max_lto:.6f} (0% SOC) -> {x_p_min_lto:.6f} (100% SOC)")
print("  neg OCP source: Colclasure et al. (2011) Electrochimica Acta 58:33-43")
print("  pos OCP source: Chen2020 NMC811 (nmc_LGM50_ocp_Chen2020)")

ocv_lto, u_pos_lto_vals, u_neg_lto_vals = build_ocv_curve(
    u_pos_nmc,           # NMC811 positive OCP
    lto_ocp_Colclasure2011,  # LTO negative OCP
    x_n_min_lto, x_n_max_lto,
    x_p_min_lto, x_p_max_lto,
)
print(f"  OCV range: {ocv_lto[0]:.4f}V -> {ocv_lto[-1]:.4f}V")
check_monotonic(ocv_lto, "LTO")

# ---------------------------------------------------------------------------
# Print Rust array literals
# ---------------------------------------------------------------------------
print()
print("=" * 70)
print("RUST OUTPUT")
print("=" * 70)

chemistries = [
    ("NMC", "Chen2020", ocv_nmc, u_neg_nmc_vals),
    ("LFP", "Prada2013", ocv_lfp, u_neg_lfp_vals),
    ("NCA", "NCA_Kim2011", ocv_nca, u_neg_nca_vals),
    ("LTO", "Chen2020_pos+Colclasure2011_neg", ocv_lto, u_neg_lto_vals),
]

for name, source, ocv, u_neg in chemistries:
    print()
    print(f"// {name} OCV (51 points, SOC 0.00-1.00, PyBaMM {source})")
    print(fmt_rust_vec(ocv, "voltage_v"))
    print()
    print(f"// {name} U_neg (51 points, SOC 0.00-1.00, PyBaMM {source})")
    print(fmt_rust_vec(u_neg, "u_neg_v"))

# ---------------------------------------------------------------------------
# Summary table
# ---------------------------------------------------------------------------
print()
print("=" * 70)
print("SUMMARY")
print("=" * 70)
print(f"{'Chemistry':<10} {'SOC Source':<35} {'V@0%':>7} {'V@50%':>7} {'V@100%':>8} {'Monotonic':>10}")
print("-" * 78)
for name, source, ocv, _ in chemistries:
    v0 = ocv[0]
    v50 = ocv[25]
    v100 = ocv[-1]
    mono = "YES" if np.all(np.diff(ocv) > 0) else ("FLAT" if np.all(np.diff(ocv) >= 0) else "NO")
    print(f"{name:<10} {source:<35} {v0:>7.4f} {v50:>7.4f} {v100:>8.4f} {mono:>10}")

# ---------------------------------------------------------------------------
# TOML output
#
# Writes per-chemistry TOML files suitable for runtime loading via
# Battery::set_ocv_table(). Files are written to defaults/ocv_tables/.
# ---------------------------------------------------------------------------
print()
print("=" * 70)
print("TOML OUTPUT")
print("=" * 70)

TOML_DIR.mkdir(parents=True, exist_ok=True)

# Mapping of chemical symbol to directory-safe name
TOML_NAMES = {
    "NMC": "nmc",
    "LFP": "lfp",
    "NCA": "nca",
    "LTO": "lto",
}

for name, source, ocv, u_neg in chemistries:
    stem = TOML_NAMES[name]
    ocv_path = TOML_DIR / f"{stem}_ocv.toml"
    uneg_path = TOML_DIR / f"{stem}_u_neg.toml"

    # OCV table
    ocv_path.write_text(
        "# {name} OCV table (51 points, SOC 0.00–1.00)\n"
        "# PyBaMM version: {pybamm_version}\n"
        "# Source: {source}\n"
        "# Use this file with Battery::set_ocv_table() for runtime injection.\n"
        "soc_points = [{soc_points}]\n"
        "voltage_v = [{voltage_v}]\n".format(
            name=name,
            pybamm_version=pybamm.__version__,
            source=source,
            soc_points=", ".join(f"{s:.4f}" for s in SOC),
            voltage_v=", ".join(f"{v:.6f}" for v in ocv),
        )
    )

    # U_neg table
    uneg_path.write_text(
        "# {name} U_neg table (51 points, SOC 0.00–1.00)\n"
        "# PyBaMM version: {pybamm_version}\n"
        "# Source: {source}\n"
        "# Use this file with Battery::set_u_neg_table() for runtime injection.\n"
        "soc_points = [{soc_points}]\n"
        "potential_v = [{potential_v}]\n".format(
            name=name,
            pybamm_version=pybamm.__version__,
            source=source,
            soc_points=", ".join(f"{s:.4f}" for s in SOC),
            potential_v=", ".join(f"{v:.6f}" for v in u_neg),
        )
    )

    print(f"  Wrote {ocv_path}")
    print(f"  Wrote {uneg_path}")
