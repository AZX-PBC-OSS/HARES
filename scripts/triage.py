#!/usr/bin/env python3
"""
Triage review findings into tickets.

- Bugs/defects/missing-implementation     → T-0001+
- Improvements/readability/nice-to-have   → T-1000+

Priority order (default):
  1. hpxml, weather           — inputs: garbage in, garbage out
  2. envelope, physics-*, *-deep  — core simulation engine
  3. equipment-*, wiring, control — equipment models & wiring
  4. agents, core, architecture   — actors, dispatch, architecture
  5. output, solver               — output & observability
  6. fleet-python, py-*, infra    — infrastructure & utilities

Usage:
    python scripts/triage.py                    # all reviews, priority order
    python scripts/triage.py --category hpxml   # just one category
    python scripts/triage.py --dry-run          # preview order
    python scripts/triage.py agents-02          # single review
"""
import argparse, json, os, subprocess, sys, time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
REVIEWS_DIR = REPO / "docs" / "reviews"
MANIFEST_DIR = REPO / "scripts" / "reviews"
TICKETS_DIR = REPO / ".shipwright" / "initiatives" / "I-02" / "tickets"
OPENCODE = os.environ.get("OPENCODE_BIN", "opencode")
MODEL = os.environ.get("OPENCODE_MODEL", "")
TIMEOUT = int(os.environ.get("TRIAGE_TIMEOUT", "600"))

# Priority: lower = triaged first. Work from core outward.
#  1 = inputs (garbage in, garbage out)
#  2 = core simulation (envelope, physics, types, constants)
#  3 = equipment + wiring (HVAC, WH, loads, DER, ports, control)
#  4 = actors + dispatch (agents, core engine loop, control)
#  5 = output + observability (telemetry, output, invariants)
#  6 = infrastructure + utilities (fleet, python, HELICS, scripts, build)
CATEGORY_PRIORITY = {
    # tier 1 — inputs
    "hpxml": 1, "weather": 1,
    # tier 2 — core simulation
    "envelope": 2, "envelope-rc-mat": 2, "types-physics": 2,
    "physics-constants": 2, "solar-deep": 2, "air-properties": 2,
    "water-mains": 2, "infiltration-deep": 2, "ashrae152-deep": 2,
    # tier 3 — equipment + wiring
    "equipment-hvac": 3, "equipment-wh": 3, "equipment-loads": 3,
    "equipment-der": 3, "hvac-config": 3, "wh-logic": 3,
    "der-catalog": 3, "pv-sizing": 3, "equipment-util": 3,
    "wiring": 3, "control-deep": 3, "cross-cutting": 3,
    # tier 4 — actors + dispatch
    "agents": 4, "core": 4, "core-deep": 4, "architecture": 4,
    "config-io": 4, "tariff-deep": 4,
    # tier 5 — output + observability
    "output": 5, "solver": 5,
    # tier 6 — infrastructure
    "fleet-python": 6, "py-binding": 6, "py-companion": 6, "python-gaps": 6,
    "infrastructure": 6, "build": 6, "scripts": 6,
    "defaults-data": 6, "tests": 6,
}


def _load_categories() -> dict[str, str]:
    """Map review ID → category from manifests."""
    mapping = {}
    for mf in sorted(MANIFEST_DIR.glob("*.json")):
        with open(mf) as f:
            data = json.load(f)
        cat = data["category"]
        for r in data.get("reviews", []):
            mapping[r["id"]] = cat
    return mapping


def find_completed_reviews():
    """Return list of review .md files that exist (completed), sorted by priority."""
    cat_map = _load_categories()
    files = []
    for md in sorted(REVIEWS_DIR.rglob("*.md")):
        if "_logs" in md.parts or "_test" in md.parts:
            continue
        if md.stat().st_size > 200:
            stem = md.stem
            review_id = "-".join(stem.split("-")[:2])
            cat = cat_map.get(review_id, "unknown")
            priority = CATEGORY_PRIORITY.get(cat, 99)
            # Schedule-related reviews are always tier 1 — broken schedule breaks everything
            if "schedule" in stem.lower():
                priority = 1
            files.append((priority, md))
    files.sort(key=lambda x: (x[0], x[1].stem))
    return [f[1] for f in files]


def triage_one(review_path: Path) -> tuple[str, bool, str]:
    rid = review_path.stem  # e.g. "agents-01-actor-trait-registry-dispatch"
    rid_category = review_path.parent.name
    prompt = f"""Read the review file at {review_path}.

This file contains code review findings for the HARES codebase. For EACH INDIVIDUAL finding, create ONE TICKET per finding:

1. Assess whether it's a valid, actionable issue.
2. Classify it as a BUG or IMPROVEMENT using the concrete examples below.
3. For INVALID findings or positive confirmations, skip — no ticket.

Severity-to-classification guide (start here, then verify against concrete examples):
  critical, high → BUG
  medium → BUG if incorrect/silent-failure, else IMPROVEMENT
  low → IMPROVEMENT (or SKIP if purely documentation)
  none, verification, positive → SKIP

## BUG → T-0001 and up

A bug is something that is WRONG. The code is broken, dead, inconsistent, or silently produces incorrect results. Examples of bugs:

- **Dead code**: infrastructure declared but never wired (e.g. `ActorInterest` filtering declared but never called in hot loop; `PROTOCOL_NATIVE` capability has zero declarants — signal always dropped).
- **Silent failure**: missing capability declaration causing control signals to be silently rejected (EV lacks `DEMAND_RESPONSE`; Baseboard/Furnace lack `MODE_OVERRIDE`).
- **Constructor inconsistency**: capability declared at one site but missing at another (EventLoad declares `POWER_SETPOINT` at line 233 but omits it at line 716).
- **Stale state**: derived fields not refreshed when their source data changes (ZoneState relative_humidity stale after temperature update).
- **Double-counting**: two equipments in same zone both injecting capacity, doubling the load.
- **NaN propagation**: NaN values passing through to signals without a guard.
- **Broken wiring**: a feature that should work but doesn't because the code path is incomplete (Occupant actor never receives presence schedule — all behavioral logic dead).

## IMPROVEMENT → T-1000 and up

An improvement is something that could be BETTER but the code technically works. Examples of improvements:

- **Architecture**: God objects, oversize modules, layering violations (io depends on equipment).
- **Missing non-critical field**: ZoneState lacks MRT; WeatherState lacks opaque_sky_cover or outdoor air density — the system works without them but would be better with them.
- **Documentation gap**: trait docs don't mention one-step lag on equipment state.
- **Missing validation**: no compile-time test that every ControlSignal variant has at least one equipment declarant.
- **Non-serializable fields impeding save/restore**: EnvironmentState has fields skipped by serde.
- **f64 for geometric constants**: volume_m3 copies every timestep as f64 but never changes.
- **Log throttling hides persistent failures**: warm! suppressed after first occurrence.
- **Constant choice differs from reference**: R_da = 287.058 vs EnergyPlus's 287.0, but both produce correct results within 0.02%.
- **Code quality / refactor**: splitting large functions, removing dead imports, clearer variable names.

## SKIP — no ticket

- Positive findings or confirmations that the code is correct ("No circular dependencies", "Zero occupancy correctly produces zero gains", "Clean design separation").
- Findings marked Severity: none.
- Findings that describe a verified-correct implementation with no action needed.

## Bug tickets — T-0001 and up

Bug tickets go in: {TICKETS_DIR}/T-XXXX/ticket.md where XXXX is the next available 4-digit number, padded with leading zeros, starting from 0001. Before creating any tickets, run `ls {TICKETS_DIR}` to see what numbers already exist. Pick the next unused number in sequence. Do not skip numbers.

## Improvement tickets — T-1000 and up

Improvement tickets go in: {TICKETS_DIR}/T-XXXX/ticket.md starting from 1000. Same rules: scan existing numbers first, pick next unused.

## Ticket file format

Set the `complexity` field based on the finding's scope:
  simple — one-function fix, one-line change, or documentation-only
  moderate — cross-function change, moderate refactor, or new test
  complex — cross-crate change, architectural refactor, or new feature

Create the directory `T-XXXX/` first, then write `ticket.md` inside it. Use this EXACT structure:

```markdown
---
id: T-XXXX
title: "Short title summarising the problem"
kind: implement
status: pending
complexity: simple
initiative_id: I-02
worktrees: inherit
validation_hooks:
  - "cargo nextest run --workspace"
  - "cargo clippy --workspace -- -D warnings"
iteration: 0
max_iterations: 5
created_at: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}
created_by: review-triage
source_review: {rid}
---

## Problem

(Describe the defect or issue. Copy from the review finding, expanding where needed. Include specific code locations with file paths and line numbers. Quote relevant code snippets.)

## Directive

(Specific fix instructions. Extract from the review's Recommendations section. Be concrete — name exact files, functions, and the change. State what the correct behaviour should be.)

## Observability

(How can this be detected or diagnosed at runtime? Specify what invariant checks, telemetry columns, observer captures, or diagnostic CSV fields should be added. Follow the pattern in docs/invariants-and-observability.md: invariant checks behind `#[cfg(any(debug_assertions, feature = "check_invariants"))]` so they compile out in `--release`. Observer captures behind `#[cfg(feature = "observe")]`. If no runtime observability is practical, say so and explain why.)

## Tests

(What tests are needed?)
- Unit test: (what specific function or behaviour to test)
- Regression test: (edge case or integration scenario to add)
If no tests are appropriate, explain why.

## References

- Review finding: `docs/reviews/{rid_category}/{rid}.md`
- Vendor reference: (if the review referenced OCHRE or EnergyPlus code)
- HARES source: (specific files and lines)
```
"""
    cmd = [OPENCODE, "run"]
    if MODEL:
        cmd += ["--model", MODEL]
    cmd.append(prompt)

    log_dir = REPO / "docs" / "reviews" / "_logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / f"triage-{rid}.log"

    t0 = time.time()
    with open(log_path, "w") as log:
        log.write(f"# triage {rid}\n\n")
        log.flush()
        try:
            r = subprocess.run(cmd, stdout=log, stderr=subprocess.STDOUT,
                               timeout=TIMEOUT, cwd=str(REPO))
        except subprocess.TimeoutExpired:
            return (rid, False, f"TIMEOUT >{TIMEOUT}s  log: {log_path}")
        except Exception as e:
            return (rid, False, f"{e}  log: {log_path}")

    elapsed = time.time() - t0
    if r.returncode == 0:
        return (rid, True, f"{elapsed:.0f}s")
    return (rid, False, f"exit={r.returncode} ({elapsed:.0f}s)  log: {log_path}")


def main():
    p = argparse.ArgumentParser(description="Triage review findings into tickets")
    p.add_argument("filter_id", nargs="?", help="Review ID to triage (e.g. agents-01)")
    p.add_argument("--dry-run", "-n", action="store_true")
    args = p.parse_args()

    reviews = find_completed_reviews()
    if args.filter_id:
        reviews = [r for r in reviews if r.stem.startswith(args.filter_id)]
        if not reviews:
            print(f"No completed review matching '{args.filter_id}'")
            sys.exit(1)

    if not reviews:
        print("No completed reviews found.")
        return

    if args.dry_run:
        print(f"Would triage {len(reviews)} reviews:")
        for r in reviews:
            print(f"  {r.stem}")
        return

    print(f"Triaging {len(reviews)} reviews (sequential)...")
    ok_count = fail_count = 0
    t0 = time.time()

    for i, r in enumerate(reviews):
        rid = r.stem
        print(f"  [{i+1}/{len(reviews)}] {rid} ...", end=" ", flush=True)
        rid_out, ok, msg = triage_one(r)
        m = "\033[32mOK\033[0m" if ok else "\033[31mFAIL\033[0m"
        print(f"{m}  {msg}")
        if ok:
            ok_count += 1
        else:
            fail_count += 1

    elapsed = time.time() - t0
    print(f"\nDone: {ok_count} OK, {fail_count} failed ({elapsed:.0f}s)")


if __name__ == "__main__":
    main()
