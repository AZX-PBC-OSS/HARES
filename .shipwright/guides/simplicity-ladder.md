# The simplicity ladder

Before writing new code, check each rung in order. Stop at the first one that resolves the need — do not descend further than the problem requires.

1. **Does this need to exist at all?** A speculative or unstated need — "might want this later," "in case the requirement changes" — is not a need. Skip it; add it when the real requirement shows up.
2. **Is there already a helper, util, or pattern in this codebase that does this?** Search before writing. A second implementation of the same behavior is a maintenance liability from the moment it lands, not a future one.
3. **Does the language or framework's standard library already do this?** Standard-library functions are tested, documented, and understood by every future reader. Hand-rolled equivalents are none of those things.
4. **Does a native platform feature cover it?** A browser, OS, or runtime primitive (e.g. a built-in element, a platform API, a native validation) is maintained by someone else and ships with the platform. Prefer it over a library that wraps the same capability.
5. **Does an already-installed dependency solve it?** Check the project's existing dependencies before reaching for a new one. A new dependency for a need one existing package already covers is unjustified weight — new surface area, new version to track, new thing to audit.
6. **Can it be expressed in one line or expression?** If the logic reduces to a short, direct expression, write that — not a named function, class, or configuration layer around it.
7. **Only then: write the minimum new code that satisfies the requirement.** No speculative configurability, no unused abstraction, no parameters without a caller. Build what the requirement needs today; extend it when a second, real requirement arrives.

The ladder is a search order, not a ranking of virtue — rung 7 is not a failure, it's simply where the first six did not apply. Landing on rung 7 without checking 1–6 is the mistake this guide exists to prevent.
