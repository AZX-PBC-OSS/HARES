# Red team: BR-21, commit d41646e9

Verdict: ATTACK-SUCCEEDED

Counts by severity: Blocker 0, Major 0, Minor 5, Informational 3

## Scope and method

Commit d41646e9 (branch `i08-head-fixes`, parent `1c18543a`) claims: every profile and feature set the tests build is linted, every build compiles clean, and `make check-ashrae-reference` passes from a fresh checkout with no prior `uv sync`.

Disposable detached worktrees, all removed with `git worktree remove --force` when done:

- `.workstrees/redteam-br21` at d41646e9, `CARGO_TARGET_DIR=.workstrees/target-redteam-br21` (clippy matrix, child test listing)
- `.workstrees/redteam-br21-parent` at 1c18543a, `CARGO_TARGET_DIR=.workstrees/target-redteam-br21-parent` (parent test listing)
- `.workstrees/redteam-br21-uv` and `.workstrees/redteam-br21-ocv` at d41646e9, `CARGO_TARGET_DIR=.workstrees/target-redteam-br21-uv` (uv gates from a pristine tree, scratch edits)

Toolchain: rustc 1.94.1, cargo-nextest, uv 0.12.8. Build jobs capped at 8.

### What held

Clippy (`--all-targets -- -D warnings`, `-j 8`) exits 0 in all 58 configurations tried:

- The seven claimed lines: default, `-F dst`, `-F profiling`, `-F observe`, `-F observe,profiling`, `--all-features`, `--release`.
- Workspace extras: `--release -F observe`, `--release -F dst`, `--release -F profiling`, `--release --all-features`, `-F observe_detailed`, `--release -F observe_detailed`.
- `-p hares-envelope -F observe_detailed` (without `observe`), in dev and release.
- Each workspace member alone (`-p X`), in dev and release, so feature unification cannot hide anything: hares-types, -physics, -envelope, -control, -equipment, -io, -core, -fleet, -tariff, -python, frame_golden.
- Every member that declares `observe`, alone with `-F observe`, in dev and release.
- `hares-python --release -F profiling`, and `frame_golden --release` with `-F dst` and with `-F profiling`. These cover the golden capture and the maturin wheel crate.

No feature is used in `cfg(feature = ...)` without being declared. The 58 runs include every crate with `-D warnings`, so `unexpected_cfgs` would have failed one of them. The `check_invariants` feature is gone from every manifest and source file, as OR-11 intended.

The claimed parent defect reproduces. At 1c18543a, `cargo nextest list --workspace --release -F observe` fails with `error[E0425]: cannot find value r_total` at `crates/hares-core/src/dwelling/solver_builder.rs:512`.

Test counts from `cargo nextest list --workspace`, parent compared with child:

| build | parent 1c18543a | child d41646e9 | conditioned_oracle (parent / child) |
|---|---|---|---|
| debug | 7151 | 7151 (identical list) | 0 / 0 |
| `-F observe` | 7210 | 7210 (identical list) | 6 / 6 |
| `--release` | 7108 | 7108 (identical list) | 0 / 0 |
| `--release -F observe` | does not compile | 7161 | n/a / 0 |

No test was dropped from any build that compiled at the parent. The cfg moves cause no behaviour change in any build that compiled before. The `r_total` binding is new only in release+observe, which did not compile before. `MoistureZoneInvariant` is used only under `all(observe, debug_assertions)`.

`make check-ashrae-reference` passes from a pristine worktree (no `.venv`) in 1m32s.

## Finding 1 (Minor): the ASHRAE gate does not run with `--locked`, so a stale lockfile is silently relocked and the gate passes

File: `scripts/check_ashrae_reference.sh:35` runs `uv run --extra ochre python ...` without `--locked`. In the same block, `[python-build]` runs `uv run --locked ...`.

Repro (scratch worktree): add `"six>=1.16",` to the `ochre` extra in `pyproject.toml` and do not relock.

```
uv lock --check                       # -> "hint: To update the lockfile, run `uv lock`." (lock is stale)
bash scripts/check_ashrae_reference.sh; echo $?   # -> PASS, exit 0
git status --porcelain                # ->  M pyproject.toml /  M uv.lock
git diff --stat uv.lock               # -> uv.lock | 2 ++ (six added to the ochre extra and requires-dist)
```

The gate resolves and installs against a lockfile it rewrote itself. It passes on a tree whose committed `uv.lock` disagrees with `pyproject.toml`, and it modifies a tracked file as a side effect. In CI, the check would go green on a branch with a stale lock. Fix: `uv run --locked --extra ochre ...`, matching the rest of the block.

## Finding 2 (Minor): `allow(unused_imports)` hides dead imports, so they are never linted in any configuration

Files: `crates/hares-equipment/src/water_heater/{gas.rs:18, heat_pump_wh.rs:18, tankless.rs:15, indirect_tank.rs:25, resistance.rs:17}`. Each has `#[allow(unused_imports)] // warn! used only under debug_assertions` above `use tracing::warn;`.

BR-21's change item 1 says "an import or item used only under a `cfg` moves under that `cfg`. No `allow` is added." These allows predate the commit, but they suppress exactly the defect class the census is meant to find, and their comment is false:

- `grep -cE '\bwarn!\(' gas.rs heat_pump_wh.rs tankless.rs indirect_tank.rs` finds 0 uses each. The import is dead in every build, debug included.
- `resistance.rs` calls `warn!` unconditionally at line 399, so its allow is redundant.

Repro (scratch worktree): delete the allow line in gas.rs, heat_pump_wh.rs, resistance.rs and tankless.rs, then run

```
cargo clippy -j 8 -p hares-equipment --all-targets -- -D warnings
-> error: unused import: `tracing::warn`  --> water_heater/gas.rs:18:5
-> error: unused import: `tracing::warn`  --> water_heater/heat_pump_wh.rs:18:5
-> error: unused import: `tracing::warn`  --> water_heater/tankless.rs:15:5
-> could not compile `hares-equipment` (lib) / (lib test)
```

This fails in the dev default build, which the claim says is linted. Removing the allow from `indirect_tank.rs` was not exercised (the scratch edit was refused by the tool policy). Its zero `warn!` uses show the import is dead there too.

The `#[cfg_attr(not(test), allow(unused_imports))]` at `crates/hares-core/src/dwelling/mod.rs:100` is the same pattern. The schedule row already defers it to a follow-up.

Fix: delete the four dead imports and the five allows. Sweep the remaining `cfg_attr(not(feature = "observe"), allow(...))` sites (`scheduled_load.rs:211`, `resolve_hvac.rs:2139`, `infiltration.rs:185`, `heat_pump/defrost.rs:285`, `pv/mod.rs:160`) for the same treatment.

## Finding 3 (Minor): the build this commit repaired, and the golden builds, have no lint line

The commit's own subject is that `--release -F observe` did not compile. It now does, and lints clean (`cargo clippy --workspace --all-targets --release -F observe -- -D warnings` exits 0). But no line in `plan.md` §3 or in `.github/workflows/ci.yml` builds `--release -F observe`, so nothing stops the defect from coming back.

The block also runs `[golden]` as `cargo run --release -p frame_golden -F dst`, and no lint line covers `--release -F dst`.

The claim "lint coverage equals test coverage" holds only for the `[test-*]` lines, not for every build the block runs. Fix: add `--release -F observe` and `--release -F dst` clippy lines, both of which pass today. The simpler alternative is `--release --all-features`, which also passes and covers both, plus `observe_detailed`.

## Finding 4 (Minor): `make check` still fails from a fresh checkout, in the sibling gate, before the ASHRAE gate runs

`make check` = `check-no-magic-config check-si-guard check-ocv check-ashrae-reference`. `scripts/check_ocv.sh:34` runs `uv run python` with the default groups, and the generator needs the `pybamm` extra. This is the defect BR-21 fixed for the ASHRAE gate.

Repro (pristine worktree `redteam-br21-ocv`, no `.venv`):

```
make check
-> ... 44 tests run: 44 passed
-> bash scripts/check_ocv.sh
-> ERROR: gen_ocv.py failed
-> PyBaMM is not installed. ... uv sync --extra pybamm
-> make: *** [Makefile:83: check-ocv] Error 1
```

CI still needs `uv sync --extra pybamm` at `ci.yml:116` for the same reason. Fix: `uv run --locked --extra pybamm python ...` in `check_ocv.sh`, and drop the CI sync step and the stale hint text.

## Finding 5 (Minor): `conditioned_oracle` is gated out of release+observe as a whole, though only one diagnostic print needs the gate

File: `tests/conditioned_oracle.rs:18-22`. The entire test module now needs `any(debug_assertions, feature = "observe_detailed")`. The only gated read is `solver.envelope_gains.int_surface_diag` (lines 433-458): an `eprintln!` dump of interior LWR surface diagnostics. That field is `#[cfg(any(debug_assertions, feature = "observe_detailed"))]` at `crates/hares-envelope/src/thermal_solver/config.rs:1030`.

`boundary_diagnostics_count()` (`thermal_solver/mod.rs:515`) is not gated. The OCHRE comparisons and all assertions use only `observe` items.

Result: 6 tests in `-F observe`, 0 in `--release -F observe`. The listing at child d41646e9 is 7161 tests with no conditioned_oracle. No test is lost relative to the parent, because that build did not compile there. But the fix moved a whole module under the cfg, where change item 1 says the item that uses the cfg moves. A future release+observe test line would silently skip the six OCHRE oracle tests. Fix: gate the diagnostic block alone and keep the module on `#[cfg(feature = "observe")]`.

## Informational 1: stale run instruction in the generator

`scripts/gen_ashrae_reference.py` lines 10-15 still tell the reader to run `uv run python scripts/gen_ashrae_reference.py`, saying `xmltodict` comes from the `ochre` group. Without `--extra ochre`, that command fails on a fresh checkout. The check script's hint was updated, but this docstring was not.

## Informational 2: the ASHRAE gate builds the Rust extension to run a pure-Python script

`gen_ashrae_reference.py` imports no HARES code. Still, `uv run --extra ochre` installs the project, which makes maturin build `ochre_next._hares` in release. On the cold run that took most of the 1m32s, and it also needs a Rust toolchain in the CI `check-ashrae-reference` job. The parent's `uv sync --extra ochre` did the same, so this is not a regression.

Run after `[python-build]` (`maturin develop --release --uv`), the gate replaced the project's `dist-info` with uv's cached editable install. The `.so` was unchanged (same sha1 `b7f63fe9...`, same mtime), so `[pytest] --no-sync` still saw the release build. A cache miss would rebuild the project.

`uv run --locked --no-project --with xmltodict==<locked>`, or a PEP 723 header, would avoid all of this.

## Informational 3: CI clippy job cost

The CI clippy job now runs seven full workspace builds one after another, with no cache step. This is correct, but it is the slowest job. `--release --all-features` (Finding 3) adds an eighth.

## Repro commands (summary)

```
git -C /home/rich/src/HARES worktree add .workstrees/redteam-br21 d41646e9
CARGO_TARGET_DIR=.workstrees/target-redteam-br21 cargo clippy -j 8 --all-targets --workspace [--release] [-F ...|--all-features] -- -D warnings   # 58 configs, all exit 0
CARGO_TARGET_DIR=... cargo nextest list --build-jobs 8 --workspace [-F observe] [--release]   # counts table above
make check-ashrae-reference                     # pristine: PASS (1m32s)
# stale lock: edit pyproject ochre extra, then bash scripts/check_ashrae_reference.sh -> PASS, uv.lock rewritten
make check                                      # pristine: fails in check-ocv
```
