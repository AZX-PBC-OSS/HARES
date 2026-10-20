# Flow definitions

Each YAML file in this directory defines a **flow recipe** — the ordered set of
steps that orchestrates an initiative from start to finish. Recipes specify
which agent role runs each step, what each step must produce, and how steps
route to one another. All shipped recipes are `engine_version: 2`.

## Two kinds of recipe file

- **Initiative recipes** (`feature.yaml`, `fix.yaml`, `review.yaml`,
  `research.yaml`, `spec.yaml`, `chore.yaml`) declare a `kind` and drive a
  whole initiative. They are the recipes an initiative is created from.
- **Child recipes** (`review-area.yaml`, `ticket.yaml`) declare no `kind` and
  are never created as initiatives. A `type: subflow` step in a parent recipe
  names one via `subflow.recipe`, and the engine runs one child flow per
  artifact matched by the subflow's `for_each.pattern` — one review area per
  `review-areas/*/area.md`, one ticket per `tickets/*/ticket.md`. A child
  recipe is validated as part of its parent's load, gated by the parent's
  `for_each.schema` (the child-input contract).

## Schema (summary)

```yaml
engine_version: 2
kind: <feature|fix|review|research|spec|chore>   # omitted on child recipes
description: "<human-readable summary>"
recipe_summary: "<comma-separated step list>"
start:
  next: <step-id>          # optional; defaults to the first declared step
steps:
  - id: <string>
    label: "<display name>"
    description: "<what this step accomplishes>"
    required: <true|false>
    skippable: <true|false>          # optional
    type: <agent|subflow>
    agent_role: <role>               # agent steps
    charter: <charter-name>          # optional
    challenge: <true|false|<name>>   # optional; true derives from the role
    includes: [<include-name>, ...]  # optional
    concerns: [<concern-name>, ...]  # optional
    output:
      file: <artifact.md>            # or pattern: '<glob>'
      schema: { <inline JSON Schema the artifact's frontmatter must satisfy> }
    must_pass: [{ from_key: <key> }] # optional gates
    must_fail: [{ from_key: <key> }] # optional gates
    edges:
      - when: { field: <routing-key>, eq: <value> }
        next: <step-id>
        max_traversals: <n>          # loop-back budget
        inject: "<context added to the re-run step's prompt>"
    default_next: <step-id | done | halt>
    side_effects: [...]              # optional
```

A `type: subflow` step declares `subflow: { recipe, for_each: { pattern,
schema }, max_concurrency, on_child_failure, allow_empty }` instead of an
agent role, plus an optional `output:` of `pattern:` entries — the per-child
deliverable contract. Agent steps and semantic validation (routing
exhaustiveness, reachability, per-type field contracts, reference existence)
are enforced at recipe load; the full vocabulary lives in the application's
recipe validator, which is the authoritative schema.

## Required, optional, and skippable

The two fields are **one ladder of permission for a step not to run**, not two
independent switches. An agent removing a step is an implicit skip, so
`required: false` grants a superset of what `skippable: true` grants:

| Flags | Who may cause the step not to run |
|---|---|
| `required: false` | The **flow advisor/customization agent** may drop it from an adapted recipe, **and** the operator may skip it — the loosest state |
| `required: true` + `skippable: true` | The agent must keep it; the **operator** may still choose to skip, making omission a deliberate, visible act |
| `required: true` alone | Nobody — it always runs |

The effective rule is therefore `skippable === true || !required`: not
required implies skippable.

Authoring rule: **do not let the agent or the operator skip a step where
skipping makes no sense.** Reserve `required: true` without `skippable` for
steps whose absence makes the flow's output meaningless — implementation and
review. Prefer `required: true` with `skippable: true` for judgement-heavy
steps: the agent should not silently drop them, but the operator keeps an
escape hatch. Reserve `required: false` for steps an adapting agent may
genuinely decide a given initiative does not need.

## Workspace copies

Default recipes ship with the application and are seeded into a workspace's
`.shipwright/flows/` on first open (copy-if-absent). A workspace copy that
has never been modified is upgraded when the bundled recipe changes; a
modified copy is preserved and must be updated by hand or by re-running
**Reinitialize**, which resets seeded files to the bundled defaults.
