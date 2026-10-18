---
artifact: review-findings
role: reviewer
description: The consolidated review report — findings from an autonomous, adversarial code review. The only machine-unchecked artifact in the review pipeline; the operator is the final gate.
---

## The artifact

An adversarial but fair review of a colleague's implementation. Its author reads the actual code — not just the diff — and names specific defects with locations and concrete consequences. Findings are ranked by severity, each grounded in something a reader can verify: a failing test, a symbol with no caller, a type that does not hold, or a reasoned engineering judgment a senior colleague would defend.

It earns its place by catching what the implementer's proximity hid: code nothing calls, options nothing supplies, tests that prove their own setup, wiring left undone. A shallow review is worse than no review — it gives false confidence.

It must never fix structural or behavioural defects. The reviewer may write a failing test that proves a finding, but the fix belongs to the implementer. A verdict that contradicts its own evidence discredits everything else in the file.

## Failure modes

- A finding with no location, no concrete consequence, and no named production path — a vibe dressed as a defect.
- A verdict that contradicts the prose body. The outcome vocabulary is `clean` (nothing warrants flagging), `clean_with_nits_applied` (only trivial fixes the reviewer applied directly), `findings` (Critical or Warning issues exist), `halt` (unresolvable by iteration), and where the schema offers it `test_gaps` (every finding is a missing test for otherwise-correct behavior). Critical or Warning findings in the prose alongside a `clean`-family verdict is a self-contradiction nobody downstream can trust.
- A suppression or documented limitation re-flagged when its named constraint is real — churn that erodes the implementer's trust in the review loop.
- A design suggestion presented as a finding — "this could be cleaner" is an Observation, not a Warning; elevating it forces an implementation cycle that costs more than it earns.
- A failing test written but not reported as a finding — evidence of a defect quietly deleted instead of filed, so the gap survives and the next reviewer re-derives it.
- Code the reviewer did not read — a diff-only review that misses broken callers, stale references, and integration points that should have changed and did not.
- An unverified claim presented as a validated finding — judgment is not weaker, but a finding wearing a "verified" badge with nothing reproducible behind it misleads the reader about what a tool could catch.
