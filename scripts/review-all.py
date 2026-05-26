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

    # Run ALL pending reviews (4 at a time by default)
    python scripts/review-all.py

    # Run just the wiring/port consistency reviews
    python scripts/review-all.py --category wiring

    # Run a single review by its ID
    python scripts/review-all.py hpxml-01

    # Run with 8 parallel workers
    python scripts/review-all.py --workers 8

    # Run sequentially (debug)
    python scripts/review-all.py --sequential

----------------------------------------------------------------------
ENVIRONMENT VARIABLES
----------------------------------------------------------------------

    OPENCODE_BIN          Path to opencode CLI (default: opencode)
    OPENCODE_MODEL        Model to use, e.g. deepseek/deepseek-v4-pro
    OPENCODE_FLAGS        Extra flags passed to opencode
    REVIEW_WORKERS        Default worker count (overridden by --workers)
    REVIEW_TIMEOUT        Seconds per review (default: 1800 = 30 min)
----------------------------------------------------------------------
"""
import argparse
import json
import os
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_DIR = REPO_ROOT / "scripts" / "reviews"
REVIEWS_DIR = REPO_ROOT / "docs" / "reviews"
OPENCODE_BIN = os.environ.get("OPENCODE_BIN", "opencode")
OPENCODE_MODEL = os.environ.get("OPENCODE_MODEL", "")
_extra_flags = os.environ.get("OPENCODE_FLAGS", "")
OPENCODE_FLAGS = _extra_flags.split() if _extra_flags else []
DEFAULT_WORKERS = int(os.environ.get("REVIEW_WORKERS", "4"))
REVIEW_TIMEOUT = int(os.environ.get("REVIEW_TIMEOUT", "1800"))  # 30 min


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


def run_one_review(review: dict) -> tuple[str, bool, str, float]:
    """Run a single review via opencode. Polls for output file; terminates opencode once file is written. Returns (id, success, message, elapsed_secs)."""
    rid = review["id"]
    out_path = review["_output_path"]
    prompt = build_prompt(review)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)

    cmd = [OPENCODE_BIN, "run"]
    if OPENCODE_MODEL:
        cmd += ["--model", OPENCODE_MODEL]
    cmd += OPENCODE_FLAGS
    cmd.append(prompt)

    print(f"  ▸ START {rid:22s}  {review['title'][:60]}", flush=True)

    log_path = os.path.join(str(REPO_ROOT), "docs", "reviews", "_logs", f"{rid}.log")
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    log = open(log_path, "w")
    log.write(f"# opencode log for {rid}\n# cmd: {cmd[0]} {cmd[1]} ...\n\n")
    log.flush()

    t0 = time.time()
    proc = None
    try:
        proc = subprocess.Popen(
            cmd, stdout=log, stderr=subprocess.STDOUT, cwd=str(REPO_ROOT)
        )
        # Poll until output file appears and has content, or timeout, or process dies
        deadline = t0 + REVIEW_TIMEOUT
        file_ready = False
        while time.time() < deadline:
            ret = proc.poll()
            if ret is not None:
                # Process exited — check result
                elapsed = time.time() - t0
                log.close()
                if ret == 0 and os.path.exists(out_path) and os.path.getsize(out_path) > 0:
                    return (rid, True, f"-> {out_path}", elapsed)
                else:
                    msg = f"exit={ret}"
                    if not os.path.exists(out_path):
                        msg = "output file not created"
                    elif os.path.getsize(out_path) == 0:
                        msg = "output file empty"
                    return (rid, False, f"{msg}  log: {log_path}", elapsed)
            if not file_ready and os.path.exists(out_path) and os.path.getsize(out_path) > 0:
                # Output written — give opencode 15s to exit gracefully, then kill
                file_ready = True
                grace = time.time() + 15
            if file_ready and time.time() > grace:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
                elapsed = time.time() - t0
                log.close()
                return (rid, True, f"-> {out_path}", elapsed)
            time.sleep(2)

        # Timeout
        elapsed = time.time() - t0
        log.close()
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        if os.path.exists(out_path) and os.path.getsize(out_path) > 0:
            return (rid, True, f"-> {out_path} (timeout)", elapsed)
        return (rid, False, f"TIMEOUT (>{REVIEW_TIMEOUT}s)  log: {log_path}", elapsed)
    except FileNotFoundError:
        elapsed = time.time() - t0
        log.close()
        return (rid, False, f"binary not found: {OPENCODE_BIN}", elapsed)
    except Exception as exc:
        elapsed = time.time() - t0
        log.close()
        if proc and proc.poll() is None:
            proc.kill()
            proc.wait()
        return (rid, False, f"{exc}  log: {log_path}", elapsed)


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
            print(f"SKIP  {r['id']}  ({out} exists)")
            skipped += 1
            continue
        try:
            rid, ok, msg, elapsed = run_one_review(r)
        except KeyboardInterrupt:
            print("\nInterrupted — stopping.")
            break
        marker = "\033[32mPASS\033[0m" if ok else "\033[31mFAIL\033[0m"
        print(f"  {marker}  {rid:22s}  {elapsed:5.0f}s  {msg[:100]}")
        if ok:
            ran += 1
        else:
            errors += 1
    return ran, skipped, errors


def run_parallel(reviews: list[dict], workers: int) -> tuple[int, int, int]:
    """Run reviews with a thread pool. Ctrl-C stops picking up new reviews, waits for in-flight. Returns (ran, skipped, errors)."""
    pending = [r for r in reviews if not os.path.exists(r["_output_path"])]
    skipped = len(reviews) - len(pending)
    for r in reviews:
        if os.path.exists(r["_output_path"]):
            print(f"SKIP  {r['id']}  ({r['_output_path']} exists)")

    if not pending:
        return 0, skipped, 0

    total = len(pending)
    print(f"\n{'='*60}")
    print(f"Dispatching {total} reviews across {workers} workers")
    print(f"Timeout: {REVIEW_TIMEOUT}s/review  |  Output: docs/reviews/")
    print(f"Logs: docs/reviews/_logs/<id>.log")
    print(f"{'='*60}\n")

    completed = 0
    errors = 0
    review_times: list[float] = []
    stop = threading.Event()  # set on Ctrl+C
    t0 = time.time()

    with ThreadPoolExecutor(max_workers=workers) as executor:
        submitted = 0
        futures: dict = {}

        def _fill():
            nonlocal submitted
            while submitted < len(pending) and len(futures) < workers and not stop.is_set():
                r = pending[submitted]
                submitted += 1
                futures[executor.submit(run_one_review, r)] = r

        _fill()

        while futures:
            try:
                done = set(as_completed(futures, timeout=5))
            except KeyboardInterrupt:
                stop.set()
                print("\nCtrl+C — no new reviews. Waiting for in-flight to finish...", flush=True)
                cancelled = sum(1 for f in list(futures) if f.cancel())
                if cancelled:
                    print(f"  Cancelled {cancelled} queued", flush=True)
                futures = {f: r for f, r in futures.items() if not f.cancelled()}
                if not futures:
                    break
                try:
                    done = set(as_completed(futures))
                except KeyboardInterrupt:
                    print("Second Ctrl+C — forcing exit.", flush=True)
                    break
                except Exception:
                    break
                continue

            if not done:
                continue

            for future in done:
                review = futures.pop(future)
                try:
                    rid, ok, msg, elapsed = future.result()
                except Exception as exc:
                    rid = review["id"]
                    ok = False
                    msg = str(exc)
                    elapsed = 0

                completed += 1
                if elapsed > 0:
                    review_times.append(elapsed)
                avg = sum(review_times) / len(review_times) if review_times else 300
                remaining = total - completed
                eta = (remaining / workers) * avg

                marker = "\033[32m\033[1mPASS\033[0m" if ok else "\033[31mFAIL\033[0m"
                print(
                    f"  {marker}  {rid:22s}  [{completed:3d}/{total}]  "
                    f"{time.time()-t0:5.0f}s elapsed  ~{eta:5.0f}s left  "
                    f"({elapsed:.0f}s)  {review['title'][:50]}",
                    flush=True,
                )
                if not ok and msg:
                    print(f"        {msg[:150]}", flush=True)
                if not ok:
                    errors += 1

            _fill()

    elapsed = time.time() - t0
    ran = completed - errors
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
        help="Run all reviews in a manifest category"
    )
    parser.add_argument(
        "--dry-run", "-n", action="store_true",
        help="List what would run without executing"
    )
    parser.add_argument(
        "--list", "-l", action="store_true",
        help="List categories with counts; with --category or ID, lists individual reviews"
    )
    parser.add_argument(
        "--workers", "-w", type=int, default=DEFAULT_WORKERS,
        help=f"Number of concurrent workers (default: {DEFAULT_WORKERS})"
    )
    parser.add_argument(
        "--sequential", "-s", action="store_true",
        help="Run reviews sequentially (no concurrency)"
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
            for r in reviews:
                done = "\033[32m✓\033[0m" if os.path.exists(r["_output_path"]) else "○"
                print(f"  {done} {r['id']:22s} {r['title']}")
        else:
            list_categories(all_reviews)
        return

    if args.dry_run:
        pending = dry_run(reviews)
        total = len(reviews)
        done_count = total - pending
        print(f"\n=== {pending} pending, {done_count} done, {total} total ===")
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

    if errors:
        sys.exit(1)


if __name__ == "__main__":
    main()
