# Decision Log

## dl-02-fix-session-closeout

```yaml
event_id: dl-02-fix-session-closeout
timestamp: 2026-09-11T18:30:00.000Z
actor: fix
action: session_closeout_with_cross_agent_coordination
reason: >
  I-02 closed: the zone-load contract carries net interior-face convection
  (opaque_conduction_kwh + window_conduction_kwh + internal_mass_kwh), the
  gross exterior flux lives in observer diagnostics with OCHRE-parity Ext.
  semantics, and the +141 MWh phantom-gain class is structurally dead —
  gate test green with its assertion untouched. The same defect class was
  then swept codebase-wide: five further name-vs-meaning defects fixed with
  pinning tests; the exterior rad_res exact parallel form derived
  independently by two concurrent agents and landed jointly (0.392 K
  measured steady-state error pre-fix, exact post-fix, oracle opaque gap
  30.9 -> 7.3 percent); fallback-R exterior boundaries routed through the
  same skin divider (superseding the in-flight runtime-branch design —
  algebraically identical, reconciled in the tracker). Hardening program
  items landed: SkinCoupling unification, ThermalSolverConfig::validate,
  streaming==batch metrics equivalence, wiring property tests, skin
  closure suite across resistance ratios 0.02-100, skin-balance runtime
  invariant (converged-gated, bound-derived 0.5 K tolerance), divergence
  ledger, flake ledger. Open and escalated with evidence: parity-corpus
  zone-temp drift (bisect in flight), P14 zone-balance residual dominated
  by the unreported StarMesh LWR term (diagnosis in the tracker, Agent B
  converging). All coordination via
  docs/physics-hardening-program.md per the contract.
```

## dl-02-fix-exterior-skin-coupling

```yaml
event_id: dl-02-fix-exterior-skin-coupling
timestamp: 2026-09-11T23:10:00.000Z
actor: fix
action: physics_correction_corroborated
target: crates/hares-core/src/dwelling/solver_builder.rs (exterior rad_res)
reason: >
  The exterior radiative split's rad_res used the bare film resistance
  instead of the exact parallel combination R_film*R_outer/(R_film+R_outer)
  -- OCHRE Envelope.py:257-258 keeps the exact form commented out under a
  res_material >> res_film assumption, and EnergyPlus CalcOutsideSurfTemp
  solves the outside face with every path in parallel. Derived
  independently here and proven by a measured steady-state test on a real
  assembled siding wall (0.392 K error pre-fix, exact post-fix,
  tests/exterior_skin_balance.rs). The production fix (skin_rad_coupling)
  landed concurrently from another agent working the same derivation in
  this tree; corroborated, not duplicated. Companion fixes in the same
  family: exterior diagnostics split to skin-level OCHRE-parity semantics
  (freefloat oracle opaque gap 30.9% -> 7.3%, now like-for-like), and
  gross_consumption_kwh redefined to the load-behind-the-meter (was the
  positive part of the net meter; an always-exporting PV home reported
  0.0 consumption -- exposed by the resstock smoke identity, now 5/5).
```

## dl-02-fix-parity-gate-darkness

```yaml
event_id: dl-02-fix-parity-gate-darkness
timestamp: 2026-09-11T21:30:00.000Z
actor: fix
action: escalated_with_evidence
target: tests/parity (zone-temperature drift vs OCHRE references)
reason: >
  The parity corpus fails at HEAD (zone-temp MAE 0.68-1.69 C vs the 0.6 C
  band) but PASSES at commit 3235d9e2 (2026-04-17, where the band was
  calibrated to an observed 0.14-0.55 C band) against the same local
  reference parquets (2026-04-01). The drift accumulated over 693 commits
  and went undetected because the reference parquets are gitignored
  (.gitignore:49 *.parquet) -- the parity gate only runs on machines that
  happen to hold local copies, so it enforced nothing for everyone else.
  A git bisect on the cz4a_ashp_hpwh zone-temp MAE was started (probe
  /tmp/bisect_parity.sh, per-step log /tmp/bisect_maes.log, worktree
  /tmp/hares-calib) but requires multi-hour rebuilds. Owner decisions
  needed: (1) commit the reference fixtures so the gate runs everywhere;
  (2) complete the bisect and judge each drift source -- legitimate
  E+-alignment changes (bands recalibrated with per-band evidence per the
  fixture_override convention) or genuine regression (fix the physics).
  Neither was done in this round: recalibrating against an unexplained
  drift would bless it, and rushed physics judgement is what this
  initiative must not ship.
```

## dl-02-fix-contract-correction

```yaml
event_id: dl-02-fix-contract-correction
timestamp: 2026-09-11T20:27:00.000Z
actor: fix
action: contract_corrected
target: crates/hares-io/src/output/metrics.rs (EnvelopeComponentLoadsKwh)
reason: >
  The ticket's implied contract — that all nine envelope_loads_kwh components
  are summable zone loads — was wrong for one component. opaque_solar_lwr_kwh
  carried the gross exterior solar+LWR absorption (an outside-face boundary
  condition, EnergyPlus ERM 26.1 "Outside Surface Heat Balance"; OCHRE's
  "{boundary} Ext. Solar/LWR Gain"), not the net heat delivered to the zone.
  It now carries the net interior-face convection ("Inside Heat Balance":
  Interior Convection) under the renamed field opaque_conduction_kwh.
  Documents that carried the old name/claim: the ticket repro script
  (initiative.md body — field renamed, scripts fail loudly on the old name),
  rca.md fix-option framing (historical record, superseded by the
  reproduction gate's rewiring decision), reproduction.md gate semantics
  (already anticipated the rename/remove option).
```

## dl-02-fix-verification-gate-pre-existing-failures

```yaml
event_id: dl-02-fix-verification-gate-pre-existing-failures
timestamp: 2026-09-11T21:35:00.000Z
actor: fix
action: resolved_and_recharacterized
target: cargo test -p hares-envelope -p hares-io -p hares-core (must_pass gate)
reason: >
  Originally escalated as three pre-existing failures proven identical at
  base commit c2e12206. Resolution during this round: the three stale
  should_panic alignment tests are FIXED (converted to real tests with the
  parity corpus's loud-skip convention; all pass against local references).
  The parity corpus failures are recharacterized from "stale bands, owner
  decision" to an evidence-backed zone-temperature drift since 2026-04-17
  with the gate dark off this machine -- see
  dl-02-fix-parity-gate-darkness. The hares-io transient flake (two
  observations, 45 consecutive green runs since, not reproduced) is
  recorded in the ticket Implementation Notes for the suite owner.
```

## dl-17523204-0367-4038-bf47-69e17261104e

```yaml
event_id: dl-17523204-0367-4038-bf47-69e17261104e
timestamp: 2026-09-11T19:40:36.962Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 10902c4131024a9024ccef1cbb96af994f1550e48c95c88d0b3fa4d00e5aed6f
reason: workspace_override
```



## Done-challenge produced changes — 2026-09-11T23:33:55.693Z

- **Phase:** fix
- **Run:** I-02-fix-577ea542-3e92-4ce8-9d69-7e367c792e81

## Done-challenge produced changes — 2026-09-12T01:01:17.088Z

- **Phase:** fix
- **Run:** I-02-fix-577ea542-3e92-4ce8-9d69-7e367c792e81

## Done-challenge produced changes — 2026-09-12T01:58:53.943Z

- **Phase:** red-team
- **Run:** I-02-red-team-d85c72bc-13ce-46e0-a758-92bb8aa39008

## Done-challenge produced changes — 2026-09-12T04:57:59.190Z

- **Phase:** fix
- **Run:** I-02-fix-c12cdebc-acde-4f33-ab92-d66d5fe709c5

## Done-challenge produced changes — 2026-09-12T05:05:36.519Z

- **Phase:** round-out-coverage
- **Run:** I-02-round-out-coverage-08e148c2-1d7f-4489-8063-2adedf224cf2

## Done-challenge produced changes — 2026-09-12T05:38:54.139Z

- **Phase:** red-team
- **Run:** I-02-red-team-9c8eda03-398d-4503-b59b-735850830ff3