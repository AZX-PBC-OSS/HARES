---
retro_outcome: sound
---

# Retrospective: I-03 — HPXML-declared EV never charges under `Immediate`/default strategy

## The bug, restated

`Ev::actor_seed()` short-circuited `ChargingStrategy::Immediate` to `None`,
on the reasoning that an immediate-charging EV needs no charging-schedule
actor. But the seed it withheld produces `EvDriverActor`, which is not just
schedule logic — it is the *only* built-in component that simulates vehicle
departures and depletes SOC. `Immediate` is the default strategy, so every
HPXML-declared or `add_ev`-constructed EV without an explicit non-`Immediate`
override got no driver: it sat at `HomePluggedIn` forever, SOC pinned at its
initial value (which defaults to `soc_max == 1.0`), and
`compute_charging_power_kw` correctly returned 0.0 kW every step because
nothing ever discharged the battery. The equipment was fully wired and its
config genuinely honoured; the auto-actor-attachment path under-provisioned
the one component responsible for simulating vehicle use, for exactly the
variant most configurations resolve to.

## Diagnosis divergence, and what was misleading

The ticket's suspected cause — "the equipment-override path accepts a bad
`charging_strategy` without error" — was wrong, and the divergence is
instructive. `Ev::new()` does have a parse fallback site, which is what made
the hypothesis plausible, but `new()` is never the equipment's final state:
every construction path calls `init()` immediately after, whose re-parse
fails loudly. The symptom ("0.0 kWh", "override content seems discarded")
*looked* like a parse-degradation bug because the fallback to `Immediate` and
the actor-withholding guard produce the same visible outcome for the same
configurations. The lesson: a suspicious code pattern near the symptom is
evidence, not a cause — the RCA's scratch tests (one reproducing 0.0 kWh with
no actor, one proving `init()` rejects malformed JSON) were what separated
the real mechanism from the decoy.

## Reassessment: what the first draft of this retrospective got wrong

The first draft of this document claimed the defense was installed and cited
the fix step's tests. Challenged on it, three honest answers emerged:

- **Overreach:** none beyond the record — the only non-defense change was
  correcting the stale Known Limitation in `initiative.md` (flagged by the
  review; the document-chain rule requires prior documents cede authority).
- **Shortfall:** the defenses I claimed were the *fix step's*, not this
  role's, and two of them leaked. The class-wide EV test enumerated the nine
  variants by hand — a tenth variant would compile and silently escape the
  assertion. The `f64` non-finite rule existed only as a constitution
  sentence while the broken shape remained representable in ~25 parse sites.
- **Abdication:** I had written "a shared finite-parse helper is follow-up
  work" — the precise failure mode of this role. The somewhere existed
  (`hares_types::text`, which already exports the shared `parse_trimmed_f64`
  used across hares-io) and the mechanism was cheap.

All three are now closed with mechanisms, installed by this retrospective:

## Defenses installed (files, lines, checks that now fail)

1. **Variant-exhaustiveness tripwire — EV.**
   `crates/hares-equipment/src/ev/tests.rs`,
   `actor_seed_returns_ev_seed_for_every_charging_strategy` now ends with an
   exhaustive `match` naming every `ChargingStrategy` variant, no wildcard
   arm. What it makes impossible: adding a variant to the enum without
   deciding whether it belongs in the every-variant seed assertion — the
   crate fails to compile (`E0004: non-exhaustive patterns`). Watched it
   fail: removing the `V2G` arm produces exactly that error; arm restored.

2. **Variant-exhaustiveness test — Battery (the sibling site).**
   `crates/hares-equipment/src/battery/mod.rs`,
   `actor_seed_pins_every_bms_mode_variant` constructs all seven `BmsMode`
   variants, asserts `Manual` — and only `Manual` — yields no seed, and
   carries the same no-wildcard exhaustive match as a compile-time tripwire.
   Previously the Battery exemption (deliberate, per the RCA's class
   measurement) was pinned only for two variants; a new variant would have
   silently inherited the default arm. 1 passed.

3. **Shared finite-parse mechanism.**
   `crates/hares-types/src/text.rs`: `parse_trimmed_f64` — the shared helper
   hares-io already funnels through — now rejects non-finite values
   (`.filter(|v| v.is_finite())`), documented *why*, with two unit tests
   (`parse_trimmed_f64_rejects_non_finite_values` covering
   nan/NaN/inf/-inf/infinity/-Infinity, plus an ordinary-numbers test).
   What it makes impossible: any caller of the shared helper silently
   accepting `"nan"`/`"inf"` from input text. Then migrated every reachable
   hand-rolled `.parse::<f64>()` site to it or added the finite check:
   - `hares-io/src/hpxml/resolve_loads.rs` (5 sites),
     `hpxml/xml_helpers.rs` (5), `hpxml/resolve_water_heater.rs` (5),
     `schedule.rs`, `resstock_csv.rs`, `tmy3.rs` (2), `epw.rs` (3),
     `defaults.rs` (3 sites; 4 others already guarded `is_finite`),
     `hares-equipment/src/ev/charging_curve.rs` (2 — EV PyBaMM LUT).
   - `psm3.rs` keeps a two-stage parse so its non-finite error names the
     cause ("value is non-finite") — pinned by the pre-existing
     `psm3_rejects_non_finite_lwdown` test, which caught my first migration
     merging the two error categories (watched it go red, then green).
   - Audited and deliberately untouched, with the reason recorded:
     `hvac/core_config.rs` (6 sites) tokenizes only `[0-9+\-.eE]` — a
     non-finite literal cannot be tokenized; `resstock.rs:404` and
     `envelope_lut.rs:190` pre-filter to digit-only strings;
     `data_patches.rs`, `defaults.rs:1590+`, `epw.rs:1163/1209/1223`,
     `pv/lut.rs::parse_csv_f64` already check `is_finite`.
     Search used: `grep -rn "parse::<f64>()" crates/hares-io/src
     crates/hares-equipment/src`, every hit classified above.

4. **Constitution** (judgement-call layer, both in
   `/home/rich/src/HARES/.shipwright/constitution.md`):
   - *Tests section:* a guard that exempts one enum variant from a shared
     mechanism is a class risk, pinned by a test over every variant — with
     Battery's `Manual` named as the correct-exemption exemplar.
   - *Failure section:* text parsed into `f64` must reject non-finite
     values, because `f64::from_str` accepts `"nan"`/`"inf"`.

5. **Doc chain.** `initiative.md`'s stale Known Limitation (claiming
   `Ev::new()` still silently falls back to `Immediate`) corrected — the
   `defer_strict` → `init_error` path removed that fallback; verified against
   current code before editing.

## Where the broken shape survives today

Nowhere reachable, per the audit in (3): every `parse::<f64>()` site in
hares-io and hares-equipment is either finite-checked, routed through the
finite-rejecting helper, or fed by a digit-only tokenizer. The
variant-exemption shape survives at zero equipment sites — the class
measurement found exactly two `actor_seed` implementations, and both now
carry every-variant tests with compile-time exhaustiveness tripwires.

## Verification (commands run, output observed)

- `cargo fmt --check --all` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo nextest run --release --no-fail-fast --profile ci -p hares-types
  -p hares-equipment -p hares-io` — 4173 passed, 0 skipped.
- `cargo nextest run --release -F check_invariants --no-fail-fast
  --profile ci -p hares-core` — 1078 passed, 21 skipped.
- `.venv/bin/python -m pytest tests/python/test_ev_tariff_integration.py -q`
  — 21 passed.
- Negative checks: EV tripwire minus one arm → `E0004` compile error
  (restored); psm3 two-stage error preserved its pinned "non-finite" message
  (watched red → green).

## Outcome rationale

`sound`: the fix addresses the confirmed root cause; the class was measured
(two `actor_seed` sites, one deliberate) and both sides are now pinned by
every-variant tests with compile-time tripwires; the review's non-finite
`f64` finding is closed as a class by the shared helper plus a full-tree
migration, not by one site's patch; and the generalizable rules live in the
constitution. The defenses a future change would hit are mechanisms that
fail — compile errors and red tests — not notes asking a reader to remember.
