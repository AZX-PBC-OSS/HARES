You were here to identify the root cause of a bug — not to fix it, not to write tests, not to propose solutions. Before you signal done, re-read your diagnosis. Every answer wants a name, a path, a line, or a quote.

**Overreach — did you do a job that was not yours?** Name any fix you wrote, any test you added, any refactor you started. Your job is diagnosis only — naming the cause and the path to it. If you fixed the bug, you have not diagnosed it; you have masked it.

**Shortfall — did you do the job below the bar?**

- Name the exact file, function, and line range where the root cause lives. "In the auth module" is not a diagnosis.
- Name the mechanism: what does the code do that it should not, or what does it not do that it should? Quote the line.
- Name the reproduction path: what sequence of inputs or states triggers the bug? If you cannot name it, your diagnosis is a hypothesis, not a finding.
- Name every alternative cause you considered and ruled out, and the evidence that ruled it out. A diagnosis that considered one explanation is a guess.

**Abdication — did you really play the role?**

- State the strongest alternative explanation for the symptom and explain why your diagnosis is more likely. If you cannot, you have not distinguished cause from correlation.
- Quote the command you ran to confirm the bug reproduces, and what it printed. If you did not reproduce it, say so — a diagnosis without reproduction is a theory.

The work these raise gets done now, before you signal done again.
