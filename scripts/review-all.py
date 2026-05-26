#!/usr/bin/env python3
"""
HARES Systematic Code Review Dispatcher
========================================

Reads review definitions from JSON manifest files in scripts/reviews/*.json
and dispatches each as a focused, single-concern code review to opencode.

Each review produces a deterministic output markdown file at:
    docs/reviews/{category}/{id}-{slug}.md

The script is IDEMPOTENT — re-running it skips any review whose output file
already exists.  You can safely stop (Ctrl-C) and restart without losing progress.

----------------------------------------------------------------------
QUICK START
----------------------------------------------------------------------

    # See everything that would run (safe, no changes)
    python scripts/review-all.py --dry-run

    # Run ALL 287 pending reviews (4 at a time by default)
    python scripts/review-all.py

    # Run just the wiring/port consistency reviews (8 reviews)
    python scripts/review-all.py --category wiring

    # Run a single review by its ID
    python scripts/review-all.py hpxml-01

    # Run with 8 parallel workers (faster on multi-core)
    python scripts/review-all.py --workers 8

    # Run sequentially (useful for debugging one-at-a-time)
    python scripts/review-all.py --sequential

----------------------------------------------------------------------
COMMON WORKFLOWS
----------------------------------------------------------------------

1. FIRST USE — see what's available:
       python scripts/review-all.py --list
   Shows all 40 categories with their review counts.

2. PREVIEW a category before running:
       python scripts/review-all.py --category hpxml --dry-run
   Lists the 12 HPXML reviews and shows which are already done.

3. RESUME after interruption:
   Just re-run the same command. Already-completed reviews are skipped.
       python scripts/review-all.py
   or
       python scripts/review-all.py --category core

4. TARGETED — run a specific review:
       python scripts/review-all.py <review-id>
   Example IDs: hpxml-01, envelope-03, core-07, wiring-01

5. BATCH BY PRIORITY — run critical categories first:
       python scripts/review-all.py --category wiring --workers 4
       python scripts/review-all.py --category envelope --workers 4
       python scripts/review-all.py --category core --workers 4

6. KICK OFF EVERYTHING overnight:
       python scripts/review-all.py --workers 8
   (~287 reviews; at ~2 min/review with 4 workers ≈ 2.5 hours)

----------------------------------------------------------------------
FLAGS
----------------------------------------------------------------------

    <review-id>           Run a single review (positional, optional)
    -c, --category NAME   Run all reviews in one manifest category
    -n, --dry-run         Preview what would run (no execution)
    -l, --list            List categories with counts; with --category
                           or a review ID, lists individual review titles
    -w, --workers N       Number of concurrent workers (default: 4)
    -s, --sequential      Run one-at-a-time instead of parallel

----------------------------------------------------------------------
ENVIRONMENT VARIABLES
----------------------------------------------------------------------

    OPENCODE_BIN          Path to the opencode CLI (default: opencode)
    OPENCODE_FLAGS        Extra flags passed to opencode, e.g. --verbose
    REVIEW_WORKERS        Default worker count (overridden by --workers)

----------------------------------------------------------------------
OUTPUT FORMAT
----------------------------------------------------------------------

Each review writes a severity-tagged markdown file:

    # <title>
    **Review ID**: <id>
    **Category**: <category>
    **Date**: YYYY-MM-DD

    ## Files Reviewed
    ## Vendor/Reference Files Consulted

    ## Findings
    ### Finding 1: [Severity: critical|high|medium|low]
    **Description**: ...
    **Code Location**: ...
    **Root Cause**: ...
    **Impact**: ...

    ## Summary
    - Total / Critical / High / Medium / Low counts

    ## Recommendations
    ## References / Citations

----------------------------------------------------------------------
MANIFEST FORMAT (scripts/reviews/*.json)
----------------------------------------------------------------------

Each manifest is a JSON file describing reviews for one category:

    {
        "category": "hpxml",
        "description": "HPXML input parsing, validation, defaults",
        "reviews": [
            {
                "id": "hpxml-01",
                "slug": "duct-leakage-cfm25-silent-skip",
                "title": "HPXML duct leakage CFM25 units silently skipped",
                "vendor_refs": "vendors/OCHRE/ochre/utils/hpxml.py",
                "files": "crates/hares-io/src/hpxml/building.rs ...",
                "prompt": "When HPXML DuctLeakage... Review the code..."
            }
        ]
    }

To add a new review area, add an entry to the appropriate JSON file.
To add a new category, create a new JSON file with the same schema.

----------------------------------------------------------------------
CONCURRENCY NOTES
----------------------------------------------------------------------

Reviews are READ-ONLY. Each worker:
  - Reads HARES source files (no writes to source)
  - Writes findings to a unique output .md file
  - Runs in an isolated subprocess (ProcessPoolExecutor)
No two reviews write to the same output file, so parallel execution is safe.
"""
import argparse
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_DIR = REPO_ROOT / "scripts" / "reviews"
REVIEWS_DIR = REPO_ROOT / "docs" / "reviews"
OPENCODE_BIN = os.environ.get("OPENCODE_BIN", "opencode")
OPENCODE_FLAGS = os.environ.get("OPENCODE_FLAGS", "").split()
DEFAULT_WORKERS = int(os.environ.get("REVIEW_WORKERS", "4"))


def load_all_manifests() -> list[dict]:
    """Load all JSON manifest files and return a flat list of review dicts."""
    manifests = []
    for mf in sorted(MANIFEST_DIR.glob("*.json")):
        with open(mf) as f:
            data = json.load(f)
        category = data["category"]
        for r in data.get("reviews", []):
            r["_category"] = category
            r["_output_path"] = str(
                REVIEWS_DIR / category / f"{r['id']}-{r['slug']}.md"
            )
            manifests.append(r)
    return manifests


def build_prompt(review: dict) -> str:
    """Build the full prompt for opencode from a review entry."""
    r = review
    vendor = r.get("vendor_refs", "")
    files = r.get("files", "")
    out = r["_output_path"]
    cat = r["_category"]

    return f"""You are performing a focused, single-concern code review of the HARES residential energy simulation codebase.
Work in the directory: {REPO_ROOT}

## Review: {r['title']}
## Review ID: {r['id']}
## Category: {cat}

## HARES Source Files to Review
{files}

## Vendor Reference Files (compare/contrast)
{vendor}

## Review Instructions
{r['prompt']}

## Output Requirements
Write your findings to the file:
  {out}

The output MUST be a markdown file with this structure:

# {r['title']}
**Review ID**: {r['id']}
**Category**: {cat}
**Date**: {datetime.now(timezone.utc).strftime('%Y-%m-%d')}

## Files Reviewed
{files}

## Vendor/Reference Files Consulted
{vendor}

## Findings
### Finding 1: [Severity: critical|high|medium|low]
**Description**: ...
**Code Location**: ...
**Root Cause**: ...
**Impact**: ...

## Summary
- Total findings: N
- Critical / High / Medium / Low: each count

## Recommendations
1. ...

## References / Citations
- ...

Be thorough but focused. Only write findings relevant to this specific review area.
Cite specific line numbers. Compare against vendor reference implementations where applicable.
"""


def run_one_review(review: dict) -> tuple[str, bool, str]:
    """Run a single review via opencode. Returns (id, success, message)."""
    rid = review["id"]
    out_path = review["_output_path"]
    prompt = build_prompt(review)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)

    cmd = [OPENCODE_BIN] + OPENCODE_FLAGS
    try:
        result = subprocess.run(
            cmd,
            input=prompt,
            capture_output=True,
            text=True,
            timeout=600,  # 10-minute timeout per review
            cwd=str(REPO_ROOT),
        )
        if result.returncode == 0:
            return (rid, True, f"OK (output: {out_path})")
        else:
            return (rid, False, f"exit={result.returncode} stderr={result.stderr[:200]}")
    except subprocess.TimeoutExpired:
        return (rid, False, "TIMEOUT (>600s)")
    except FileNotFoundError:
        return (rid, False, f"opencode binary not found: {OPENCODE_BIN}")
    except Exception as exc:
        return (rid, False, str(exc))


def dry_run(reviews: list[dict]) -> int:
    """Print what would run. Returns count of pending reviews."""
    pending = 0
    for r in reviews:
        out = r["_output_path"]
        done = os.path.exists(out)
        status = "SKIP" if done else "RUN "
        if not done:
            pending += 1
        print(f"  {status}  {r['id']:20s} | {r['_category']:20s} | {r['title'][:60]}  ->  {out}")
    return pending


def run_sequential(reviews: list[dict]) -> tuple[int, int, int]:
    """Run reviews one at a time. Returns (ran, skipped, errors)."""
    ran = skipped = errors = 0
    for r in reviews:
        out = r["_output_path"]
        if os.path.exists(out):
            print(f"SKIP {r['id']}  —  {out} already exists")
            skipped += 1
            continue
        print(f"START {r['id']}: {r['title']}")
        rid, ok, msg = run_one_review(r)
        if ok:
            print(f"PASS  {rid}")
            ran += 1
        else:
            print(f"FAIL  {rid}: {msg}")
            errors += 1
    return ran, skipped, errors


def run_parallel(reviews: list[dict], workers: int) -> tuple[int, int, int]:
    """Run reviews with multiprocessing. Returns (ran, skipped, errors)."""
    pending = [r for r in reviews if not os.path.exists(r["_output_path"])]
    skipped = len(reviews) - len(pending)
    for r in reviews:
        if os.path.exists(r["_output_path"]):
            print(f"SKIP {r['id']}  —  {r['_output_path']} already exists")
    if not pending:
        return 0, skipped, 0

    print(f"\nDispatching {len(pending)} reviews with {workers} workers...\n")
    ran = errors = 0
    with ProcessPoolExecutor(max_workers=workers) as executor:
        futures = {executor.submit(run_one_review, r): r for r in pending}
        for future in as_completed(futures):
            review = futures[future]
            try:
                rid, ok, msg = future.result()
                if ok:
                    print(f"PASS  {rid}")
                    ran += 1
                else:
                    print(f"FAIL  {rid}: {msg}")
                    errors += 1
            except Exception as exc:
                print(f"FAIL  {review['id']}: {exc}")
                errors += 1
    return ran, skipped, errors


def list_categories(reviews: list[dict]):
    """Print category summary."""
    from collections import Counter
    cats = Counter(r["_category"] for r in reviews)
    for cat, n in sorted(cats.items()):
        print(f"  {cat:30s} {n:4d} reviews")


def main():
    parser = argparse.ArgumentParser(
        description="HARES Systematic Code Review Dispatcher"
    )
    parser.add_argument(
        "filter_id", nargs="?", default=None,
        help="Run a single review by ID (e.g. hpxml-01)"
    )
    parser.add_argument(
        "--category", "-c", default=None,
        help="Run all reviews in a category"
    )
    parser.add_argument(
        "--dry-run", "-n", action="store_true",
        help="List what would run without executing"
    )
    parser.add_argument(
        "--list", "-l", action="store_true",
        help="List categories with review counts; with --category or review ID, lists individual review titles with completion status"
    )
    parser.add_argument(
        "--workers", "-w", type=int, default=DEFAULT_WORKERS,
        help=f"Number of concurrent review workers (default: {DEFAULT_WORKERS})"
    )
    parser.add_argument(
        "--sequential", "-s", action="store_true",
        help="Run reviews sequentially (no multiprocessing)"
    )
    args = parser.parse_args()

    all_reviews = load_all_manifests()

    # Apply filters
    if args.filter_id:
        reviews = [r for r in all_reviews if r["id"] == args.filter_id]
        if not reviews:
            print(f"No review found with ID '{args.filter_id}'")
            sys.exit(1)
    elif args.category:
        reviews = [r for r in all_reviews if r["_category"] == args.category]
        if not reviews:
            print(f"No reviews found in category '{args.category}'")
            sys.exit(1)
    else:
        reviews = all_reviews

    if args.list:
        if args.filter_id or args.category:
            # List individual review titles
            for r in reviews:
                done = "✓" if os.path.exists(r["_output_path"]) else "○"
                print(f"  {done} {r['id']:22s} {r['title']}")
        else:
            list_categories(all_reviews)
        return

    if args.dry_run:
        pending = dry_run(reviews)
        total = len(reviews)
        done_count = total - pending
        print(f"\n=== {pending} pending, {done_count} already done, {total} total ===")
        return

    # Execute
    t0 = time.time()
    if args.sequential or args.workers <= 1:
        ran, skipped, errors = run_sequential(reviews)
    else:
        ran, skipped, errors = run_parallel(reviews, args.workers)

    elapsed = time.time() - t0
    print(f"\n{'='*60}")
    print(f"SUMMARY: {ran} ran, {skipped} skipped, {errors} errors  ({elapsed:.0f}s)")
    print(f"{'='*60}")


if __name__ == "__main__":
    main()
