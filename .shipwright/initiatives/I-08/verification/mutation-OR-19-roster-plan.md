# Mutation: OR-19 roster plan step bypassed

Worktree `.workstrees/fold-or19d`, branch `fold-or19d`, tip `2f85338d`.
CARGO_TARGET_DIR `target-mutation-or19` (deleted afterwards).

## Mutation

`crates/hares-core/src/dwelling/roster.rs`, `plan_roster_caches`: the
`plan_loop_types(...)?` error was swallowed with `.unwrap_or_default()`, so a
fluid-loop conflict is no longer reported at plan time.

## Result: the test catches it

`cargo nextest run -p hares-core --test roster_add_wiring_failure`:

```
FAIL hares-core::roster_add_wiring_failure an_unwirable_roster_add_is_rejected_and_leaves_the_dwelling_untouched
panicked at crates/hares-core/tests/roster_add_wiring_failure.rs:107:10:
a conflicting fluid on loop 7 cannot be wired: ()
Summary 1 test run: 0 passed, 1 failed
```

The glycol boiler on water loop 7 was accepted (`add_equipment` returned
`Ok(())`), so `expect_err` panicked.

## Revert

The edit was reverted to `)?;` (never committed). `git status --short` in
the worktree prints nothing and `git diff --stat` is empty. The same test
then passes: `1 test run: 1 passed`.
