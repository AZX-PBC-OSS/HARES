---
plan_outcome: ready
---

# Fix Plan: assign unique `EquipmentId`s to dwelling equipment (I-06, gh #26/#27)

*Revision 6. The work items below are addressed to the implementer and stand alone:
each states its decision and its grounds without requiring any other document. The
resolution matrix and corrections ledger at the end carry the traceability to the
five critique rounds this plan survived; they are records, not instructions.*

## Target

The RCA's confirmed root cause is an **identity collapse**: no code path in a real
dwelling build assigns unique `EquipmentId`s, so every equipment instance is
`EquipmentId(0)` and every `EquipmentId`-keyed structure in the dwelling collapses
to one entry. The EV driver's observation channel then reads the wrong equipment's
core output, its SOC belief decays, and mobility is cancelled against a phantom
empty pack. The reproduction's structural test pins the rule (one core-output
entry per equipment); its 45-day test pins the symptom. This plan targets the
identity collapse itself, at **both** entrances through which equipment enter a
dwelling, per the constitution's "fix the class, not the instance".

## Chosen approach

**Assign a unique, deterministic `EquipmentId` at both entrances**: at assembly, a
pre-pass over the specs injects the id through the `equipment_id` config channel
constructors already read; at `add_equipment`, the dwelling assigns a never-used id
to equipment whose constructor left it unassigned. Align the six constructors whose
id reads are one-sided; reserve `equipment_id` against user overrides; reject id
collisions loudly at both entrances. One identity mechanism per entrance fixes
every consumer the RCA audited — `equipment_core`, `equipment_id_by_name`,
`prev_equipment_modes`, the active-id retain, `EvDriverActor` and
`BatteryManagementActor` observation — with **zero consumer-side edits**.

Scope structure: work items 1–3 are one self-contained change — equipment identity
at the dwelling boundary — gated by the two existing reproduction tests. Work item
4 is the secondary decision the RCA delegated to this phase, landed as a separate
commit with its own gates. Work item 5 is test hygiene plus two tracking
obligations. Nothing else rides along; the scope question itself was weighed and
answered in Alternative 6.

This phase changes no code. Facts marked "verified" were established by reading
the codebase during planning; the corrections ledger records every claim that
failed checking and was fixed.

### Work item 1 — Assembly-level id assignment

**The pass and where it lives.** A new function in
`crates/hares-io/src/hpxml/equipment.rs`, beside `assign_instance_names`, assigns
each spec the next id from a counter starting at 1 and writes it into the spec's
existing config channel. It is called from the dwelling assembly
(`dwelling/mod.rs:2193`, immediately after `assign_instance_names`) and **only**
there — not folded into `assign_instance_names` itself, which also runs at parse
time (`hpxml/equipment.rs:65`) where the set of equipment that will be
instantiated is not yet known. The cohesion bought is that both identity passes
live side by side in one crate; their call sites are deliberately asymmetric
(names canonicalize at parse and assembly, ids at assembly only). The invariant
split mirrors the existing name-identity split: canonicalization pass in
`hares-io`, uniqueness validation in `hares-core` where the equipment set lives.

**The write path.** Typed specs: insert the top-level `equipment_id` key into the
typed payload's JSON object, through the public `ConfigPayload` enum fields
(`Typed { data, .. }`'s `serde_json::Value`; `Raw { data }`'s map) or via a
purpose-built non-test setter on `EquipmentConfig` if the implementer prefers
encapsulation. The test-gated accessors are not merely discouraged — from
`hares-io` they are a **compile error**: `test_extras_mut()` is `#[cfg(test)]`
and no non-test mutable accessor exists, so the pre-pass cannot route through
them in any build that compiles. The narrow residual hazard is a write attempted
from inside `hares-equipment`'s own unit tests, and the integration gate covers
it: `crates/hares-core/tests/` link `hares-io` and `hares-equipment` as ordinary
dependencies **without** `cfg(test)`, so a write that only lands under `cfg(test)`
leaves ids at 0 and fails `equipment_core_has_one_entry_per_equipment`. The gate
is the discriminator; the guarantee is not exempted from testing.

**Inventory, verified**: all 22 production typed config structs accept a
top-level `equipment_id` key — 20 carry `pub equipment_id: Option<u32>` directly
(`EvConfig`, `BatteryConfig`, `VentilationConfig`, `ProtocolBridgeConfig`,
`PvConfig`, `GeneratorConfig`, the five `wh_config` structs,
`GasFurnaceConfig`, `GasBoilerConfig`, `ElectricBoilerConfig`,
`ElectricBaseboardConfig`, `IdealHvacConfig`, `ElectricFurnaceConfig`,
`CentralAirConditionerConfig`, `RoomAcConfig`, `DehumidifierConfig`), and the two
heat-pump configs accept it via `#[serde(flatten)] pub common:
HeatPumpCommonConfig` — `flatten` puts `common.equipment_id` at the top level,
exactly where the constructors' `typed_f64(config, "equipment_id")` reads look.

**`equipment_id` is reserved against user overrides.**
`merged_equipment_config` (`conversions.rs:777-796`) merges user overrides from
three sources over the typed base — `root["all"]`/`root["*"]` and `root[name]`
(`apply_equipment_overrides`, `conversions.rs:843-853`) — and the base at that
point already contains the injected id, so a reserved-key check placed on the
*merged result* (where the existing `"zip"` peel sits) would fire on every
equipment and fail every build. The check therefore sits on the **override
delta**: `equipment_id` appearing in any of the three override sources is a hard,
typed error at dwelling build, naming the source (`all`/`*`/equipment name), the
field, and that equipment ids are assigned by the dwelling, not configurable. A
wildcard override is the worst case (one id assigned to every equipment) and is
covered by an explicit test. The `"zip"` peel is cited only as evidence that the
merge point already recognizes reserved keys; the behaviour here is the opposite
of peeling — reserved means rejected.

Both config derivations the loop feeds — `create_equipment_from_spec`
(construction) and `merged_equipment_config` (init) — then carry the same id with
no signature changes. `HANDLED_OUTSIDE_REGISTRY` specs are skipped as today.

**Ids are sparse, by design**: the counter runs over specs and the loop drops
non-critical equipment whose `init` fails, so surviving ids have gaps. The
invariants are uniqueness, non-zero, and stability — no test may assert
contiguity.

Determinism: ids derive from spec order, which is deterministic by construction
(the loads resolver iterates `BTreeMap`s specifically to fix spec order), so same
input → same ids, and checkpoint round-trips stay aligned — `load_checkpoint`
restores state into a dwelling rebuilt from the same config, hence the same ids.

Id 0 is the **unassigned sentinel**; assigned ids start at 1; an explicit
`equipment_id: 0` anywhere in a config means "unassigned" and never survives into
a dwelling's equipment vector (assembly rejects it as unassigned; entrance 2
auto-assigns over it).

### Work item 2 — One id reader, every constructor, payload-agnostic

The planning audit of every production constructor found the id-read channels
split three ways:

| Read channel | Constructors | Sees the injection? |
|---|---|---|
| Raw-or-typed (`parse_u32`, `equipment_id_from_config`) | `scheduled_load.rs:178`, `event_load.rs:314,926`, all HVAC (furnace `114/488`, boiler `172/518`, AC `504`, baseboard `75`, ideal `162`, dehumidifier `176`, HP heater `508`, HP cooler `65/356/765`), all water heaters (resistance `174`, indirect `138`, tankless `121`, gas `178`, HPWH `247`) | ✅ both payload kinds |
| **Raw-only** (`config.get_f64(KEY_EQUIPMENT_ID)` — blind to typed payloads) | **`ev/mod.rs:207`, `battery/mod.rs:430`, `ventilation.rs:352`, `protocol_bridge/mod.rs:116`, `pv/mod.rs:267`** | ❌ typed specs |
| **Typed-only** (`typed.and_then(c.equipment_id)` — blind to raw payloads) | **`generator.rs:911`** | ❌ raw specs |

The five raw-only readers are why the ticket's own EV is `EquipmentId(0)` even
though `EvConfig` *has* an `equipment_id` field nothing ever populates. All six
sites are aligned onto the shared reader.

**The consolidated reader's contract — the decision both work items depend on.**
There are today two raw-or-typed readers (`equipment_id_from_config` in
`hvac/helpers.rs:119`, `parse_u32` in `schedule_helpers.rs:141`); they merge into
**one home** in the `hares-equipment` config module, and its contract is
**tri-state**: key absent / key present with a valid value / malformed value
(typed error, as `equipment_id_from_config` already does for non-finite,
negative, fractional, out-of-range). Absent must be distinguishable from an
explicit `0`, because two different defaulting sites need different defaults:
construction sites default absent → 0 (the sentinel, as today), while the
dehumidifier's init re-assignment site defaults absent → **preserve the
descriptor's current id**. Both existing readers conflate absent with 0
(`equipment_id_from_config` returns `Ok(0)` for absent,
`hvac/helpers.rs:119-131`); the consolidated reader must not.

**The dehumidifier's init site adopts the sibling shape.** Six of the
seven init/reconfig re-assignment sites already preserve when absent
(`self.descriptor.id = EquipmentId(c.equipment_id.unwrap_or(self.descriptor.id.0))`
— the five water heaters and `generator.rs:1150`). The dehumidifier does not:
`dehumidifier.rs:540-541` calls `equipment_id_from_config(config)?` and assigns
the result unconditionally, so an absent key **clobbers the id to 0** at init.
With the tri-state reader the site becomes preserve-when-absent, matching its six
siblings — a one-line change that closes the "init re-assignment clobbers
identity" class instead of masking it behind the injection.

### Work item 3 — Identity and validation at both entrances

**Entrance 1 — assembly loop** (~2252, where the existing duplicate-name check
lives): after construction, reject any duplicate descriptor id and any descriptor
id of 0, with `HaresError::Equipment` naming the equipment — the same error type
and shape as the adjacent name check. Always-on, not debug-gated: init boundary,
one-time, off the hot path, and the constitution's loud-errors rule prefers
`Err` over warn/debug-only.

**Entrance 2 — `Dwelling::add_equipment` (`dwelling/mod.rs:3249`) assigns
identity, then validates.** New behaviour:

- **One id algorithm**: the Dwelling holds a monotonic
  never-reused id counter, initialized to 1 at construction (so dwellings built
  without the assembly path are covered), and advanced past every id that enters
  the equipment vector — assembly-assigned or caller-explicit. Auto-assignment
  draws the counter's value and increments it; an explicit id ≥ the counter
  advances the counter past it. `remove_equipment` never reuses ids. Because the
  counter advances past explicit ids too, the sequence "assembly yields 1–12;
  caller adds explicit 13; next id-less add" assigns 14 — it can never collide
  with an id already in the vector, so auto-assignment never errors spuriously.
- **Auto-assign unassigned**: equipment whose descriptor id is 0 (never set, or
  explicitly zeroed — same meaning) is assigned the counter's id before it joins
  the vector.
- **Reject explicit collisions**: an explicit id that duplicates an id already in
  the vector is a typed error naming both equipment. Only *explicit* collisions
  error; defaulted ids are assigned, never rejected.
- **Postcondition — the assignment is verified, not assumed**: after the
  write and before the push, `add_equipment` checks **`descriptor().id ==
  the assigned value`** — the equality, not a weaker "non-zero and
  unique" gloss: assigned ids start at 1, so equality subsumes non-zero,
  and the counter never issues an in-use id, so equality subsumes
  uniqueness. A body that wrote nothing (id stays 0), wrote the wrong
  field, or wrote a *wrong-but-plausible* value (non-zero, unused, but not
  the id the counter issued) all fail this check — the last case matters
  because an id that enters without going through the counter is one the
  counter cannot have advanced past, so a later auto-assign could hand it
  out again; rejecting the add closes that path. The failure surfaces as
  entrance 2's **own** typed error — naming the equipment, stating the
  identity write did not take the assigned value, and pointing at the
  equipment's `Equipment` implementation of the setter — not entrance 1's
  "unassigned id" message, which would misdirect the implementer to the
  injection path when the defect is the body they just wrote. This is the
  check that makes the invariant *enforced* at entrance 2 rather than hoped
  for: the write's landing is strictly less certain than the assembly
  injection (fresh hand-written bodies versus one config write), so it is
  validated, exactly as entrance 1 validates its injection.

The invariant is entrance-uniform and entrance-*enforced*: **id 0 never
survives into a dwelling's equipment vector, through either entrance**, and
every equipment in every dwelling is uniquely addressable — checked at the
door on both sides, not asserted. Two Python `add_*` calls on one dwelling
(`add_ev` + `add_pv` — the ordinary DER scenario; every Python builder
hard-codes `equipment_id: None`) keep working, each auto-assigned a distinct id.

**The mechanism, and its safety argument.** The
write is a narrow trait method on `Equipment` that sets the descriptor id.
Decisions that determine whether it is sound:

1. **Required method, no default body** — the shape of `rename`
   (`lib.rs:168`). The workspace has 57 `impl Equipment for` sites across two
   crates — 25 files in `hares-equipment` and 7 in `hares-core` (test stubs,
   e.g. the guard-implementing stub at `dwelling/mod.rs:7662`). **Six of the
   57 are macro-delegated** (`delegate_equipment!`: the four heat-pump heater
   newtypes at `heater.rs:459-462`, `GasGenerator` and `FuelCell` at
   `generator.rs:2064-2065`); the remaining ~51, including test stubs, are
   hand-written and each needs a one-line body. A default body is rejected on
   both available forms: a no-op default silently keeps `EquipmentId(0)` and
   breaks the zero-never-survives guarantee — the silent-identity-failure
   class this initiative exists to close; an erroring default moves a
   compile-time obligation to runtime. The compiler enumerates every site;
   that is the point of choosing it — at the honest price of ~51 one-line
   bodies, not at an illusory price absorbed by a macro.

   **The out-of-tree half of the population, weighed** (the trait is public,
   and the workspace's own `workspace_gate_guard.rs` is built on the premise
   that "any sibling crate can implement it" — custom equipment outside the
   core crates is a stated product goal). A required method is a hard
   compile break for every out-of-tree implementor, and for those the
   compiler enumerates nothing — their crate simply fails until they write a
   body. Weighed and accepted: the trait is pre-1.0 and the constitution
   allows no back-compat constraints ("no legacy paths to maintain"), the
   break is the loud, conscious failure — each implementor must deliberately
   write something — and a deliberately-written no-op is caught by the
   entrance-2 postcondition above. The erroring-default alternative is
   re-weighed against this and rejected: for out-of-tree implementors its
   "loud at first use" is exactly matched by the postcondition's
   loud-at-first-use, so it buys nothing the postcondition does not already
   provide, while losing the in-tree compile-time enumeration. Both halves of
   the population are now priced, not one.
2. **Guard-engaged to the extent the guard exists, with the residual
   exposure closed by an invariant — not by a sentence.** `add_equipment` is
   where the codebase installs its post-registration mutation guard:
   `eq.mark_initialized()` before the push (`dwelling/mod.rs:3259-3262`),
   with LUT setters hard-erroring once initialized (`lib.rs:433-463`) and the
   trait documenting an `unmark → mutate → mark` pattern for Dwelling-mediated
   reconfiguration (`lib.rs:402-422`). `rename` ignores the guard entirely and
   has no production caller — it is not the model. Two facts about the guard,
   verified, shape this decision:
   - **The guard is opt-in per type**: `is_initialized()` defaults to `false`
     (`lib.rs:402-405`) and is overridden in production by exactly two types —
     EV and Battery (`ev/mod.rs:1912`, `battery/mod.rs:1946`), the two the
     guard was built for (T-0534). For the other 55 implementors, a
     guard-checking setter's check is a constant `false` — the check alone
     protects nothing there.
   - **The delegation macro is guard-blind**: `delegate_equipment!`
     forwards neither `is_initialized` nor `mark_initialized` (verified —
     zero matches in `macros.rs`), so its six wrapper types report `false`
     and no-op regardless of their inner type. Routing a guard-checking
     setter through it would bypass the guard by construction. The fix
     **forwards the three guard methods in `delegate_equipment!`**, closing
     this pre-existing broken window while the plan is in that macro anyway.
     Stated honestly: the fix's present-day effect is **zero** — the wrapped
     inner types (`Generator`, `HeatPumpHeaterCore`) do not override the
     guard, so both the bypass and its closure are prospective; the window
     is fixed so it can never open, which is what fixing a broken window
     means, and the forwarding gets a regression test regardless (coverage
     item 12).
   The new setter **checks `is_initialized()` and hard-errors when true** —
   real protection for EV and Battery, the two types that actually track the
   flag — and `Dwelling::add_equipment` performs the write **before**
   `mark_initialized()` and the push: pre-registration, Dwelling-mediated,
   and the ordering is verified sound (nothing sets the flag except
   `mark_initialized` itself, so the write is unblocked on every type). For
   the 55 types with no guard override, the residual exposure is stated, not
   papered over: a post-registration id mutation would desync
   `equipment_id_by_name` (built at registration, rebuilt only by
   `refresh_equipment_caches`) from the live descriptor — the end-of-step
   snapshot keys `equipment_core` through the map (`dwelling/mod.rs:5856-5869`)
   while every equipment-side consumer reads `descriptor().id` live, which is
   the I-06 misbinding reintroduced. **No public API can reach it** —
   verified: `Dwelling::equipment()` returns `&[Box<dyn Equipment>]`
   (immutable, `dwelling/mod.rs:3080`), `PyEquipment` holds snapshots only
   (descriptor/core-output copies, `py_equipment.rs:1070-1075`), and
   `remove_equipment` evicts before returning the box, after which re-adding
   rebuilds the map — so the exposure is internal-code-only. It is closed by
   a **debug invariant at the desync point**: the snapshot's name→map lookup
   already holds both the map's id and the live `desc.id`; a
   `debug_assert_eq!` there fails loudly in debug builds if any code path —
   this setter or any future mutator — drifts an id after registration. And
   because that threat model ("any future mutator") is a *static* property
   of the code, it gets the workspace's static tool for exactly this class:
   the three AST-scanning guard tests (`si_guard.rs`,
   `control_gate_guard.rs`, `workspace_gate_guard.rs`) exist on the stated
   premise that "a doc comment is not enforcement", and one already scans
   sibling crates. The fix adds a guard test in that established pattern —
   scanning for identity-setter call sites outside the authorized caller
   (`Dwelling::add_equipment`), the trait definition, and tests — firing in
   CI on *existence*, whether or not any test executes the path, and
   reaching sibling crates the runtime assert never can. **Characterized
   honestly — this guard is a heuristic, not a decision like its three
   siblings**: the precedent guards scan `impl Equipment for …` blocks for
   *method definitions*, a syntactically unambiguous target `syn` resolves
   without type information; a *call-site* scan is a different kind of
   target — `syn` is purely syntactic, so a name-matched scan can
   false-positive on same-named methods on unrelated types and
   false-negative on UFCS or macro-generated calls. It is worth keeping
   because its failure mode is a loud CI error (a false positive is
   resolved by renaming; a false negative leaves the other layers), and
   its reach is stated: workspace `src` only — an in-repo test cannot scan
   out-of-tree crates, and out-of-tree pre-registration use of the setter
   on one's own equipment is neither harmful nor guardable: whatever id is
   present at `add_equipment` is validated by the postcondition (explicit
   or assigned), so the guard's rule concerns workspace `src` alone. The
   defence is layered, each layer matching its threat and honestly
   graded: the verified exposure boundary (no public mutable handle), the
   entrance-2 postcondition (a failed, no-op, or wrong write fails loudly
   at the only caller), the static guard (future mutators, workspace
   `src`, heuristic), and the debug assert (runtime drift, as the last
   net). One observation recorded rather than dropped:
   in-crate tests are not bound by the public API
   (`replace_equipment_for_test` already reaches the private equipment
   vector), so a drift *could* be test-driven; given the three preceding
   layers it is judged marginal and left untested — stated, not assumed
   away.
3. **Post-init mutation is safe here, on these specific grounds** (verified):
   every equipment-side id consumer reads the descriptor **live** — the
   equipment-side hits are inline `self.descriptor().id` reads at telemetry,
   core-output, and port-construction sites (e.g. `ventilation.rs:920`,
   `scheduled_load.rs:794`, `battery/mod.rs:1702`, `generator.rs:1873`), with no
   init-time copy of the id into cached state; `PortDeclaration`
   (`hares-types/src/ports.rs:134-144`) carries **no equipment id**, so port
   declarations cannot hold a stale one; the equipment has not stepped in this
   dwelling yet, so nothing has been emitted under the old id; and
   `refresh_equipment_caches` (`dwelling/mod.rs:3342`) rebuilds the name→id
   map, the execution order, the dispatch targets, and the output schema after
   the push — everything identity touches. The implementer re-verifies the
   no-cached-id grep over the touched impls before landing.

**What entrance 2 inherits that this plan does not fix** (pre-existing, tracked
in work item 5): `refresh_equipment_caches` does **not** rebuild `PortSlots` —
`PortSlots::from_declarations` runs only at assembly (`dwelling/mod.rs:2282-2283`)
and in a test helper (`7525-7526`) — so equipment added after construction
declares ports against a slot table that predates it. Every Python `add_*` call
is in this position today. Promoting entrance 2 to a first-class identity path
makes this state worth recording; fixing it is a separate change (slot-table
rebuild) and is tracked, not folded in.

The RCA's "why the safety nets didn't catch it" list needs **no edits**: the
`resolve_equipment_id` name-absent warning, the debug SOC-divergence warning,
and the "equipment_core missing entry" debug invariant were blind only because
of the id collapse and become effective automatically once ids are unique.

### Work item 4 — Range-anxiety override yields in-window to a higher strategy target

The RCA left one decision to this phase: "whether the override should also yield
in-window when the strategy target exceeds the band." **Decision: yes —
included, landed as a separate commit.** Pre-empting a higher active target with
a lower one charges less than the driver configured for zero safety gain; the
override's own doc comment states the intent ("the strategy's own target governs
once the pack is back above the band") and the urgency gate exists "not to
preempt the driver's configured strategy".

**Size, stated honestly.** The decision input does not exist at the decision
point: `evaluate_charging` (`ev_driver/mod.rs:1014`) calls
`maybe_push_range_anxiety_override` (:927) first and returns if it pushed — the
composer never runs that step. The change is a **precedence inversion plus one
condition**: the composer's in-window resolution is computed (or queried — its
resolved-plan state already exists for the `last_action` label) before the
override decision; the override stands down when that target is ≥ the anxiety
band. Two hard requirements:

1. **No silence**: a step where the override stands down must still dispatch the
   composer's plan for that step — fall-through is a requirement, not an
   implication (constitution, "Silence is not a decision"; I-05 precedent).
2. **Path exclusivity, as today**: a step's charging dispatch comes from exactly
   one of the two paths — the override pushes and returns, *or* the composer
   evaluates — never both, never neither. This is today's shape (the early
   `return` guarantees it) and the inversion must preserve it. Note the invariant
   is *path* exclusivity, not signal count: the composer may legitimately emit
   more than one request per step (e.g. `EvSetReadyBy` alongside a target); the
   implementer verifies the composer's per-step emission shape at the seam
   before implementing, since the plan commits to the either/or, not to a count.

The override continues to fire as designed out-of-window, when no strategy
target is active, or when the strategy's own target is below the band. The
urgency gate, the band computation, and the cancellation-clearing-`todays_event`
mechanics stay exactly as they are — the RCA verified those are correct for a
genuinely empty pack. The exact seam for reading the composer's resolved target
is the implementer's; the plan commits to the precedence rule, the two
requirements, and the honest size.

**Attribution**: WI-4 lands as its own commit after the identity change, gated by
its own tests plus the workspace suite. If EV behaviour moves unexpectedly on the
combined tree, isolating or reverting the WI-4 commit attributes the failure
without touching the root-cause fix. If WI-4 must ultimately be dropped, it gets
its own initiative first — no untracked deferral — but the default is that it
lands.

### Work item 5 — Test hygiene and two tracking obligations

`crates/hares-core/tests/i06_ev_nightly_diag.rs` is superseded by
`equipment_id_regressions.rs`; its distinct-id probe is absorbed into the
permanent regression file (coverage item 2), and the diagnostic file is removed
with explicit user approval, requested at the top of the fix step's work.

**Two pre-existing defects sit directly under this change's blast radius and are
tracked, not folded in. The fix step creates two tracking initiatives — one per
defect — before I-06 closes; they share nothing but the fact that this change
surfaced them, and each is independently verifiable:**

1. **Checkpoint restore is positional** — `load_checkpoint`
   (`dwelling/mod.rs:4166`) restores equipment state by positional `zip` after
   only a length check (4180-4189); `DwellingCheckpoint` (`checkpoint.rs:46`)
   carries opaque `equipment_states` with no equipment identity, so a spec
   reorder between save and restore silently loads state into the wrong
   equipment. Fixing it requires extending the checkpoint format (record
   identity per state blob, validate alignment, bump `format_version`).
2. **Port slots are never rebuilt for post-construction equipment** — only
   assembly (`dwelling/mod.rs:2282-2283`) and a test helper (`7525-7526`) call
   `PortSlots::from_declarations`; `add_equipment` → `refresh_equipment_caches`
   does not, so equipment added after construction declares ports against a
   slot table that predates it (every Python `add_*` call today). Fixing it
   requires a slot-table rebuild path in `refresh_equipment_caches` (safe only
   before any step has run, like the output-schema rebuild it sits beside).

Both dependencies are recorded concretely; neither is silently dropped.

## Alternatives considered and rejected

1. **Re-key `equipment_core` (and the other `EquipmentId`-keyed maps) by name.**
   Rejected: fixes one map while leaving the defaulted descriptor ids — the
   actual defect — in place; every other id-keyed structure stays collapsed or
   needs a cross-crate re-key; any future id consumer reintroduces the bug.
   More churn for less coverage; violates "fix the class."

2. **Patch the observation consumers to resolve by name.** Rejected: targets the
   symptom's consumer, leaves the collapsed maps wrong, creates a parallel
   identity scheme next to a working id scheme.

3. **Hard-error in every constructor on a missing `equipment_id`.** Rejected: a
   constructor sees one config and cannot know uniqueness; it breaks every
   direct-construction caller without preventing collisions. The loudness is
   delivered by validation at both entrances.

4. **Post-construction descriptor mutation as the *assembly* injection
   mechanism** — mutate `descriptor.id` between `registry.create` and `eq.init`.
   Rejected on the two grounds that discriminate it from the config injection:
   (a) the init re-assignment paths read identity from *config*, not the
   descriptor — the dehumidifier would zero the mutated id at init (work item 2),
   and the class generally re-derives identity from the config channel, so a
   descriptor-only mutation creates two competing identity sources; (b) the
   config injection reaches every constructor through the channel it already
   reads, with no ordering hazard. (That it requires new trait API surface is
   *not* a ground here — the adopted entrance-2 mechanism pays that cost too,
   willingly, because at entrance 2 the config channel is closed: the caller has
   already constructed and initialized the equipment. The mechanism family is
   the same; the interval decides.)

5. **An `equipment_id` field on `EquipmentSpec` as the carrier.** Rejected: the
   spec's typed config already carries the field every constructor reads; a
   parallel field means two homes for one value and threading through both
   config conversions for no gain. (The *location* half of this question —
   which crate the assignment pass lives in — was decided the other way: the
   pass lives in `hares-io` beside `assign_instance_names`, work item 1.)

6. **Assembly-only scope — assign ids at assembly and leave `add_equipment`
   exactly as today.** Weighed, because the RCA's confirmed root cause and both
   reproduction gates exercise the assembly path only, and this is the
   smallest diff. The comparison, both halves priced honestly:
   - **What assembly-only leaves broken** — narrower than everything, wider
     than nothing: a *single* Python-added equipment is correctly addressable
     (id 0 is unique in that vector); the collapse begins at two or more
     defaulted adds — `add_battery` + `add_pv`, the ordinary DER scenario —
     whose maps collapse into each other and whose observation actors misbind
     among the added equipment. It also leaves the invariant
     entrance-conditional (id 0 rejected as unassigned through assembly,
     silently valid through `add_equipment`) and `PyEquipmentDescriptor.id`
     reporting `0` for every added equipment.
   - **What full coverage costs — re-priced at the current inventory, not
     revision 3's**: a required trait method with ~51 hand-written one-line
     bodies plus 6 macro-forwarded (work item 3, decision 1); guard-method
     forwarding in the delegation macro; the never-reused counter; the
     entrance-2 postcondition with its own diagnostic; the debug desync
     invariant; the static guard test; the setter-contract population test
     over `CANONICAL_EQUIPMENT_NAMES` plus the `lifecycle.rs` entry; and
     the machinery's coverage arms. **The decision is explicitly re-run at
     this price and stands**: the class argument (the multi-add DER
     scenario at the Python entrance, the incoherent entrance-conditional
     invariant, `PyEquipmentDescriptor.id` staying wrong) is independent of
     the price and has not changed across five revisions; the price has
     only been counted honestly.
   Rejected on the class rule, at that price: the multi-add DER scenario is
   the ordinary one at the entrance where the Python API — a first-class
   product surface — lives; the entrance-conditional invariant is incoherent
   on its face; and the constitution calls a half-fixed class "not a fix".
   The price is real and is paid knowingly, not absorbed by a macro.

## Blast radius

**Touched (production):**

- `crates/hares-io/src/hpxml/equipment.rs` — the id-assignment pass beside
  `assign_instance_names` (assembly-invoked only; the resolvers and
  `resolve_loads.rs` do not change).
- `crates/hares-core/src/dwelling/mod.rs` — assembly loop: call the pass,
  collision/unassigned validation; `add_equipment`: never-reused-id
  auto-assignment, explicit-collision rejection, docstring contract update; the
  id counter (construction-initialized).
- `crates/hares-equipment/src/config.rs` — the single tri-state id reader
  (consolidating `hvac/helpers.rs:119` and `schedule_helpers.rs:141`), and — if
  the setter route is chosen — the non-test write path for the injected id.
- Six constructor read alignments: `ev/mod.rs:207`, `battery/mod.rs:430`,
  `ventilation.rs:352`, `protocol_bridge/mod.rs:116`, `pv/mod.rs:267`,
  `generator.rs:911` (+ import sites).
- `crates/hares-equipment/src/hvac/dehumidifier.rs:540-541` —
  preserve-when-absent at init (work item 2).
- `crates/hares-equipment/src/lib.rs` + `macros.rs` (the identity write; the
  guard-method forwarding added to `delegate_equipment!`) + every `impl
  Equipment` block — **57 sites across two crates**: 25 files in
  `hares-equipment` and 7 in `hares-core` (test stubs, e.g. the
  guard-implementing stub at `dwelling/mod.rs:7662`); 6 of the 57
  macro-delegated, ~51 hand-written one-line bodies. A required trait method
  breaks compilation in both crates (work item 3).
- `crates/hares-equipment/tests/lifecycle.rs` — the identity-write assertion
  joins the `assert_equipment_lifecycle` calls that exist today (10 call
  sites, 9 types), and the stale header — whose growth rule names a
  `test_equipment_lifecycle` function that does not exist (the real
  function is `assert_equipment_lifecycle`, `lifecycle.rs:59`) — is
  corrected as part of touching the file; plus a new AST guard test in
  the established `*_guard.rs` pattern (identity-setter call sites outside
  the authorized caller); plus the setter-contract population test
  iterating `CANONICAL_EQUIPMENT_NAMES` in the `registry_coverage.rs`
  precedent (coverage item 12).
- `crates/hares-core/src/actors/ev_driver/mod.rs` — `maybe_push_range_anxiety_override`
  (:927) / `evaluate_charging` (:1014): the precedence inversion (work item 4,
  separate commit).
- `crates/hares-core/src/dwelling/conversions.rs` — the reserved-key rejection
  on the override delta at `apply_equipment_overrides`' three sources
  (`conversions.rs:843-853`).

**User-visible API change, decided (not merely affected)**:
`PyEquipmentDescriptor.id` (`crates/hares-python/src/py_equipment.rs:891-894`,
reached via `PyDwelling::equipment()`, `py_dwelling.rs:941-947`) returns `0`
today and `1..N` after the fix. This is the fix working — correct identity
surfacing to Python consumers — and is recorded as a release-note item, not
silently absorbed. (The plan's earlier claim that `RangeViolationEvent` was "the
only other id consumer" was about *internal* consumers and was incomplete as
stated; corrected here.)

**Affected tests — derivation stated so the reviewer can re-derive it.** The
list is the union of **three** greps over the three surfaces this change
moves: `add_equipment(` call sites in tests; direct `EquipmentId` construction
in tests; and `impl Equipment for` blocks — the last because a required trait
method breaks compilation of every implementor, including test stubs that
neither of the first two greps finds (the stubs live in the 7 `hares-core`
files named above and in `hares-equipment`'s **integration-test crates** —
`tests/control_gate_guard.rs`, 4 `impl Equipment` blocks, and
`tests/workspace_gate_guard.rs`, 3 — which are separate integration-test
binaries, not `#[cfg(test)]` modules, and so are invisible to the first two
greps). `equipment_ordering_tests.rs`, `dispatch_ordering_regressions.rs`,
`zone_state_ordering.rs`, `safety_monitor_regressions.rs`,
`multi_instance_naming.rs` (15 sites), `checkpoint_regressions.rs` (9),
`grid_outage_islanding.rs` (2), dwelling internal tests, `hares-python`
fixtures; and `core_output_regressions.rs:291` (`EquipmentId(42)`),
`core_output_invariants.rs:163,633,651` (`EquipmentId(101/201/202)`) — the
latter are explicit unique ids, so under entrance-2 rules they are
collision-checked, not auto-assigned, and are expected to pass; they are listed
because the change moves the semantics of what an explicit id means. Under
entrance-2 auto-assignment, colliding *defaulted* ids no longer error — they are
assigned — so only tests adding equipment with explicitly colliding ids fail.
`grid_outage_islanding` and `checkpoint_regressions` are also the most likely to
show expected behavioural shifts (both involve DER equipment with id-keyed
actors — see "Expected shifts"). This is a derived list, not a completeness
claim; a reviewer re-running the three greps extends it the same way.

**Deliberately untouched, with reasons:**

- **All `EquipmentId` consumers** — `EvDriverActor`, `BatteryManagementActor`,
  the end-of-step `equipment_core` snapshot (`dwelling/mod.rs:5841-5869`),
  `prev_equipment_modes` / `equipment_mode_changed` (`4602`), the active-id
  retain (`5841-5843`), `equipment_id_by_name` construction: correct by
  construction once ids are unique.
- **The checkpoint format** — behavior byte-identical (none of the 35
  `save_state` impls serializes the descriptor or its id; verified by the
  critique's ledger); its positional-restore fragility is tracked (WI-5).
- **The port-slot allocation path** — pre-existing gap, tracked (WI-5), not
  folded into an identity change.
- **Equipment physics, the #25 hold/`SOCTarget` contract, `TimeWindowPref`** —
  RCA verified all work.
- **The override's surrounding mechanics** — urgency gate, band computation,
  cancellation-clearing-`todays_event`: correct per the RCA; WI-4 changes only
  the in-window precedence.
- **`hares-types`** — no type changes; `EquipmentId` already serializes and the
  maps it keys already exist.

## Expected shifts and the attribution criterion

Shifts in existing numeric expectations are expected; they are the fix working.
The observation wiring this fix repairs is behaviorally load-bearing wherever an
id-keyed consumer exists today: `equipment_core` goes from one entry to N, so
SOC-feedback actors (BMS, EV driver) stop reading the last-inserted equipment's
output and start reading their own — different dispatch, different energy, in
every fixture with a battery + BMS or other SOC-feedback actor; and
`prev_equipment_modes` stops collapsing, so interest filtering wakes a different
set of actors on different steps in every dwelling with two or more
mode-capable equipment. Most likely movers: `grid_outage_islanding`,
`checkpoint_regressions`, any parity/BESTEST fixture with storage. (One shift
mechanism is verified closed: no seeded or hashed path reads the equipment id —
the RNG contract is name-based and `derive_sub_rng` is keyed on a loop counter.)

**The criterion that makes a shift explained**: a moved expectation's delta must
trace to an id-keyed consumer now addressing the correct equipment — name the
consumer, the equipment it previously misread (under the collapsed map, the
last-inserted one), and show the new expected value matches the correct
equipment's state or flow. The RCA's MELs-vs-EV trace is the template. A shift
that cannot be traced that way is unexplained — investigate; never widen a
tolerance to absorb it. Updating an expected value **with the trace written into
the test** is correct and required; weakening an assertion is a forbidden
non-fix.

## Test coverage (comprehensive)

All new tests behavior-named per the constitution; dwelling-level coverage where
the bug lives. The resolution matrix maps each issue to the item that proves it.

1. **The two reproduction gates go green** (exist already, fail today):
   `equipment_core_has_one_entry_per_equipment` and
   `nightly_strategy_keeps_ev_charged_over_long_horizon` (real assembly, 45
   days, ≥30 charge-days, zero cancellations). The structural gate doubles as
   the write-path discriminator (work item 1).
2. **Distinct-id probe, absorbed from the diagnostic harness, entrance-uniform**:
   every equipment in a `from_config`-built dwelling has a distinct, non-zero
   id; a companion assertion covers a dwelling built by assembly *plus*
   `add_equipment` entries — pinning that id 0 never survives through either
   entrance, including two id-less Python-style adds (the DER scenario).
3. **Determinism**: the same dwelling config built twice yields an identical
   name→id mapping; the mapping is asserted, never contiguity.
4. **Checkpoint round-trip**: save → `load_checkpoint` → `equipment_core` still
   one entry per equipment and actor bindings still resolve — proves ids and
   bindings survive the round-trip (the active-id retain is the production site;
   the positional-restore fragility is tracked separately, WI-5).
5. **Validation arms, each with its own test**: assembly rejects duplicate ids
   and unassigned (0) ids; a user override containing `equipment_id` is
   rejected from **each** of the three override sources, including the wildcard
   form; `add_equipment` auto-assigns (two id-less adds get distinct non-zero
   ids), rejects an explicitly colliding id, and the never-reused counter
   survives the explicit-then-auto sequence (explicit 13 after assembly 1–12,
   next auto-add takes 14); and the entrance-2 postcondition — a test stub
   equipment whose setter body is a no-op fails `add_equipment` with the
   typed unassigned-id error, proving a silent body cannot join the vector
   (the arm that would have caught an unenforced entrance). All on returned
   typed `Err`s / observable state — never log capture.
6. **Channel-agnostic read coverage, exhaustive over the aligned sites**: for
   each of the six constructors, a test per payload kind (twelve cases); a
   heat-pump `#[serde(flatten)]` pin; tri-state reader unit coverage (absent /
   present / malformed); a consistency test that every init/reconfig
   re-assignment path keeps — never clobbers to 0 — the injected id, including
   the dehumidifier's corrected site.
7. **BMS observation through real assembly** — the RCA's second misbound
   consumer: a dwelling-level test with a battery (injected into the HPXML the
   way the reproduction injected the EV — `hares-io` parses DER via
   `resolve_der.rs` — or via the blueprint `add_equipment_spec` path) asserting
   the `BatteryManagementActor` observes its own battery's SOC.
8. **Interest filtering under unique ids**: a real-assembly test that a mode
   change on one equipment is attributed to that equipment (two mode-capable
   equipment, change one, assert the other's targeting actor is not spuriously
   woken and the changed one's is).
9. **Override-yield coverage (work item 4), vehicles named**: the stand-down
   side (in-window, strategy target ≥ band → composer's higher target
   dispatched) is not reachable through real assembly post-fix (the RCA's
   ruling-out argument: observation reconciles the belief to ~0.9, far above the
   band), so its vehicle is the existing `ev_driver` actor-level harness
   (`make_actor` / `plugged_in_step` / `set_core_soc`, `ev_driver/mod.rs:1461,
   1520, 1569`) with hand-set belief state — legitimate here because the defect
   under test is actor precedence logic, not assembly identity. The raise side
   (strategy target below band in-window on a genuinely low pack → band target
   dispatched) is reachable through real assembly — a low-target strategy with
   real driving — and gets a dwelling-level test. Assert on dispatched
   `ControlSignal`s, never logs.
10. **Behavioral breadth across fixtures**: the 45-day nightly scenario
    parameterized over at least two in-repo ResStock fixtures (`bldg0000007`
    plus one other with a different equipment mix). The ticket's second home
    (Tarrant County 46363) is not an in-repo fixture (verified) and needs none:
    the mechanism is fixture-independent and the structural probe asserts the
    rule for any assembly-built dwelling.
11. **Everything existing stays green** — read through the "Expected shifts"
    protocol: a moved value with a written trace is a pass-condition update,
    not a failure.
12. **The new trait machinery's arms (work item 3)**, each with its own
    test, and with the setter contract's coverage mechanism chosen against
    the workspace's declared population rather than a hand-maintained list:
    the **setter contract across the equipment population** — an exhaustive
    iteration over `CANONICAL_EQUIPMENT_NAMES`
    (`registry.rs:17`, 63 entries) in the precedent pattern of
    `crates/hares-equipment/tests/registry_coverage.rs` (which already
    iterates the constant to prove registration): for each canonical name,
    construct the equipment via the registry (the factories are infallible
    by contract — invalid configs defer errors to init, which this
    contract does not need), call the identity setter with a test id,
    assert the descriptor took it. Membership derives from the declared
    population, so every future equipment type is covered automatically —
    a hand-maintained harness cannot promise that, and the existing one has
    already drifted (`lifecycle.rs` covers 9 of the 63 canonical types). A
    type whose constructor cannot build from a minimal config is an
    **explicitly listed exception** in the test, so every gap is visible
    rather than silent. The `lifecycle.rs` harness additionally gains the
    identity-write assertion in the `assert_equipment_lifecycle` calls
    that exist today (10 call sites, 9 types), and its stale header — the
    growth rule names a `test_equipment_lifecycle` function that has zero
    occurrences; the real function is `assert_equipment_lifecycle`
    (`lifecycle.rs:59`) — is corrected as part of touching the file. What
    this covers, stated exactly: every registered production class
    (exhaustive, auto-growing); test-stub implementors are compile-checked
    for a body and postcondition-checked at use in any test that adds them
    — not iterated, because they are not registered. The **guard error
    path** — on a flag-tracking type (EV or Battery): `mark_initialized`,
    then the setter, expects the typed initialized-error (unreachable from
    the plan's own flow, which is precisely why it needs its own test); the
    **macro forwarding regression** — through a public wrapper type (e.g.
    `GasGenerator`): the wrapper's guard methods forward to its inner type
    — the test that would have caught the guard-blindness; and the
    **static guard** — the new `*_guard.rs` scan passes in CI like its
    three siblings, its pass being its proof.

## Verification

Using exactly the paths the RCA and reproduction established:

**Failing tests that must go green:**

```
cargo test -p hares-core --test equipment_id_regressions equipment_core_has_one_entry_per_equipment
cargo test -p hares-core --test equipment_id_regressions nightly_strategy_keeps_ev_charged_over_long_horizon
```

**Passing commands that must stay green:**

```
cargo nextest run --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check --all
```

**The suite gate is green under the Expected-shifts protocol, and only under
it** — the qualification the protocol section and coverage item 11 already
carry, stated here because this section is the one the fix step reads as its
definition of done: a moved numeric expectation **with a written trace** (the
id-keyed consumer named, the previously-misread equipment named, the new value
matched to the correct equipment, the trace written into the test) is a
pass-condition update, not a failure; an unexplained shift is a failure; and
editing an expectation quietly to restore green without the trace is the
forbidden non-fix the Expected-shifts section exists to prevent. An
implementer holding an unqualified "must stay green" against a legitimately
moved fixture has two ways out — thrash on a shift the plan predicted, or
quietly absorb it — and this qualification removes both.

**Commit protocol**: the identity change (WI-1–3) lands first, gated by the two
reproduction tests and the suite; the override change (WI-4) lands second as its
own commit, gated by coverage item 9 and the suite. The 45-day gate runs on the
combined tree; attribution on any failure is by commit isolation.

## Resolution matrix — every issue, its resolution, its proof

Issues 1–17 are inherited from the RCA, reproduction, planning, and critique
round 1 (their resolutions stand except where noted); 18–31 are critique round 2;
32–36 are critique round 3; 37–41 are critique round 4; 42–48 are critique
round 5.

| # | Issue | Resolved by | Proven by |
|---|---|---|---|
| 1 | No unique `EquipmentId` assignment in any real build (RCA root cause) | WI-1 | coverage 1, 2, 10 |
| 2 | EV driver observation cross-wired (RCA causal chain) | WI-1, zero consumer edits | coverage 1 |
| 3 | `BatteryManagementActor::read_soc` identically misbound (RCA class audit) | WI-1 | coverage 7 |
| 4 | `prev_equipment_modes` collapse → spurious/missed wakeups (RCA class audit) | WI-1 | coverage 8 |
| 5 | Active-id retain no-op under one shared id (RCA class audit, site corrected in rev 2) | WI-1 | coverage 4 |
| 6 | "Last equipment wins" — spec-order dependence (RCA class audit) | WI-1 | coverage 2 |
| 7 | Six one-sided constructor id reads | WI-2 | coverage 6 |
| 8 | Two duplicate id readers | WI-2 consolidation | coverage 6 |
| 9 | Id collisions never validated at assembly | WI-3 | coverage 5 |
| 10 | Entrance 2 needs identity, not rejection (round-1 C1) | WI-3 auto-assignment | coverage 2, 5 |
| 11 | Three blind safety nets (RCA) | No edits; effective post-fix through gated behaviour | coverage 1, 2, 7 |
| 12 | Coverage gap: unit tests inject ids manually, short horizons (RCA + ticket) | The reproduction's dwelling-level tests, extended | coverage 1, 10 |
| 13 | Override pre-empts higher in-window target (RCA secondary, delegated) | WI-4, separate commit | coverage 9 |
| 14 | Superseded diagnostic harness (reproduction note) | WI-5, removed with sign-off | post-fix repo state |
| 15 | Ticket's second home not in-repo (verified) | Determination: mechanism fixture-independent | coverage 2 + 10 |
| 16 | Id-keyed RNG shift risk | Verified closed: name-based seeding | "Expected shifts" |
| 17 | Typed-config inventory (list corrected in rev 2) | Verified closed: 22 structs | WI-1 |
| 18 | `equipment_id` unreserved against overrides (round-1 W4) | WI-1: reserved, rejected on the override **delta**, all three sources | coverage 5 |
| 19 | Assignment location unweighed (round-1 W5) | WI-1: pass in `hares-io` beside `assign_instance_names`; `EquipmentSpec`-field carrier rejected (Alt. 5) | coverage 3 |
| 20 | Dehumidifier init clobbers id to 0 (round-1 W1) | WI-2: tri-state reader + preserve-when-absent | coverage 6 |
| 21 | "No output shift expected" wrong (round-1 C2) | "Expected shifts": classes + trace criterion | coverage 11 via protocol |
| 22 | Typed-config write path hazard (round-1 W9; premise corrected this round — round-2 C3): cross-crate use of test accessors is a **compile error**, the residual hazard is hares-equipment's own unit tests, and the integration gate **is** the discriminator (links hares-io/hares-equipment without `cfg(test)`) | WI-1 write path: public payload fields or non-test setter; guarantee gated, not exempted | coverage 1 (the structural gate) |
| 23 | Affected-test list incomplete (round-1 W10; extended this round — round-2 W8) | Blast radius: both surfaces grepped, derivation stated, `core_output_*` files added | blast radius |
| 24 | `load_checkpoint` positional restore (round-1 D4) | Tracked: WI-5 obligation 1 | WI-5 |
| 25 | **`refresh_equipment_caches` does not rebuild declarations/PortSlots — rev 2's safety claim was false (round-2 C1)** | WI-3 safety argument rebuilt on verified facts (live reads, no id in `PortDeclaration`, pre-step, what refresh *does* rebuild); the port-slot gap itself is tracked | WI-5 obligation 2 |
| 26 | **Trait write collides with the post-registration mutation guard, unengaged (round-2 C2)** | WI-3 decision 2: setter checks `is_initialized()`; Dwelling writes pre-`mark_initialized()`, pre-push (ordering verified sound) — **and the guard's true coverage stated (row 32), since the guard alone protects only EV and Battery** | coverage 5 + code review of the ordering |
| 27 | **"Next free id" specified two incompatible ways (round-2 W1)** | WI-3: one algorithm — monotonic never-reused counter, construction-initialized, advanced past every entering id | coverage 5 (explicit-then-auto sequence test) |
| 28 | **Dehumidifier alignment not expressible with the consolidated reader (round-2 W2)** | WI-2: tri-state reader contract (absent ≠ present-0); explicit 0 = unassigned | coverage 6 |
| 29 | **Trait method default-body question unstated (round-2 W3)** | WI-3: required, no default — compiler enumerates 57 sites; both default forms rejected on the record | compile |
| 30 | **`PyEquipmentDescriptor.id` is a public API value change, unlisted (round-2 W7)** | Blast radius: named, decided desirable, release-note entry | blast radius + release note |
| 31 | **Assembly-only scope never weighed (round-2 W6)** | Alternative 6: weighed, rejected on the class rule — **re-priced and re-stated at round 3's honest figures (rows 33, 36)** | Alternative 6 |
| 32 | **The guard is inert for 55 of 57 implementors; "stays blocked exactly as the guard intends" was false (round-3 C1)** | WI-3 decision 2 rewritten: guard coverage stated truly (EV + Battery only); macro guard forwarding added (row 35); exposure boundary verified (no public mutable handle — `equipment()` returns `&[...]`, `PyEquipment` holds snapshots, `remove_equipment` evicts first); **debug desync invariant at the snapshot** (`dwelling/mod.rs:5856-5869`) catches any post-registration id drift loudly | code review + the invariant (the desync is not drivable through any public API, so the invariant's proof is review, not a test) |
| 33 | **Trait-method cost stated as its inverse — 6 of 57 macro-delegated, not "most" (round-3 W1)** | WI-3 decision 1 + blast radius re-priced: ~51 hand-written one-line bodies + 6 macro-forwarded, two crates; Alternative 6 re-weighed at that price | the decision text |
| 34 | **`impl Equipment` blocks live in `hares-core` too (7 files); the derivation could not find compile-breaking stubs (round-3 W2)** | Blast radius: both crates named; derivation gains the third grep (`impl Equipment for`) | blast radius |
| 35 | **`delegate_equipment!` forwards no guard methods — guard-blind wrappers (round-3 W3, pre-existing broken window)** | WI-3 decision 2: the fix forwards `is_initialized`/`mark_initialized`/`unmark_initialized` in the macro — present-day effect zero (inner types override nothing), closed so it cannot open, with its inertness stated (row 41) | the macro-forwarding regression test (coverage item 12) |
| 36 | **Alternative 6's counterfactual overstated ("identical" failure) (round-3 W4)** | Alternative 6 restated: single defaulted add is addressable; collapse begins at 2+ adds among themselves; entrance-conditional invariant and Python `id == 0` also weigh — rejection re-carried on the honest version | Alternative 6 |
| 37 | **Entrance-2 invariant asserted but unenforced — rests on ~51 unverified bodies; the workspace's lifecycle harness unmentioned (round-4 C1)** | WI-3: entrance-2 **postcondition** (now the equality check with its own diagnostic, rows 43/47); population coverage is the exhaustive iteration (row 45), with `lifecycle.rs` as a supplementary role | coverage 5 (postcondition arm) + 12 (population iteration) |
| 38 | **Three new behaviours (guard error path, macro forwarding, debug invariant) with no coverage items (round-4 W1)** | Coverage item 12: the guard error path and macro forwarding each get a drivable test; the debug invariant stays review-proven for the public-API half, with the in-crate drivability observation recorded in WI-3 and judged marginal against the layered defence | coverage 12 + WI-3 |
| 39 | **Required-vs-default priced only on in-tree implementors; the trait is public and out-of-tree implementors are a product goal (round-4 W2)** | WI-3 decision 1: the out-of-tree half weighed on the record — required accepted (pre-1.0, no back-compat constraints, conscious break, deliberate no-op caught by the postcondition); erroring default re-weighed and rejected (buys nothing the postcondition doesn't, loses in-tree enumeration) | the decision text |
| 40 | **The debug assert is the weaker mechanism for a static threat; the guard-test pattern the workspace already uses three times was unweighed (round-4 W3)** | WI-3 decision 2: a guard test in the established `*_guard.rs` pattern scans for identity-setter call sites outside the authorized caller — existence-level, workspace-wide; the defence layered by threat | the guard's CI pass |
| 41 | **Two affected test files named only by category; the macro fix's present-day inertness unstated (round-4 D1/D2)** | The derivation names `tests/control_gate_guard.rs` (4 impl blocks) and `tests/workspace_gate_guard.rs` (3) as integration-test crates; decision 2 states the forwarding fix's present-day effect is zero | blast radius + WI-3 |
| 42 | **"Covering all ~51 hand-written bodies" overstated ~5× — the harness covers 9 types / 10 call sites, zero stubs; the growth rule names a non-existent function (round-5 W1)** | Coverage item 12 rewritten: population coverage is the exhaustive iteration (row 45); `lifecycle.rs` right-sized to a supplementary role with its stale header corrected; what is and is not covered stated exactly | coverage 12 |
| 43 | **The postcondition specified two non-equivalent checks in one sentence (round-5 W2)** | WI-3: one check — `descriptor().id == the assigned value`; equality subsumes non-zero (ids start at 1) and uniqueness (the counter never reissues); wrong-but-plausible writes rejected, closing the rogue-id path the counter cannot have advanced past | coverage 5 |
| 44 | **The Verification gate contradicted the Expected-shifts protocol and never mentioned it (round-5 W3)** | Verification: the suite gate qualified — green *under the protocol*; trace-written updates are pass-condition updates, unexplained shifts are failures, quiet edits are the forbidden non-fix | Verification |
| 45 | **The workspace declares the equipment population (`CANONICAL_EQUIPMENT_NAMES`, `registry.rs:17`; `registry_coverage.rs` iteration precedent) — unweighed (round-5 W5)** | Coverage 12: the setter contract iterates the declared population — membership auto-growing, exceptions explicitly listed; the hand-maintained harness kept only as a supplement, its drift (9 of the 63 canonical types) stated | coverage 12 |
| 46 | **The static guard scans a different AST target kind than its precedent — call sites vs impl-block definitions (round-5 W4)** | WI-3 decision 2: the guard characterized as a **heuristic** (its false-positive/false-negative modes stated), its reach stated (workspace `src`), and the authorization settled (out-of-tree pre-registration use is harmless — the postcondition governs whatever id is present) | the guard's CI pass |
| 47 | **The postcondition's diagnostic reused entrance 1's "unassigned id" message, misdirecting the implementer (round-5 D1)** | WI-3: entrance 2's own error — names the equipment, states the identity write did not take the assigned value, points at the setter implementation | coverage 5 |
| 48 | **Alternative 6 was priced once; the price rose three times without the comparison being re-run (round-5 D2)** | Alternative 6 re-priced with the current full inventory (trait method, guard forwarding, counter, postcondition, debug invariant, guard test, population test, coverage arms); the decision explicitly re-run at that price and standing on the class argument | Alternative 6 |

Also corrected, document-level: `rename`'s line (168, not 184 — round-2 D1);
"both invoked at assembly" overstated (round-2 D3); "exactly one charging
signal" reframed as path exclusivity (round-2 D2); the affected-test derivation
extended to three greps (round-3 W2); the count of tracking initiatives decided
at two (round-3 D1); and revision-narration phrasing removed from the work
items so they read as instructions, with traceability confined to this matrix
and the ledger (round-2 D4, round-3 D2).

## Verified facts and corrections

**Verified this round (round 3)** (each checked against the code before
writing, including the splits, not just the totals — the round-2 failure mode
was verifying a count and not its adjacent share): the guard's true coverage —
`is_initialized()` defaults `false` (`lib.rs:402-405`), overridden in
production only by Battery (`battery/mod.rs:1946`) and EV (`ev/mod.rs:1912`),
the two it was built for (T-0534); `delegate_equipment!` invoked six times
(four heat-pump heater newtypes `heater.rs:459-462`, `GasGenerator`/
`FuelCell` `generator.rs:2064-2065`) and forwarding **neither** guard method
(zero matches in `macros.rs`); `impl Equipment for` blocks in 25
`hares-equipment` files and 7 `hares-core` files (e.g. the guard-implementing
stub `dwelling/mod.rs:7662`); the exposure boundary — `Dwelling::equipment()`
returns `&[Box<dyn Equipment>]` (`dwelling/mod.rs:3080`, immutable),
`PyEquipment` holds snapshot copies only (`py_equipment.rs:1070-1075`),
`remove_equipment` evicts before returning the box; and the desync mechanics —
the end-of-step snapshot keys `equipment_core` through `equipment_id_by_name`
(`dwelling/mod.rs:5856-5869`) while equipment-side consumers read
`descriptor().id` live.

**Verified this round (round 4)** (checked against the code before
writing): the lifecycle harness (`crates/hares-equipment/tests/lifecycle.rs`)
with its documented growth rule ("Each new equipment type must add a
`test_equipment_lifecycle` call here") — the established home for the
per-implementor trait contract the new setter creates; the three guard
tests (`si_guard.rs`, `control_gate_guard.rs`, `workspace_gate_guard.rs`)
and the workspace guard's stated premise ("any sibling crate can implement
it"); the wrapped inner types (`Generator`, `HeatPumpHeaterCore`) override
no guard method — the macro forwarding's present-day effect is zero, both
the bypass and its closure prospective; and the two integration-test crates
(`tests/control_gate_guard.rs`, 4 impl blocks; `tests/workspace_gate_guard.rs`,
3).

**Addressed this round (round 4)** — gaps rather than false claims (this
critique's ledger independently confirmed revision 4's load-bearing
claims: the exposure boundary, the impl arithmetic, the guard facts,
Alternative 6's pricing, the debug invariant's site, and the one-line-body
achievability across six structurally diverse implementors): the
entrance-2 postcondition closes the unenforced invariant (round-4 C1); the
lifecycle harness entry and the three drivable machinery arms close the
coverage gap (round-4 W1); the out-of-tree population is weighed in
decision 1 (round-4 W2); the static guard joins the layered defence in
decision 2 (round-4 W3); the two integration-test files are named and the
macro fix's inertness stated (round-4 D1/D2).

**Verified this round (round 5)** (checked against the code before
writing — the shares this time, not just the existences):
`assert_equipment_lifecycle` (`lifecycle.rs:59`) with **10 call sites
across 9 lifecycle tests** (five of them water heaters); the quoted
growth-rule name `test_equipment_lifecycle` has **zero occurrences** —
stale documentation; `CANONICAL_EQUIPMENT_NAMES` (`registry.rs:17`, **63
entries**, comments stripped — counted exactly, not estimated, per this
round's own lesson) and the exhaustive-iteration precedent `registry_coverage.rs`,
which iterates the constant to prove registration; and the registry
factories' infallibility (invalid configs defer errors to init) — the
basis for constructing every canonical type in the population test
without per-type fixtures.

**Corrected this round (round 5)** — including the ledger's own failure,
recorded as such: round 4's "verified" entry checked the harness's
*existence and header*, not its *share* — the round-2 lesson ("verifying
a count and not its adjacent share") repeated, this time as
existence-not-share; the resulting "covering all ~51 hand-written bodies"
claim (round-5 W1) and the stale function name quoted as an instruction
in the blast radius are both corrected; the postcondition's
two-checks-in-one-sentence is disambiguated to the equality check
(round-5 W2) with its own diagnostic (round-5 D1); the Verification gate
gains the protocol qualification it was missing (round-5 W3); the static
guard is recharacterized as a heuristic with its reach and authorization
settled (round-5 W4); and Alternative 6 is re-priced and the decision
explicitly re-run (round-5 D2).

**Verified in round 2 and standing** (re-checked where round 3 touched them):
`refresh_equipment_caches` (`dwelling/mod.rs:3342`) rebuilds the name→id map,
execution order, dispatch targets, and output schema — and no `PortSlots`;
the mutation-guard contract (`lib.rs:402-463`; `mark_initialized()` before the
push at `dwelling/mod.rs:3259-3262`, and nothing sets the flag except
`mark_initialized` itself — the round-3 critique's ledger verified the
ordering sound); `fn rename` at `lib.rs:168`; `apply_equipment_overrides`
merging `root["all"]`/`root["*"]` then `root[name]` (`conversions.rs:843-853`)
over a base that already contains the injected id — the basis for the
reserved-key check on the override delta; `PyEquipmentDescriptor.id`
(`py_equipment.rs:891-894`); the direct `EquipmentId` test sites
(`core_output_regressions.rs:291`, `core_output_invariants.rs:163,633,651`).

**Confirmed by this round's critique ledger**: `assign_instance_names` called at
parse (`hpxml/equipment.rs:65`) and assembly (`dwelling/mod.rs:2193`); the
20-struct inventory; `DwellingCheckpoint.equipment_states: Vec<Vec<u8>>`
(`checkpoint.rs:50`); no `save_state` impl serializes the descriptor (checkpoint
behavior byte-identical); `PortDeclaration` carries no equipment id; no test
asserts `EquipmentId(0)`; no actor holds a stale cached id across entrance-2
assignment.

**Corrected in round 3 after failing verification** (revision-3 claims the
critique refuted, confirmed by my own re-check before correcting): the
"stays blocked exactly as the guard intends" sentence (round-3 C1 — the guard
is real for two of 57 types; replaced by the true coverage, the macro
forwarding fix, the verified exposure boundary, and the debug desync
invariant); the "most macro-delegated" cost figure, stated twice (round-3 W1 —
six of 57; re-priced in decision 1, the blast radius, and Alternative 6); the
blast radius's single-crate trait change (round-3 W2 — two crates, and the
derivation gained the third grep); the macro's guard forwarding, previously
assumed (round-3 W3 — verified absent, and the fix now forwards the guard
methods); Alternative 6's "identical failure" counterfactual (round-3 W4 —
restated at its honest narrower scope and honest price).

**Corrected in round 2 after failing verification** (revision-2 claims, all
re-checked before correcting): the `refresh_equipment_caches` "rebuilds
declarations" claim (round-2 C1); the "a test cannot distinguish a
`cfg(test)`-gated write" premise (round-2 C3 — integration tests link the
dependencies without `cfg(test)`; the gate is the discriminator); `rename`
cited at `:184` (round-2 D1); "the only other id consumer" (round-2 W7 — the
Python getter is a user-facing consumer); the "complete list" of affected
tests (round-2 W8 — replaced by a derived list with its derivation stated).
Adopted from the critique's own correction of its round-1 W9: the cross-crate
silent-no-op characterization was overstated — it is a compile error, and the
real hazard is narrower.

## Corrections from the fix phase (this section supersedes the claims it names)

1. **The plan's "both entrances" claim was wrong.** Work item 3 and the
critique's absence sweep stated there are exactly two equipment-push paths
into a dwelling (`build_from_blueprint` assembly and `add_equipment`);
`Dwelling::replace_equipment` (`dwelling/mod.rs`, Python-reachable via
`py_dwelling.rs::replace_equipment`) is a third — it swaps a new equipment
into the vector by name. The implemented invariant is entrance-uniform
across all three: unassigned ids never survive (auto-assigned), explicit
collisions are rejected naming both equipment, the identity write is
verified, and the replacement is `mark_initialized`-guarded like any other
registered equipment (a pre-existing registration-guard gap in that
function, fixed while touching it). The critique's attack 13 ("no second
equipment-push path beyond the two entrances") is the verification that
missed it.

2. **WI-5's two tracking obligations were discharged by fixing, not
tracking, on explicit operator instruction ("no deferrals").** Both
pre-existing defects are fixed end to end in this change: checkpoint
restore is identity-keyed (`DwellingCheckpoint` format v8; a spec reorder
between save and restore is a typed error naming both sides, where the
positional `zip` silently loaded one equipment's state into another), and
port slots are rebuilt for post-construction equipment (monotonic pre-step
rebuild in `refresh_equipment_caches`; post-step adds with declarations the
frozen table cannot satisfy, or zones unknown to the environment model, are
rejected loudly at the entrance). The two initiative stubs the fix phase
first created were withdrawn and removed by the operator on 2026-09-23.

3. **The same-class audit found one more member the plan did not list.**
The positional `equipment_column_map`/`actor_column_map`, zipped with the
live equipment/actor vectors in `record_step`, desynced after any mid-run
removal/reorder once rows had been recorded — silently misattributed
output values. `refresh_equipment_caches` now re-derives both maps against
the frozen column index whenever rows exist. Pinned by a behavioral test
proven to fail with the fix neutered.

4. **A second audit pass (replace entrance, actor wiring) found two more.**
Actor equipment bindings never re-resolved on identity changes (user-added
actors never resolved at all; an actor surviving `replace_equipment` kept
the evictee's id) — fixed with a trait-level `Actor::resolve_equipment_id`
called on every identity refresh. And equipment entering mid-run had no
`equipment_core` entry until its first step completed — fixed by seeding
the refresh with each new equipment's current `core_output()`. Both pinned;
the rebind pin is proven by neutering (fails "last soc = 0" without the
resolve loop).
