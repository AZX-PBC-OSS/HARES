# Agent role specifications

Each file here defines a named agent persona: the human role it plays, what that role is accountable for, the failure mode of that role, and the tools it may use within a workflow.

## Writing a role prime

A model given a task performs the task. A model given a *role* brings the judgement, priorities and suspicions that come with the role — which is most of what you are buying. Write the prime, not just the job.

**Name the human role.** "You are the implementer", "You are the scrutineer", "You are the last gate before this ships." A recognisable human role carries a register the model already knows. Do not invent a title nobody holds.

**Name what the role is accountable for.** Not the outputs it produces — the outcome it owns. An implementer is accountable for code that works in production, not for code that passes checks. A reviewer is accountable for what reaches production, not for the length of the findings list. State the accountability and the standard follows from it.

**Name the failure mode of the role.** Every role has a characteristic way of failing that is not laziness: the reviewer who confirms rather than challenges, the implementer who builds what was described instead of what was needed, the last gate that inherits its verdict from the fact that earlier rounds converged. Naming it is the single highest-leverage line in a persona, because the model can then recognise itself doing it.

**Name the register.** Direct, adversarial, sceptical, exhaustive, terse — say which. A role without a register defaults to the model's own, which is agreeable.

**A role with a weak prime defaults to eager-assistant** — helpful, affirming, and inclined to return the answer that closes the interaction. Eager-assistant asked for a verdict returns approval; asked whether the work is done says yes. This is not a jailbreak or a defect in the model; it is what an unprimed assistant is for. The prime is what replaces it.

## What does not belong here

**Do not enumerate what the model already knows.** It knows what a code smell is, what DRY means, what a good test looks like. Listing those spends the attention budget of the lines that actually change behaviour. Prime the role and let its knowledge come with it.

**Do not put mechanism here.** Gates, schemas, and validators live in the flow and the coordinators. A persona that recites rules a mechanism already enforces is redundant; a persona that recites rules no mechanism enforces is a wish.

**Do not name a stack.** These personas ship to every project. Nothing here may presuppose a language, a test runner, a linter, or a package manager — every line must be actionable in a shell-script repo and a systems-language repo alike.
