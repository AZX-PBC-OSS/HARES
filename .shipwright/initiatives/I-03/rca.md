---
diagnosis_outcome: root_cause_found
complexity: moderate
passing_commands:
  - command: "cargo fmt --check --all"
  - command: "cargo clippy --workspace -- -D warnings"
  - command: "cargo build --workspace --release"
  - command: "cargo nextest run --release --no-fail-fast --profile ci -p hares-equipment"
  - command: "cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core"
---

## Symptom, restated

An HPXML-declared EV never charges (0.0 kWh over a 30‑day simulation) whenever its
`charging_strategy` equipment override is `Immediate` or is left unset. Overrides
`Nightly` (two different windows) and `LowSoc 0.15` all produce the same non‑zero
total (332.1 kWh), which the operator read as "override content is discarded, only
its presence/absence matters." `add_ev_with_driver` is a documented workaround that
avoids the symptom entirely.

## Investigation

### 1. Where `charging_strategy` is actually parsed

`Ev::new()` (`crates/hares-equipment/src/ev/mod.rs:305-308`) reads the raw config
string and silently falls back to `Immediate` on any parse failure
(`.and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Immediate)`), which is what
the ticket flags. But `new()` is never the equipment's final state: every
construction path in the codebase calls `Equipment::init()` immediately after
`create()`/`new()`:

- `hares-core/src/dwelling/mod.rs:2206-2220` — `create_equipment_from_spec(...)`
  then `eq.init(&merged_cfg, &initial_env)` (the HPXML/dwelling-build path).
- `hares-python/src/py_dwelling.rs` — `add_ev` (line ~1053) and
  `add_ev_with_driver` (line ~1140) both call `eq.init(&config, ...)` right after
  `create()`.

`init()` → `Ev::init_typed()` (`ev/mod.rs:460-465`) re-parses `charging_strategy`
from the *typed, override-merged* config and, unlike `new()`, returns a real
`Err(HaresError::Equipment("invalid charging_strategy: ..."))` on malformed JSON
via `require_typed::<EvConfig>()` and the `?` operator. EV is in the
"is_critical" end-use list at `dwelling/mod.rs:2222-2228`
(`EndUse::EV` alongside HVAC/water-heating/battery/PV), so a malformed override
already hard-fails dwelling construction rather than being silently accepted.
**Conclusion: a mistyped `charging_strategy` string is not actually
accept‑and‑ignore in the real construction path** — `new()`'s fallback is
overwritten by `init_typed()` before any equipment is used. It is still a latent
code smell (duplicated silent-fallback pattern, dead in practice, misleading to a
reader), but it is not the cause of the reported symptom. I confirmed this by
checking every call site of `EquipmentRegistry::create` in the workspace — none
uses the equipment without an immediately-following `init()` call.

### 2. Confirming HPXML never populates `initial_soc` / `soc_max` / `charging_strategy`

`crates/hares-io/src/hpxml/resolve_der.rs:206-232` builds `EvConfig` for an
HPXML-declared EV with `soc_max: None`, `initial_soc: None`, `ready_soc: None`,
and `charging_strategy: None`. Defaults then apply from
`crates/hares-equipment/src/ev/config.rs:55-56`:

```rust
pub(super) const DEFAULT_SOC: f64 = 1.0;
pub(super) const DEFAULT_SOC_MAX: f64 = 1.0;
```

and `ev/mod.rs:464` (`ChargingStrategy::Immediate { target_soc: 1.0 }` when no
strategy override is present). So an HPXML EV with no override — or with an
override that resolves to `Immediate` — starts the simulation **already at its
own `soc_max`** (both default to 1.0).

### 3. Why the EV never needs to charge: no actor ever depletes it

`Ev` equipment has no built-in driving/trip model. The only way its
`connection_state` changes, or its SOC is drawn down by driving, is through an
external `ControlSignal::EvDrive` / `EvSetConnectionState`
(`ev/mod.rs:1510-1620` — `apply_control` handling), which nothing sends unless an
actor (or a test/script) dispatches it. `Ev::actor_seed()`
(`ev/mod.rs:1369-1380`) is the mechanism that lets the dwelling auto-attach the
one built-in actor that does this — `EvDriverActor` — but:

```rust
fn actor_seed(&self) -> Option<crate::ActorSeed> {
    if matches!(self.charging_strategy, ChargingStrategy::Immediate { .. }) {
        return None;
    }
    Some(crate::ActorSeed::Ev { strategy: self.charging_strategy.clone(), ... })
}
```

it deliberately returns `None` whenever the resolved strategy is `Immediate` —
which is exactly the default when no override is given, and exactly what any
`Immediate` override resolves to. `build_actors_from_seeds()`
(`hares-core/src/dwelling/mod.rs:6740-6752`) filters equipment through
`eq.actor_seed()`, so **no `EvDriverActor` is ever created for this EV.**

`EvDriverActor` is not just "charging strategy logic" — reading
`hares-core/src/actors/ev_driver/mod.rs:97-239` shows it implements the
per-strategy charge-window/threshold logic (`Immediate`, `Nightly`, `LowSoc`,
`QuickThenWait`, `PreDeparture`, `TouAware`, `SolarSurplus`, `V2H`, `V2G`) *and*
it is the component that simulates vehicle departures/trips (draining SOC via
`fuel_economy_kwh_per_mi`) and returns/plugs the vehicle back in. There is no
other actor or default equipment behaviour that does this for a bare `Ev`.

This is confirmed directly by the documented workaround,
`add_ev_with_driver` (`hares-python/src/py_dwelling.rs:1079-1160`): it builds the
`Ev` equipment exactly as `add_ev` does, then **manually and unconditionally**
constructs an `EvDriverActor` and attaches it, bypassing `actor_seed()`/
`build_actors_from_seeds()` entirely. That is precisely what compensates for the
gap: it always gets a driver, regardless of what `charging_strategy` resolves to.

### 4. Closing the loop: why 0.0 kWh exactly, every time

With no actor: `connection_state` stays at its default,
`EvConnectionState::HomePluggedIn`, forever — the EV never leaves home, so it
never drives, so SOC never drops below its initial value. Since
`initial_soc == soc_max == 1.0` by default, `compute_charging_power_kw`
(`ev/mod.rs:518-556`) hits its very first guard on every single step:

```rust
if self.soc >= soc_limit {
    return Ok(0.0);
}
```

`soc_limit` is derived from `soc_target.or(ready_by_soc).unwrap_or(ready_soc)`,
and `ready_soc` itself defaults to `soc_max` (`ev/mod.rs:296`,`433`) — so
`soc_limit == 1.0 == self.soc` from step one. The EV is, correctly, never asked
to charge, because nothing ever discharges it. It still appears in the equipment
list and telemetry because `Ev::init()` succeeds and the equipment is added
normally — only its charging behaviour is inert.

### 5. Why `Nightly` / `LowSoc` overrides "work" and look identical

Any strategy other than `Immediate` makes `actor_seed()` return `Some`, so
`build_actors_from_seeds` *does* build an `EvDriverActor`, which then drives trips
(depleting SOC) and applies the configured strategy's window/threshold logic
correctly (verified by reading the strategy match arms in
`ev_driver/mod.rs:97-239`, which do consume `off_peak_start_hour`,
`off_peak_end_hour`, `target_soc`, `min_soc`, etc.). The fixed 30‑day driving
schedule used in the operator's scenario apparently gives ample overnight dwell
time to always fully replenish whatever was consumed driving regardless of which
window or SOC threshold is configured — so identical 332.1 kWh totals across
`Nightly`/`LowSoc` variants are consistent with an energy-balance result (driving
consumption == charging replenishment when there's always enough time), not with
the strategy's parameters being ignored. I did not find any code path that drops
`off_peak_start_hour`/`off_peak_end_hour`/`target_soc`/`min_soc` on the floor; the
"content is discarded" read in the ticket is well-supported for `Immediate` (its
`target_soc` truly is never consulated, because no actor exists to consult it) but
is not established for `Nightly`/`LowSoc` — that would need a scenario where the
strategies' specific timing actually diverges the schedule (e.g. a shorter dwell
window) to be distinguished from this energy-balance explanation. I'm not treating
that as a separate confirmed defect; it is a follow-up worth a quick check but is
out of the root cause this ticket is asking about.

## Reproduction

I did not stop at static reading — I confirmed the mechanism by temporarily
appending two throwaway `#[test]` functions to
`crates/hares-equipment/src/ev/tests.rs`, running them with `cargo test`, and
then reverting the file with `git checkout` (no test or source changes are left
behind by this diagnosis; writing the permanent regression test is the
`reproduce` step's job).

**Test 1 — reproduces the reported symptom at the equipment level.** Built an
`Ev` with `KEY_INITIAL_SOC` and `KEY_CHARGING_STRATEGY` both absent from the raw
config (mirroring `resolve_der.rs:206-232`'s `None`s exactly), asserted
`charging_strategy() == Immediate { target_soc: 1.0 }` and `actor_seed().is_none()`,
then called `ev.step()` directly 2880 times (30 days × 15‑minute steps, no actor,
no driving) and summed `active_power_kw`:

```
$ cargo test -p hares-equipment --lib \
    ev::tests::scratch_rca_hpxml_default_ev_never_charges_and_gets_no_actor \
    -- --nocapture --exact
running 1 test
RCA repro: total_kwh over 30 days with no actor = 0, final soc = 1
test ev::tests::scratch_rca_hpxml_default_ev_never_charges_and_gets_no_actor ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2169 filtered out
```

`total_kwh == 0` and `final soc == 1` for the entire 30‑day window — the exact
shape of the ticket's "0.0 kWh" observation, produced purely by the equipment
sitting at `soc_max` with no actor to deplete it, with no dwelling/actor
machinery involved at all. This confirms the mechanism, not just its plausibility.

**Test 2 — rules out the ticket's suspected alternative cause with evidence,
not just reading.** Set `KEY_CHARGING_STRATEGY` to the literal string
`"not valid json"`, confirmed `Ev::new()` swallows it into
`Immediate { target_soc: 1.0 }` per `ev/mod.rs:305-308`, then called
`ev.init(&config, &env)`:

```
$ cargo test -p hares-equipment --lib \
    ev::tests::scratch_rca_malformed_charging_strategy_override_errors_on_init \
    -- --nocapture --exact
running 1 test
RCA repro: init() result for malformed charging_strategy =
    Err(Equipment("invalid charging_strategy: expected value at line 1 column 1"))
test ev::tests::scratch_rca_malformed_charging_strategy_override_errors_on_init ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2170 filtered out
```

`init()` returns `Err`, not a silently-accepted `Immediate`. This directly
disproves "the equipment-override path accepts a bad `charging_strategy`
without error" as the mechanism — the real, load-bearing parse in
`init_typed()` fails loudly. The only way `Immediate` is reached silently is by
it being genuinely absent or genuinely requested — which is exactly what Test 1
reproduces.

Both scratch tests were removed afterward:
`git status --porcelain -- crates/hares-equipment/src/ev/tests.rs` returns
empty.

### Reproduction path (concrete)

1. Build an `Ev` equipment from a config where `charging_strategy`,
   `initial_soc`, and `soc_max` are all unset — exactly what
   `hares-io/src/hpxml/resolve_der.rs:206-232` produces for any HPXML-declared
   EV that doesn't set an explicit override for these fields (or where the
   override explicitly requests `Immediate`).
2. Construct the dwelling normally: `create_equipment_from_spec` →
   `eq.init()` → `Ev::actor_seed()` returns `None` (`ev/mod.rs:1370-1371`) →
   `build_actors_from_seeds()` (`dwelling/mod.rs:6740-6752`) attaches no
   `EvDriverActor` for this equipment.
3. Run the simulation for any number of steps. `connection_state` never
   changes from its default `HomePluggedIn` (nothing ever sends
   `ControlSignal::EvDrive`/`EvSetConnectionState`), so `soc` never drops below
   its initial value, which equals `soc_max` (both default to `1.0`).
4. Every step, `compute_charging_power_kw` (`ev/mod.rs:554-556`) hits
   `if self.soc >= soc_limit { return Ok(0.0); }` on its first line — the EV
   reports 0 kW for the entire run.

## Root cause

`Ev::actor_seed()` (`crates/hares-equipment/src/ev/mod.rs:1369-1380`) treats
`ChargingStrategy::Immediate` — the default strategy used whenever no
`charging_strategy` override is supplied, and whenever one is — as "this EV needs
no built-in actor at all." In reality, the actor it withholds
(`EvDriverActor`) is the *only* mechanism (short of a caller manually injecting
`ControlSignal::EvDrive`/`EvSetConnectionState`, or manually attaching a driver as
`add_ev_with_driver` does) by which an EV ever leaves `HomePluggedIn`, drives, and
depletes its own SOC. An EV built through the ordinary HPXML/`add_ev` path with
`Immediate` or no strategy override therefore starts at `soc_max` (both default to
1.0), is never driven anywhere, and so is — correctly, given no depletion —
never asked to charge. The equipment is fully functional and its config is
genuinely honoured; the defect is that the auto-actor-attachment path
under‑provisions the one component responsible for simulating vehicle use for
this specific (and default) strategy value.

This is a single root cause, not two independently-caused symptoms: "no
override" and "explicit `Immediate`" fail identically because both resolve to the
exact same `ChargingStrategy::Immediate` value that `actor_seed()` special-cases.

## What the fix needs to resolve (for the fix step, not implemented here)

- `actor_seed()`'s `Immediate` short-circuit conflates "no special *charging*
  decision logic needed" with "no driver/trip simulation needed," which is wrong.
  The fix should ensure an HPXML/`add_ev`-constructed EV gets vehicle-use
  simulation (an `EvDriverActor`, or equivalent) regardless of which
  `ChargingStrategy` it resolves to, `Immediate` included.
- Per the constitution's "measure the class before you fix the instance": check
  whether any other equipment/actor pairing in this codebase uses the same
  "skip the actor for the default/trivial variant" pattern
  (`crates/hares-equipment/src/battery/mod.rs:1631` has its own `actor_seed()`
  with a `_manual_returns_none` test — worth a quick look for the same shape,
  though Battery's manual mode is a materially different case: a
  user‑driven battery doesn't need a driving model to make sense of "no
  actor").
- The existing test `actor_seed_immediate_returns_none`
  (`crates/hares-equipment/src/ev/tests.rs:2469-2474`) currently pins the buggy
  behaviour and will need to be replaced with a test asserting the corrected
  contract, alongside `ev_charging_strategy_backward_compat`
  (`ev/tests.rs:2457-2466`), which only asserts the *parsed* default and is not
  affected by the fix's direction.
- Any legitimate use case for "no auto-actor" (e.g. tests or callers that drive
  the EV entirely through manual `ControlSignal`s without wanting a competing
  built-in actor) should be preserved deliberately, not accidentally — this
  needs a real decision in the fix step, not an assumption here.
- The ticket's second question — "should a strategy-less EV really never
  charge?" — is answered by this RCA: no, that is the bug, not intended
  behaviour; a strategy-less/`Immediate` EV should still be driven and should
  still charge like any other EV once its SOC actually drops.
