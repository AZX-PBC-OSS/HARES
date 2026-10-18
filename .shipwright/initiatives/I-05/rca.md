---
diagnosis_outcome: root_cause_found
complexity: moderate
passing_commands:
  - command: cargo test -p hares-core --lib
  - command: cargo test -p hares-equipment --lib
  - command: cargo clippy --workspace -- -D warnings
  - command: cargo fmt --check --all
---

# RCA (revision 2 — supersedes the prior root cause): actor silence, not the
# range-anxiety override, produces the ticket's symptom

This revision was triggered by `fix-plan-critique.md`'s `halt` verdict (H1),
which showed that the previously-certified root cause (the range-anxiety
override preempting the composer) cannot produce the ticket's observed
profile, and traced the actual mechanism to the actor/equipment silence
contract. This RCA independently re-verifies every code claim the critique
made — reading the same files at the same lines — before adopting it, and
extends the analysis to the fix-scoping question the prior RCA left open.

## Symptom (restated, unchanged from revision 1)

An HPXML-declared EV with an `Equipment["EV"].charging_strategy` override
produces the same charging profile regardless of which strategy is
configured (`Nightly 22:00–06:00`, `Nightly 02:00–06:00`, `LowSoc 0.15`,
`Immediate`, or none): ~329.8 kWh over 30/35 days, charging concentrated
18:00–23:00 + 00:00–02:00 — i.e. "charge on plug-in" — 28 of 30 nights,
identical peak hour and night count. The equivalent
`Dwelling.add_ev_with_driver(...)` archetype path differentiates correctly.

## Why revision 1's root cause is wrong

Revision 1 concluded `EvDriverActor::evaluate_charging`'s range-anxiety
override (`maybe_push_range_anxiety_override`, `mod.rs:786-804`) preempts the
`ChargingStrategy` composer whenever perceived SOC drops below the
anxiety threshold (~0.25–0.28 for this fixture's parameters), and that this
is what makes every strategy converge. That mechanism is real (both
reproduction tests in `reproduction.md` genuinely fail against it) — but it
cannot be what produces *this ticket's* numbers:

- The fixture drains ~30 mi/day. From a full charge, that is roughly
  0.15–0.18 SOC/day. If the override only fired below ~0.28 SOC, it would
  idle on alternating nights once SOC recovers, and `Nightly`'s window would
  then govern on those nights. The ticket shows **identical** profiles on
  28 of 30 nights — a pattern the override, which fires conditionally on a
  threshold crossing, cannot produce. Only "charges to full every single
  night regardless of SOC" produces that shape.
- Revision 1's own reproduction tests only prove the override preempts the
  composer *when SOC is inside the 0.15–0.28 band*. They say nothing about
  what happens on the far more common nights where SOC is well above that
  band (e.g. the fixture's typical ~0.83 SOC at plug-in after a 30 mi
  drive) — which is every night in the ticket's fixture, since the vehicle
  is (per the actual mechanism below) always being charged to 100% and
  never allowed to drain into the anxiety band at all.

## Root cause (confirmed)

**The actor/equipment "silence" contract is undefined, and the equipment's
default behaviour silently overrides every "don't charge" decision a
`ChargingStrategy` makes.** Three facts, each re-verified directly against
current code (not merely re-cited from the critique):

**(a) When a strategy decides not to charge, the actor dispatches nothing —
there is no "hold" or "inhibit" signal.**

- `TimeWindowPref::constraint`, outside its window, returns
  `Constraint::Override(PreferenceVote::idle("time_window:outside"))`
  (`crates/hares-core/src/actors/ev_driver/time_window.rs:33-39`, read in
  full — confirmed).
- `SocGate::constraint`, above its upper threshold or in the hysteresis
  hold, likewise returns `Constraint::Override(PreferenceVote::idle(...))`
  (`crates/hares-core/src/actors/ev_driver/soc_gate.rs:38-51`, read in full
  — confirmed).
- `ChargingComposer::emit_vote` (`composer.rs:211-343`, read in full):
  an idle vote carries `target_soc: None, power_kw: None`. The function has
  no branch that fires when both are `None` — the `departure_hour`+
  `target_soc` branch and the `power_kw` branch are both skipped, and the
  function returns having pushed nothing to `out`. Even an explicit
  `power_kw: Some(0.0)` short-circuits at line 234-237 with a comment
  stating the design intent outright: `"Zero power = idle, no dispatch
  needed"`. **This composer** never emits a dispatch that asserts "do not
  charge" — confirmed by inspection of every `ControlSignal` it emits
  (`EvSetReadyBy`, `PowerSetpoint`, `SOCTarget` — all positive
  instructions). This is a statement about `ChargingComposer`'s emission
  logic specifically, not a claim that the actor/equipment protocol lacks
  the vocabulary to express a hold: the equipment already accepts and
  honours `PowerLimit`/`PowerSetpoint` with a zero or negative-sense value
  (`ev/mod.rs:1640-1657`), and `EvSetReadyBy` already expresses "delay
  charging until it's needed" through the BMS's own ready-by logic —
  demonstrated by the existing, currently-passing test
  `ev_set_ready_by_delays_then_charges` (`ev/tests.rs:1738-1770`: a
  `ready_by(07:00, 0.8)` signal sent once at plug-in produces zero power at
  18:00 and positive power at 05:00 the next day, entirely from the
  equipment's own BMS timing, no further actor input required). So the
  primitives to express "wait" exist; the gap is that `ChargingComposer`'s
  idle path uses none of them, and (per fact (c) above) each of them
  latches once set, so `plan-fix` has real options here — not "invent a new
  signal vocabulary" — but must weigh them knowing they latch.

**(b) A plugged-in EV under actor control, receiving no instruction on a
given step, charges itself toward 100% at rated power — the BMS default,
not "no charging".**

- `compute_charging_power_kw` (`crates/hares-equipment/src/ev/mod.rs:574-604`,
  read in full): `target = self.soc_target.or(self.ready_by_soc)
  .unwrap_or(self.ready_soc)`. `ready_soc` defaults to `soc_max`
  (`ev/mod.rs:359`: `ready_soc: config.get_f64(KEY_READY_SOC)
  .unwrap_or(soc_max)`), and `soc_max` defaults to `DEFAULT_SOC_MAX = 1.0`
  (`ev/config.rs:56`, `ev/mod.rs:245`). `EvConfig` does have a settable
  `ready_soc` field (`config.rs:42` `KEY_READY_SOC`, `:288`) — it is not
  missing from the schema — but it is a raw, independent config key with no
  relationship to `charging_strategy` (confirmed: `KEY_CHARGING_STRATEGY`
  and `KEY_READY_SOC` are parsed independently in `config.rs`, and
  `Ev::actor_seed()`, `ev/mod.rs:1437-1447`, which is what feeds
  `ActorSeed::Ev` for the auto-registration path, does not include
  `ready_soc` at all — only `strategy`, `plug_in_policy`, `capacity_kwh`,
  `max_charge_kw`, `fuel_economy_kwh_per_mi`). The ticket's `Equipment["EV"]`
  override sets `charging_strategy` only, never `ready_soc`, so it stays
  unset and falls through to the `soc_max` default of 1.0 regardless of
  which `ChargingStrategy` variant is configured — no strategy variant
  populates or influences `ready_soc`.
- `step`'s `HomePluggedIn` arm calls `run_charging_physics(env, dt,
  self.rated_power_kw)` unconditionally — every step, with no actor-signal
  gate (`ev/mod.rs:1300-1305`, confirmed by direct read).
- The existing equipment test `bms_default_charges_to_soc_max`
  (`crates/hares-equipment/src/ev/tests.rs:1617`) states this as the
  intended contract in its own words: *"No actor controls, just step many
  times … BMS should charge to ready_soc (default=soc_max)"*. This is not
  incidental behaviour; it is a deliberately tested default for an
  un-actored EV, applied here to an actor-controlled one that is
  momentarily silent.
- The actor module's own telemetry-fold comment
  (`crates/hares-core/src/actors/ev_driver/mod.rs:464-467`) documents the
  same assumption from the actor's side: *"Goal-based strategies dispatch
  no rate; the equipment BMS applies its own power, observed via
  `observed_charge_kw`."* The actor's authors believed silence was safe;
  the equipment's default makes silence mean "charge to full."

**(c) Every actor-writable setpoint on the EV — not only `SOCTarget` — is
write-once-until-reinitialized: once set, it survives plug-out/plug-in and
is never countermanded by silence.** (Corrected and broadened after a
second read prompted by external review of this revision; the original text
below understated both the scope and the clearing mechanism.)

- `SOCTarget`: `self.soc_target = Some((*target_soc).clamp(0.0, 1.0))`
  (`ev/mod.rs:1689`, confirmed).
- `PowerSetpoint`: `self.power_setpoint_kw = Some(*active_power_kw)`
  (`ev/mod.rs:1652`, confirmed).
- `PowerLimit`: `self.power_limit_kw = Some((*max_power_kw).max(0.0))`
  (`ev/mod.rs:1657`, confirmed).
- **Correction:** the prior text said these are cleared by "`reset()`". Re-
  read: the block that clears `soc_target`/`power_setpoint_kw` to `None`
  (`ev/mod.rs:540-545`, `"// Reset transient state"`) is inside
  `init_typed` (`fn init_typed`, `ev/mod.rs:411`), not a runtime `reset()`
  method invoked during simulation — `init_typed` runs at equipment
  construction/config (re)load, not per-step, per-day, or per-plug-in.
  `power_limit_kw` is not in that cleared block at all — it is only ever
  *set*, from config at init (`ev/mod.rs:491`,
  `self.power_limit_kw = c.power_limit_kw`) and from a `PowerLimit` control
  signal (`:1657`); nothing in the runtime step/control path clears it back
  to `None` once a `PowerLimit` signal has set it, for the life of the
  simulation.
- The `EvPlugIn` control arm's `Disconnected` case clears `ready_by_hour`
  and `ready_by_soc` (`ev/mod.rs:1726-1731`, confirmed by direct read) but
  touches none of `soc_target`, `power_setpoint_kw`, or `power_limit_kw`.
  So whichever of these the actor sends first (`SOCTarget{1.0}` from
  `Immediate` or the range-anxiety override; a `PowerSetpoint`/`PowerLimit`
  from some other preference) governs `compute_charging_power_kw`
  (`ev/mod.rs:592-604`, `target = self.soc_target.or(self.ready_by_soc)
  .unwrap_or(self.ready_soc)`, and the `power_limit_kw`-derived cap applied
  alongside it) on every subsequent plug-in for the rest of the simulation,
  even on nights the composer is later silent.

**Why this specifically constrains the fix, not just describes more of the
bug:** the most obvious repair for fact (a) — have the composer's idle path
dispatch an explicit hold, e.g. `PowerSetpoint{active_power_kw: 0.0}` or
`PowerLimit{max_power_kw: 0.0}` — would itself latch under this same
mechanism. `compute_charging_power_kw` takes
`requested = power_setpoint_kw.unwrap_or(derated_rated)` (`ev/mod.rs:632-636`);
a zero setpoint sent once during an idle window is never cleared by
anything before the *next* window opens, so when `Nightly`'s window later
opens and the composer sends `SOCTarget{0.95}`, `PowerSetpoint`'s latched
`0.0` (not `soc_target`) is what `compute_charging_power_kw` still consults
first if the setpoint branch is checked ahead of the target branch — the
naive hold-signal fix would trade "always charges" for "never charges", not
fix the strategy differentiation. Any hold mechanism `plan-fix` chooses must
either be explicitly countermanded every single step (not just on
transitions) or be paired with a clearing rule for the specific field it
uses — this is not optional plumbing, it is the difference between the fix
working and silently producing the opposite failure mode.

## How (a)+(b)+(c) together produce the exact ticket profile

For the fixture (HPXML EV, `Equipment["EV"]` override, 30 mi/day drive):
the vehicle plugs in around 18:00. Whatever strategy is configured:

- `Nightly 22:00–06:00` at 18:00: outside its window → idle → no dispatch
  (fact a). The equipment, receiving nothing, charges at rated power from
  18:00 toward `ready_soc` = 1.0 (fact b). Once SOC is at/near 1.0 no
  charging is needed regardless of what the window later permits.
- `Nightly 02:00–06:00`: identical — idle at 18:00, same BMS default fires.
- `LowSoc 0.15`: SOC at plug-in (~0.83 after a 30 mi day) is above 0.15 →
  gate says idle → no dispatch (fact a) → same BMS default (fact b).
- `Immediate`: dispatches `SOCTarget{target_soc: ...}` explicitly — reaches
  the same charge-to-full destination by an actual signal rather than by
  default, indistinguishable in the resulting profile.
- No override: no strategy at all is configured — same BMS default.

All five converge on the same "charge to full starting ~18:00" behaviour —
exactly the ticket's reported profile, including the specific 18:00 start
time (typical plug-in after the day's drive) and the ~9.4 kWh/night volume
(consistent with a full recharge from a 30 mi/0.3 kWh-mi day, not with the
anxiety override's charge-to-full-from-a-much-lower-SOC signature). Fact
(c) reinforces this once any `SOCTarget` (e.g. from `Immediate`, or from
the range-anxiety override firing even once early in the run) has been
sent: subsequent nights charge to that latched target even before the
composer is consulted.

## Reproduction of the mechanism (temporary probe, run and reverted)

Facts (a) and (b) above were each independently true in existing,
already-passing tests — but no existing test exercised them *together*:
an actor whose configured strategy is silent, wired into a real equipment
instance that is then left to run on its own, observed over time. Citing
the two facts separately is not the same as demonstrating the composition,
so this revision added one temporary test (not a permanent artifact — see
below), following the same reproduction convention revision 1 used:

- Built an `EvDriverActor` with `ChargingStrategy::Nightly { off_peak_start_hour:
  22.0, off_peak_end_hour: 6.0, target_soc: 0.95 }`, SOC 0.5 (well above the
  ~0.25-0.28 anxiety band, so this is not a range-anxiety scenario), evaluated
  at minute `18*60` (18:00, outside the window). Asserted `out.is_empty()` —
  confirmed the actor dispatches nothing, matching fact (a).
- Built a real `hares_equipment::ev::Ev` (imported directly into
  `hares-core`, which depends on `hares-equipment`) with matching parameters
  (60 kWh capacity, 7.2 kW L2, `initial_soc: 0.5`, `ready_soc: None` — as the
  ticket's `Equipment["EV"]` override, which sets only `charging_strategy`,
  leaves it), and stepped it 200 times at 5-minute resolution (~16.7
  simulated hours) with **no control signal ever applied**, mirroring what
  the actor's silence at 18:00 means for the equipment for as long as the
  strategy keeps idling.
- Ran: `cargo test -p hares-core --lib
  actors::ev_driver::tests::rca_probe_actor_silence_bms_default_ignores_nightly_window
  -- --nocapture`. Output: `RCA PROBE: soc after 200 x 5min steps with no
  control signal = 1` — the equipment charged fully to `soc = 1.0` despite
  the configured `Nightly` window never being open during the scenario and
  the actor never sending a single instruction. Test passed (the assertion
  `soc > 0.9` held).

This is the reproduction path `plan-fix`/`reproduce` should build a
permanent regression test around: an actor+equipment pair (or, closer to
the ticket's literal report, the full `build_actors_from_seeds` →
`Dwelling` step path) where a strategy idles and the resulting equipment
energy is asserted, not just the actor's dispatch output. The probe test
itself was reverted after this run — `git status --porcelain
crates/hares-core/src/actors/ev_driver/mod.rs` shows no diff beyond the
pre-existing `reproduction.md` tests from the prior flow step — diagnosis
does not own adding permanent tests; that is `reproduce`'s job, now with a
concrete, verified target (assert on equipment-observed charging energy
across an idle window, using this probe's construction as a starting
point) instead of having to discover from scratch that dispatch-only
assertions cannot express the ticket's actual symptom. Hooks available for
that step: `Dwelling::step()` + equipment telemetry (`observed_charge_kw`,
referenced in the actor's own telemetry-fold comment,
`ev_driver/mod.rs:464-467`) for measuring energy by hour without reaching
into equipment-private fields; `core_output().state.soc` (public, used
above) for direct SOC checks in a lower-level test.

## Why `add_ev_with_driver` (archetype path) looked correct but wasn't proof of strategy handling

The prior RCA cited this path's differentiated output (distinct energy,
charger rating, night counts, start-time concentration per archetype) as
evidence "the driver machinery works." Re-examined here: the archetype path
differs across `PlugInPolicy` (e.g. `PlugInPolicy::LowSoc{0.3}` in
`crates/hares-equipment/src/ev/catalog.rs`, confirmed present at line ~729)
and per-archetype charger ratings/arrival distributions — none of which
depend on a `ChargingStrategy` window being honoured. An archetype whose
policy makes it plug in less often, or that has a smaller/larger charger,
produces different totals and start-time histograms through fact (b)'s same
"charge to full when silent" default, with no `Nightly`/`LowSoc` window
logic exercised at all. This is not evidence the strategy composer's output
is respected by the equipment; it is a different, unrelated source of
variation that happens to produce different numbers per archetype.

## Why the range-anxiety override was initially mistaken for the cause

The reproduction tests in `reproduction.md` are real and directly observe
the override dispatching `SOCTarget{1.0}` at SOC 0.20 when `LowSoc{0.15}`
or `Nightly{22:00-06:00}` would otherwise idle — that defect exists and is
confirmed independently of this revision. But every existing "idles" test
in the module (including the two reproduction tests) asserts only "`out`
contains no `SOCTarget`" — none of them go one layer further to assert what
the *equipment* then does with that silence. That gap in test design is
exactly why fact (b) was never caught: "the actor asserted nothing" was
treated as equivalent to "the EV did not charge," which facts (a)-(c) show
is false. A grep across the workspace for `Nightly`/`off_peak_start_hour`
outside `ev_driver/*.rs` finds only the Python enum bindings — there is no
end-to-end test anywhere that measures actual equipment energy against a
configured window.

## Alternatives considered and ruled out

1. **Range-anxiety override is the sole cause (revision 1's conclusion).**
   Ruled out above — cannot produce the observed 28/30-night identical
   profile, and the reproduction tests only demonstrate the defect in a
   narrow SOC band the ticket's fixture numbers show it isn't consistently
   in.
2. **Config-wiring defect (override never reaches the actor).** Already
   ruled out in revision 1 by tracing HPXML → `merged_equipment_config` →
   `Ev::init_typed` → `Ev::actor_seed()` and confirming each strategy
   variant parses distinctly into `ActorSeed::Ev`. Not re-litigated here;
   nothing in this revision's findings touches that path, and it remains
   correct — the override reaches the actor's `ChargingStrategy` field
   intact. The bug is downstream of that, in what the actor's *silence*
   means to the equipment.
3. **Composer preference logic itself broken (`SocGate`/`TimeWindowPref`
   compute the wrong idle/active decision).** Ruled out in revision 1 by
   three passing unit tests exercising the preference stack directly, and
   reconfirmed here by direct code read: both `constraint()` implementations
   correctly return `Override(idle(...))` when outside their configured
   condition. The preference stack computes the *decision* to idle
   correctly; the bug is that "idle" has no on-the-wire representation
   between the actor and the equipment.

## Class of the bug

This is not scoped to `Nightly`/`LowSoc`/the EV window feature — it is a
missing primitive in the actor/equipment protocol. Any `ChargingStrategy`
variant whose composer preference resolves to "don't act this step"
(`Nightly`, `LowSoc`, `QuickThenWait` while waiting, `TouAware` outside
off-peak, `SolarSurplus` outside a surplus window) degrades to
charge-on-plug-in whenever it idles, because idling emits nothing and the
equipment fills the gap with its own default. Only strategies that assert a
positive signal on effectively every step (`Immediate`, an urgent
`DepartureDeadline`) behave as configured today — not because their logic
is more correct, but because they happen to never rely on silence meaning
"wait."

Two further, related but separately-scoped findings, both real and both
currently masked by the above (so they cannot be observed to matter until
the primary defect is fixed):

- **Range-anxiety override preempts a strategy's own SOC/window gate in the
  0.15–0.28 band** (revision 1's root cause, confirmed still real by its
  passing-when-run-standalone reproduction tests). This becomes an
  *externally observable* defect only once fact (a)/(b)/(c) above no longer
  mask it — i.e., once the composer's idle decision has a real effect, the
  override's unconditional preemption of that decision becomes visible in
  integration runs. `fix-plan.md`'s urgency-gating design is a reasonable
  fix for this, but it is downstream of, not a substitute for, the primary
  fix.
- **Non-driving days never run the composer at all** (`decide`'s `None` arm,
  `mod.rs:942-968` — confirmed by direct read: the arm either fires the
  anxiety override or falls through to `populate_telemetry` and returns;
  there is no call to `self.composer.evaluate(...)` anywhere in that arm).
  On ~20% of days (no drive scheduled), no strategy — including `Immediate`
  — ever asserts anything, and behaviour is entirely the equipment's BMS
  default plus whatever `SOCTarget` was last latched. This is architecture
  consistent with facts (a)-(c): it is not a new mechanism, just another
  path that currently relies on the assumption that actor silence means
  "the equipment does the right thing," which is false today regardless of
  driving day/non-driving day.
- **Hardcoded driving parameters for HPXML-declared EVs**
  (`build_actors_from_seeds`, `ActorSeed::Ev`,
  `crates/hares-core/src/dwelling/mod.rs:6967-7034`: 30 mi/day, 20-mile
  buffer, 0.8 event-day ratio, all hardcoded with no `EvConfig` field to
  override them) — unchanged from revision 1's finding. This sets the
  anxiety band (~0.25-0.28 for this fixture) above any `LowSoc` threshold
  configured below it, so such thresholds cannot govern even once the
  override-preemption defect above is fixed. Real, but genuinely
  independent, scoped, config-plumbing work — not blocking the primary fix.

### Checked: is this pattern present in other equipment? (bounded check, not a full audit)

Prompted to check whether "actor silence silently substituted with an
aggressive default, and any actor setpoint latches indefinitely" recurs
elsewhere — a full cross-equipment audit is disproportionate scope for an
EV ticket, but a bounded check is cheap and load-bearing for whether "Class
of the bug" undersells the blast radius:

- **Battery** (`crates/hares-equipment/src/battery/mod.rs`,
  `determine_target_power`, lines 611-651, read in full): absent both
  `power_setpoint_kw` and `soc_target`, it falls through to Priority 3,
  "self-consumption" — discharge to offset net load if load exceeds PV,
  charge from PV surplus if there is one, otherwise idle
  (`self_consumption_enabled` branch, `:640-651`). This is a
  context-sensitive default, not "charge to a fixed target regardless of
  context" — confirming, with code evidence rather than by extension, the
  RCA's revision-1 finding that `ActorSeed::Battery`'s construction is the
  non-hardcoded, non-outlier case. This RCA did not check whether
  `Battery`'s own `power_setpoint_kw`/`soc_target` latch identically to the
  EV's (no plug-in/plug-out cycle exists for a battery, so the failure
  mode, if any, would look different) — left unaudited as out of scope for
  an EV ticket, not asserted clean.
- **Water heater / HVAC** (`crates/hares-equipment/src/water_heater/`,
  `crates/hares-equipment/src/hvac/*.rs`, greped for the same
  `unwrap_or(self.*)` defaulting shape): no equivalent "charge to 100%
  when silent" pattern found — these are thermostatically self-regulating
  around a setpoint band (`thermostat.rs:166-167`,
  `heating_c.unwrap_or(self.heating_c)` — defaulting to the *previous*
  setpoint, not to a fixed extreme), architecturally different from an EV's
  binary "reach `ready_soc`" target. Not the same bug class on the surface
  this RCA checked; not a claim that their actor-silence semantics are
  otherwise sound.

**HPXML/schedule wiring specifically:** not re-audited beyond what revision
1 already traced (HPXML → `merged_equipment_config` → `Ev::init_typed` →
`Ev::actor_seed()`, confirmed correct in "Alternatives considered and ruled
out" item 2 above). Nothing in this revision's findings implicates that
path — the defect is downstream of a correctly-parsed config, in what the
equipment does when the correctly-differentiated actor is silent.

## What the fix must target (scoping note for plan-fix, not a design choice)

The confirmed root cause is that **the actor/equipment contract has no way
to express "hold, do not charge"**, and the equipment's default behaviour
silently substitutes "charge to full" for that silence. Closing this
requires resolving, as a design decision (left to `plan-fix`, per this
flow's role separation — this RCA states the defect, not the fix):

1. Whether the composer's idle path should assert an explicit hold/inhibit
   signal, or whether actor-controlled equipment should stop defaulting to
   charge-to-full when under actor control (or both).
2. That the latch (fact c — `soc_target`, `power_setpoint_kw`, and
   `power_limit_kw`, all of which persist until `init_typed` re-runs, none
   of which are cleared by disconnect) must be addressed regardless of
   which of the above is chosen — otherwise a single early `SOCTarget{1.0}`
   (from `Immediate`, or from the range-anxiety override before it is
   fixed), or a latched-zero `PowerSetpoint`/`PowerLimit` if the chosen hold
   mechanism uses one, continues to govern every later plug-in even after
   (1) is resolved — either silently reproducing the same convergence by a
   different route, or (per the "Why this specifically constrains the fix"
   analysis above) silently producing the opposite failure mode (the EV
   never charges once a hold latches).

Verification of any fix must be end-to-end on the ticket's actual reported
path: an HPXML EV with a `charging_strategy` override, built through
`build_actors_from_seeds`, stepped over multiple simulated days, asserting
*equipment* charging energy per hour (not "no `SOCTarget` in `out`") —
confirming two `Nightly` configurations differing only in
`off_peak_start_hour` produce measurably different hourly profiles, and
`LowSoc` differs from `Immediate`. No such test exists anywhere in the
workspace today; unit-level assertions on the actor's dispatch output alone
cannot certify this ticket's completion bar, because (as this revision
shows) an actor that behaves perfectly can still produce the reported
symptom if the equipment's silence handling is wrong.

## Complete-when check

The ticket's completion bar — "Two `Nightly` overrides differing only in
`off_peak_start_hour` produce measurably different hourly charging
profiles, and `LowSoc` differs from `Immediate`" — requires closing the
actor/equipment silence gap described above. The range-anxiety
urgency-gating fix already planned in `fix-plan.md` is necessary but not
sufficient on its own; it must land after (or together with) a fix to what
actor silence means to the equipment, or the ticket's profiles will not
change even with the urgency gate merged and its unit tests green, per
`fix-plan-critique.md`'s H1.

## Complexity

`moderate`: the primary fix is scoped to the EV actor/equipment boundary
(the composer's idle-signal vocabulary and/or the equipment's
actor-controlled charging default, plus a clearing rule for whichever
actor-writable setpoint(s) — `soc_target`, `power_setpoint_kw`,
`power_limit_kw` — the chosen mechanism relies on) — not a one-line change,
but bounded to
`crates/hares-core/src/actors/ev_driver/` and
`crates/hares-equipment/src/ev/mod.rs`, without touching HPXML resolution,
`EvConfig` schema, or the archetype/`add_ev_with_driver` path. The
previously-planned range-anxiety urgency gate remains a secondary,
independently scoped follow-on within the same complexity band.
