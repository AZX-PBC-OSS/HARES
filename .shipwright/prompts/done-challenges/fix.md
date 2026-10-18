You were here to remove the cause of each finding — not to rebuild the code around it, and not to quiet the check that found it. Before you signal done, read the findings and your diff side by side. Every answer wants a name, a path, a count, or a quote.

**Overreach — did you do a job that was not yours?** Walk your diff hunk by hunk and name the finding each hunk answers. Name every hunk that answers none: a section rewritten, an approach re-taken, an adjacent improvement folded in. Refactoring you needed to land a fix is legitimate — say which finding forced it. What no finding asked for arrived unreviewed, and the next reviewer cannot tell it from the fix.

**Shortfall — did you do the job below the bar?**

- For each fix, name where the defect originates and where you changed the code. If those differ, say why the cause lives where you changed it. A change where a defect surfaced moves it rather than removes it.
- Name where you looked for the same mistake next door — the sibling call site, the other branch, the second implementation of the same pattern — and what you found. A fix addresses the class, not the instance. "There are none" counts only if you say what you searched for.
- Name every suppression, skipped test, broadened catch, widened type, weakened assertion, and loosened comparison in your diff, and quote the specific constraint each cites. A check that fires is doing its job; anything here without a named constraint is the signal silenced, not the cause removed.
- For each finding that exposed a real bug or coverage gap: name the test that goes red if your fix is reverted, or name where that failure mode is already caught.

**Abdication — did you really play the role?**

- Walk the findings item by item and quote the change you made for each. A finding whose only trace is a note acknowledging it is still open — prose about a defect is not a fix for it.
- For each finding you did not act on: name the concrete tool, contract, runtime, or upstream constraint that blocks the fix, and what you tried. "Works as-is", "style preference", "complex" name nothing and re-open the round. Disagreeing is legitimate when you name what the finding got wrong.
- Quote the command you ran to verify and what it printed. If you are recalling, run it now.

The work these raise gets done now, before you signal done again — not recorded as a limitation, not left to the next round.
