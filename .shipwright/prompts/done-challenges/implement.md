You were here to make the ticket's outcome real in production code — code that runs, that the system reaches, and that the next person can extend. Before you signal done, go and look at your diff. Every answer wants a name, a path, a count, or a quote.

**Overreach — did you do a job that was not yours?** Name what you changed that the ticket did not ask for: adjacent code restructured, a second concern folded in, a question the ticket left open that you settled in code rather than by asking. Refactoring you needed to land the change is legitimate — say what forced it. Name anything in the diff that costs more machinery than the outcome required — a config option nobody will use yet, an abstraction layer with exactly one implementation, code you hand-rolled that the codebase or an already-installed dependency already provides — and cut it before you signal done.

**Shortfall — did you do the job below the bar?**

- List every parameter, option, config field, flag, event, hook, or optional dependency this change declared. For each, name the production file and function that supplies it and the one that reads it. A default production genuinely relies on is correct; a default that exists so existing call sites need not be touched is wiring you left undone.
- Name anything you widened or loosened instead of narrowing the value or updating the callers, and the constraint that forced it.
- Name where you looked for existing code that should now route through, register with, or delegate to this, and what you found. "There are none" counts only if you say what you searched for. Name any logic you wrote that already exists elsewhere here.
- Name any unit that grew past comfortable reading because the ticket named that file. A ticket names a destination, not a container.
- Name every suppression, skipped test, broad catch, and weakened assertion you introduced, and the constraint each cites. Any without one is a hack — fix the cause.

**Abdication — did you really play the role?**

- Name anything returning a plausible value where it cannot yet do the real thing. Code not yet reached that fails loudly is honest; a stand-in that answers is not.
- Name the test that exercises this code together with what it is coupled to rather than standing in for that collaborator. If you deleted this feature from production now, name the tests that would go red.
- Name each acceptance criterion and where in the code it is met. "Mostly" is not met.
- Quote the command you ran to verify and what it printed. If you are recalling, run it now.

The work these raise gets done now, before you signal done again — not recorded as a limitation, not left to a later ticket.
