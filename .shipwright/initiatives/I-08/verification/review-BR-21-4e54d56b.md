# Review of BR-21 follow-ups, commits 4872f244 and 4e54d56b (parent d41646e9)

Verdict: APPROVED-FOR-FOLD

Findings: 0 blocker, 0 major, 1 minor, 2 nit.

Reviewed in a detached worktree at 4e54d56b (vendors/OCHRE initialised, no .venv), CARGO_TARGET_DIR outside the repo, -j 8. Worktree and target dir removed afterwards.

## Verification results

- `cargo fmt --check --all`: exit 0.
- All nine plan.md section 3 clippy lines (`[clippy]` all-features, default, dst, profiling, observe, observe+profiling, release, release dst, release observe), each `--workspace --all-targets -- -D warnings`: exit 0.
- nextest `--workspace --no-fail-fast`: default 7151 passed; `-F observe` 7210 passed; `--release` 7108 passed; `--release -F observe` 7167 passed. No failures.
- Test counts against d41646e9 (counts and per-config lists recorded in redteam-BR-21-d41646e9.md): default 7151 = 7151, observe 7210 = 7210, release 7108 = 7108. Release observe 7161 -> 7167: the six `conditioned_oracle` tests now compile and pass there, which the commit states. No test removed from any build. I did not rebuild the parent myself; the comparison is by count against the prior review's recorded figures (`cargo nextest list --message-format oneline` produced no output here), so per-test list identity for the cfg-only files rests on counts plus the passing runs.
- `frame_golden compare-all` release and release `-F dst`: exit 0.
- `make check` in the fresh worktree with no .venv: exit 0 in 1m57s (44 no-magic-config tests, OCV all OK including the new `defaults/ocv_tables` diff, ASHRAE PASS). `git status` clean afterwards, uv.lock untouched.
- ruff check, ruff format --check and pyright on `scripts/gen_ocv.py` and `scripts/gen_ashrae_reference.py`: all clean (0 errors). `tests/python/test_ev_archetypes.py`: 35 passed.

## Claim by claim

1. Locked gate fails on a stale lock without rewriting uv.lock: real. In a scratch copy I added `six>=1.16` to the pybamm extra without relocking; `check_ocv.sh` exited 1 with "The lockfile at uv.lock needs to be updated, but --locked was provided", and uv.lock was byte-identical afterwards. `check_ashrae_reference.sh:35` carries the same `--locked` flag (textual check; the ASHRAE script was not re-run against a stale lock). Closes red-team findings 1 and 4.
2. `make check` from a fresh worktree with no .venv: passes (above).
3. `check_ocv.sh` fails on a changed table value: real. In a scratch copy I changed one voltage in `defaults/ocv_tables/nmc_ocv.toml` (2.862492 to 2.862493); the new `diff -ru` step printed the diff, set DIFF_FOUND and the script exited 1 with the updated FAIL text. The regenerated tables differ from the previous ones only in the version header (diff reviewed).
4. Every removed suppression's cause is fixed, not moved; no `allow(` added (grep of both diffs: zero added `allow(`).
   - Water heater `use tracing::warn` (gas, heat_pump_wh, indirect_tank, tankless): imports deleted, dead in every build. resistance.rs keeps the import (used unconditionally), allow removed.
   - `dwelling/mod.rs`, `autosize.rs`: `derive_dwelling_rng` and `serde_json::Value` moved into `mod tests`; non-test imports lose the cfg_attr.
   - `scheduled_load.rs:211-240`: role chosen first, `zone_map.get(role)` and the zone assignment run in every build, only the tracing match is under `observe`. Same assignment semantics as before; the old `single_match` allow is gone because the match no longer exists in non-observe builds.
   - `resolve_hvac.rs:2139`: `heating_loop_end` bound only under `observe`; all readers are observe-gated (clippy clean in all configs).
   - `infiltration.rs:185-212`: tuple kept, `q_nat_m3_s` taken in all builds, the other three bound under `observe`. Values unchanged.
   - `defrost.rs:285`: `frost_before` is read in every build; suppression deleted, clippy clean.
   - `pv/mod.rs:160-170,511-550`: the two telemetry-only fields compile under `any(test, observe)` at the struct and both constructors. Previous allow(dead_code) removed.
   - `hpxml/building.rs:1602-1630`: `pitch_source` derived from `pitch.is_some()` inside the observe block; the default-pitch warn and `pitch_absent_ids` path are unchanged.
   - `fluid_solver.rs` `allow(unused_assignments)` removed; clippy clean in all nine configs.
5. fluid_solver conservation telemetry (`fluid_solver.rs:588-700`): correct. The prior code accumulated a local `violations` and stored it only after the loop, but the loop returns `Err` on the first violation, so the store was unreachable on exactly the case it counted. Now `num_conservation_violations` and `max_mass_imbalance_kg_s` are updated immediately before the return, inside the observe cfg block. Peak value is the running max of node imbalances in this call including the violating node; the post-loop update still covers the no-violation path. Whole change is `#[cfg(feature = "observe")]`; non-observe builds contain no new code. Test at `fluid_solver.rs:1993-2002` (observe-gated) asserts count 1 and imbalance 0.2 (1.0 in vs 1.2 out); it ran and passed in the observe builds.
6. Pyo3 seed removal (`py_equipment.rs:766-767`): `from_vehicle_with_archetype` has no other caller (grep over crates, tests, python, docs, notebooks, benches, tools); `_hares.pyi:1349` and the three Python test call sites updated; the Python suite passes.
7. Script changes: `gen_ocv.py --toml-dir` defaults to the previous directory, the check passes its tmp dir; `gen_ashrae_reference.py` slab record and `raw_dt` bindings are behaviour-preserving (ASHRAE fixture diff in `make check` is empty).
8. CI: `ci.yml` clippy job lists nine lines matching plan.md section 3 lines 1985-1993 exactly. The OCV job's `uv sync --extra pybamm` step is removed, correct since the script now supplies the extra.
9. Writing check: no em dashes, ticket IDs or history terms in added diff lines. See N-2 for the commit messages.

## Findings

### minor

M-1. Stale PyBaMM pin left contradicting the lock. `scripts/pybamm-requirements.txt:1-5` still pins `pybamm==26.3.0` and says the hardcoded tables were generated with that exact version and that CI enforces it; `crates/hares-equipment/src/battery/ocv.rs:7-9` still says "generated ... using PyBaMM 26.3.0 (2026-03-23)". The commits changed the regeneration instruction in the same ocv.rs paragraph but left the false version claim, while uv.lock pins 26.9.0.0 and the tables now state 26.9.0.0. The requirements file is now referenced nowhere (the only reference, in check_ocv.sh, was removed by 4872f244). Fix: delete `scripts/pybamm-requirements.txt` and drop the version from the ocv.rs doc (say "the PyBaMM release pinned in uv.lock").

### nit

N-1. The fluid_solver test covers the splitter case only; the commit message says "splitter or mixer". Both share the one code path, so the risk is low; a mixer case would pin the claim.

N-2. Commit message of 4872f244 uses "used to regenerate" and "no longer need" (history phrasing); message-only, not in code or docs. Acceptable for a commit body; noted for the writing check.

## Residual risk

- Per-test list identity versus d41646e9 is supported by equal counts and clean runs, not by a diff of listings.
- ASHRAE `--locked` behaviour on a stale lock was verified for the OCV script only; the flag in the ASHRAE script is identical.
- No test line exercises `observe_detailed` (unchanged from the prior review).
