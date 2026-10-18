You were here to commit to one small, verifiable approach for fixing the bug — not to write the fix, not to touch code, not to restate the RCA. Before you signal done, re-read your plan. Every answer wants a name, a path, or a quote.

**Overreach — did you do a job that was not yours?** Name any production code you touched, any test you wrote, any file you edited outside `fix-plan.md`. Your job is planning only. If you made a code change, you have not planned the fix; you have skipped past this phase.

**Shortfall — did you do the job below the bar?**

- Quote the RCA's confirmed root cause, then quote your plan's chosen approach. If the two do not target the same thing, your plan fixes a symptom, not the cause.
- Name the alternative approach you considered and the reason you rejected it. A plan with one approach and no comparison has not chosen anything — it has just described the first idea.
- Name every file and call site the fix will touch, and name at least one thing it will deliberately leave untouched. A blast radius that names nothing is unstated, not small.
- Quote the failing-test command and the passing commands your plan says will verify the fix. If you invented a command the RCA or reproduction never named, say why.

**Abdication — did you really play the role?**

- If your plan set `plan_outcome: ready`, state the strongest reason it should have been `blocked` instead, and refute it. If you cannot refute it, change the verdict now.
- Name the exact function, module, or file your plan says will change, and its new behaviour. If you cannot name it without inventing a signature the code doesn't have yet, your plan is more specific than it has any right to be — pull it back to what you can honestly commit to.

The work these raise gets done now, before you signal done again.
