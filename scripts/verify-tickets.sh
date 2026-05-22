#!/usr/bin/env bash
# verify-tickets.sh
#
# Loop over every ticket markdown file in docs/tickets/, invoke Claude CLI
# in non-interactive mode with auto permissions to:
#   1. Read the ticket
#   2. Explore the HARES codebase + vendors/OCHRE
#   3. Cross-reference EnergyPlus Engineering Reference
#   4. Use WebSearch + WebFetch to verify standards citations
#   5. Determine whether the issue is legitimate
#   6. Propose a fix (but NOT implement it — only write tests & update the ticket)
#
# Claude updates the ticket in-place with a ## Verification Audit section.
#
# Usage:
#   ./scripts/verify-tickets.sh                    # all tickets
#   ./scripts/verify-tickets.sh 088 089            # specific ticket numbers
#   DRY_RUN=1 ./scripts/verify-tickets.sh          # just print what would run
#   MAX_TICKETS=5 ./scripts/verify-tickets.sh      # limit count
#   MAX_TURNS=20 ./scripts/verify-tickets.sh       # lower turn limit
#   MAX_BUDGET_USD=2 ./scripts/verify-tickets.sh   # cap spend per ticket
#   VERBOSE=1 ./scripts/verify-tickets.sh          # verbose claude output
#   STREAM=1 ./scripts/verify-tickets.sh           # stream-json to stdout (live view)
#   MODEL=opus ./scripts/verify-tickets.sh         # pick model (default: sonnet)
#
# Requirements:
#   - claude CLI installed and authenticated
#   - Working directory is the HARES repo root

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TICKET_DIR="${REPO_ROOT}/docs/tickets"
PROMPT_FILE="${REPO_ROOT}/scripts/verify-ticket-prompt.md"

# ── Config ──────────────────────────────────────────────────────────────────
DRY_RUN="${DRY_RUN:-0}"
MAX_TICKETS="${MAX_TICKETS:-0}"                # 0 = unlimited
SKIP_CONSOLIDATED="${SKIP_CONSOLIDATED:-1}"    # skip docs/tickets/consolidated/ by default
SKIP_INDEX="${SKIP_INDEX:-1}"                  # skip INDEX_* files by default
SKIP_VERIFIED="${SKIP_VERIFIED:-1}"            # skip tickets that already have a Verification Audit section
VERBOSE="${VERBOSE:-0}"                        # pass --verbose to claude
MAX_TURNS="${MAX_TURNS:-0}"                    # agentic turn limit per ticket; 0 = no limit
MAX_BUDGET_USD="${MAX_BUDGET_USD:-0}"          # per-ticket USD cap; 0 = no cap
MODEL="${MODEL:-sonnet}"                       # claude model to use
STREAM="${STREAM:-0}"                          # 1=stream-json to stdout (live), 0=text to log only

# ── Build ticket list ───────────────────────────────────────────────────────
if (($# > 0)); then
    TICKETS=()
    for num in "$@"; do
        match="$(find "${TICKET_DIR}" -maxdepth 1 -name "${num}-*.md" -print -quit 2>/dev/null || true)"
        if [[ -z "$match" ]]; then
            echo "WARN: No ticket file found for number ${num}" >&2
            continue
        fi
        TICKETS+=("$match")
    done
else
    find_maxdepth=1
    if [[ "${SKIP_CONSOLIDATED}" == "0" ]]; then
        find_maxdepth=2
    fi
    mapfile -t TICKETS < <(find "${TICKET_DIR}" -maxdepth "${find_maxdepth}" -name '*.md' -not -name 'INDEX_*' | sort)
fi

# Apply filters
FILTERED=()
for t in "${TICKETS[@]}"; do
    if [[ "${SKIP_CONSOLIDATED}" == "1" && "${t}" == */consolidated/* ]]; then
        continue
    fi
    if [[ "${SKIP_INDEX}" == "1" && "$(basename "${t}")" == INDEX_* ]]; then
        continue
    fi
    if [[ "${SKIP_VERIFIED}" == "1" ]] && grep -q "## Verification Audit" "${t}" 2>/dev/null; then
        echo "   skip (already verified): $(basename "${t}")"
        continue
    fi
    FILTERED+=("$t")
done
TICKETS=("${FILTERED[@]+${FILTERED[@]}}")

if ((${#TICKETS[@]} == 0)); then
    echo "No tickets to verify." >&2
    exit 0
fi

# Apply max limit
if ((MAX_TICKETS > 0 && ${#TICKETS[@]} > MAX_TICKETS)); then
    TICKETS=("${TICKETS[@]:0:${MAX_TICKETS}}")
fi

echo "----------------------------------------------------------"
echo " Ticket verification -- ${#TICKETS[@]} ticket(s), model=${MODEL}"
echo "----------------------------------------------------------"

# ── Ensure the shared prompt template exists ───────────────────────────────
if [[ ! -f "${PROMPT_FILE}" ]]; then
    cat > "${PROMPT_FILE}" << 'PROMPT_EOF'
You are verifying an engineering ticket for the HARES building-energy simulation
project (a Rust re-implementation of EnergyPlus / OCHRE concepts). Your job is
to **audit** the ticket, NOT to implement any fix.

**CRITICAL: You MUST use WebSearch and WebFetch to independently verify every
claim and citation in the ticket. Do not simply assert that a citation is correct
or incorrect — you must actually search for and read the referenced standard,
paper, or documentation and quote what it says. An audit without web-verified
sources is an incomplete audit and will not be accepted.**

## What you must do — step by step

### Step 1: Read and understand the ticket

Read the ticket at {{TICKET_PATH}}. Identify:
- The specific code location(s) cited (file path, line number, function name)
- The claimed bug or mismatch (expected vs actual behavior)
- Any numerical values, formulas, or coefficients mentioned
- Every standards citation (ASHRAE, NFRC, DOE, ISO, EnergyPlus, etc.)

### Step 2: Verify the code

Search the HARES Rust crates under `crates/` and Python modules under
`python/` for the code the ticket references. Open the file, navigate to
the cited line, and confirm:
- The line numbers still match (if shifted, note the correct location)
- The variable names and logic described in the ticket match the current code
- The bug described is actually present (not already fixed)

### Step 3: Cross-reference OCHRE

Read the relevant OCHRE source under `vendors/OCHRE/ochre/` to see how OCHRE
handles the same concern. Specifically:
- Search OCHRE for the same variable names, formulas, or coefficients
- Determine whether HARES matches OCHRE or diverges
- If it diverges, determine whether the divergence is intentional (HARES
  correcting a known OCHRE limitation) or accidental

### Step 4: Cross-reference EnergyPlus (MANDATORY — local code + local docs + web)

Many HARES algorithms originate from EnergyPlus. You MUST:
- **Read the EnergyPlus source code** under `vendors/EnergyPlus/src/EnergyPlus/` —
  search for the same variable names, formulas, or coefficients.
- **Read the local EnergyPlus Engineering Reference docs** under `docs/eplus/`.
  These are markdown versions of the E+ Engineering Reference. Use `Grep` to
  search `docs/eplus/` for relevant keywords related to the ticket's topic.
- If the local docs don't cover the topic, use WebSearch and WebFetch to find
  the relevant page at bigladdersoftware.com
- Quote the relevant passage from both the source code AND the documentation
- Note any divergence between HARES and EnergyPlus

### Step 5: Verify every standards citation (MANDATORY WEB SEARCH)

For EVERY citation in the ticket (ASHRAE, NFRC, DOE, ISO, etc.), you MUST:
- Use WebSearch to find the cited document or a reliable summary of it
- Use WebFetch to READ the source and confirm the cited values, formulas,
  and section numbers
- If the citation is wrong, state what the correct source actually says
- If the citation is correct, quote the relevant passage as proof
- NEVER just check a checkbox without quoting the source — show your work

Example: if the ticket cites "ASHRAE HoF 2021 Ch. 15 §15.6 — NFRC procedure
and the 34 W/(m²·K) default", you must WebSearch for that chapter, WebFetch
the relevant page, and quote the passage confirming 34 W/(m²·K).

### Step 6: Determine legitimacy

Based on ALL evidence gathered (code, OCHRE, EnergyPlus, standards), determine:
- **Legitimate**: the bug/mismatch is real, the ticket's description is
  accurate, and citations check out
- **Partially Legitimate**: the core issue is real but details (line numbers,
  severity, citation accuracy, proposed fix) need refinement
- **Not Legitimate**: the ticket's claim is incorrect or already addressed
- **Cannot Verify**: you were unable to find sufficient evidence (explain what
  you searched and why it was insufficient)

### Step 7: Write regression tests

If the ticket is legitimate or partially legitimate, write a failing regression
test that demonstrates the bug. Place it in the appropriate test module under
`crates/*/tests/` or within an existing `#[cfg(test)]` module. If a test
already exists, note that.

### Step 8: Update the ticket

If a `## Verification Audit` section already exists, replace it in full.
Otherwise, append it at the end of the ticket file. Use this structure:

```markdown
## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: {{DATE}}

### Code Confirmation
- [ ] Referenced line numbers still match (or note corrected location)
- [ ] Described logic matches current implementation
- [ ] OCHRE cross-check result: <matches / diverges / N/A — with file+line evidence>
- [ ] EnergyPlus cross-check result: <matches / diverges / N/A — with quoted passage>

### Web-Verified Citations
For each citation in the ticket, provide:
- **Citation**: <what the ticket claims>
- **Source found**: <URL or document title you fetched>
- **Quoted passage**: <the relevant text from the source>
- **Verdict**: <confirmed / incorrect / partially correct>

### Legitimacy
- **Verdict**: <Legitimate / Partially Legitimate / Not Legitimate / Cannot Verify>
- **Rationale**: <one-paragraph explanation referencing your evidence>

### Proposed Fix Summary
<Summarise the minimal fix. DO NOT implement it.>

### Test Written
- File: <path or "none needed">
- What it tests: <description>
```

### Step 9: Do NOT implement the fix

You may write tests, run `cargo test`, `cargo check`, `cargo clippy`, etc.,
but do NOT change production code under `crates/*/src/` to fix the bug itself.

## Hard Rules

1. **WebSearch and WebFetch are MANDATORY**, not optional. Every citation must
   be independently verified with a quoted source. If you skip web verification,
   the audit is incomplete and must be marked "Cannot Verify".

2. Do NOT modify any file under `crates/*/src/` except test modules (i.e. files
   containing `#[cfg(test)]` or within `tests/` directories).

3. Do NOT modify `vendors/OCHRE/` or `vendors/EnergyPlus/` — they are read-only
   submodules for cross-referencing only.

4. You MAY run `cargo test`, `cargo check`, `cargo clippy`, and other read-only
   build commands.

5. You MAY create new test functions in existing test modules.

6. You MAY update the ticket markdown file itself.

7. You MAY use WebSearch and WebFetch freely — there is no budget constraint on
   the number of web searches you perform. Thoroughness is valued over brevity.
PROMPT_EOF
    echo "Created prompt template at ${PROMPT_FILE}"
fi

# ── Process each ticket ────────────────────────────────────────────────────
SUCCESS=0
FAILED=0
SKIPPED=0

for ticket_path in "${TICKETS[@]}"; do
    ticket_name="$(basename "${ticket_path}" .md)"
    today="$(date +%Y-%m-%d)"

    prompt="$(sed \
        -e "s|{{TICKET_PATH}}|${ticket_path}|g" \
        -e "s|{{DATE}}|${today}|g" \
        "${PROMPT_FILE}")"

    # Write prompt to a temp file so we can pipe it to claude -p.
    # Passing a huge prompt as a CLI argument can silently fail on some shells.
    prompt_tmp="$(mktemp)"
    printf '%s' "${prompt}" > "${prompt_tmp}"

    echo ""
    echo "--> ${ticket_name}"

    if [[ "${DRY_RUN}" == "1" ]]; then
        rm -f "${prompt_tmp}"
        echo "   [DRY RUN] claude -p --model ${MODEL} --permission-mode auto --max-turns ${MAX_TURNS}"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    output_format="text"
    ((STREAM)) && output_format="stream-json"

    log_file="${REPO_ROOT}/.verify-${ticket_name}.log"

    # Build claude flags (no -p argument — prompt comes via stdin pipe)
    claude_args=(
        -p
        --model "${MODEL}"
        --permission-mode auto
        --allowedTools "Read" "Write" "Edit"
            "Bash(cargo*)" "Bash(uv *)" "Bash(rustc *)" "Bash(rustup *)"
            "Bash(maturin *)" "Bash(pytest *)" "Bash(python3 *)" "Bash(python *)"
            "Bash(cat *)" "Bash(echo *)" "Bash(head *)" "Bash(tail *)"
            "Bash(grep *)" "Bash(rg *)" "Bash(find *)" "Bash(ls *)"
            "Bash(wc *)" "Bash(sort *)" "Bash(diff *)" "Bash(which *)"
            "Bash(env *)" "Bash(date *)" "Bash(make *)"
            "Bash(mkdir *)" "Bash(rustfmt *)"
            "Bash(sed *)" "Bash(awk *)" "Bash(cut *)" "Bash(tr *)" "Bash(xargs *)"
            "Bash(bc *)" "Bash(curl *)"
            "Glob" "Grep" "WebFetch" "WebSearch"
        --max-turns "${MAX_TURNS}"
        --output-format "${output_format}"
        --name "verify-${ticket_name}"
    )
    ((VERBOSE || STREAM)) && claude_args+=(--verbose)
    ((MAX_BUDGET_USD > 0)) && claude_args+=(--max-budget-usd "${MAX_BUDGET_USD}")

    if ((STREAM)); then
        # stream-json: pipe through stream-filter.sh for readable output,
        # tee raw JSONL to log file
        if cat "${prompt_tmp}" | claude "${claude_args[@]}" 2>&1 \
             | "${REPO_ROOT}/scripts/stream-filter.sh" \
             | tee "${log_file}"; then
            echo "   ok done"
            SUCCESS=$((SUCCESS + 1))
        else
            echo "   FAIL (see ${log_file})"
            FAILED=$((FAILED + 1))
        fi
    else
        # text mode: all output to log file only
        if cat "${prompt_tmp}" | claude "${claude_args[@]}" \
             >>"${log_file}" 2>&1; then
            echo "   ok done"
            SUCCESS=$((SUCCESS + 1))
        else
            echo "   FAIL (see ${log_file})"
            FAILED=$((FAILED + 1))
        fi
    fi

    rm -f "${prompt_tmp}"
done

# ── Summary ─────────────────────────────────────────────────────────────────
echo ""
echo "----------------------------------------------------------"
echo " Done. ok=${SUCCESS}  fail=${FAILED}  skip=${SKIPPED}"
echo "----------------------------------------------------------"

if ((FAILED > 0)); then
    exit 1
fi
