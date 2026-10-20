---
retro_outcome: sound
---

# Retrospective — I-02 (envelope_loads_kwh components don't balance)

## The bug, restated

`opaque_solar_lwr_kwh` reported the gross radiant flux absorbed at the
exterior skin of the opaque envelope (full irradiance × area × absorptance,
injected as a boundary condition on each wall's own exterior RC node) inside
a struct documented as net "thermal loads imposed on the conditioned zone."
The true net quantity — interior-face convection into zone air — was already
computed every timestep but never wired into the reporting. Result: a
+121 MWh/yr phantom gain on a 15 MWh/yr home. The RC solve was correct all
along; only the reported semantics were wrong. This is the "face confusion"
defect class: a boundary-condition quantity presented as a delivered load.

## What I changed (the defence, and only the defence)

Three files. Nothing else — no behaviour, no refactor, no drive-by.

1. **`tests/envelope_opaque_loads.rs`** — added
   `envelope_loads_field_count_pins_budget_membership`. This is the
   mechanism, and it closes the one hole the fix phase left open: the
   budget-closure test (`envelope_load_budget_is_signed_summable_and_closes`)
   enumerates the ten summable fields *by name*, so a field added to
   `EnvelopeComponentLoadsKwh` would escape the balance assertion silently —
   which is precisely how this defect class re-enters the struct (the fix
   phase itself added two fields to it). Rust has no field reflection, so
   the test counts `_kwh:` members in the struct's `Debug` form and pins the
   count at 11; adding a field fails it with instructions to either wire the
   new field into the budget closure and per-field integral tests, or
   document it inline as a gross non-summable exception the way
   `interior_lwr_kwh` is. What it makes impossible: a new reported
   envelope-load quantity that no balance invariant ever sees. Verified:
   `cargo test -p hares-core --test envelope_opaque_loads` → 7 passed, 0
   failed (ran after the addition).
2. **`.shipwright/constitution.md`** ("Your project's own rules") — the
   judgement half no mechanism can carry, recorded where the next author of
   a metric will read it: *"Every reported physical quantity is truthful
   about the face it is measured at"* (gross/net distinguishable by name;
   non-summable gross metrics carry an explicit caveat) and *"A summable
   budget contract is pinned by a balance invariant, not by the sum of its
   parts."* My first pass at this edit accidentally absorbed the section's
   placeholder boilerplate into the second bullet; caught on the re-read and
   corrected.
3. **This file** — the record.

Note on the working tree: `git status` shows a wholesale rewrite of
`.shipwright/constitution.md` and large diffs across the crates. Those are
pre-existing working-tree state from the fix phase and concurrent agents,
not my changes; per the operating rules I left them untouched. My diff to
the constitution is the two bullets above.

## Where the same shape can still appear

Searched `crates/hares-io/src/output/metrics.rs` for every `pub *_kwh`
field (20 hits) and for `latent`/`gross`:

- **Electrical side**: now explicitly face-named — `net_energy_kwh`,
  `gross_consumption_kwh`, `gross_pv_generation_kwh` — with the
  net == gross_consumption − gross_pv identity pinned by tests
  (`metrics.rs:1636-1726`). Clean.
- **Envelope struct**: the only gross member is `interior_lwr_kwh`,
  carrying the documented "not summable" caveat. Clean, and now tripwired
  against silent field additions.
- **Humidity (latent vs sensible at the coil face) and ducts (loss at the
  duct wall vs load at the register)**: named in the hardening program's
  lessons taxonomy as the two places this shape is *expected* to appear,
  but no latent-face metric exists in the reported structs today (no
  `latent` field in `metrics.rs`). There is no live instance to fix; when
  those metrics arrive, the constitution rule and the field-count tripwire
  pattern are the defences that greet them.

## The defence vocabulary this initiative adds

- **Face confusion** — the class name; a boundary-condition flux presented
  as a delivered load. Its electrical twin (net-at-the-meter vs.
  load-behind-the-meter) was found and fixed in the same sweep.
- **Pin the contract, not the path** — the reproduction/gate tests assert
  the physical invariant against independently-computed quantities, so they
  survive refactors and catch sibling-component instances.
- **Enumerate-then-tripwire** — a balance test that names its members must
  ship with a count/membership tripwire, or the next field added to the
  struct reopens the class it closed.
- **A renamed-away field gets an absence pin** — `assert not hasattr(env,
  "opaque_solar_lwr_kwh")` keeps the misleading name from returning.

## Diagnosis divergence

One, and it is instructive. The RCA's first pass concluded the
`*_heat_gain_w` fields were dead because they are initialized to `0.0` at
construction — a plausible-looking reading (why initialize an accumulator
you never use?) that would have reframed the bug as "the net quantity does
not exist" and pushed the fix toward building it from scratch. What made
the wrong diagnosis look right was exactly that: initialization sites are
visible in one file, accumulation sites are elsewhere
(`stepping.rs:815-831`), and the negative claim "nothing writes these" felt
established by the constructor alone. Grepping the accumulation sites
showed the fields live and correct, shrinking the fix from "implement a new
quantity" to "wire an existing one." The constitution already names this
("a negative claim is the hardest to support"); this is a confirmed
instance worth citing when the rule is next questioned.

## Verdict

The fix is solid, the class was swept with each instance pinned, the review
found nothing blocking, and the residual hole this retrospective found —
silent struct-field additions escaping the balance invariant — is now
closed by a mechanism, not a note. Sound.
