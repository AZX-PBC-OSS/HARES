---
description: "Adversarial test design for a just-changed surface: attacker/misuse mindset, boundary and equivalence-class analysis, hostile or malformed input, race conditions and repeated access, and resource exhaustion — what to attack and how, as distinct from whether a given test is well-built (test-quality) or at the right level (testing)"
---

# Concern: Red-teaming

Applies to a pass whose job is to actively try to break code that already exists
and already has coverage — not to write the first tests for new behaviour (that is
the testing concern) and not to judge whether an existing test is well-shaped (that
is the test-quality concern). This concern is about **where an attacker or a hostile
caller would look**, and how to turn that into a test that proves the code holds or
does not.

This document is language- and framework-independent. Translate every example into
whatever the project's stack and test tooling already use.

## Think like a misuser, not a happy-path caller

The implementer wrote code that does what they intended. Your job is to find what
they didn't intend. For each piece of changed or added behaviour, ask: what input,
sequence, or environment would this code mishandle? A hypothesis that isn't grounded
in something specific you read in the code is not an attack plan — it's generic
fuzzing dressed up as rigor. Name the file, the function, and the exact assumption
you're testing before you write the test.

## Boundary and equivalence-class analysis

For every input with a range, a length, a count, or a type union, identify the
partitions and attack the edges between them, not the middle of each partition:

- **Numeric ranges**: zero, negative, the documented maximum, one past the maximum,
  the smallest representable increment, overflow/underflow for the type in use.
- **Collections**: empty, single element, the documented maximum size, one past it.
- **Strings**: empty string, whitespace-only, maximum length, encoding edge cases
  (multi-byte characters, null bytes, control characters) if the surface accepts
  free text.
- **Optional/nullable fields**: every combination of present-with-value,
  present-as-empty, and absent that the type system allows — not just the one the
  happy path exercises.
- **Enums and unions**: every variant, especially ones added recently or rarely
  exercised by existing callers.

## Error paths and dependency failures

Anywhere the new or changed code calls another component — a service, a database, a
filesystem, another module — ask what happens when that dependency fails, times out,
returns malformed data, or returns successfully but with an unexpected shape. A
function that assumes its dependencies always succeed is one failure away from an
unhandled exception or, worse, a silently wrong result.

## Concurrency, repetition, and ordering

- **Idempotency**: does calling this twice with the same input produce the same
  result, or does it corrupt state, double-charge, or duplicate a record?
- **Races**: can two callers reach this at the same time and interleave in a way
  that violates an invariant the code assumes is single-threaded?
- **Ordering**: does the code assume operations happen in a particular sequence
  that a caller is free to violate?

## Misuse of the new surface

Anything the type system permits but the implementer didn't expect is fair game: a
caller that passes the maximum of one field with the minimum of another, a caller
that skips an expected setup step, a caller that holds a reference past the point
the implementer assumed it would be discarded. If the new surface is a public
function, API endpoint, or exported type, read its full signature and ask what
combination of arguments its author never tried.

## Resource exhaustion

Where the change accepts caller-controlled size (a list length, a file size, a
recursion depth, a number of retries), ask what happens at a size large enough to
degrade or crash the process, not just large enough to be "a lot."

## What earns a test versus what doesn't

A hypothesis earns a test when you can name the specific line or assumption it
targets and the concrete way it could fail. A hypothesis that is really "more tests
are always better" does not — write it down, reject it, and say why in your
reflection rather than turning it into noise. Genuinely finding nothing after a real
search is a legitimate, reportable outcome.

## A red-team test that fails is the deliverable, not a mistake

If an attack lands — a new test goes red — that is a real defect in code everyone
assumed worked. Report it exactly as written, leave it failing, and let the next
implementation pass make it pass. Weakening the assertion, adjusting the expected
value, or deleting the test converts a finding into a false "clean."

## Relationship to neighbouring concerns

- **Testing** (test strategy, level selection, doubles): use it to decide where an
  attack test belongs — unit, integration, or end-to-end — once you know what
  you're attacking.
- **Test-quality** (is a given test well-built): use it to make sure the attack test
  itself asserts observable behaviour and earns its place once you've written it.
- **Security**: an attack that specifically targets authentication, authorization,
  injection, or data exposure is also a security concern — flag it there too if the
  project applies that concern to this change.

## Reviewer checklist

- [ ] Each new test names the specific hypothesis it attacks, not a generic variation
- [ ] Boundary values and equivalence-class edges covered for every constrained input touched by the change
- [ ] Error paths of every dependency the changed code calls are exercised, not just the success path
- [ ] Idempotency, ordering, or concurrency addressed where the change touches shared or mutable state
- [ ] A failing attack test is reported as a finding and left red, never weakened or deleted
- [ ] No production code changed by this pass
- [ ] If nothing was found, the reasoning and search performed is stated, not just asserted
