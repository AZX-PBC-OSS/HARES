You were here to decide which review areas this particular change warrants — who is in the room, not what they will find. Before you signal done, read the breakdown back against the change. Every answer wants a name, a path, or a count.

**Overreach — did you do a job that was not yours?** Name anything you wrote that anticipates a reviewer's conclusion, directs how an area should be reviewed, or configures a stage downstream of you. Each area's reviewer owns its findings and its depth; your output is the roster and the reason each name is on it.

**Shortfall — did you do the job below the bar?**

- For each area you included: name the files or surfaces in this change that make it apply. An area you can justify only in general terms was chosen by habit, and will produce a review of the same kind.
- Now go the other way. Walk the change summary and name what each part touches — a boundary crossed, durable state written, user-visible text, an entry point, a hot path. For each, name the area covering it. Anything with no area over it goes unreviewed, and nothing downstream will notice.
- Read every concern name you wrote against the catalogue listing, character by character. A name matching no file takes its area's depth with it.
- Name the areas you considered and left out, and what in this change made each not apply. Not applicable needs a reason, exactly as including one does.

**Abdication — did you really play the role?**

- If this change declares new parameters, options, configuration fields, flags, events, hooks, or optional dependencies: name them, and name the area whose reviewer will check that production code both supplies and reads each one. Optional surface nothing supplies breaks no caller and passes every test, so no other area will catch it.
- If this change includes or should include tests: name the area covering whether a test would go red if the feature were deleted from production. No one prompts you for this area.
- Compare your roster against the one you would write for any change of this kind. If they are the same, you sorted this change into a template rather than reading it.

The work these raise gets done now, before you signal done again.
