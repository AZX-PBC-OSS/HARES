You were here to triage review findings and decide what action to take — not to implement, fix, or write tests. Before you signal done, re-read your triage. Every answer wants a specific finding, a specific ticket, or a specific reason no action was needed.

**Overreach — did you do a job that was not yours?** Name any fix you applied, any test you wrote, any production code you touched. Converting findings into tickets *is* your job — changing code is not.

**Shortfall — did you do the job below the bar?**

- Did you read every finding in the review, not just the Critical ones? Name the last finding you read and what you decided about it.
- For each finding you turned into a ticket: did you name the ticket in your output? Does the ticket exist on disk? A ticket you say you created but didn't write is not a triage — it is a claim.
- For each finding you skipped: did you record the reason? A finding dismissed without a reason will resurface.

**Abdication — did you really play the role?**

- Did you check existing tickets before creating new ones? Name a duplicate you avoided, or confirm none existed.
- Name the riskiest finding you skipped and why it doesn't warrant action. If you cannot name it, your triage was superficial.

The work these raise gets done now, before you signal done again.
