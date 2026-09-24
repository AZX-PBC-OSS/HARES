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

### Step 4: Cross-reference EnergyPlus (MANDATORY WEB SEARCH)

Many HARES algorithms originate from EnergyPlus. You MUST:
- Use WebSearch to find the relevant EnergyPlus Engineering Reference section
  or source code for the formulas/coefficients in the ticket
- Use WebFetch to READ the actual EnergyPlus documentation page
- Quote the relevant passage from EnergyPlus
- Note any divergence between HARES and EnergyPlus
- Example searches: "EnergyPlus Engineering Reference exterior film coefficient",
  "EnergyPlus source code biquadratic curve normalization",
  "EnergyPlus ideal loads audit SEER2 conversion"

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

3. Do NOT modify `vendors/OCHRE/` — it is a read-only submodule for
   cross-referencing only.

4. You MAY run `cargo test`, `cargo check`, `cargo clippy`, and other read-only
   build commands.

5. You MAY create new test functions in existing test modules.

6. You MAY update the ticket markdown file itself.

7. You MAY use WebSearch and WebFetch freely — there is no budget constraint on
   the number of web searches you perform. Thoroughness is valued over brevity.
