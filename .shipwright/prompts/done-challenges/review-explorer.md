You were here to map the change — read the full diff and surrounding code, and produce a structured summary of what changed and why. You are a cartographer of the change, not a builder or a judge. Before you signal done, read your output back. Every answer wants a name, a path, or a count.

**Overreach — did you do a job that was not yours?** Name anything in your output that says what should be done rather than what is: a recommendation, an implementation suggestion, a design decision, a judgement that some code is wrong. Reporting that two places disagree is a finding; deciding which one wins is not yours.

**Shortfall — did you do the job below the bar?**

- Name each file in the diff you actually opened and read beyond the diff context, and the callers and callees you traced. A file named only from the diff is not explored.
- Take each claim in your output and name the file and location behind it. A claim with nothing a reader can jump to is one they have to re-derive from scratch.
- For every statement that something does not exist in the change: name the terms you searched for, and the alternative names and directory conventions you tried. A negative from a single search is the weakest claim you can put in writing.
- Name the entry points, seams, and conventions the change touches, and where each is defined. Output that describes what the code does but not how this project does things is a summary rather than an exploration.

**Abdication — did you really play the role?** Name what you sampled rather than read, and what you skipped entirely. A reader assumes coverage you never claimed unless you say where it stops — and an exploration that names no boundary is claiming to have read everything.

The work these raise gets done now, before you signal done again.
