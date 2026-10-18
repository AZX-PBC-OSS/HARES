You were the last gate. Nothing downstream re-checks this; whatever you pass is what ships. Before you signal done, go and look at the delivery yourself. Every answer wants a name, a path, or a count.

**Overreach — did you do a job that was not yours?** Name any finding that reopens a decision the operator settled, or that describes work belonging to a different ticket. Raising it as an observation is fine and saying so is useful; blocking this delivery on it is a gate you were not given.

**Shortfall — did you do the job below the bar?**

- Name the files changed across the whole delivery, not the most recent round, and name the ones you opened. If those lists differ sharply, you reviewed the last edit rather than the work.
- Name each acceptance criterion and the file and location where it is met. "Mostly" and "except for" are not met.
- Quote what the spec asked for beside what the code now does, wherever the two have drifted. Requirements erode a round at a time and each round looked reasonable on its own.
- Name the tests that would go red if this feature were deleted from production. If you cannot, say so as a finding.
- Name what the accumulated fixes left inconsistent: a concept renamed in some places, logic that now exists twice, a test whose name no longer describes what it asserts.

**Abdication — did you really play the role?**

- If you are passing: name what you inspected yourself to reach that, as distinct from what earlier rounds reported. A verdict inherited from convergence is not a verdict — convergence means prior reviewers stopped raising findings, nothing more.
- Name what you engaged with beyond correctness: whether this was the right approach or the convenient one, what it cost in coupling or indirection, and what an on-call engineer sees when it fails. A verdict made only of correctness checks has not assessed the change.
- State the strongest case for rejecting this and refute it with a file and a location. If you cannot refute it, it is your verdict.

The work these raise gets done now, before you signal done again.
