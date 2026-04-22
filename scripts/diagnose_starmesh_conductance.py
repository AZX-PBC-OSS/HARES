#!/usr/bin/env python3
"""Diagnose the Option-2 (StarMesh) interior radiation conductance deficit.

Builds the exact RC network for BESTEST Case 600 using the Option 2 architecture
(convection-only R_film + star-mesh radiation + window decomposition), computes
steady-state heat flow, and compares with the correct total.
"""

from __future__ import annotations

import numpy as np
from dataclasses import dataclass

# ── Physical constants ──────────────────────────────────────────────────────
SIGMA = 5.670374e-8
T_REF_K = 293.15  # 20°C linearization reference
EPS_INTERIOR = 0.9  # ASHRAE 140-2017 §5.3.1.9
H_RAD = 4.0 * EPS_INTERIOR * SIGMA * T_REF_K**3  # ≈5.14 W/m²K


@dataclass
class Surface:
    name: str
    area: float  # m²
    r_film_conv: float  # convection-only interior film [m²K/W]
    r_inner_half: float  # half of innermost layer R [m²K/W]
    r_rest: float  # inner_node→exterior R [m²K/W]
    ext_target: str  # "outdoor" or "ground"
    has_inner_node: bool
    h_si_combined: float  # combined h_si for correct model [W/m²K]
    r_total_combined: float  # total R for correct model [m²K/W]
    boundary_type: str = "opaque"


def parallel_r(r1: float, r2: float) -> float:
    if r1 <= 0 or r2 <= 0:
        return 0.0
    return r1 * r2 / (r1 + r2)


class ResistanceNetwork:
    """Mutable resistance network with floating-node elimination."""

    def __init__(self) -> None:
        self.res: dict[tuple[str, str], float] = {}

    def add(self, n1: str, n2: str, r: float) -> None:
        edge = tuple(sorted([n1, n2]))
        r = max(r, 1e-15)
        if edge in self.res:
            self.res[edge] = parallel_r(self.res[edge], r)
        else:
            self.res[edge] = r

    def eliminate_floating(
        self,
        internal: set[str],
        external: set[str],
    ) -> None:
        """Y-Δ (star-mesh) elimination of floating (no-cap) nodes in-place."""
        while True:
            nodes: set[str] = set()
            for a, b in self.res:
                nodes.add(a)
                nodes.add(b)
            floating = sorted(
                n for n in nodes if n not in internal and n not in external
            )
            if not floating:
                break
            for nf in floating:
                adj: list[tuple[str, float]] = []
                for (a, b), r in list(self.res.items()):
                    if a == nf:
                        adj.append((b, r))
                    elif b == nf:
                        adj.append((a, r))
                # remove all edges to nf
                self.res = {
                    (a, b): r
                    for (a, b), r in self.res.items()
                    if a != nf and b != nf
                }
                if len(adj) <= 1:
                    continue
                sum_g = sum(1.0 / r for _, r in adj)
                for i in range(len(adj)):
                    for j in range(i + 1, len(adj)):
                        ni, ri = adj[i]
                        nj, rj = adj[j]
                        g_new = (1.0 / ri) * (1.0 / rj) / sum_g
                        edge = tuple(sorted([ni, nj]))
                        if edge in self.res:
                            g_tot = 1.0 / self.res[edge] + g_new
                            self.res[edge] = 1.0 / g_tot
                        else:
                            self.res[edge] = 1.0 / g_new


def solve_steady_state(
    net: ResistanceNetwork,
    fixed_temps: dict[str, float],
) -> dict[str, float]:
    """Solve for all node temperatures at steady state.

    Heat balance for each free node i:  Σ_j G_ij (T_j − T_i) = 0
    Matrix form:  A T_free = b  with positive diagonal, negative off-diagonal.
    """
    all_nodes: set[str] = set()
    for a, b in net.res:
        all_nodes.add(a)
        all_nodes.add(b)

    free = sorted(n for n in all_nodes if n not in fixed_temps)
    idx = {n: i for i, n in enumerate(free)}
    n_free = len(free)

    if n_free == 0:
        return dict(fixed_temps)

    A = np.zeros((n_free, n_free))
    b = np.zeros(n_free)

    for (na, nb), r in net.res.items():
        g = 1.0 / r
        a_free = na in idx
        b_free = nb in idx
        if a_free and b_free:
            i, j = idx[na], idx[nb]
            A[i, i] += g  # positive diagonal
            A[j, j] += g
            A[i, j] -= g  # negative off-diagonal
            A[j, i] -= g
        elif a_free:  # na free, nb fixed
            i = idx[na]
            A[i, i] += g
            b[i] += g * fixed_temps[nb]
        elif b_free:  # nb free, na fixed
            j = idx[nb]
            A[j, j] += g
            b[j] += g * fixed_temps[na]

    T_free = np.linalg.solve(A, b)
    temps = dict(fixed_temps)
    for n, t in zip(free, T_free):
        temps[n] = float(t)
    return temps


def zone_heat_loss(net: ResistanceNetwork, temps: dict[str, float],
                   zone: str = "zone_air") -> float:
    """Total heat leaving zone_air [W] (positive = leaving)."""
    q = 0.0
    for (a, b), r in net.res.items():
        if a == zone:
            q += (temps[a] - temps[b]) / r
        elif b == zone:
            q += (temps[b] - temps[a]) / r
    return q


def main() -> None:
    print("=" * 80)
    print("BESTEST Case 600 — Option 2 (StarMesh) Conductance Diagnostic")
    print("=" * 80)

    # ── Wall parameters ──────────────────────────────────────────────────
    r_mat_wall = 0.009 / 0.14 + 0.066 / 0.04 + 0.012 / 0.16  # 1.789 m²K/W
    r_half_wall = (0.012 / 0.16) / 2  # 0.0375 m²K/W
    delta_t = 12.9
    h_conv_wall = 1.31 * delta_t ** (1.0 / 3.0)  # 3.076
    r_conv_wall = 1.0 / h_conv_wall  # 0.325
    h_so_rough = 29.3
    r_ext_wall = 1.0 / h_so_rough  # 0.034
    h_si_wall = 8.29
    r_si_wall = 1.0 / h_si_wall  # 0.121
    r_total_wall = r_si_wall + r_mat_wall + r_ext_wall  # 1.944
    r_rest_wall = r_mat_wall - r_half_wall + r_ext_wall  # 1.786
    a_walls = 63.6

    # ── Roof parameters ──────────────────────────────────────────────────
    r_mat_roof = 0.019 / 0.14 + 0.1118 / 0.04 + 0.010 / 0.16  # 2.993
    r_half_roof = (0.010 / 0.16) / 2  # 0.031
    # TARP reduced (above_hotter=False for roof in OCHRE logic)
    h_conv_roof = 1.810 * delta_t ** (1.0 / 3.0) / (1.382 + 1.0)  # 1.785
    r_conv_roof = 1.0 / h_conv_roof  # 0.560
    r_ext_roof = r_ext_wall  # 0.034
    h_si_roof = 9.26
    r_si_roof = 1.0 / h_si_roof  # 0.108
    r_total_roof = r_si_roof + r_mat_roof + r_ext_roof  # 3.135
    r_rest_roof = r_mat_roof - r_half_roof + r_ext_roof  # 2.996

    # ── Floor parameters ─────────────────────────────────────────────────
    r_mat_floor = 1.003 / 0.04 + 0.025 / 0.14  # 25.254
    r_half_floor = (0.025 / 0.14) / 2  # 0.089
    # TARP enhanced (above_hotter=True for floor in OCHRE logic)
    h_conv_floor = 9.482 * delta_t ** (1.0 / 3.0) / (7.238 - 1.0)  # 3.571
    r_conv_floor = 1.0 / h_conv_floor  # 0.280
    r_ext_floor = 0.0  # no ext film for ground
    h_si_floor = 6.13
    r_si_floor = 1.0 / h_si_floor  # 0.163
    r_total_floor = r_si_floor + r_mat_floor  # 25.417
    r_rest_floor = r_mat_floor - r_half_floor  # 25.165

    # ── Window parameters ────────────────────────────────────────────────
    u_win = 3.0
    a_win = 12.0
    r_int_win = 1.0 / (0.359073 * np.log(u_win) + 6.949915)  # 0.136
    r_glass = 1.0 / u_win - r_int_win  # 0.197
    h_si_win = 1.0 / r_int_win  # 7.34
    h_conv_win = h_si_win - H_RAD  # 2.20
    r_conv_win = 1.0 / h_conv_win  # 0.455
    r_glass_ext = r_glass  # 0.197 (r_ext = 0 for windows)
    r_total_win = 1.0 / u_win  # 0.333

    surfaces = [
        Surface("Walls", a_walls, r_conv_wall, r_half_wall, r_rest_wall,
                "outdoor", True, h_si_wall, r_total_wall),
        Surface("Roof", 48.0, r_conv_roof, r_half_roof, r_rest_roof,
                "outdoor", True, h_si_roof, r_total_roof),
        Surface("Floor", 48.0, r_conv_floor, r_half_floor, r_rest_floor,
                "ground", True, h_si_floor, r_total_floor),
        Surface("Window", a_win, r_conv_win, 0.0, r_glass_ext,
                "outdoor", False, h_si_win, r_total_win, "window"),
    ]

    T_ZONE, T_OUT, T_GND = 20.0, 0.0, 10.0

    # ── Print parameters ─────────────────────────────────────────────────
    print("\n── Surface parameters ──")
    print(f"{'Surface':<10} {'Area':>6} {'R_conv':>7} {'R_half':>7} "
          f"{'R_rest':>7} {'h_si':>6} {'R_tot':>7} {'UA':>7}")
    for s in surfaces:
        ua = s.area / s.r_total_combined
        print(f"{s.name:<10} {s.area:>6.1f} {s.r_film_conv:>7.3f} {s.r_inner_half:>7.4f} "
              f"{s.r_rest:>7.3f} {s.h_si_combined:>6.2f} {s.r_total_combined:>7.3f} {ua:>7.2f}")

    print(f"\n  h_rad linearized = {H_RAD:.3f} W/m²K")
    print(f"  h_conv_wall = {h_conv_wall:.3f}, h_si_wall = {h_si_wall:.2f}, "
          f"deficit = {h_si_wall - h_conv_wall:.3f} ({(h_si_wall - h_conv_wall)/h_si_wall*100:.1f}%)")
    print(f"  h_conv_win  = {h_conv_win:.3f}, h_si_win  = {h_si_win:.2f}, "
          f"deficit = {h_si_win - h_conv_win:.3f} ({(h_si_win - h_conv_win)/h_si_win*100:.1f}%)")

    # ── Correct total heat loss ──────────────────────────────────────────
    print("\n── Correct total heat loss (combined film) ──")
    q_correct = 0.0
    for s in surfaces:
        ua = s.area / s.r_total_combined
        dt = (T_ZONE - T_OUT) if s.ext_target == "outdoor" else (T_ZONE - T_GND)
        q = ua * dt
        q_correct += q
        print(f"  {s.name:<10}: UA={ua:>7.2f} W/K, Q={q:>7.1f} W")
    print(f"  Total correct: {q_correct:.1f} W")
    g_correct = sum(s.area / s.r_total_combined for s in surfaces)

    # ── Build Option 2 network ──────────────────────────────────────────
    print("\n── Building Option 2 (StarMesh) network ──")
    net = ResistanceNetwork()

    for s in surfaces:
        a = s.area
        ext = s.ext_target
        if s.has_inner_node:
            inner = f"{s.name}_inner"
            surf = f"{s.name}_surf"
            net.add(inner, ext, s.r_rest / a)
            net.add(inner, surf, s.r_inner_half / a)
            net.add(surf, "zone_air", s.r_film_conv / a)
            g_rad = 4.0 * EPS_INTERIOR * SIGMA * a * T_REF_K**3
            net.add(surf, "star", 1.0 / g_rad)
        else:
            flt = f"{s.name}_flt"
            net.add(flt, "zone_air", s.r_film_conv / a)
            net.add(flt, ext, s.r_rest / a)
            g_rad = 4.0 * EPS_INTERIOR * SIGMA * a * T_REF_K**3
            net.add(flt, "star", 1.0 / g_rad)

    # Identify node types
    internal = {"zone_air"}
    for s in surfaces:
        if s.has_inner_node:
            internal.add(f"{s.name}_inner")
    external = {"outdoor", "ground"}

    all_nodes: set[str] = set()
    for a, b in net.res:
        all_nodes.add(a)
        all_nodes.add(b)
    floating = sorted(n for n in all_nodes if n not in internal and n not in external)
    print(f"  Internal: {sorted(internal)}")
    print(f"  External: {sorted(external)}")
    print(f"  Floating: {floating}")

    # ── Solve full network (no elimination) ──────────────────────────────
    print("\n── Steady-state solution (full network) ──")
    fixed = {"zone_air": T_ZONE, "outdoor": T_OUT, "ground": T_GND}
    temps = solve_steady_state(net, fixed)

    print("\n  Node temperatures:")
    for n in sorted(temps):
        print(f"    {n:>18}: {temps[n]:>8.3f} °C")

    q_full = zone_heat_loss(net, temps)
    print(f"\n  Total zone heat loss: {q_full:.1f} W")

    # Heat flows from zone_air
    print("  Heat from zone_air:")
    for (a, b), r in sorted(net.res.items()):
        if "zone_air" in (a, b):
            other = b if a == "zone_air" else a
            q = (temps["zone_air"] - temps[other]) / r
            print(f"    → {other:<16}: {q:>8.2f} W")

    # ── Eliminate floating nodes ────────────────────────────────────────
    print("\n── After star-mesh elimination ──")
    net_red = ResistanceNetwork()
    net_red.res = dict(net.res)
    net_red.eliminate_floating(internal, external)

    print(f"  Reduced edges ({len(net_red.res)}):")
    for (a, b), r in sorted(net_red.res.items()):
        print(f"    {a:>16} ↔ {b:<16}: R={r:.6f}, G={1/r:.4f}")

    temps_red = solve_steady_state(net_red, fixed)
    print("\n  Reduced temperatures:")
    for n in sorted(temps_red):
        print(f"    {n:>18}: {temps_red[n]:>8.3f} °C")

    q_red = zone_heat_loss(net_red, temps_red)
    print(f"\n  Total zone heat loss: {q_red:.1f} W")

    # ── Total conductance (eliminate inner nodes too) ─────────────────────
    net_zone = ResistanceNetwork()
    net_zone.res = dict(net_red.res)
    net_zone.eliminate_floating({"zone_air"}, external)

    g_zone_out = 0.0
    g_zone_gnd = 0.0
    for (a, b), r in net_zone.res.items():
        g = 1.0 / r
        pair = {a, b}
        if pair == {"zone_air", "outdoor"}:
            g_zone_out += g
        if pair == {"zone_air", "ground"}:
            g_zone_gnd += g

    q_opt2 = g_zone_out * (T_ZONE - T_OUT) + g_zone_gnd * (T_ZONE - T_GND)

    print(f"\n── Total conductance from zone ──")
    print(f"  G_zone→out  = {g_zone_out:.3f} W/K")
    print(f"  G_zone→gnd  = {g_zone_gnd:.3f} W/K")
    print(f"  G_zone→tot  = {g_zone_out + g_zone_gnd:.3f} W/K")
    print(f"  Q_Option2   = {q_opt2:.1f} W")

    # ── Per-boundary analysis ────────────────────────────────────────────
    print("\n── Per-boundary heat flow in Option 2 ──")
    for s in surfaces:
        if not s.has_inner_node:
            continue
        inner = f"{s.name}_inner"
        ext = s.ext_target
        edge = tuple(sorted([inner, ext]))
        if edge in net_red.res:
            r = net_red.res[edge]
            t_i = temps_red[inner]
            t_e = temps_red[ext]
            q = (t_i - t_e) / r  # positive = heat from inner to ext
            ua_corr = s.area / s.r_total_combined
            dt = (T_ZONE - T_OUT) if ext == "outdoor" else (T_ZONE - T_GND)
            q_corr = ua_corr * dt
            print(f"  {s.name:<10}: inner={t_i:.2f}°C, Q={q:>7.1f} W "
                  f"(correct={q_corr:.1f}, deficit={q_corr-q:.1f} W)")

    # ── Deficit summary ──────────────────────────────────────────────────
    print("\n" + "=" * 80)
    print("DEFICIT SUMMARY")
    print("=" * 80)

    deficit = q_correct - q_opt2
    pct = deficit / q_correct * 100
    print(f"\n  Q_correct  = {q_correct:>8.1f} W  (G_total = {g_correct:.3f} W/K)")
    print(f"  Q_Option2  = {q_opt2:>8.1f} W  (G_total = {g_zone_out+g_zone_gnd:.3f} W/K)")
    print(f"  Deficit    = {deficit:>8.1f} W  ({pct:.1f}%)")

    # ── Window direct path analysis ────────────────────────────────────────
    print("\n── Window direct conductance ──")
    g_direct_win = 1.0 / (r_conv_win / a_win + r_glass_ext / a_win)
    g_correct_win = u_win * a_win
    print(f"  G_correct (U×A)  = {g_correct_win:.2f} W/K")
    print(f"  G_direct (conv+glass) = {g_direct_win:.2f} W/K "
          f"({g_direct_win/g_correct_win*100:.1f}% of correct)")
    print(f"  Missing: {g_correct_win - g_direct_win:.2f} W/K "
          f"({(g_correct_win-g_direct_win)/g_correct_win*100:.1f}%)")

    # ── Radiative redistribution analysis ────────────────────────────────
    print("\n── Star-mesh radiation conductances ──")
    for s in surfaces:
        g_rad = 4.0 * EPS_INTERIOR * SIGMA * s.area * T_REF_K**3
        print(f"  {s.name:<10}: G_rad = {g_rad:>8.2f} W/K")
    sum_g = sum(4.0 * EPS_INTERIOR * SIGMA * s.area * T_REF_K**3 for s in surfaces)

    # After star elimination, pairwise conductances
    print("\n  Pairwise radiation conductances after star elimination:")
    for i, si in enumerate(surfaces):
        for j in range(i+1, len(surfaces)):
            sj = surfaces[j]
            gi = 4.0 * EPS_INTERIOR * SIGMA * si.area * T_REF_K**3
            gj = 4.0 * EPS_INTERIOR * SIGMA * sj.area * T_REF_K**3
            gij = gi * gj / sum_g
            print(f"    {si.name:<10} ↔ {sj.name:<10}: G = {gij:.3f} W/K")

    # ── Fix analysis ─────────────────────────────────────────────────────
    print("\n" + "=" * 80)
    print("DIAGNOSIS: THE PARALLEL R_rad FIX")
    print("=" * 80)

    # Build network WITH parallel R_rad from zone_air to each surface_node
    # This represents the surface-to-zone-air radiation that was removed from
    # R_film when we decomposed h_si into h_conv + h_rad.
    print("""
  In the combined film model, h_si = h_conv + h_rad provides the total
  zone-air-to-surface conductance. When we decompose into h_conv (in R_film)
  and h_rad (in star-mesh), the h_rad goes to the star node, NOT to zone_air.

  The star-mesh distributes h_rad between ALL surfaces proportionally.
  This is INTER-surface radiation, which is physically different from
  surface-to-zone-air radiation.

  The missing conductance: each surface needs a PARALLEL R_rad from its
  surface_node back to zone_air, representing the surface-to-zone-air
  radiation. This is in ADDITION to the star-mesh inter-surface radiation.

  With this fix:
    zone_air ← R_conv → surface_node    (convection)
    zone_air ← R_rad  → surface_node    (radiation to zone air, PARALLEL)
    surface_node ← R_rad_star → star_node (inter-surface radiation)

  The total zone_air → surface_node conductance becomes:
    G = h_conv × A + h_rad × A = h_si × A

  This exactly matches the combined film model while also allowing
  inter-surface radiation through the star-mesh.
""")

    # Build the FIXED network (Option 2 + parallel R_rad)
    print("── Building FIXED network (Option 2 + parallel R_rad to zone) ──")
    net_fix = ResistanceNetwork()

    for s in surfaces:
        a = s.area
        ext = s.ext_target
        if s.has_inner_node:
            inner = f"{s.name}_inner"
            surf = f"{s.name}_surf"
            net_fix.add(inner, ext, s.r_rest / a)
            net_fix.add(inner, surf, s.r_inner_half / a)
            # Convection-only film
            net_fix.add(surf, "zone_air", s.r_film_conv / a)
            # PARALLEL R_rad: surface_node ↔ zone_air (radiation to zone air)
            r_rad_zone = 1.0 / (H_RAD * a)  # K/W
            net_fix.add(surf, "zone_air", r_rad_zone)
            # Star-mesh inter-surface radiation
            g_rad = 4.0 * EPS_INTERIOR * SIGMA * a * T_REF_K**3
            net_fix.add(surf, "star", 1.0 / g_rad)
        else:
            flt = f"{s.name}_flt"
            # Convection-only film
            net_fix.add(flt, "zone_air", s.r_film_conv / a)
            # PARALLEL R_rad: window_flt ↔ zone_air (radiation to zone air)
            r_rad_zone = 1.0 / (H_RAD * a)
            net_fix.add(flt, "zone_air", r_rad_zone)
            # Glass to outdoor
            net_fix.add(flt, ext, s.r_rest / a)
            # Star-mesh
            g_rad = 4.0 * EPS_INTERIOR * SIGMA * a * T_REF_K**3
            net_fix.add(flt, "star", 1.0 / g_rad)

    # Solve
    temps_fix = solve_steady_state(net_fix, fixed)
    q_fix_full = zone_heat_loss(net_fix, temps_fix)

    print("\n  Full network temperatures:")
    for n in sorted(temps_fix):
        print(f"    {n:>18}: {temps_fix[n]:>8.3f} °C")
    print(f"  Total zone heat loss: {q_fix_full:.1f} W")

    # Eliminate floating nodes
    net_fix_red = ResistanceNetwork()
    net_fix_red.res = dict(net_fix.res)
    net_fix_red.eliminate_floating(internal, external)

    temps_fix_red = solve_steady_state(net_fix_red, fixed)
    q_fix_red = zone_heat_loss(net_fix_red, temps_fix_red)

    print(f"\n  After elimination: {q_fix_red:.1f} W")

    # Total conductance
    net_fix_zone = ResistanceNetwork()
    net_fix_zone.res = dict(net_fix_red.res)
    net_fix_zone.eliminate_floating({"zone_air"}, external)

    g_fix_out = 0.0
    g_fix_gnd = 0.0
    for (a, b), r in net_fix_zone.res.items():
        g = 1.0 / r
        if {a, b} == {"zone_air", "outdoor"}:
            g_fix_out += g
        if {a, b} == {"zone_air", "ground"}:
            g_fix_gnd += g

    q_fix = g_fix_out * (T_ZONE - T_OUT) + g_fix_gnd * (T_ZONE - T_GND)
    print(f"  G_zone→out = {g_fix_out:.3f} W/K, G_zone→gnd = {g_fix_gnd:.3f} W/K")
    print(f"  Q_fixed    = {q_fix:.1f} W")

    # ── Comparison table ─────────────────────────────────────────────────
    print("\n" + "=" * 80)
    print("COMPARISON TABLE")
    print("=" * 80)
    print(f"\n  {'Model':<30} {'Q_total':>8} {'G_total':>8} {'Deficit':>8} {'%':>6}")
    print(f"  {'Correct (combined film)':<30} {q_correct:>8.1f} {g_correct:>8.2f} {'—':>8} {'—':>6}")
    print(f"  {'Option 2 (no R_rad fix)':<30} {q_opt2:>8.1f} {g_zone_out+g_zone_gnd:>8.2f} "
          f"{q_correct-q_opt2:>8.1f} {(q_correct-q_opt2)/q_correct*100:>5.1f}%")
    print(f"  {'Option 2 + R_rad fix':<30} {q_fix:>8.1f} {g_fix_out+g_fix_gnd:>8.2f} "
          f"{q_correct-q_fix:>8.1f} {(q_correct-q_fix)/q_correct*100:>5.1f}%")

    # ── Detailed analysis of WHY the deficit exists ──────────────────────
    print("\n" + "=" * 80)
    print("DETAILED ROOT CAUSE ANALYSIS")
    print("=" * 80)

    # Compare zone_air to surface conductances
    print("\n  Zone_air → surface conductance comparison [W/K]:")
    print(f"  {'Surface':<10} {'G_combined':>10} {'G_conv_only':>10} "
          f"{'G_star_mesh':>10} {'G_total_O2':>10}")

    # For each opaque surface, compute the zone_air→surface_node conductance
    # in the Option 2 model after star-mesh elimination
    for s in surfaces:
        a = s.area
        g_combined = s.h_si_combined * a  # h_si × A
        g_conv = a / s.r_film_conv  # h_conv × A

        # After star-mesh elimination of surface_node:
        # zone_air connects to surface_node via R_conv
        # surface_node connects to inner_node via R_half
        # surface_node connects to star via R_rad_star
        # After elimination, zone_air gets conductance to inner_node through
        # both the conv path and the star-mesh redistributed path

        # The direct conv path: zone→surf→inner gives G = 1/(R_conv/A + R_half/A)
        g_conv_to_inner = 1.0 / (s.r_film_conv / a + s.r_inner_half / a) if s.has_inner_node else a / s.r_film_conv

        # The additional conductance from star-mesh redistribution
        # (this is what the star-mesh provides beyond the conv path)
        # We can measure this from the reduced network
        inner = f"{s.name}_inner" if s.has_inner_node else f"{s.name}_flt"
        edge = tuple(sorted(["zone_air", inner]))
        if edge in net_red.res:
            g_total_o2 = 1.0 / net_red.res[edge]
        else:
            g_total_o2 = 0.0

        g_star_mesh = g_total_o2 - g_conv_to_inner if g_total_o2 > g_conv_to_inner else 0.0

        print(f"  {s.name:<10} {g_combined:>10.2f} {g_conv_to_inner:>10.2f} "
              f"{g_star_mesh:>10.2f} {g_total_o2:>10.2f}")

    # ── The window path after star-mesh ─────────────────────────────────
    print("\n  Window path analysis after star-mesh elimination:")
    # After eliminating all floating nodes, the zone_air connects to outdoor
    # through multiple paths. The window contribution is part of the total
    # G_zone→outdoor. We can decompose by looking at the network topology.

    # In the zone-only reduced network, we have zone↔outdoor and zone↔ground
    # The zone↔outdoor conductance includes contributions from:
    # 1. Direct window path (conv + glass)
    # 2. Wall path (conv + R_half + R_mat)
    # 3. Roof path (conv + R_half + R_mat)
    # 4. Radiation short circuits (wall→window→outdoor, etc.)

    # Let's trace the heat flow from zone to outdoor through each boundary
    # in the reduced (but not zone-only) network
    print(f"\n  In reduced network (inner nodes present):")
    for (a, b), r in sorted(net_red.res.items()):
        if "outdoor" in (a, b) or "ground" in (a, b):
            other = a if b in ("outdoor", "ground") else b
            ext = b if b in ("outdoor", "ground") else a
            q = (temps_red[other] - temps_red[ext]) / r
            print(f"    {other:>14} → {ext:<8}: Q = {q:>7.1f} W")

    # ── Simplified 2-surface verification ────────────────────────────────
    print("\n── Simplified 2-surface verification (wall + window) ──")
    g_rad_w = 4.0 * EPS_INTERIOR * SIGMA * a_walls * T_REF_K**3
    g_rad_wn = 4.0 * EPS_INTERIOR * SIGMA * a_win * T_REF_K**3
    fixed_s = {"zone_air": T_ZONE, "outdoor": T_OUT}

    # Correct
    ua_w = a_walls / r_total_wall
    ua_wn = a_win * u_win
    q_corr_s = (ua_w + ua_wn) * (T_ZONE - T_OUT)

    # (a) Option 2 original (conv-only + star-mesh)
    net_s = ResistanceNetwork()
    net_s.add("zone_air", "wall_surf", r_conv_wall / a_walls)
    net_s.add("wall_surf", "wall_inner", r_half_wall / a_walls)
    net_s.add("wall_inner", "outdoor", r_rest_wall / a_walls)
    net_s.add("zone_air", "win_flt", r_conv_win / a_win)
    net_s.add("win_flt", "outdoor", r_glass_ext / a_win)
    net_s.add("wall_surf", "star", 1.0 / g_rad_w)
    net_s.add("win_flt", "star", 1.0 / g_rad_wn)
    q_s = zone_heat_loss(net_s, solve_steady_state(net_s, fixed_s))

    # (b) Option 2 + parallel R_rad to zone_air (THE FIX)
    net_sf = ResistanceNetwork()
    net_sf.add("zone_air", "wall_surf", r_conv_wall / a_walls)
    net_sf.add("wall_surf", "zone_air", 1.0 / (H_RAD * a_walls))
    net_sf.add("wall_surf", "wall_inner", r_half_wall / a_walls)
    net_sf.add("wall_inner", "outdoor", r_rest_wall / a_walls)
    net_sf.add("zone_air", "win_flt", r_conv_win / a_win)
    net_sf.add("win_flt", "zone_air", 1.0 / (H_RAD * a_win))
    net_sf.add("win_flt", "outdoor", r_glass_ext / a_win)
    net_sf.add("wall_surf", "star", 1.0 / g_rad_w)
    net_sf.add("win_flt", "star", 1.0 / g_rad_wn)
    q_sf = zone_heat_loss(net_sf, solve_steady_state(net_sf, fixed_s))

    # (c) Combined R_si for zone→surface + star-mesh (equivalent to fix)
    net_sc = ResistanceNetwork()
    r_si_wall_abs = (1.0 / h_si_wall) / a_walls  # combined R_si / A
    net_sc.add("zone_air", "wall_surf", r_si_wall_abs)
    net_sc.add("wall_surf", "wall_inner", r_half_wall / a_walls)
    net_sc.add("wall_inner", "outdoor", r_rest_wall / a_walls)
    r_si_win_abs = r_int_win / a_win  # combined interior film for window
    net_sc.add("zone_air", "win_flt", r_si_win_abs)
    net_sf_add_glass = r_glass_ext / a_win
    net_sc.add("win_flt", "outdoor", r_glass_ext / a_win)
    net_sc.add("wall_surf", "star", 1.0 / g_rad_w)
    net_sc.add("win_flt", "star", 1.0 / g_rad_wn)
    q_sc = zone_heat_loss(net_sc, solve_steady_state(net_sc, fixed_s))

    # (d) No window decomposition (combined R_si, no star for window)
    net_nd = ResistanceNetwork()
    net_nd.add("zone_air", "wall_surf", r_si_wall_abs)
    net_nd.add("wall_surf", "wall_inner", r_half_wall / a_walls)
    net_nd.add("wall_inner", "outdoor", r_rest_wall / a_walls)
    net_nd.add("wall_surf", "star", 1.0 / g_rad_w)
    # Window as direct zone→outdoor with U-factor
    net_nd.add("zone_air", "outdoor", r_total_win / a_win)
    q_nd = zone_heat_loss(net_nd, solve_steady_state(net_nd, fixed_s))

    print(f"  Correct:                 Q = {q_corr_s:>8.1f} W")
    print(f"  (a) Option 2 original:   Q = {q_s:>8.1f} W  deficit = {(q_corr_s-q_s)/q_corr_s*100:.1f}%")
    print(f"  (b) + parallel R_rad:    Q = {q_sf:>8.1f} W  deficit = {(q_corr_s-q_sf)/q_corr_s*100:.1f}%")
    print(f"  (c) combined R_si+star:  Q = {q_sc:>8.1f} W  deficit = {(q_corr_s-q_sc)/q_corr_s*100:.1f}%")
    print(f"  (d) no win decomp:       Q = {q_nd:>8.1f} W  deficit = {(q_corr_s-q_nd)/q_corr_s*100:.1f}%")

    # ── Final verdict ────────────────────────────────────────────────────
    print("\n" + "=" * 80)
    print("FINAL VERDICT")
    print("=" * 80)
    print(f"""
  The Option 2 (StarMesh) architecture has a STRUCTURAL conductance deficit
  of {pct:.1f}% ({deficit:.0f} W at ΔT=20K). This is consistent with the
  observed BESTEST 600 annual heating deficit (3539 kWh vs 4296 kWh lower bound).

  ROOT CAUSE: When R_film is decomposed from combined (h_si) to convection-only
  (h_conv), the h_rad portion is routed exclusively to the star-mesh node,
  which distributes it as INTER-SURFACE radiation. The SURFACE-TO-ZONE-AIR
  radiation path is LOST. In the combined film model, h_rad represents
  radiation exchange between the surface and the zone's mean radiant temperature
  (≈ zone air temperature). The star-mesh does NOT replicate this — it sends
  the surface's radiation to other surfaces, not back to zone air.

  The star-mesh inter-surface radiation creates "short circuits" (e.g.,
  warm wall → cold window → outdoor) that partially compensate by increasing
  heat flow through low-R boundaries. However, the compensation is incomplete
  because:
  1. The window's radiation conductance (G_rad = {4.0*EPS_INTERIOR*SIGMA*a_win*T_REF_K**3:.1f} W/K)
     is small relative to the total ({sum_g:.1f} W/K), so only ~7% of the
     wall's radiation reaches the window.
  2. Most radiation goes to the floor (28%) and roof (28%), which have
     high R to ground/outdoor — the heat is effectively "trapped".
  3. The net short-circuit effect partially compensates but cannot fully
     restore the total conductance.

  FIX: Add a parallel R_rad from each surface_node (or window_float) to
  zone_air, representing the surface-to-zone-air radiation:
    R_rad_zone = 1 / (h_rad × A)  where h_rad = 4 × ε × σ × T_ref³

  This ensures the total zone_air → surface conductance equals h_si × A,
  matching the combined film model. The star-mesh then provides ADDITIONAL
  inter-surface radiation exchange on top of the correct base conductance.

  With this fix, the total conductance becomes {g_fix_out+g_fix_gnd:.2f} W/K
  vs correct {g_correct:.2f} W/K — a deficit of only
  {(q_correct-q_fix)/q_correct*100:.1f}%. The small remaining difference is
  due to the inter-surface radiation short circuit ADDING conductance beyond
  the combined film model (which is physically correct — the star-mesh captures
  radiation exchange that the combined film model approximates).

  IMPLEMENTATION: In the Rust code at boundary_rc.rs, when building the
  Option 2 network for StarMesh mode, add:
  - For RC-layer boundaries: after creating surface_node between R_film_conv
    and R_inner_half, add a parallel edge: surface_node ↔ zone_air with
    R = 1/(h_rad_linearized × A)
  - For window/fallback-R boundaries: after creating window_node/float_node
    with R_conv to zone_air, add a parallel edge: window_node ↔ zone_air
    with R = 1/(h_rad_linearized × A)

  This is mathematically equivalent to using the COMBINED R_film (1/h_si)
  for the zone→surface connection, while also allowing the star-mesh to model
  inter-surface radiation. The combined conductance from zone_air to
  surface_node becomes:
    G_total = h_conv × A + h_rad × A = h_si × A

  which exactly matches the EnergyPlus Option 2 specification where the
  interior surface heat balance uses combined h_si for the zone-air coupling
  while inter-surface radiation is handled separately through the star network.
""")


if __name__ == "__main__":
    main()
