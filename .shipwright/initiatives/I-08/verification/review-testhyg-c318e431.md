# Review: fix-testhyg at c318e431 (22 commits on 2f85338d; 9 new since 6c2e34d5)

## Verdict: CHANGES-REQUIRED (fold blocked, not by a defect in the code)

0 high, 0 medium, 4 low. All 8 previous findings are fixed at the root and mutation-proven. Nothing was lost in the rebase.
The blocker is mechanical: the head is now `1b708ccb` (6 operator commits on `2f85338d`), `c318e431` is not a fast-forward,
and cherry-picking the 22 commits onto `1b708ccb` conflicts (see Fold readiness). Resolving it needs the implementer; the
review below stands for the branch content.

## Findings

### Low

**L1. The `uname -p` exception keys on argv only, so it can be used to run any program.**
`tests/python/offline_guard.py` `_local_command` / `_violation`. `subprocess.run(["uname","-p"], executable="/bin/echo")`
was not refused and ran `/bin/echo` (printed `-p`). The same holds for a PATH that resolves `uname` to another program.
Contrived (the test author must do it on purpose), but the exception is narrower in argv than in what it admits. Fix:
require `executable` (or `argv[0]` resolved) to be `uname`/`/usr/bin/uname`/`/bin/uname`.

**L2. A federate registration retry fires for any failure while the broker is reachable, not just a core port bind.**
`python/ochre_next/helics/federate.py` `create_value_federate` (the `except helics.HelicsException` + `_accepts_connections`).
Scenario (probe, HELICS 3.6.1, zmq broker): register federate `dup` twice. The second registration fails with
`duplicate federate name detected`, is retried 3 more times on fresh cores, and then raises with the note "a core that
could not bind its port is the likely cause", which is false. Cost on a persistent error: up to 4 times the connect
timeout. The failed cores `core_dup_1..3` stay connected to the broker (HELICS query `cores` listed them). Orphan cores are
not new: a single failed registration (the pre-branch behaviour, `helicsCreateValueFederate` once) also left an orphan that
made the legitimate federate's `enterExecutingMode` hang for the full probe time (100 s, probe killed), so the retry only
multiplies them. For the real port-bind case no orphan appeared (the repo's test enters executing mode after the retry).
Fix: retry only when the exception text is the bind/unreachable error, or drop the "likely cause" note unless the
failure message matches. Also `_held_port_pair` in the tests can hit the same probe-then-bind race it simulates (very rare).

**L3. Ctrl-C can still wait for a connect or TLS handshake in flight.**
`python/ochre_next/data/resstock.py` `_Connections.trace`/`cut`. Only sockets already connected are cut; a worker blocked in
`connect_tcp` or `start_tls` (up to `_DOWNLOAD_TIMEOUT_S` = 120 s on a black-holed route) is joined by
`pool.shutdown(cancel_futures=True)`. Normal networks connect in milliseconds, so this is a tail case; the docstring does
not state it. The urllib fallback tail is documented.

**L4. README still describes the removed path.** `README.md:120-123` says fleet downloads run "concurrently via `httpx` when
available" and "`httpx` is recommended for async fleet downloads". Downloads are now a thread pool on every transport and
there is no async path; only the Ctrl-C cut needs httpx.

Note (not a finding): the `network` marker is declared but no test uses it. A failing Python test keeps its directory in
`pytest-of-<user>` by design (policy `failed`, count 3, which equals pytest's default); the `[stray]` check excludes that
directory.

## 1. Rebase: nothing lost

Method: for every file touched by `38398bd4..6c2e34d5` or by `2f85338d..83e7a801` (the 13 rebased commits), the +/- line
multisets of the two diffs were compared; `git range-diff` pairs all 13 commits (2 are identical, 11 differ only by
conflict resolution). The differences, each read:
- `schedule_path` `Some(...)` / `scratch.schedule()` (engine.rs, autosize_from_config.rs, fleet.rs, dwelling/mod.rs
  helper name): the head's `Some` is kept, the branch's temp-dir helper is used. Present at the tip.
- `SyntheticTomlSpec` (interest_filtering_regression.rs): kept; only the `&Path` parameter is the branch's.
- `TypedDict` `_OutputKwargs` (test_ev_tariff_integration.py, test_py_metrics.py): kept, with `NotRequired[str]`
  and the branch's `output_in_test_dir` fixture; `Mapping` typing in conftest.py.
- `is_json_object` (helics/dwelling.py, fleet.py): present (`dwelling.py:1013-1025`, `fleet.py:839-902`).
- `test_observer_diagnostics.py`: deleted on the head (`git ls-tree 2f85338d` and `c318e431` both empty); the branch's edits
  to it are correctly dropped.
- `test_ochre_parity.py`: head had removed `_run_hares_7d`; the branch's `output_dir` edit survives on `_run_hares_simulate`.
- pre-rebase `_new_federate_info` / `helics = None` guards: superseded by `_federate_info` + module-level skip.
- Every line the head added (`38398bd4..2f85338d`) was checked for presence in the tip file: all absent lines are
  the head's async client protocols (`_AsyncGetClient`, `Coroutine`), its fixed `FixedOffset::west_opt(7*3600)` spellings
  and fixed `/tmp` writers (replaced by `denver_offset()` and temp dirs), and the head's unquoted `pl.DataFrame` hint
  (replaced with the fixture signature). Nothing else is missing.
- Async fleet path: `_fetch_fleet_async`, `_download_building_async`, `_download_bytes_async`, `_noop` have no remaining
  reference in the tree or in `~/src/HARES_CALIBRATION_SYSTEM` (it calls only `fetch_resstock_building`; its `HARES/` is a
  separate copy with its own tests). `fetch_resstock_fleet`'s signature is unchanged and it is exported from
  `ochre_next.data`. Side benefit: no `asyncio.run`, so it works inside a running loop. Log wording changed
  ("skipped: ErrType: msg"); nobody parses it. See L4 for the stale README.

## 2. The 8 previous findings

| Previous | Fixed at | Mutation (scratch copy under the worktree's `target/`) |
|---|---|---|
| M1 unknown version returns `[]` | `_parse_version(version)` at `resstock.py:1083` | removed it: `test_an_unknown_version_is_refused_before_any_building_is_fetched` FAILS |
| L1 16 clients built | `_SharedClient.get` under a lock | removed the lock: `test_the_first_burst_of_downloads_builds_one_client` FAILS (with M1's test, 2 failed, 80 passed) |
| L2 `-E`, hostnames | `ISE` parsing and `_check_before_resolving` | probes: `-E`, `-I`, `-S`, `-sSE`, `-bE`, `-OOE`, `-X dev -E`, `-Xdev -E`, `-W d -sE`, `--check-hash-based-pycs always -E`, `-Ic`, `python3 -X dev -E` all refused at spawn; hostname connect/sendto refused unresolved |
| L3 xdist import-time attempt | `pytest_testnodedown` | disabled it: `test_an_attempt_outside_any_test_fails_the_session[2]` FAILS; probe `-n 0` and `-n 2` both exit 1 with the message |
| L4 failed dirs kept | policy pinned in `pyproject.toml` | not mutable; behaviour is the documented policy |
| L5 pyright error | `typings/helics.pyi` gains `helicsCleanupLibrary`, `HelicsException` | `pyright` on the tip: 0 errors, 0 warnings |
| L6 Ctrl-C | timed waits + socket cut | (a) `future.result()` instead of the slice loop: `test_ctrl_c_stops_a_fleet_with_downloads_in_flight` FAILS (child never exits, 60 s timeout); (b) removing `_shared_client.abort()`: same test FAILS (60 s). The child must be given the mutated `PYTHONPATH`; with the default env it imports the unmutated worktree, which is how a first run falsely survived |
| L7 port race | `create_value_federate` | `_CORE_PORT_ATTEMPTS = 1`: `test_a_federate_whose_core_port_is_taken_registers_on_a_fresh_one` FAILS |

(L8, the incomplete offset consolidation, is done by `34503778`; no `west_opt(7 * 3600)` remains outside the helper.)

## 3. Offline guard: bypass attempts

Probes run through the real plugin; a refused attempt fails its test at teardown, a passing test means a bypass.
Refused: ssl `wrap_socket` connect with a hostname and with an IP, `http.client`, asyncio `open_connection`,
`sock_connect` to an IP, datagram endpoints with a hostname and an IP, a `socket.socket` subclass overriding `connect`,
unbound `socket.socket.connect(s, ...)`, `fromfd` duplicates, `os.fork`+`execv`, `shell=True`, `os.system`, `uname -a`,
multiprocessing spawn, forkserver and fork (child got `NetworkRefused`). Bypasses: `ctypes` libc `connect` (documented),
raw `_socket.socket` with a hostname (documented), and L1. `-E` handling is correct, including `-X dev -E`, clusters, and
`-c`/`-m` ending the options.

## 4. L6 details

- Slices are not busy-waiting: 5 s wait through `_result_interruptibly` used 0.007 s CPU (10 wakeups a second).
- Root cause confirmed by minimal reproduction (`concurrent.futures` result waited on the main thread, SIGINT sent after 1 s):
  without polars `KeyboardInterrupt` after 1.0 s; after `import polars` not until the future ends (8.0 s).
  `sigaction(SIGINT)` flags are `0xc000000` without polars and `0x14000004` with it (SA_RESTART and SA_SIGINFO set).
- httpcore trace: httpx 0.28.1 and httpcore 1.0.9 are locked (`uv.lock`). `Trace` builds the event name from the logger name
  (`connection` + `connect_tcp` + `complete`) and passes `{"return_value": stream}` (`_trace.py`); `get_extra_info("socket")`
  is the public stream API. `start_tls` behind HTTP proxies is named `start_tls.complete`, so only the TCP socket is
  recorded there; shutting it down still kills the TLS stream. httpx has no upper pin (`>=0.28.1`), so a rename of those
  events would silently degrade the cut to the L3 case; the Ctrl-C test would fail and say so.

## 5. L7

Bounded: 4 attempts, fresh core names (`core_<fed>_<n>`), fresh port draws, the last error raised with a note listing
every init string, an unreachable broker raised at once, in-process cores not retried. See L2 for the over-broad retry and orphan cores.
No orphaned broker was seen (disconnect returned in 0.0 s).

## 6. Temp-file proof (isolated `TMPDIR` under the worktree, and a private empty tmpfs `/tmp` with `TMPDIR` unset)

| Line | Result | Left behind |
|---|---|---|
| nextest debug | 7326 passed | nothing (both) |
| nextest release | 7283 passed | nothing (both) |
| `-F observe` | 7384 passed | nothing |
| `-F dst` | 7339 passed | nothing |
| `-F profiling` | 7332 passed | nothing |
| `-p hares-core -F observe,profiling` | 1454 passed | nothing |
| `cargo bench --workspace --no-run` | pass | nothing |
| `cargo bench --workspace -- --test` | 417 benches pass | nothing (both) |
| `frame_golden compare-all`, default and `-F dst` | identical | nothing |
| full pytest (`-m ""`, `TMPDIR` inside the repo) | 1473 passed, 4 xfailed, 0 skipped | only the empty `pytest-of-rich`; `[stray]` tree diff empty |
| full pytest, private `/tmp` | 1473 passed, 4 xfailed | only the empty `pytest-of-rich` |

Also: `cargo fmt --check` and all 9 clippy lines with `-D warnings` pass; `maturin develop --release` left nothing.

## 7. HELICS stress

`pytest -n 8 -m "" tests/python/test_helics_integration.py` (35 tests): 10 of 10 runs passed, 92 to 112 s each, load
average 19 to 30 on 28 cores (the review's own builds ran at the same time), nothing left in `TMPDIR`. Not the 50 the
ledger asks for.

## 8. Block lines on the tip

- `[pyright]`: 0 errors, 0 warnings (run with the worktree venv; without it the imports do not resolve).
- `[writing]`: run on each of the 22 commits (message and added lines): no match; one author, no trailers.
- `[stray]`: tree diff empty after the full pytest; `TMPDIR` holds only `pytest-of-rich`.

## 9. Fold readiness

- `feat/output-results` is now `1b708ccb`, so `git merge-base --is-ancestor feat/output-results c318e431` fails (it was
  `2f85338d`, a fast-forward, at the start of this review).
- Cherry-pick of `2f85338d..c318e431` onto `1b708ccb` in a scratch worktree (deleted): conflicts at the second commit,
  `aa079f15`, in `crates/hares-core/tests/engine.rs` (the head now sets `defaults_path: Some(repo_defaults_path())` where
  the branch uses `scratch.*()`; three hunks) and `crates/hares-fleet/src/fleet.rs`. The head's commits also touch
  `tests/regression/helpers.rs` and `tests/warmup_regression.rs`, which the branch edits, so later commits may conflict
  too. The resolution is mechanical (keep `Some(repo_defaults_path())` and the scratch paths).
- Leaks added by the new head: none found by reading its diff of `.rs`/`.py` files. Its new writers use
  `tempfile::tempdir()` / `TempDir` (auto-removed); it adds no fixed `/tmp` path, `env::temp_dir` or `unique_temp_path`.
  The leak runs above are on `c318e431` only; they must be re-run after the rebase, since the conflicting files are the temp-file writers.
