You were here to adversarially review a fix — verify it resolves the bug, check for regressions, look for similar classes of issues, and identify any gaps. Before you signal done, read your review back.

**Did you actually verify the fix?** Name the code path you traced from the RCA's root cause to the change. Did you run the project's verification commands and confirm green? Did you run the reproduction test and confirm it fails before the fix and passes after? If you assumed from reading the diff, say so — an unverified fix is a guess.

**Did you check for regressions?** Name the callers and dependents of what was changed. What edge cases could break? Did you trace them, or list them and move on?

**Did you look for similar issues?** The root cause often generalises. Name the files you searched for the same pattern. What did you find? If nothing, say what you searched for and where.

**Is your review_outcome honest?** `clean` means you found no issues — did you actually look, or did you rubber-stamp? `findings` means issues exist — are they real and actionable, or nitpicks inflated to findings? `halt` means a fundamental issue — did you justify it?

**Did you identify all gaps?** Anything incomplete, incorrect, or missing in the fix — is it in your findings with a specific file, line, and what to do about it?

The work these raise gets done now, before you signal done again.
