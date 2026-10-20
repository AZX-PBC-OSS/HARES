---
description: "GitHub Actions workflow correctness and cost: job dependency ordering so a cheap gate fails before an expensive lane starts, platform, job-count and runner-size choices priced against the published per-minute rates and the per-job one-minute rounding floor, timeout-minutes on every job, concurrency and cancellation, shell exit-code handling inside run steps, and jobs whose green result is not evidence. Establishing a project's gates at all is the project bootstrap concern and the configuration of an individual linter or checker is the tooling config concern; this file is about the workflow that runs them."
triggers:
  file_globs:
    - "**/.github/workflows/*.yml"
    - "**/.github/workflows/*.yaml"
    - "**/.github/actions/**/action.yml"
    - "**/.github/actions/**/action.yaml"
  content_patterns:
    - "runs-on:\\s*[\"']?(macos|windows)"
    - "continue-on-error:"
    - "\\$\\(.*\\|\\s*(tr|head|tail|grep|sed|awk|cut)\\b"
    - "^\\s*set -[eux]+\\s*$"
---

# GitHub Actions

A workflow is billed per minute, rounded up, per job, and it is the only thing standing
between a defect and `main`. Both properties fail quietly. A workflow that costs three times
what it needs to still goes green, and a workflow whose gates cannot fail also goes green.
This file is about the graph, the runners, and the shell inside `run:`,
not about which checks are worth having, which is the project bootstrap concern, nor about what an individual
linter enforces, which is the tooling config concern.

## Read the current graph before changing it

Write out the `needs:` edges, the `runs-on:` of every job, and each job's measured healthy
duration from a recent green run before adding or moving anything. The cost of a workflow is
concentrated in one or two lanes, and which ones is not guessable, so you need the run summary.
Without those numbers every argument below is a preference rather than a measurement.

## Every job is billed at a one-minute minimum

GitHub's pricing reference states it plainly: "GitHub rounds the minutes and partial minutes
each job uses up to the nearest whole minute." The rounding is **per job, per run**, not per
workflow, and three consequences follow that people routinely get wrong.

**A sub-minute job already costs a full minute.** Moving it to a larger runner cannot save
money, because it pays a higher rate for the same rounded minute. Sizing up is advice for lanes
measured well above the rounding floor, never for fast ones, where the only thing bought is
latency.

**Job count is a floor cost.** N jobs cost at least N billed minutes even if every one of them
finished instantly. Splitting work into more, smaller jobs therefore costs money for its own
sake, on every push, forever, and consolidating several sub-minute jobs into one is a real
saving. This is the counterweight to fine-grained fan-out, and it is what gets missed when a
workflow is decomposed for a tidier dependency graph rather than for a measured reason. A
workflow with eleven jobs, five of them sub-minute, pays at least eleven billed minutes per
run; those five are about 45 percent of the job count and a fixed floor on every push
regardless of what changed.

**It sharpens the gating argument rather than weakening it.** Gating buys almost nothing on the
cheap tier, where the jobs cost a rounded minute whether they run or not. It buys a great deal
on the expensive tier, where a lane genuinely runs for tens of minutes. So the rule is not
"gate everything": gate the lanes whose duration is far above the rounding floor, and keep the
cheap tier small in job *count* rather than splitting it further.

## How many jobs: a balance of three things, not one

"Fewer jobs is cheaper" taken to its conclusion is bad advice, and so is fan-out for a tidier
graph. Job count is decided by three axes together: correctness of the signal, legibility of
the result, and cost plus wall-clock. Optimising any one alone lands in another's failure
mode. Too few jobs gives coarse signal and forfeits parallelism the runner would have given you
free; too many pays the per-job floor on every push forever and adds a runner spin-up to the
critical path for each one.

Concrete guidance:

- **Bundle several very fast checks into one job.** Each separate job pays the one-minute floor
  and its own spin-up. Putting a handful of sub-minute checks in one job so it lands nearer a
  minute is close to free and gives the same signal.
- **A lane that genuinely runs two minutes or more earns its own job.** Above that rough
  threshold it can be gated, sized, timed out and retried independently, and it parallelises
  against its siblings on real wall-clock rather than against the rounding floor.
- **Split when the failures are legible differently.** One job bundling everything gives a
  single red X and makes someone read the log to learn whether it was lint, types or a test.
  Separate jobs name the failure in the run summary. That is worth a billed minute when the
  checks fail for genuinely different reasons and are fixed by different people, and it is not
  worth it when they are three flavours of the same static pass over the same code.

## Gate cheap before expensive

An ungated workflow starts every job at once, so a lint error that a one-minute job already
knows about still pays for the slowest lane in full.

Measured in a real repository: eleven jobs, all ungated. The e2e lane took 36 minutes and
accounted for 56 percent of the workflow's total cost, while lint, typecheck, gitleaks, build
and the security scan each finished in under a minute. Gating `test` on `lint` and `typecheck`,
and the expensive lanes on `test`, cut the critical path from 36 minutes to about 20 and made
the workflow cheaper at the same time. That is the part worth internalising: this is not a
speed-versus-spend trade. The expensive lane stops running on commits where a sub-minute job
already has the answer, so latency and cost both fall.

The ordering rule is the same one at every level: whatever is cheapest and most likely to fail
runs first, and nothing pricier starts until it passes. Note where the saving actually came
from: the 36-minute lane, not the sub-minute ones. Gating those against each other would have
moved a rounded minute around and bought nothing.

## Do not create an edge that costs more latency than the failure it prevents

A `needs:` edge is not free. It serialises two jobs, and the second one's start is delayed by
the first one's full duration on every green run, which is most runs.

In the same repository, e2e was deliberately **not** gated on integration. That edge would have
added 9 minutes of latency to every successful run in order to guard against a failure the unit
lane usually catches first. Those two lanes run in parallel instead.

So the question for each edge is: how often does the upstream job fail in a way the downstream
job would not have caught anyway, and what does the wait cost on the runs where it passes? An
edge that guards a rare failure that something cheaper already catches is a permanent tax paid
for an occasional saving.

## Platform ordering: Linux, then Windows, then macOS

Runner rates differ by close to an order of magnitude, so platform order is a cost decision
before it is anything else. Read the current numbers from GitHub's published rates rather than
from memory or from this file. They change, and the arithmetic below is only as good as its
inputs:

<https://docs.github.com/en/billing/reference/actions-runner-pricing>

As read from that page on 2026-08-30, per minute, x64:

| Runner | Rate | Relative to standard Linux |
| --- | --- | --- |
| Linux 2-core (standard) | $0.006 | 1x |
| Linux 4-core (larger) | $0.012 | 2x |
| Linux 8-core (larger) | $0.022 | 3.7x |
| Windows 2-core (standard) | $0.010 | 1.7x |
| Windows 4-core (larger) | $0.022 | 3.7x |
| macOS 3-core or 4-core (standard) | $0.062 | 10.3x |
| macOS 12-core (larger) | $0.077 | 12.8x |

A minute spent on macOS costs what ten minutes on Linux cost. So anything that can fail on
Linux must fail on Linux, before a Windows runner starts, and the whole Windows phase must
pass before a macOS runner starts. `.github/workflows/ci.yml` in this repository is the
exemplar: `static` (ubuntu, 15m) then `check-ubuntu` (ubuntu, 20m) then `build-and-e2e`
(ubuntu, 30m) then `check-windows`, `windows-e2e`, `package-windows` (windows) then
`check-macos`, `macos-pty-smoke`, `package-macos` (macos). A break anywhere stops the chain
before the next, pricier platform ever starts a runner.

The trade-off is real and should be stated when you make it: a fully serialised chain has a
longer wall-clock time on a green run than a concurrent matrix. That is the cost of not paying
for Windows and macOS concurrently on a commit that Linux alone would have failed.

Keep genuinely platform-independent work on one platform. Static analysis does not differ
between operating systems, so running it three times buys nothing but three bills.

## `timeout-minutes` on every job

A job with no `timeout-minutes` inherits GitHub's 6 hour default. A real e2e job stopped
emitting progress and then ran for 1 hour 53 minutes before a human noticed and cancelled it.
Left alone it would have burned six hours of runner time while holding the pipeline, and on a
gated graph everything downstream of it waits that whole time. The bound is both a spend
control and a liveness control.

Size each bound at roughly 2 to 2.5x the job's measured healthy runtime. Two cautions:

- A bound derived from an unhealthy baseline understates the problem. If the run you measured
  was itself the stalled one, or was slowed by a fault you have since fixed, you have encoded
  the fault into the budget.
- A bound tight enough to trip on a slow-but-healthy run converts an infrastructure hiccup into
  a red build that gets blamed on the code. Where a platform is known to be slower at the work
  (Windows on I/O-heavy suites), say so in a comment and give it the wider bound deliberately.

## Runner sizing is a cost decision, not only a speed one

The instinct is that a bigger runner buys speed at a price. It frequently buys both, because
billing is per minute: if the rate rises by less than the duration falls, the larger runner is
cheaper as well as faster. This only applies above the one-minute rounding floor. For a job
already finishing in under a minute, a larger runner is strictly more expensive for the same
billed minute.

Measured: moving an e2e lane from the standard 2-core runner to a larger 4-core runner made it
2.6x faster and 42 percent cheaper on that account's rate card. The 8-core option was rejected:
it costs more than 4-core for a marginal gain, because that lane's parallelism was bounded by
something other than core count, so the extra cores sat idle at a higher rate.

Two things follow. First, redo this arithmetic against the rates above and your own account's
rate card rather than copying the numbers; at list price a larger 4-core Linux runner is 2x
standard, which needs a duration drop below half to break even. Second, the ceiling is set by
what the lane is actually bounded by. Measure the speed-up on the specific job before sizing up
again; cores you cannot use are pure cost.

## `set -o pipefail` wherever a pipeline's exit code matters

Under `bash -e`, a pipeline's exit status is its **last** command's. The failure is silent:

```console
$ bash -e -c 'digest="$(false | tr -d q)"; echo "exit=$? digest=[$digest]"'
exit=0 digest=[]
```

A real release workflow piped `docker buildx imagetools inspect` into `tr` to extract an image
digest. Had the inspect failed, the step would have exited 0 with an empty digest and published
that empty value to a release asset another repository consumes.

- Set `set -euo pipefail` at the top of any multi-line `run:` block, or configure
  `defaults.run.shell: bash` and write it into each script. GitHub's default shell for `run:`
  on Linux and macOS runners already applies `-e` and `-o pipefail`, but an explicit
  `shell: bash` **replaces** that with `-e` only, so the step that names its shell is the one
  that loses pipefail.
- A shape check on the extracted value is a **separate** defence, not a duplicate one. pipefail
  catches a command that failed; it does nothing about a command that succeeded and returned
  something unusable. Anything captured from a pipeline and then published, tagged, or compared
  should be validated against its expected shape before use, and the step should fail loudly
  when it does not match.

## A job name must not imply coverage it does not have

A job called `build` that builds a Python wheel, in a project whose deployment image is only
built at release time, tells you nothing on a pull request about whether the artifact that
actually ships compiles. The name says the build is covered. The graph says otherwise, and
nobody reads the graph.

Name a job for what it verifies. If the name is broader than the coverage, either narrow the
name or widen the job.

## Steps and jobs that never run are not gates

Every one of these produces a green check with no evidence behind it:

- Tests behind an environment variable or secret that nothing in the workflow sets. The step
  runs, skips everything, and exits 0.
- A job whose `if:` condition never fires: a branch name that no longer exists, an event type
  the workflow is not triggered on, a `github.actor` check that no real actor satisfies. A
  skipped job is not a failed job, and a skipped required check does not block a merge.
- `continue-on-error: true`, which makes a step's failure invisible to the job's result.

`continue-on-error` has one honest use: a diagnostic probe whose output you want on every run
and whose failure genuinely means nothing about the code. When you use it that way, say so in a
comment at the step, and make sure the real assertion lives in a step that can fail.

After changing a workflow, check the run: which jobs executed, which were skipped, and whether
the steps you believe are enforcing something actually ran.

## Pin third-party actions to a SHA

`uses: some-org/some-action@v3` resolves a mutable tag at run time. Whoever controls that tag
controls a step running in your workflow, with your `GITHUB_TOKEN` and whatever secrets the job
exposes. Pin third-party actions to a full commit SHA with the version in a trailing comment,
and let Dependabot move the pin. Actions in the same repository (`uses: ./.github/actions/...`)
are already pinned by the checkout.

Set `permissions:` explicitly, at the workflow level and narrowed per job where one job needs
more than the rest. The default token grant is wider than most jobs need.

## `concurrency` and cancellation

A `concurrency` block with `cancel-in-progress` stops a superseded run rather than paying for a
result nobody will read. It also has a failure mode of its own: a lane slower than the interval
between pushes may be cancelled every single time and therefore never report. In one repository
a push cancelled an in-flight e2e job on three consecutive occasions. That is correct behaviour
by the rule, and yet that lane produced no result at all.

So scope the group to what you actually want deduplicated. Keying on `github.ref` shares one
group across every push and every PR synchronize on a branch; keying on the pull request number
with a fallback to the commit SHA, and cancelling only on `pull_request` events, keeps the
deduplication for PR updates while giving every push its own group that nothing supersedes.
Check the recent run history of your slowest job for a string of `cancelled` conclusions. That
is what this failure looks like, and it looks like nothing at all in the workflow file.

## Footguns

- **No `timeout-minutes`** means a 6 hour default, and a hung job holds every downstream job for
  all 6 hours.
- **`shell: bash` on a `run:` step drops `pipefail`** that the default shell would have applied.
- **A pipeline's exit code is its last command's.** `$(cmd | tr ...)` succeeds when `cmd` fails.
- **A skipped required check does not block a merge.** Skipped is not failed.
- **`continue-on-error: true` on the step that does the verifying** makes the job's green
  meaningless.
- **Every job is rounded up to a whole billed minute**, so N jobs cost at least N minutes per
  run and a sub-minute job cannot be made cheaper by any runner choice.
- **Splitting a fast job in two doubles its billed cost** and buys nothing unless the two halves
  fail for different reasons that different people fix.
- **An ungated expensive lane pays in full for a failure a sub-minute job already found.**
- **A `needs:` edge you added for safety** is paid on every green run; price it against how
  often it actually catches something first.
- **macOS minutes cost roughly 10x Linux minutes.** Anything reachable on Linux belongs on
  Linux.
- **A larger runner can be cheaper**, and an oversized one is pure cost when the lane is bounded
  by something other than cores.
- **A job named more broadly than what it checks** is read as coverage it does not provide.
