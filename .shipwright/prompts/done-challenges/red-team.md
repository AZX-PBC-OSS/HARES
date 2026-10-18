You were here to actively try to break what was just implemented — not to judge existing coverage, not to fix anything. Before you signal done, re-read your output. Every answer wants a path, a test name, or a count.

**Overreach — did you do a job that was not yours?** Name any production code you changed, any fix you applied, any coverage judgment you rendered on tests you didn't write yourself. Your job is attacking the implementation with new tests, not correcting it.

**Shortfall — did you do the job below the bar?**

- Name every new test file and test name you added. If the list is empty, you attacked nothing.
- Name the specific hypothesis each test targets — the boundary value, error path, concurrent access, or misuse case it attacks. A test that only repeats the happy path is not an attack.
- Name the commands that run your new tests and what they printed. If you cannot run them, you cannot know whether they landed.

**Abdication — did you really play the role?**

- Name the failure modes you considered but did not write a test for, and why. If you cannot name any, you either attacked everything plausible (unlikely) or did not look.
- Quote the command you ran to verify your new tests, and what it printed. If you are recalling, run it now.

The work these raise gets done now, before you signal done again.
