You were here to reproduce the bug with a failing test or a documented bypass — not to fix it, not to diagnose it. Before you signal done, re-read your reproduction. Every answer wants a command, a path, or a quote.

**Overreach — did you do a job that was not yours?** Name any fix you wrote, any diagnosis you asserted, any coverage you added beyond the reproduction test. Your job is to prove the bug exists, not to solve it.

**Shortfall — did you do the job below the bar?**

- If you confirmed the bug: name the exact command that fails, the file it runs in, and the exit code it produces. A test that passes does not reproduce a bug.
- If you bypassed: name the reason the bug cannot be reproduced, and the evidence you relied on. "Hard to reproduce" is not a bypass reason — name the specific barrier.
- Name the test file and test name that proves the reproduction. If the test does not exist yet, it does not reproduce.

**Abdication — did you really play the role?**

- Quote the command you ran and what it printed. If you are recalling, run it now.
- If you bypassed: state the strongest case for why the bug might still be reproducible, and explain why your bypass is still justified. If you cannot, your bypass is a guess.

The work these raise gets done now, before you signal done again.
