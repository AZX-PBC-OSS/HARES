# verify-tickets.sh and verify-ticket-prompt.md: ticket verification completeness
**Review ID**: scr-06
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
scripts/verify-tickets.sh scripts/verify-ticket-prompt.md

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical]
**Description**: `verify-tickets.sh` does not discover or parse review manifest files (`scripts/reviews/*.json`). The review manifest system defines 40 JSON files each containing a `reviews` array with review IDs (e.g., `scr-01`, `core-01`, `test-03`), source file paths, and verification prompts. The script makes zero use of this system—it has no `glob` for JSON manifests, no `jq`/JSON parsing, and no logic to map reviews to tickets. Instead, it indiscriminately collects all `*.md` files in `docs/tickets/` and sends each to Claude for independent verification, completely decoupled from the structured review framework.

**Code Location**: `scripts/verify-tickets.sh:59-64` — the ticket list is built solely from `find "${TICKET_DIR}" -maxdepth ... -name '*.md'`; no reference to `scripts/reviews/` appears anywhere in the file.

**Root Cause**: The script was designed as a generic ticket-audit loop rather than as a manifest-driven verification orchestrator. The review JSON files (41 manifests defining reviews across `scripts`, `core`, `envelope`, `hvac-config`, `solver`, `infrastructure`, etc.) exist but are never consumed.

**Impact**: 
1. The script cannot verify that every review ID has corresponding tickets generated—it only processes tickets that happen to exist in `docs/tickets/`. If a review generated zero tickets (e.g., the review found no issues), the script has no way to record or report that result.
2. Verification is not scoped by review category; every ticket is verified independently without awareness of its parent review context, defined vendor references, or category-specific prompt instructions.
3. The review JSON files carry `vendor_refs` fields (e.g., `"vendors/OCHRE/ochre/"`, `"vendors/EnergyPlus/src/EnergyPlus/"`) that are never passed to the verification agent, limiting the agent's ability to cross-reference correctly.
4. There is no feedback loop from ticket verification back to the review manifests—success/failure of verification for each review ID is not tracked or aggregated.

### Finding 2: [Severity: critical]
**Description**: The prompt template embedded in `verify-tickets.sh` (lines 100-246, the fallback that creates `verify-ticket-prompt.md` if absent) differs substantively from the standalone `verify-ticket-prompt.md` file already on disk. The standalone file has been stripped of all references to local EnergyPlus source code and documentation.

**Embedded version** (lines 139-150):
```
### Step 4: Cross-reference EnergyPlus (MANDATORY — local code + local docs + web)
- **Read the EnergyPlus source code** under vendors/EnergyPlus/src/EnergyPlus/
- **Read the local EnergyPlus Engineering Reference docs** under docs/eplus/.
- If the local docs don't cover the topic, use WebSearch and WebFetch...
```

**Standalone version** (lines 39-49):
```
### Step 4: Cross-reference EnergyPlus (MANDATORY WEB SEARCH)
- Use WebSearch to find the relevant EnergyPlus Engineering Reference section
- Use WebFetch to READ the actual EnergyPlus documentation page
- Quote the relevant passage from EnergyPlus
```

**Code Location**: 
- Embedded: `scripts/verify-tickets.sh:139-150`
- Standalone: `scripts/verify-ticket-prompt.md:39-49`

**Root Cause**: The standalone file was independently edited (to remove local-code instructions) without updating the embedded fallback in the shell script. Since the embedded version only writes the file when it doesn't exist (`[[ ! -f "${PROMPT_FILE}" ]]` at line 99), the actual prompt used at runtime is the standalone version—which omits valuable local references.

**Impact**:
1. The verification agent is not instructed to search `vendors/EnergyPlus/src/EnergyPlus/` for EnergyPlus source code, losing the ability to perform precise code-level cross-references.
2. The verification agent is not instructed to search `docs/eplus/` (which exists on disk with EnergyPlus Engineering Reference markdown files), forcing it to rely solely on web searches which may be slower, less reliable, or unavailable.
3. The `docs/eplus/` directory is a curated local resource that provides deterministic, offline-accessible reference material—bypassing it entirely degrades verification quality.
4. Hard Rule 3 in the standalone file (line 133-134) only prohibits modifying `vendors/OCHRE/`, omitting the prohibition on `vendors/EnergyPlus/` that exists in the embedded version (line 247-248). The standalone version is less protective of the EnergyPlus vendor submodule.

### Finding 3: [Severity: high]
**Description**: `verify-ticket-prompt.md` does not instruct the agent to verify ticket completeness (whether all required sections are filled in) or severity classification validity. The audit template at lines 88-117 defines a `## Verification Audit` with `Code Confirmation`, `Web-Verified Citations`, `Legitimacy`, `Proposed Fix Summary`, and `Test Written` sections, but none of these verify that the ticket itself has all required structural elements (title, description, severity, file references, line numbers, proposed fix). The `Code Confirmation` checklist checks line number currency but does not validate that the ticket's own line number references were correctly numbered when written. There is no instruction for the agent to assess whether the ticket's severity classification (e.g., "high", "medium", "low") is appropriate.

**Code Location**: `scripts/verify-ticket-prompt.md:83-117` (Step 8: Update the ticket — the audit template)

**Root Cause**: The prompt was designed as a code-level audit (is the bug real?) rather than a document-quality audit (is the ticket complete and well-formed?). It assumes all tickets are structurally complete.

**Impact**:
1. Tickets with missing sections (e.g., no "Expected Behavior", no "Proposed Fix", no line number references) will pass verification without the gap being flagged.
2. Inflated or deflated severity classifications will not be corrected by the verification process.
3. Inconsistencies between the ticket's self-described file paths and the actual codebase will only be caught incidentally via Step 2 ("Verify the code"), not through systematic completeness checking.

### Finding 4: [Severity: medium]
**Description**: Failure reporting produces no structured or machine-readable output. When a ticket verification fails (`claude` exits non-zero), the script prints `FAIL (see .verify-<ticket>.log)` to stdout (line 316 or 327) and increments a counter. The final summary (lines 336-338) prints only aggregate counts (`ok=N fail=M skip=K`). There is no aggregated list of which specific tickets failed, no structured report (JSON, CSV, markdown), and no summary of failure reasons extracted from the log files. The individual `.verify-*.log` files are plain-text Claude session logs buried in the repo root.

**Code Location**: `scripts/verify-tickets.sh:314-328` (failure handling within the loop), `scripts/verify-tickets.sh:336-342` (summary + exit)

**Root Cause**: The script treats Claude CLI process exit code as the sole failure signal and treats success/failure as a binary outcome per ticket, with no attempt to parse or summarize the verification results.

**Impact**:
1. After a batch run of 100+ tickets, a developer must manually open each `.verify-*.log` file to find which tickets failed and why.
2. The script cannot be integrated into CI pipelines that expect structured output (e.g., GitHub Actions annotations, JUnit XML, SARIF).
3. There is no way to re-run only the failed tickets without manually reconstructing the list from log output.

### Finding 5: [Severity: medium]
**Description**: The script does not verify that `claude` CLI is installed or authenticated before proceeding. Line 27 documents the requirement (`claude CLI installed and authenticated`), but the first actual invocation of the `claude` command occurs at line 310/321 inside the per-ticket loop. If `claude` is missing, all tickets will fail sequentially with shell errors; if `claude` is present but unauthenticated (missing API key), the failures may be less obvious (the CLI might hang, prompt interactively, or produce a cryptic error in the log file).

**Code Location**: `scripts/verify-tickets.sh:26-27` (documentation comment), `scripts/verify-tickets.sh:310-321` (first CLI invocation)

**Root Cause**: No pre-flight check (`command -v claude`, `claude --version`, `claude whoami`, or equivalent) is performed before the ticket loop.

**Impact**:
1. Running without `claude` installed produces 100+ sequential failures with no early-exit or graceful error message.
2. Running without authentication may cause hangs or confusing errors buried in per-ticket log files.
3. The script cannot be used in a "discovery-only" or "offline" mode that produces a report of what would run and which prerequisites are missing.

### Finding 6: [Severity: low]
**Description**: The script is not fully idempotent due to log file side effects. While `SKIP_VERIFIED=1` (default, line 41) skips tickets that already contain `## Verification Audit` in their markdown, preventing re-verification of already-processed tickets, the script still writes new log files to `.verify-*.log` in the repo root for every ticket it does process. The `{{DATE}}` template substitution (line 260) also changes between runs, so the same ticket re-verified on a different day will have a different date in the audit section even if the verification result is identical. Running `verify-tickets.sh` twice in succession on the same day will produce partial idempotency: all already-verified tickets are skipped (same outcome), but any unverified tickets get new audit sections appended.

**Code Location**: `scripts/verify-tickets.sh:76-78` (SKIP_VERIFIED filter), `scripts/verify-tickets.sh:282` (log file naming), `scripts/verify-tickets.sh:260` (date substitution)

**Root Cause**: Log files are created unconditionally for processed tickets; the date is always the current day rather than a stable timestamp.

**Impact**:
1. Disk state differs between runs (new or overwritten log files). A clean `git status` after a run will show untracked `.verify-*.log` files.
2. Running the same unverified ticket on different days will produce different ticket content (different `{{DATE}}`), making `git diff` more noisy.
3. Minimal practical impact since `SKIP_VERIFIED` prevents most redundant processing.

### Finding 7: [Severity: low]
**Description**: `mapfile` (used at line 64) is a bash 4.0+ builtin. macOS ships bash 3.2 due to GPLv3 licensing concerns. On a stock macOS system with `/bin/bash`, this line will produce a `mapfile: command not found` error, and the `TICKETS` array will be empty, causing the script to print "No tickets to verify." and exit 0 silently—a false negative where the script appears to succeed but doesn't process any tickets. The shebang `#!/usr/bin/env bash` picks up whatever `bash` is on PATH; if the user has installed bash 5.x via Homebrew (common), this works fine, but the stock system bash will fail silently.

**Code Location**: `scripts/verify-tickets.sh:64`

**Root Cause**: `mapfile` is a bashism available only in bash 4.0+; no POSIX fallback or version check is provided.

**Impact**: On stock macOS, the script exits 0 without processing any tickets and without warning, leading the user to believe verification succeeded.

## Summary
- Total findings: 7
- Critical: 2 (no manifest integration, embedded/standalone prompt mismatch)
- High: 1 (no ticket completeness verification in prompt)
- Medium: 2 (no structured failure report, no pre-flight CLI credential check)
- Low: 2 (imperfect idempotency, mapfile bash-version portability)

## Recommendations
1. Implement manifest-driven verification: glob `scripts/reviews/*.json`, parse the `reviews` array from each, and use the `vendor_refs`, `files`, and `prompt` fields to construct a targeted verification for each review ID. Track per-review-ID success/failure rather than just per-ticket.
2. Reconcile the embedded prompt template in `verify-tickets.sh` (lines 100-246) with the standalone `verify-ticket-prompt.md`. The standalone version should be authoritative, but it must restore the local-code cross-reference instructions for EnergyPlus (`vendors/EnergyPlus/src/EnergyPlus/` and `docs/eplus/`). The embedded fallback should either be removed (relying on the standalone file exclusively) or updated to match.
3. Add ticket completeness checks to the verification prompt: the agent should confirm the ticket has all mandatory sections (title, severity, description, file paths, line numbers, proposed fix), validate the severity classification, and flag missing or malformed sections in the audit.
4. Add a pre-flight check at script startup: verify `claude` is installed (`command -v claude`), authenticate with a quick non-destructive call (e.g., `claude --version` with timeout), and print a clear error message with remediation instructions if either check fails. Consider adding a `--offline` flag that produces a report of what tickets exist and what would be verified without calling Claude.
5. Add structured failure reporting: after the ticket loop, produce a markdown or JSON report listing each failed ticket name, log file path, and a one-line failure summary extracted from the log. Exit non-zero only if `FAILED > 0`, but also generate the report even on partial success.
6. Replace `mapfile` with a portable alternative (e.g., `while IFS= read -r line; do TICKETS+=("$line"); done < <(find ...)`) to support stock macOS bash 3.2.

## References / Citations
- `scripts/verify-tickets.sh:59-64` — ticket discovery by `find` on `docs/tickets/*.md`; no manifest integration
- `scripts/verify-tickets.sh:99-105` — embedded prompt fallback only triggers when `verify-ticket-prompt.md` doesn't exist
- `scripts/verify-ticket-prompt.md:39-49` — standalone Step 4 stripped of local-code references
- `scripts/verify-tickets.sh:139-150` — embedded Step 4 includes local EnergyPlus code + doc instructions
- `scripts/verify-ticket-prompt.md:83-117` — audit template lacks ticket-completeness checks
- `scripts/verify-tickets.sh:336-342` — exit handling with aggregate counts only, no per-ticket failure report
- `scripts/verify-tickets.sh:310-321` — first `claude` invocation with no pre-flight check
- `scripts/verify-tickets.sh:64` — `mapfile` usage requiring bash 4.0+
- `scripts/reviews/` — 41 JSON manifest files defining the structured review system
- `docs/eplus/` — exists on disk; EnergyPlus Engineering Reference markdown files available for local cross-referencing
