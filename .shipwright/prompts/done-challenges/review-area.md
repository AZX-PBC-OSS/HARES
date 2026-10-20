You were here to review this change through one area's lens, at a depth a generalist could not reach, and to produce findings — not fixes, and not a pass over the whole change. Before you signal done, read your findings back. Every answer wants a name, a path, or a count.

**Overreach — did you do a job that was not yours?** Name every finding that sits outside your assigned area. Another specialist owns each of those, and the consolidator has to merge the duplicates — a finding raised twice costs more than one raised once, and it crowds out the one only you could have made. Name any finding that prescribes the fix rather than the defect, and any warning raised to demonstrate thoroughness.

**Shortfall — did you do the job below the bar?**

- For each finding, name the thing in this area's body of knowledge it turns on — the rule, the specification clause, the failure mode, the platform behaviour. Findings any generalist would have made are not what this area was spawned for.
- Name the surfaces in this change where this area applies, and the ones you examined. Anything on the first list and not the second is unreviewed, not clean.
- Name the files you opened beyond the diff and the callers you traced. What should have changed for this area and did not is invisible to a diff-only read.
- Name where you checked how the rest of the project already handles this area's concerns, and what you found. Handling it differently here is a finding.

**Abdication — did you really play the role?**

- If you found nothing: name what you looked for, where you looked, and the specific thing whose absence would have been a finding. "No issues in this area" states an outcome and shows no work.
- For every finding you downgraded or chose not to raise: name the consequence you judged too small and the production path proving the code is live. "Nothing breaks today" is not an answer — nothing breaks today when code is never reached.
- Name the reference you worked from for this area and one thing in it you decided does not apply here, and why. An area applied wholesale is a checklist; an area applied with judgment names its exclusions.

The work these raise gets done now, before you signal done again.
