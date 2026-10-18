---
coverage_outcome: adequate
new_test_paths:
  # Files carrying the coverage phase's own additions across all
  # seventeen rounds, plus triage's probe (the solar-override file,
  # whose gates the fix adopted). Fix-phase gate files are not
  # attributed here.
  - crates/hares-equipment/src/ev/tests.rs
  - crates/hares-core/tests/equipment_id_regressions.rs
  - tests/python/test_py_config.py
  - tests/python/test_actor_param_boundary.py
  - tests/python/test_solar_override_boundary.py
  - tests/python/test_non_finite_input_boundaries.py
failing_tests: []
---

# Coverage triage: I-07 — EV pack thermal model alignment

## Verdict: `adequate` — seventeen coverage rounds; the routed
## four-member boundary finding is fixed at one shared helper home with
## all 28 gates green (xfail markers self-destructed as designed);
## the class re-sweep is closed at zero; I-07-T4 is resolved. Nothing
## live to route.

This is the ninth triage run over the round-out-coverage output. Since
the previous run (which routed the non-finite boundary class's four
remaining members as `defect_found`): the **fix division landed guards
for all four members through shared helpers consolidated in one home**
(`finite_f64_required`/`finite_f64_optional` in
`crates/hares-python/src/utils.rs`, beside the existing validation
family — the DRY shape the constitution requires), the routed
strict-xfail gates **flipped green and had their markers removed** —
the self-destruct pattern working as designed — and **coverage round
17** measured the fixes, re-swept the class (including a surface the
earlier sweeps had never enumerated — the `SimulationConfig` setters),
and added no tests, with the one candidate dismissed under the
one-test-per-mechanism rule. **I-07-T4 was resolved by the fix loop**
(the comment now states the verified mechanism; both layers re-verified
by execution). This run assesses the note through round 17.

## What I verified this pass (against the tree, not the note's word)

- **The guards are at the fields, through the shared helpers.** Read
  `py_control.rs`: `finite_f64_optional`/`finite_f64_required` applied
  at the previously-raw fields (`:415-417` ThermalSetpoint's three,
  `:425-426` PowerSetpoint's pair, and on through the arm map — 20 uses
  in py_control.rs, 6 in py_dwelling.rs). One helper home, applied per
  field — the mechanism-level fix, not 15 site patches.
- **The routed gates green with markers removed:**
  `tests/python/test_non_finite_input_boundaries.py` → **28 passed,
  0 xfailed** (run this pass) — the self-destruct-then-remove cycle
  completed exactly as the pattern intends, and the gates are now
  permanent regression coverage.
- **The boundary family whole:** the file plus the solar and
  actor-param boundary files → **45 passed** (28 + 4 + 13, the note's
  arithmetic confirmed by execution).
- **Round 17's extra-surface claim spot-checked:** the
  `SimulationConfig` f64 setters are guarded and gated —
  `test_invalid_setpoint_deadband_raises_value_error` and the
  site-location setter gates are inside the full-suite run below.
- **Full suites, run this pass:** `uv run pytest tests/python/ -q -n 4`
  → **1249 passed / 12 skipped / 0 failed / 0 xfailed** — no expected
  failures remain anywhere in the Python suite;
  `cargo test -p hares-equipment --lib` → **2337 passed / 0 failed**
  (this cycle's changes are hares-python + Python tests; the Rust
  signal is unchanged and re-confirmed).
- **I-07-T4 resolved:** read the ticket — the fix loop corrected the
  comment to the verified mechanism (huge ints fail pyo3 extraction
  and fall to the catch-all; the `is_finite` arm rejects genuine
  NaN/±inf) and re-verified both layers by execution. All four
  initiative tickets are now done.

## Every finding assessed, with disposition

1. **The routed four-member boundary finding** (price signal, grid
   voltage, equipment LUT/OCV, ControlSignal's raw fields) — **fixed**
   at the shared-helper level, all 28 gates green, class re-sweep at
   unguarded count 0 including the config-setter surface. Verified at
   source and by execution. Closed.
2. **Round 17's no-tests-added conclusion** — correct: the flipped
   routed gates are the permanent coverage, and the dismissed candidate
   (per-site parametrize ids for the remaining guarded fields) would
   repeat the mechanism test per site — the exact repetition the
   one-test-per-mechanism rule weighs against, with round 15's
   arm-level-gate precedent. No action.
3. **The failure modes surfaced across this initiative's cycles**
   (silent SOC-monitor skip; zero-flows `Discharging`; sub-threshold
   `Off` window; non-finite config substitution; the overrides/tariff
   channels; the actor-param boundary; the solar-override boundary;
   the four-member boundary sweep) — all fixed, all gated, all green
   this pass. The non-finite Python boundary class is closed with a
   crate-wide enumeration on record (my prior run's: every
   float-carrying extraction site guarded, fixed, or routed — and the
   routed set now fixed).
4. **The standing dismissals and tickets** — T1–T4 all resolved;
   modeling-boundary dismissals unchanged and uncontradicted. No
   action.

## What would go wrong if nothing further is done

Nothing nameable. Every failure mode surfaced across seventeen coverage
rounds, five red-team runs, a review round, and triage's own probes is
fixed and gated; both Python input boundaries and the runtime control
boundary reject non-finite input loudly at the boundary that received
it; the monitors detect absent input; the mode classifier and the
guard share exactness. The suite carries zero expected-failures — the
first time in this initiative's arc that no known-defect markers are
outstanding.

## Residual accepted risk, stated

The full Rust workspace count was not re-derived this pass (concurrent
staged work; the cycle's changes are hares-python + Python tests); the
Rust signal (2337/0 lib, 1272/0 hares-core from the prior runs) was
partially re-confirmed (lib this pass). The ControlSignal arm map's
per-field guard coverage beyond the spot-checked fields rests on the
fix's helper application pattern (20 call sites, one helper family) —
the gates pin the mechanism on both entry styles, and the fix round's
own read of each constructor is on record.

Hence `adequate`; `failing_tests` is empty per the schema — no live
defect exists for a must_fail gate to prove.

## Challenge response (post-writing reassessment)

**Overreach:** none in this run — this document plus read-only commands
(reads, greps, test runs). No production code, no tests, no other
artifacts touched.

**Shortfall closed — the leg my own residual-risk section admitted
resting on the note's word:** the ControlSignal arm map's per-field
coverage. The 15 mapped faces were execution-verified (their gates
pass), but round 17's claim that *every* f64 field in `from_dict` is
guarded — including the beyond-map fields — I had not read myself.
Read the complete arm map now (`py_control.rs:410-542`): every f64
field in every arm carries a guard — `finite_f64_required`/
`finite_f64_optional`, or `dict_required` + `validate_range`/
`validate_non_negative`. The one real trap in the second form — a
range/non-negative check without a finiteness pre-check passes NaN
silently (NaN comparisons are false) — is closed at the helper level:
read both helpers (`utils.rs`), and **both call `validate_finite`
first**, with the doc comment naming exactly the NaN-slip reason. The
arms carrying no f64 fields (ModeOverride, SelfConsumption,
GridConnect, ProtocolNative, IdealCapacityModeOverride, EvPlugIn) are
genuinely float-free. Also read `set_grid_voltage` itself (the one
routed member I had confirmed only via its gate): guarded with
`validate_finite` plus a rationale comment naming the release-silence
mechanism. The class-closure claim now rests on my own read of the
arm map, not the note's.

**Abdication — the strongest case for action despite `adequate`:** the
NaN-through-`validate_non_negative` trap above was it — a helper that
*looked* like a guard but would have passed NaN if written
comparison-first. It is written finiteness-first, and the gates cover
both entry styles. The riskiest unaddressed finding: none live —
every candidate either closed by reading this pass (arm map, helpers,
grid voltage) or tracked (T4, resolved). The riskiest hypothetical —
a *future* ControlSignal arm adding an f64 field without the helper —
is bounded by the 15-face parametrized gate standing as the mechanism
pin and by the uniform helper pattern a new arm would follow. Nothing
nameable goes wrong if no further work is done.
