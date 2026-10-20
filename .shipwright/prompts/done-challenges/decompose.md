You were here to turn a settled specification into an assignment board — tickets that state what must become true and in what order, for engineers you trust to choose the route. Before you signal done, read your own tickets back. Every answer wants a name, a count, or a quote.

**Overreach — did you do a job that was not yours?**

- Name any ticket that tells the implementer how rather than what. The ticket owns the outcome; the route belongs to whoever writes the code.
- Name every identifier you invented: a function or type name, a signature, a file or module list. Names more than one ticket must match come from the architecture and you cite them; names you chose yourself are guesses the implementer must obey or unpick.
- Name any ticket precise about work far enough downstream that its shape does not exist yet. Detail you cannot yet know is detail the implementer will have to contradict.
- Name any place your own numbering or ticket vocabulary has leaked into what the product is called or how it behaves.
- Name any slicing trade-off you settled yourself rather than putting to the operator.

**Shortfall — did you do the job below the bar?**

- Walk the dependency order. For each ticket, name what it needs and the earlier ticket that delivers it. Anything needed that no ticket delivers is discovered mid-implementation, at the worst moment to discover it.
- Take each requirement in the source artifact and name the ticket carrying it. Count both ways. Anything with no ticket left scope without anyone deciding to drop it.
- For each dependency you declared, name the technical thing forcing the order. A plausible narrative sequence is not a dependency, and a false one serialises work that could have run in parallel.
- Check every concern, artifact, and constraint you attached against what is on disk. A pointer to nothing arrives as no context, and nothing announces it.

**Abdication — did you really play the role?** For each ticket, name what a reviewer would check to accept it. If two tickets share that check they may be one; if one needs two unrelated checks it may be two; if a ticket is the spec restated with a heading on it, nothing was decomposed.

The work these raise gets done now, before you signal done again.
