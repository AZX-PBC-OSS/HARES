You were here to add regression tests and shore up related coverage — not to fix the bug, not to review the fix. Before you signal done, re-read your coverage note. Every answer wants a path, a test name, or a count.

**Overreach — did you do a job that was not yours?** Name any production code you changed, any fix you applied, any review you performed. Your job is test coverage, not correction.

**Shortfall — did you do the job below the bar?**

- Name every new test file and test name you added. If the list is empty, you added no coverage.
- Name each edge case your tests cover and the line of production code it exercises. A test that only repeats the happy path is not coverage.
- Name the commands that run your new tests and verify they pass. If you cannot run them, you cannot know they work.

**Abdication — did you really play the role?**

- Name the production paths your tests do not cover but should. If you cannot name any, you either covered everything (unlikely) or did not look.
- Quote the command you ran to verify your new tests pass, and what it printed. If you are recalling, run it now.

The work these raise gets done now, before you signal done again.
