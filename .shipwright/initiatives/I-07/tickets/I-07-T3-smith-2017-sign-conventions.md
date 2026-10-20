---
id: I-07-T3
title: "Degradation model diverged from Smith 2017 / NREL SSC on four constants and lacked the negative-electrode branch"
status: done
created_at: 2026-09-24
created_by: i07-fix
resolved_at: 2026-09-24
resolution: >-
  Fixed in the same session (the operator's no-deferral directive). The
  plan's merge-condition verification was completed against BOTH the
  open-access NREL preprint (NREL/CP-5400-67102) and NREL's own reference
  implementation (SSC lib_battery_lifetime_nmc.{h,cpp}) — decisive for
  four sign conventions the PDF's inner-fraction minus glyphs hide from
  every text extractor. Outcome: α_b1 = −1 was CONFIRMED correct (the
  paper's printed "+1.0" line is an artifact; SSC ships −1); b3_ref is a
  POSITIVE break-in loss (+2.805e-2, not OCHRE's negated boost), θ is
  +0.135, and b0 = 1.07 — all three corrected. The model also gained the
  pieces HARES (like OCHRE) lacked entirely: the negative-electrode
  site-loss branch (Eq. 8–11, c0/c2 with the runaway site-loss update) and
  the d0 reference ratio, with usable capacity = min(QLi, Qneg) per Eq. 1
  — BOL capacity now sits ≈ +0.9% above nameplate (the reference model's
  state) instead of OCHRE's +2.8% boost-from-rated. The rainflow counter
  gained the generalized Σ count·DOD^β damage sum (β = 2 for the Li
  branch, βc2 = 4.54 for the site-loss branch). The Li-branch cycle
  aggregation (Miner-rule Σ count·DOD²) is HARES's documented deliberate
  choice — the paper (Eq. 4's −b2·N), OCHRE (per-cycle Arrhenius, no
  DOD), and SSC (b2²·√Σ(DOD·count)²) each aggregate differently; noted in
  the code and the initiative notes. Degradation checkpoint schemas
  bumped (Battery v3, EV v5); every degradation-pinned test re-derived
  against the reference behavior; the full workspace, parity corpus
  (battery-SOC metric is SKIPped per-fixture — pre-existing), and the
  Python suite all green.

# Smith 2017 sign-convention alignment (degradation recalibration class)

## Finding (from the I-07 merge-condition verification)

The I-07 fix plan's merge condition — verify `deg_const` against Smith et
al. 2017 — was executed against the open-access NREL preprint
(NREL/CP-5400-67102, "Life Prediction Model for Grid-Connected Li-ion
Battery Energy Storage System", 2017 American Control Conference). Result:

**Verified verbatim** (fitted-parameter list and Eq. 5–7):
`b1_ref` 3.503e-3, `Ea_b1` 35392 J/mol, `γ` 2.472, `β_b1` 2.157, `b2_ref`
1.541e-5, `Ea_b2` −42800 J/mol, `b3_ref` magnitude 2.805e-2, `Ea_b3` 42800
J/mol, `α_b3` 0.0066, `τ_b3` 5 days, `θ` magnitude 0.135, and the reference
constants (Tref 298.15 K, Vref 3.7 V, U−,ref 0.08 V). The validated-domain
claim also verifies: the aging tests span 0 °C to 55 °C (Table I).

**Divergent** — the in-tree values follow OCHRE's Matlab-derived constants
(`vendors/OCHRE/ochre/Equipment/Battery.py:380-390`, which itself flags
them: `b0 = 1  # 1.07  # TODO: Matlab value varies`, `theta = -0.135  #
TODO: might be positive? using value from Matlab, not paper`) rather than
the paper's printed values:

| Constant | HARES/OCHRE | Paper (NREL/CP-5400-67102) |
|---|---|---|
| `B0` | 1.0 | 1.07 |
| `ALPHA_B1` | −1.0 | +1.0 (Eq. 5) |
| `B3_REF` | −2.805e-2 | +2.805e-2 (Eq. 7's −b3(1−exp) form) |
| `THETA` | −0.135 | +0.135 |

Net effect: HARES implements the break-in transient as a small
beginning-of-life capacity *boost* relaxing from rated (~+2.8 %), while the
paper models a decay from a 7 %-above-nameplate sqrt-time intercept toward
+4.2 % — opposite-sign transients relative to nameplate. The activation
energies (which the I-07 domain guard rests on) are unaffected.

## Why this was initially tracked rather than fixed in I-07 (superseded — fixed in-session)

The original scoping note treated the alignment as a recalibration-class
deferral (moving every fade trajectory, re-deriving every pinned test,
interacting with the drift-locked parity bands). The operator's
no-deferral directive resolved it in the same session; the measured
outcome: the full workspace suite green after re-deriving the pinned
tests, and the parity corpus unaffected (the Battery-SOC metric is
SKIPped per-fixture for all fixtures — pre-existing; the energy/temperature
metrics move only at the ~1e-7 scale one hour of degradation implies).
The complete evidence trail — including the decisive cross-check against
NREL's SSC that overturned half of the initial finding (α_b1 was correct
all along) — is in the resolution above and the initiative notes.

## Work (done — see the resolution)

1. Decide the target convention (paper's printed values vs the
   Matlab/OCHRE lineage — the paper is the primary authority unless the
   Matlab values are shown to be a later correction by the authors).
2. Re-derive affected constants and every test that pins a fade value.
3. Re-baseline the OCHRE parity Battery-SOC band through the reference
   regeneration path if the residual moves beyond the 1 % margin.
