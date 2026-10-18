---
review_outcome: clean_with_nits_applied
---

# Review: I-07 — re-review after the fix loop addressed the prior round

The prior round of this review filed one Critical (non-finite floats
silently becoming defaults at the `overrides` and tariff Python
boundaries, with two failing gate tests left red) and three Warnings
(citation-gate coverage narrowing, the `i07_diag.rs` tombstone
deferral, the untracked HELICS flake). This pass verified each
disposition against the tree, re-ran every gate myself, and attacked
the new code. Everything filed is fixed or genuinely resolved; what
remains is two Observations. One trivial artifact nit (a stale Known
Limitations entry for the now-deleted tombstone) I fixed directly —
recorded under `fixes_applied`.

## Disposition of the prior round's findings

- **Critical — non-finite floats at the `overrides`/tariff
  boundaries: FIXED, and the fix went further than the finding.**
  The duplicated converters are one shared home
  (`crates/hares-python/src/utils.rs:28`, `python_to_json_value`),
  whose f64 arm rejects non-finite floats with `PyValueError` naming
  the value and the mechanism (utils.rs:41-54). All three per-module
  copies are deleted (the py_config.rs third copy is gone from the
  tree — I confirmed via `git show HEAD:` that it existed and via grep
  that only the shared one remains). My two red gates pass **unedited**
  (`uv run pytest tests/python/test_py_dwelling_integration.py::TestConstructionRoundTrip
  tests/python/test_tariff_builder.py` → 27 passed). The implementer
  also closed a fourth channel my finding did not name — the
  `EquipmentSpec` parameters path — at the `make_spec` choke point
  (`reject_nulled_value`, utils.rs:135-159, rejecting `null` at any
  depth with the key path; sound because `insert_opt` skips `None`s,
  utils.rs:91-94), with its own permanent pin
  (`test_non_finite_spec_parameter_fails_loudly`). I grep-verified that
  every `EquipmentSpec` construction in hares-python crosses
  `make_spec` (no direct struct literals remain) and that exactly one
  `insert_opt` definition exists.
  My own edge-case probes against the unified converter, all loud:
  nested NaN in a list-in-dict (`ValueError`), `10**400` (`cannot
  convert ... type 'int'` — the documented fall-through), deep-nested
  `inf`, and `numpy.float64('nan')` all raise; a numpy *array* is
  rejected as an unsupported type. The loop's own class sweep then
  closed the remaining members with the same mechanism — actor params
  (`py_to_config_value`, py_dwelling.rs:2720-2768, f64 and Vec<f64>
  arms), solar override (post-parse walk, py_dwelling.rs:1573-1608,
  covering every parse arm by construction), price signal and grid
  voltage (`finite_f64_optional`/`validate_finite`, py_dwelling.rs:882-897),
  ControlSignal constructors and from_dict (23 faces, nine constructors
  now `PyResult<Self>`), and the OCV/U-neg tables
  (`battery/ocv.rs:156-163, 303-313`). I spot-checked the ControlSignal
  faces by execution: NaN `reactive_setpoint`/`power_factor`/
  `event_delay`/`power_setpoint(min_soc=NaN)` all raise
  field-named `ValueError`s; the finite controls build. The class is
  closed.
- **Warning — citation-gate narrowing: FIXED with the right shape.**
  Both scripts now `find -prune` by full path instead of
  `grep --exclude-dir` basenames, so `crates/hares-io/src/hpxml/` is
  scanned while `docs/hpxml` and `tests/fixtures/hpxml` stay exempt —
  the fix names the basename-matching trap in its own comment
  (check-ashrae-chapters.sh:22-30). Verified by execution: both scripts
  exit 0, and the informational section of the ashrae log now lists
  `crates/hares-io/src/hpxml/building.rs` citation lines — the
  production parser directory is visibly back under the gate.
- **Warning — the `i07_diag.rs` tombstone: RESOLVED.** The file is
  deleted (`git status`: `AD`; `ls` confirms absence) — the operator
  removed it, as the round note records (initiative.md:1364). The one
  leftover is the stale Known Limitations entry, fixed under
  `fixes_applied`.
- **Warning — the HELICS flake: FIXED, and I re-ran it in the flaking
  configuration.** The two wall-clock assertions (`elapsed < 5.0`,
  `< 15.0`) are now load-insensitive (30 s / 90 s) with the backstop
  preserved: each file's thread-method `pytest.mark.timeout` (60 s /
  120 s) still catches a true hang, and the real discrimination (fires
  vs hangs) was always carried by the `pytest.raises(TimeoutError)`
  arms. Weakening an assertion is normally a smell; here the asserted
  property (promptness on an absolute wall clock) is inherently
  machine-dependent, and the safety property (no hang) keeps its guard
  — the constraint named at each site is real. Verified:
  `uv run pytest tests/python/test_helics_federate.py
  tests/python/test_helics_integration.py -q -n auto` → **33/33
  passed** at the 28-worker default that used to flake.
- **Prior Observations: both addressed.** `heater_threshold_c`'s field
  doc now states the deliberate no-ordering-validation contract
  (protect-only policy, ev/config.rs:408-416); `deg_const` names the
  SSC lineage and warns that the vendored copy is an older revision
  with different constants (degradation.rs:223-226) — exactly the note
  I asked for.

## What I attacked this round (and what I found instead)

- **The unified converter's recursion and ordering** — bool-before-int
  (Python bool is an int subclass) is correct; dict keys must be
  strings (loud); the catch-all rejects unsupported types loudly. All
  verified by the probes above.
- **A false-positive hunt on `reject_nulled_value`** — the guard fires
  on any null in spec params; a legitimate null would break real
  builds. The full Python suite (1249 passed, below) exercises every
  `*_spec_from_py` path and is green, so no legitimate null exists in
  practice; the code comment's grep claim (`insert_opt` skips Nones) I
  re-verified by reading.
- **The weather boundary in the same class** — `hares-io/src/weather.rs`
  *deliberately* treats NaN as a missing-data sentinel (documented at
  :350, :1107; ZOH fallback; test-pinned at :1560-1568, :1909-1918).
  That is the meteorological missing-value convention, designed and
  covered — not the silent-substitution class. Not a finding; naming it
  so the next sweep doesn't re-derive it.
- **The nine `PyResult<Self>` constructor changes** — value-triggered
  rejection only; valid inputs build identically (probed, and the
  Python suite's 1249 includes the control-signal consumers).
- **Suite-wide gates, run by me on this tree:** workspace
  `cargo test --workspace --tests` → **7037 passed, 0 failed**; full
  Python suite (`-n 4`) → **1249 passed, 12 skipped** (the named
  pre-existing observer/Helics feature-gated skips); clippy
  `--workspace --tests -D warnings` → 0; `cargo fmt --check --all` →
  clean; both citation scripts → exit 0. Every number the round notes
  claim, reproduced.

## fixes_applied

- `initiative.md` Known Limitations: the `i07_diag.rs` entry claimed
  the tombstone "remains" and named a future operator action — but the
  operator removed the file in the red-team-round-4 session (recorded
  at line ~1364). A Known Limitations entry describing a resolved item
  as outstanding is a silent artifact self-contradiction; I marked the
  entry resolved with a pointer to the round note. One edit, no
  history rewritten.

## Observations

- The unified converter's rejection message names the value and the
  mechanism but not the **key path** — its siblings do
  (`reject_nulled_value` formats `key[2]`/`key.sub`; `finite_walk`
  tracks dotted paths). For a large nested `overrides` payload the
  operator must bisect for the NaN themselves. The failure is loud and
  the common case (a flat per-equipment dict) self-locates, so this is
  diagnosability polish, not a silent failure — noted rather than
  filed. For the implementer's weighing, not mine: the constitution's
  boundary-error rule literally reads "names the field, the value
  received, and the expected form" (the converter names value and
  mechanism), and `reject_nulled_value` in the same file already
  demonstrates the path-threading pattern — if the loop ever touches
  this function again, that is the fifteen lines to add.
- The `python_to_json_value` f64-arm comment's huge-int claim ("pyo3's
  f64 extraction of such ints fails in practice") is hedged prose for a
  behavior I verified by execution (`10**400` → loud `cannot convert
  ... type 'int'`). The hedge is honest; the gate in
  `test_py_config.py` pins the loudness without pinning the layer,
  which is the right call for a pyo3-behavior dependency.

## Acknowledged limitations

The five modeling boundaries from earlier rounds (Bernardi entropic
term, road-speed convective cooling, constant-power heater, sub-zero
degradation extrapolation, OCHRE thermal parity) stand as conceded —
each names a real data constraint, the magnitude, and the error
direction; nothing about this round's changes touches them. The HELICS
bound widening is conceded above with its reasoning verified, not
inherited.

## Deep review questions

1. **Solving the named problem?** Yes — the gh #32 cold-blackout fix
   stands (all reproduction gates green again this pass), and the
   boundary class my prior round re-opened is now closed across every
   channel I enumerated plus ones I missed.
2. **Right approach?** Yes — one converter home, choke-point guards at
   each boundary entry, post-parse walks where parse arms multiply.
   The loop consistently chose boundary-entry guards over downstream
   screens, which is the right layer.
3. **Smells/hacks?** None new. The xfail lifecycle on the routed gates
   was honored exactly as designed (run green with `--runxfail`, then
   markers removed — the gates are now permanent regressions).
4. **Right architecture?** Yes — utils.rs is the correct home; the DAG
   is untouched.
5. **Idiomatic?** Yes — `PyResult` constructors at the raising
   boundary, field-named errors, no swallowed causes anywhere I read.
6. **Pure/derived over duplication?** This round *removed* the
   duplication my prior finding named (three converters → one) plus
   the `dict_optional`/validator copies. The tree is strictly less
   duplicated than before the finding.
7. **Gates satisfied or worked around?** Satisfied — and the one gate
   whose coverage had narrowed (the citation script) now scans more
   production code than it did before this initiative touched it.
8. **Debuggable/observable?** Yes, with the one key-path polish item
   noted under Observations.

## Strongest case against this verdict

The strongest argument for `findings` is that the converter message
missing the key path is an operability defect under the review
charter's own 3am test ("the signal carries the operation, the inputs
identifying the occurrence, and the cause"). I refute the elevation:
the constitutional bar for this class is *loudness* ("fail loudly with
a typed error naming the field, the value received"), and the message
names the value and the channel; the *sibling* paths name the key
because they walk structured configs where a path exists — the
converter walks an untyped Python object where the payload itself is
the user's own dict, printed in the traceback context. Loud, specific
about mechanism, immediately actionable. Observation is the honest
tier; elevating it would be manufacturing a Warning.

## Verified

Commands run on this tree, with results:

- `cargo test --workspace --tests` → **7037 passed, 0 failed**.
- `uv run pytest tests/python/ -q -n 4` → **1249 passed, 12 skipped**
  (includes my two prior-round gates, green unedited).
- `uv run pytest tests/python/test_py_dwelling_integration.py::TestConstructionRoundTrip
  tests/python/test_tariff_builder.py` → 27 passed (the gates).
- `uv run pytest tests/python/test_non_finite_input_boundaries.py
  test_actor_param_boundary.py test_solar_override_boundary.py
  test_blueprint_errors.py` → 81 passed; `test_ev_pack_thermal_boundary.py
  test_battery_pack_thermal_boundary.py test_py_dwelling_integration.py
  test_tariff_builder.py test_py_config.py` → 116 passed.
- `uv run pytest tests/python/test_helics_federate.py
  tests/python/test_helics_integration.py -q -n auto` → 33 passed at
  the previously-flaking 28-worker default.
- `cargo clippy --workspace --tests -- -D warnings` → exit 0;
  `cargo fmt --check --all` → exit 0; `bash
  scripts/check-ashrae-chapters.sh` and
  `scripts/check-energyplus-sections.sh` → both exit 0.
- Interactive probes (uv run python): nested/deep/numpy NaN and
  `10**400` through `overrides` all raise; ControlSignal NaN faces
  raise field-named errors while finite controls build; pre-fix
  `py_config.rs` third converter copy confirmed removed
  (`git show HEAD:...` vs grep of current tree).

Facts established by reading (file:line):

- utils.rs:28-88 (shared converter; non-finite f64 arm), :91-94
  (`insert_opt` skips Nones), :110-159 (`make_spec` +
  `reject_nulled_value` with key paths), :295-380 (the shared
  validators; finiteness-first ordering).
- py_dwelling.rs:1573-1608 (solar post-parse walk over all four
  fields, step/surface indexed), :2720-2768 (actor-param f64 and
  Vec<f64> guards), :882-897 (price/voltage guards).
- check-ashrae-chapters.sh:22-30 — the find-prune fix names the
  basename trap itself; the gate's informational output lists
  `crates/hares-io/src/hpxml/building.rs` (production parser back under
  scan).
- ev/config.rs:408-416 and degradation.rs:223-226 — both prior
  Observations' doc notes present.
- `git status` shows `AD crates/hares-core/tests/i07_diag.rs` — the
  tombstone is deleted from the working tree (operator action, recorded
  at initiative.md:1364).
- weather.rs:350/:1107/:1560-1568 — the weather NaN-as-sentinel design
  is deliberate, documented, and test-pinned (checked so the class
  sweep doesn't mis-flag it).

Not re-verified this round (unchanged since my last pass, stated not
implied): the pre-fix failure evidence (my scratch-worktree probe last
round), the SSC trunk constant comparison (last round; the code and
the reference have not moved since).
