# Decision Log

## dl-e735a7c8-7a13-4829-99f4-bd58329bff96

```yaml
event_id: dl-e735a7c8-7a13-4829-99f4-bd58329bff96
timestamp: 2026-09-13T04:33:07.476Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 10902c4131024a9024ccef1cbb96af994f1550e48c95c88d0b3fa4d00e5aed6f
reason: workspace_override
```



## Done-challenge produced changes — 2026-09-13T04:50:52.109Z

- **Phase:** diagnose
- **Run:** I-04-diagnose-15b297aa-9c0f-43cc-8893-29e0cdebf6cc

## Done-challenge produced changes — 2026-09-13T05:45:24.092Z

- **Phase:** fix
- **Run:** I-04-fix-e08c7f3b-3051-400b-b331-82d739a733d5
## Verification gate raced a concurrent agent's edit windows — needs an operator decision

- **Phase:** fix (gate re-run after challenge)
- **What happened:** the `must_pass` gates `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` failed with type errors in files the I-04 diff never touched: `Option<ZipLoad>` vs `Option<ResolvedZip>` at `crates/hares-python/src/py_dwelling.rs:914` and `crates/hares-core/src/dwelling/conversions.rs:1606`, and "missing fields `azimuth_deg`/`tilt_deg`" at `crates/hares-core/src/dwelling/solver_builder.rs:1106/1220`. None of those APIs exist at this checkout's HEAD (`git status` clean for every failing file; `InteriorSurfaceInfo` in `hares-envelope/src/thermal_solver/config.rs` ends at `driving_temp`).
- **Evidence of the cause:** a concurrent agent works in this shared checkout while also driving the `/tmp/hares-f13` worktree (`git worktree list`, branch `f13-final`). Their edits apply and revert in windows: `crates/hares-python/src/py_dwelling.rs` mtime 2026-09-12 16:17 and `crates/hares-equipment/src/ev/config.rs` mtime 13:29 — hours after checkout — with content identical to HEAD, i.e. written then restored. Builds launched inside a half-applied window (rlibs stamped 22:45 during the failing gate run) compile the transient API mismatch; builds outside the window compile HEAD cleanly. An isolated-target run of the same HEAD passed 6518/0 while the shared-dir run failed minutes earlier.
- **Resolution state:** with the tree quiescent, the gate's exact commands pass: `cargo test --workspace` → 6787 passed / 0 failed (109 result lines, 0 failures); `cargo clippy --workspace --all-targets -- -D warnings` → exit 0; `cargo fmt --all -- --check` → clean. No code change can remove this cause from inside I-04 — it is the workspace-sharing policy, not a defect in the tree.
- **Needs deciding (operator):** how concurrent agents verify in one shared checkout — per-agent `CARGO_TARGET_DIR` isolation, serialization of verification windows, or landing/quarantining the f13 work. Until decided, any verification gate in this checkout can flake red on another agent's in-flight edits, independent of the code under review.


## Done-challenge produced changes — 2026-09-13T07:06:53.774Z

- **Phase:** round-out-coverage
- **Run:** I-04-round-out-coverage-93e7ec69-f70c-4dea-be28-ef4939e0b61e

## Done-challenge produced changes — 2026-09-13T07:43:51.938Z

- **Phase:** fix
- **Run:** I-04-fix-b7d0063f-36a4-40cf-896a-1ff734f8f8b9

## Done-challenge produced changes — 2026-09-13T08:16:03.955Z

- **Phase:** red-team
- **Run:** I-04-red-team-1e3a992d-e64f-4df2-b170-9e7bb5345e3e

## Correction — SolarSurplus was never affected by the needed_charge_hours sentinel collapse

- **Phase:** fix (review round)
- **What was wrong:** the RCA's mechanism 2 (rca.md, "### 2.") claimed "`DepartureDeadline` is used only by `PreDeparture` and `TouAware`" and listed `SolarSurplus` among the strategies reporting `needed_charge_hours == 0.0`. Both halves are false: `build_preferences` (crates/hares-core/src/actors/ev_driver/mod.rs) has always installed a `DepartureDeadline { target_soc: 1.0 }` in the `SolarSurplus` stack, and `DepartureDeadline`'s `needed_charge_hours` override predates this initiative — the composer's minimum-fold picked it up, so `SolarSurplus` reported a real (to-full) estimate all along. Surfaced by the review (review.md, "Observations").
- **What it says now:** rca.md mechanism 2 names the affected set as `Immediate`, `Nightly`, `LowSoc`, `QuickThenWait`, `V2G`, `V2H` and marks `SolarSurplus` explicitly not affected, with the correction noted inline; the ticket's Implementation Notes bullet drops `SolarSurplus` from the previously-affected list with a pointer to this entry.
- **Other documents citing the old version:** the ticket's Implementation Notes (corrected in the same pass). The reproduction test's strategy set (Immediate, Nightly, LowSoc, QuickThenWait) never included `SolarSurplus`, so `reproduction.md` carries no version of the wrong claim.
- **Fix correctness unaffected:** the fix (real estimates from `SocTarget`/`SocGate` through the shared helper) is still correct for the six genuinely affected strategies; `SolarSurplus` merely also benefits from the later observed-SOC sourcing.