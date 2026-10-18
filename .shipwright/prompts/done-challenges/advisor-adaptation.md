You were here to design the flow that will actually run this initiative, not a flow that merely passes validation. Before you touch the done-signal, go back over the recipe you wrote as if you were the operator about to trust it unattended. Every answer wants a step id, a name, or a quote from the recipe.

**Overreach — did you include steps that do not earn their place?** Name every step whose removal would cost this initiative nothing: a step that restates what an earlier step already produced, a review or gate on work too small to need one, a step included because the canonical recipe had it rather than because this task needs it. A step is performative when nothing downstream would break, and no risk would go unchecked, if it were deleted. Cut it, or name the specific risk it catches that nothing else does.

**Shortfall — did you omit a step that would improve the outcome?**

- Walk the task end to end and name any point where the flow moves forward on an unverified claim: an artifact nothing checks, a decision nothing reviews, a change nothing gates. For each, say what step would close the gap and why you left it out.
- Name any risk specific to this task — an area of the codebase with a history of regressions, a cross-cutting concern (a11y, i18n, security, performance) the task's scope touches — that the flow does not address anywhere.
- If a step you omitted is genuinely handled elsewhere (an earlier step's scope covers it, or the operator declined it), say where. "It wasn't needed" only counts when you can say why.

**Abdication — did you actually reconsider, or just re-read your own reasoning?**

- State the strongest case that this flow is wrong for this task — too heavy, too thin, mis-ordered, missing its one load-bearing step — and refute it with something specific from the recipe. If you cannot refute it, fix the recipe before you touch the done-signal again.
- Quote the step id and rationale for the one step you were least sure about when you wrote the plan. If you cannot name one, you did not reconsider anything.

Revise the recipe now if this reflection found anything real, then re-validate it against the schema before you signal done again — not recorded as a limitation, not left for the operator to catch.
