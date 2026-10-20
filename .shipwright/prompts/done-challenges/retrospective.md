You were here to install a defence that catches this class of bug earlier next time. The retrospective is the record of that; it is not the defence. Before you signal done, go and look at what you actually changed. Every answer wants a name, a path, or a quote.

**Overreach — did you do a job that was not yours?** Name anything you changed beyond the defence and the record: behaviour altered, a broader hardening pass, code refactored because you were in the file. A defence that arrives with unrelated changes attached is harder to accept than one that stands alone.

**Shortfall — did you do the job below the bar?**

- Name the file you changed to install the defence, and quote the line you added. If the only place the defence exists is the retrospective you are writing, it is not installed — nobody opens this file again.
- Say what the defence makes impossible, and name the check that would now fail if this same shape reappeared elsewhere. If it makes nothing impossible and only asks a future reader to remember, it is a note rather than a defence — ask whether a mechanism could carry it instead.
- Name the other places in the codebase where this same broken shape is still possible today, and say what you searched for to find them. A defence installed at one site while the shape survives next door fixes this instance only.

**Abdication — did you really play the role?**

- If you installed nothing anywhere: name specifically what stopped you. Some classes genuinely have no mechanism available and saying so precisely is a real result — but writing that this ought to be recorded somewhere is the failure of this role, and if that somewhere exists, go and record it now.
- Compare the first diagnosis against the cause that turned out to be real. Name what made the wrong one look right. That is the part of this worth carrying forward, and it is the part a retrospective written from the fix alone never contains.

The work these raise gets done now, before you signal done again.
