---
reproduction_outcome: confirmed
failing_tests:
  - command: "cargo test -p hares-equipment --lib ev::tests::actor_seed_default_strategy_returns_ev_seed -- --exact"
    path: crates/hares-equipment/src/ev/tests.rs
    name: actor_seed_default_strategy_returns_ev_seed
  - command: "cargo test -p hares-equipment --lib ev::tests::actor_seed_immediate_strategy_returns_ev_seed -- --exact"
    path: crates/hares-equipment/src/ev/tests.rs
    name: actor_seed_immediate_strategy_returns_ev_seed
  - command: "cargo test -p hares-equipment --lib ev::tests::actor_seed_returns_ev_seed_for_every_charging_strategy -- --exact"
    path: crates/hares-equipment/src/ev/tests.rs
    name: actor_seed_returns_ev_seed_for_every_charging_strategy
---

# Reproduction: HPXML/default EV with `Immediate` (or no) charging strategy never charges

## Cause (from `rca.md`, restated)

`Ev::actor_seed()` (`crates/hares-equipment/src/ev/mod.rs:1369-1380`) returns
`None` whenever the resolved `charging_strategy` is `ChargingStrategy::Immediate`
— which is both the default when no override is given and what any explicit
`Immediate` override resolves to. The seed it withholds is the only source of
an `EvDriverActor`, which is the only built-in component that simulates vehicle
departures and depletes SOC. Without it, an HPXML/`add_ev`-constructed EV sits
at `HomePluggedIn` forever, SOC never drops below its initial value (which
defaults to `soc_max == 1.0`), and `compute_charging_power_kw` returns 0.0 on
every step — the ticket's "0.0 kWh over 30 days".

## Tests written

Three tests appended to `crates/hares-equipment/src/ev/tests.rs`, pitched at
the broken rule (charging strategy selects *when/how* to charge, never
*whether* the vehicle is driven) rather than the single reported path:

1. **`actor_seed_default_strategy_returns_ev_seed`** — config with no
   `charging_strategy` key (mirrors `hares-io/src/hpxml/resolve_der.rs:206-232`
   exactly) must yield `Some(ActorSeed::Ev)` carrying the resolved
   `Immediate { target_soc: 1.0 }`.
2. **`actor_seed_immediate_strategy_returns_ev_seed`** — an explicit
   `Immediate { target_soc: 0.8 }` override must yield the same seed shape,
   with the override's `target_soc` preserved (also pins that the strategy's
   content is carried through, addressing the ticket's "override content is
   discarded" observation).
3. **`actor_seed_returns_ev_seed_for_every_charging_strategy`** — class-wide
   guard: all nine `ChargingStrategy` variants must produce an `Ev` driver
   seed, so the same "skip the actor for the trivial variant" mistake cannot
   be reintroduced next door.

`actor_seed()` is the shared mechanism every construction path
(`build_actors_from_seeds` in `hares-core`, the python bindings) funnels
through, so asserting the invariant there covers all of them with no
dwelling-level scaffolding.

## Confirmation

```
$ cargo test -p hares-equipment --lib actor_seed --
test ev::tests::actor_seed_default_strategy_returns_ev_seed ... FAILED
test ev::tests::actor_seed_immediate_strategy_returns_ev_seed ... FAILED
test ev::tests::actor_seed_returns_ev_seed_for_every_charging_strategy ... FAILED

---- ev::tests::actor_seed_default_strategy_returns_ev_seed ----
expected Some(ActorSeed::Ev) for default strategy, got None
---- ev::tests::actor_seed_immediate_strategy_returns_ev_seed ----
expected Some(ActorSeed::Ev) for Immediate strategy, got None
---- ev::tests::actor_seed_returns_ev_seed_for_every_charging_strategy ----
strategy Immediate { target_soc: 0.9 } must still produce an Ev driver seed, got None
```

All three fail on the intended assertion (`actor_seed()` returning `None`),
not on setup. Existing tests `actor_seed_nightly_returns_ev_seed`,
`actor_seed_immediate_returns_none`, and the battery seed tests still pass.

## Note for the fix step

`ev::tests::actor_seed_immediate_returns_none` (`tests.rs:2469-2474`) pins the
buggy behaviour and will go red the moment the fix lands — replace it with the
corrected contract rather than weakening it (per `rca.md`, "What the fix needs
to resolve"). It is intentionally not included in `failing_tests`: it passes
today and must not pass after the fix.
