# Review and red team: fix-testhyg at 6c2e34d5 (range 0216a35a..6c2e34d5, base 38398bd4)

Worktree `.workstrees/fix-testhyg` (detached at 6c2e34d5), `CARGO_TARGET_DIR=/home/rich/src/HARES/target-review-testhyg`,
`RUSTC_WRAPPER=` (the shared sccache server hung during the run; the build was not cached, which does not change
any result). Every run below used an isolated `TMPDIR` under the worktree's `target/rv/`; the namespace runs also had
a private, empty tmpfs as `/tmp` with `TMPDIR` unset, so a hard-coded `/tmp` path or `std::env::temp_dir()` use would
have shown up as well.

## Verdict: CHANGES-REQUIRED

0 high, 1 medium, 7 low. All 13 findings of the previous review are fixed at their root, the temp-file problem is
solved (nothing is left behind by any Rust line, the benches, `frame_golden` or the full pytest), and the HELICS file
passed 22 of 22 stress runs. The one medium is a behaviour regression the branch introduces in the fleet fetch: a
mistyped dataset version now returns an empty fleet instead of raising. It is a two-line fix with a test. The lows
are bypasses and gaps in the new offline guard, a race that defeats the "one shared client" claim, one pyright error
the head's gate will report, and leftovers from the previous review.

## Findings

### Medium

**M1. A fleet fetch with an unknown ResStock version now returns an empty fleet silently instead of raising.**
`python/ochre_next/data/resstock.py:945-995` (`_fetch_fleet`). Commit 183b0821 deleted
`cfg = _version_config(version)` from the top of `_fetch_fleet`. That call was the only early validation of
`version` for a fleet: `fetch_resstock_fleet` does not call `_version_config`, and `fetch_resstock_building` calls it
inside each worker, where the `ValueError` is caught by the per-building `except Exception` (line 985) and logged.
Failure scenario: `fetch_resstock_fleet(meta, n_buildings=500, version="2024.99")` used to raise
`ValueError: Unknown ResStock version`. It now logs 500 lines, downloads nothing and returns `[]`; a caller that does
not check `len(result)` carries on with an empty fleet (the project's rule is to error loudly, never to substitute a
silent default).
Evidence: probe test run through the repo's own plugin,
`resstock._fetch_fleet([1, 2, 3], "2024.99", tmp_path, 0, {})` returned `RESULT []` and logged three
`ResStock building N skipped: ValueError: Unknown ResStock version '2024.99'`; the temp tree was empty.
Fix: call `_parse_version(version)` (or `_version_config`) once in `_fetch_fleet` before submitting, and add a test
that an unknown version raises from `fetch_resstock_fleet`. No existing test covers it (the new fleet tests all mock
`fetch_resstock_building`).

### Low

**L1. The shared HTTP client is not shared on the first burst: 16 clients are built and 15 stay alive to interpreter exit.**
`python/ochre_next/data/resstock.py:380-392`. `functools.cache` does not hold a lock while the function runs, so
every worker that reaches `_http_client()` before the first call returns builds its own `httpx.Client` and registers
its own `atexit` close. A fleet starts with 16 workers at once, which is exactly that case. The commit message and
docstring promise one client and connection reuse.
Evidence: probe with a stand-in `httpx.Client` and 16 threads on a barrier: `CLIENTS 16 DISTINCT RETURNED 16 ATEXIT 16`.
Effect: 16 connection pools and TLS handshakes for the first wave, 15 pools never reused. The timeout is correct.
Fix: guard construction with a `threading.Lock` (double-checked), or create the client once before the pool starts.
`test_downloads_share_one_client_with_the_long_timeout` calls twice from one thread, so it cannot see this.

**L2. The offline guard has bypasses the module docstring does not state.**
`tests/python/offline_guard.py:39-56, 90-100`. The previous review's M2 is fixed in its main parts (see "Offline guard"
below). What still gets through, each shown by a probe test run under the real plugin, the test passing with nothing
recorded:
- `sock.connect(("name", port))`, `sock.sendto(b"x", ("name", port))` and a UDP `connect` with a hostname: CPython
  resolves the name inside the socket call before it raises the audit event, so an unresolvable name raises
  `gaierror` with no attempt recorded, and a resolvable name has already had its DNS query sent when the guard refuses
  the connect. (`create_connection`, `urllib`, `httpx`, `requests` and `asyncio` go through `getaddrinfo` first and
  are caught.)
- A Python child started with `-E` (`subprocess.run([sys.executable, "-E", ...])`): `_guarded_python_child` rejects
  `-S` and `-I` only, but `-E` also drops `PYTHONPATH`, so the child never loads `sitecustomize`. Probe: the child
  printed `NOGUARD`; without `-E` it printed `GUARD`.
- `ctypes` calls into libc (`connect`, `system`): the audit hook does not see them. The probe passed.
Fix: reject `-E` in `_guarded_python_child` and say in the docstring that `ctypes`/native code and
hostname-addressed `connect`/`sendto` are out of scope, or add a resolve-before-connect check on the hostname.

**L3. An attempt made outside any test (import, collection) is not reported under xdist.**
`tests/python/offline_guard_plugin.py:53-61`. The docstring says such an attempt "fails the session". The record lives
in the worker's stash and `pytest_sessionfinish` only sets the worker's exit status; the controller's
`pytest_terminal_summary` reads its own, empty, stash. The repo's default is `-n auto`.
Evidence: a module that does `try: socket.getaddrinfo("example.invalid", 80) except OSError: pass` at import, run with
`-n 0` printed `code outside any test reached the network` and exited 1; the same file with `-n 2` exited 0 with no
message. (A refusal that is not swallowed still shows up as a collection error in both modes.) Fix: report worker
attempts through `pytest_testnodedown` or as a failing synthetic test, or fail the worker's collection.

**L4. A failing Python test leaves its temp directory behind.**
`pyproject.toml:91` (`tmp_path_retention_policy = "failed"`). It is deliberate and pytest prunes to the last three
base directories, but the task's requirement is nothing left "on failure", and a failing session also fails the
block's `[stray]` TMPDIR check. Evidence: a failing test using `tmp_path` run with `TMPDIR` isolated left
`pytest-of-rich/pytest-0/test_fails0/out.csv` and `pytest-current`. The Rust side is clean on panic (`TempDir` drops on
unwind; no `panic = "abort"`, no `mem::forget`/`keep`/`into_path` anywhere, and nextest has no kill timeout
configured). Residual that no code change fixes: SIGKILL or Ctrl-C of nextest or pytest skips every destructor.
Decide whether to keep failed directories (and say so in BR-24) or drop them.

**L5. Pyright (head configuration) reports one error on a branch-added line.**
`tests/python/test_helics_integration.py:163`: `"helicsCleanupLibrary" is not a known attribute of module "helics"`
(`reportAttributeAccessIssue`). The head's `typings/helics.pyi` declares neither `helicsCleanupLibrary` nor
`helicsCloseLibrary`. Method: head tree `13881b48` (python, tests, scripts, typings, `pyproject.toml`) with the
branch's changed `.py` files overlaid, `pyright` 3.13 environment of the worktree venv, errors filtered to lines added
by `38398bd4..6c2e34d5` (1433 lines) and, separately, by `0216a35a..6c2e34d5` (819 lines). Result: 1 error, on a line
the reviewed range touches. The other 481 errors in the overlay are on lines the branch did not add (they are the
head's own fixes in files the branch also edits, so the merge needs those resolved by hand). New files
`offline_guard.py`, `offline_guard_plugin.py`, `test_network_guard.py`, `conftest.py`,
`_offline_site/sitecustomize.py` have no pyright error. Fix: add `helicsCleanupLibrary` to `typings/helics.pyi` when the
branch merges.

**L6. Ctrl-C of a fleet fetch still waits for the downloads already in flight.**
`python/ochre_next/data/resstock.py:988-991`. `shutdown(wait=False, cancel_futures=True)` cancels the queue, but
`concurrent.futures` joins every worker thread at interpreter exit (documented behaviour of `ThreadPoolExecutor`), so
the process stays up until up to 16 in-flight buildings finish or exhaust `_download_with_retry`'s backoff. The commit
message says the fleet stops "now". Also, `fetch_building = fetch_resstock_building` (line 964) with its comment exists
so a test's patch can be undone while workers run; production code shaped by a test. Not verified by a signal test
(the repo's test raises `KeyboardInterrupt` from a worker, which exercises the `except BaseException` path but not a
real SIGINT); reported as a gap.

**L7. Randomly drawn ports leave two small nondeterminism sources in the zmq tests.**
`python/ochre_next/helics/broker.py:225` (`random.randint`, unseeded) and `core_init_string` in
`python/ochre_next/helics/federate.py:138`. A broker port is retried on a bind failure (`_BROKER_PORT_ATTEMPTS = 16`),
but each federate core's own `--port=` comes from `allocate_ephemeral_port()` with no connected check, so a pair that
another worker's probe-then-bind race also picked gives a failed bind or, per the existing comment, a stall at
`enterExecutingMode` that ends only on the connect timeout. With 8 workers and about 5800 candidate ports the chance
per run is far below 1 in 1000, and no failure was seen in 22 runs, but "deterministic" is not literally true for
the three zmq tests.

**L8. The fixed-start duplication from the previous review's L3 is only partly removed.**
`FixedOffset::west_opt(7 * 3600)` is still spelled out in `tests/resstock_smoke.rs:174,364,464`,
`tests/regression/smoke_test.rs:232,455`, `tests/envelope_oracle.rs:42`, `tests/envelope_opaque_loads.rs:101`,
`tests/freefloat_oracle.rs:42`, `tests/conditioned_oracle.rs:52`. The new `tests/support/fixture_start.rs` covers
five sites. Most of these write other times or carry their own offsets, so this is not a defect, only an incomplete
consolidation.

## The 13 findings of the previous review

| # | Finding | Status | Evidence |
|---|---|---|---|
| H1 | 13 unbounded `federate_ready.wait()` | Fixed at the root | `_FederateThread` now owns `_ready_or_ended`, set by `_mark_ready` and in `_run`'s `finally`; `wait_ready` re-raises the federate's own exception, or an `AssertionError` if it ended without being ready (`test_helics_integration.py:55-111`). All 13 sites call `wait_ready()` (`grep` finds no `federate_ready` or `baseline_ready` left; 13 `wait_ready()` calls plus the 2 new tests). Two new tests cover the exception and the silent-exit cases. 22 stress runs and one serial run pass. |
| M1 | stale-process guard | Removed | `helics_environment_guard`, `_STALE_PROCESS_PATTERN`, `lsof`/`ps` code and the port probe are gone; no reference remains. The file no longer fails when another HELICS session runs on the host (22 runs, none failed). |
| M2 | guard missed module fixtures, raw `_socket`, UDP, lookups | Fixed in the main parts | An audit hook (`offline_guard.py`) replaces the monkeypatch. See "Offline guard" below for the remaining holes (L2, L3). |
| M3 | fleet fetch lost the shared client and 120 s timeout | Fixed, with one flaw | `_http_client()` has `timeout=120.0`; the urllib path passes `timeout=`; tests assert both. L1: the client is built up to 16 times on the first burst. |
| L1 | leftover system-temp writers | Fixed | `test_observer_diagnostics.py`, `test_py_fleet.py`, `test_rl_warning_surfacing.py` use `tmp_path`; the scripts use `TemporaryDirectory` or a required `--out-dir`. No `mkdtemp`, `gettempdir` or fixed `/tmp` write remains (grep of `*.rs`, `*.py`, `*.sh`, `*.toml`). |
| L2 | performative start-time tests | Fixed | Both tests deleted (7238 to 7236 debug tests, 7195 to 7193 release). |
| L3 | start expression and temp helper copied | Mostly fixed | `tests/support/fixture_start.rs` and `temp_file.rs` included by path; see L8. |
| L4 | fleet tests did not test what they name | Fixed | The unused `httpx_installed` parameter is gone; new tests make building 1 finish after building 2, assert the log text, hold `_WEATHER_LOCK`, and interrupt a fleet. |
| L5 | one zmq test | Fixed | `test_single_dwelling_completion_signal_unblocks_aggregator` and `test_fleet_completion_signal_unblocks_aggregator` are parametrized over `("inproc", "zmq")`, plus the broker-cleanup test. |
| L6 | assert in `finally`, one failing disconnect stops the rest | Fixed | `_Federation.__exit__` collects disconnect errors, always runs broker disconnect and joins, raises its own errors only when the test passed; two tests pin the behaviour. |
| L7 | port range held HELICS default ports | Fixed | Range is 24200-29999; a test asserts none of 23404, 23405, 23414, 23415, 23500, 23901, 24160 is drawable. |
| L8 | Ctrl-C blocked, misleading log | Partly fixed | Queue is cancelled and the log names the real error (`ResStock building 2 skipped: ValueError: ...`); see L6 for the in-flight tail. |
| L9 | `unique_temp_path` misnamed | Fixed | Renamed `numbered_path` (`benches/common.rs`). |

### HELICS waits, joins and teardown

Every blocking path in `test_helics_integration.py` is bounded or released:
- `_FederateThread.wait_ready` waits on an event that is set when the target readies or ends. The only way it can
  block is a federate stuck inside `helicsCreateValueFederate` or `register_*`, and the core init string carries
  `--timeout=20000ms` (`federate.py:123-138`, applied to inproc and zmq alike, asserted by the two new
  `core_init_string` tests).
- The test thread's own aggregator uses `enter_executing_mode_with_timeout` and `request_time_with_timeout` (20 s and
  30 s); the dwelling and fleet federates use the same bounds in `run()`. `thread.join()` and `result()` are unbounded
  by design, which the docstring justifies, and holds because every call a federate thread makes has such a bound and
  `__exit__` disconnects the broker before it joins, which releases a federate waiting on a peer.
- No `time.sleep`, wall-clock or random use is left in the file.
- `helicsCleanupLibrary()` runs only after the test's federates are disconnected, the broker is disconnected, every
  federate thread is joined and `wait_for_pending_aborts()` (10 s bound) returned true. If it does not, the library is
  left alone and the test's own error is kept (`__exit__`, lines 145-165; two tests cover both outcomes). No path frees
  the library under a live federate. A federate created inside `_FaultyAggregator.run` and not disconnected because
  registration raised stays connected, but its thread is already gone and the broker disconnect releases it.

## Temp-file proof (BR-24)

Runs, each with its own isolated `TMPDIR` (`target/rv/tmp-<line>`), `find $TMPDIR -mindepth 1` after:

| Line | Result | Left in TMPDIR |
|---|---|---|
| `nextest --workspace` (debug) | 7236 run, 7236 passed | nothing |
| `-F observe` | 7294 passed | nothing |
| `-F dst` | 7249 passed | nothing |
| `-F profiling` | 7242 passed | nothing |
| `-p hares-core -F observe,profiling` | 1379 passed | nothing |
| `--release` | 7193 passed | nothing |
| `cargo bench --workspace --no-run` | pass | nothing |
| `frame_golden compare-all`, default and `-F dst` | pass, every case identical | nothing |
| `cargo bench --workspace -- --test` (runs all 43 benches once) | pass | nothing |
| `pytest -n 8 -m "" test_helics_integration.py` x 22 | 28 passed each | only the empty `pytest-of-rich/` |

Private-`/tmp` runs (`unshare -rm`, `mount -t tmpfs tmpfs /tmp`, `TMPDIR` unset, so `/tmp` is a fresh empty tmpfs):

| Line | Result | `/tmp` afterwards |
|---|---|---|
| nextest debug | rc 0 | empty |
| nextest release | rc 0 | empty |
| `cargo bench --workspace -- --test` | rc 0 | empty |
| `maturin develop --release --uv` | rc 0 | empty |
| full `pytest -m "" -rs` | 1412 passed, 19 skipped, 3 xfailed | only `pytest-of-<user>/`, empty |

After all of it the worktree has no new untracked or ignored path (the block's `[stray]` tree expression prints
nothing). `fmt --check` and all nine clippy lines (`--all-features`, default, `dst`, `profiling`, `observe`,
`observe,profiling`, `--release`, `--release dst`, `--release observe`) pass with `-D warnings`.

Prefixes, checked against the base tree (`git grep 38398bd4`) and the tip: the writers of `hares-core-*`
(`dwelling/mod.rs`, `tests/engine.rs`, `tests/autosize_from_config.rs`), `hares-test-*`, `obs_diag`
(`resstock_observer_diag.rs`) and the regression and fleet outputs are all gone from the tip; `hares-regr-*`,
`hares-fleet-*` and `pv_lut*` have no writer at the tip. After the runs above none of `hares-regr-*`, `hares-fleet-*`,
`hares-test*`, `hares-core*`, `pv_lut*`, `obs_diag*` or `rt*` exists in any isolated `TMPDIR` or private `/tmp`.
(The host's real `/tmp` held 34161 entries during this review, 24402 of them `hares-regr-output*`, from branches
without this fix; nothing here wrote to it.)

Creation sites: every `tempfile`, `tempdir`, `NamedTempFile`, `TempDir`, `mkstemp`, `TemporaryDirectory`,
`mkdtemp`, `gettempdir`, `mktemp` and `/tmp` hit in `*.rs`, `*.py`, `*.sh`, `*.toml` and `Makefile` was read.
- Rust tests and benches: each holds a `TempDir` or `NamedTempFile` in scope until the end of the test, so it is
  removed on return, on panic (unwind; no `panic = "abort"` profile) and on failure. The runs above are the
  evidence. A `TempDir` that is moved out of a helper is returned with its path (`temp_file`, `write_inputs`,
  `built_consumer_shape_home`, `write_temp_epw`).
- `/tmp` literals (`hares-io/src/config.rs:252,266,423`, `hares-python/src/py_dwelling.rs:3142,3147`,
  `tests/python/test_py_config.py:41,52`) are parsed or compared, never written.
- Production Python: `resstock.py:319` uses `TemporaryDirectory`; `helics/runner.py:102` unlinks in `finally`.
- Scripts not collected by pytest (`bench_4month_der.py`, `diag_pv_trace.py`, `parity_diagnostics.py`,
  `ochre_bestest_600.py`, `download_resstock_fixtures.py`) use `TemporaryDirectory` or an explicit output directory.
  The shell scripts and `Makefile` use `mktemp` outside the test suite.
- Residual that cannot be fixed in code: a process killed by SIGKILL, or an interrupted nextest or pytest run, skips
  its destructors. A failing Python test keeps its directory by policy (L4).

## Offline guard

Probes written as pytest files and run through `-p offline_guard_plugin` (the real plugin), reading the outcome:

| Attempt | Result |
|---|---|
| module-scoped and session-scoped fixture that reaches out | caught (`test_an_attempt_fails_the_test_that_made_it_even_when_swallowed`, `test_fixtures_above_function_scope_run_refused`, both pass) |
| raw `_socket.socket().connect` | caught |
| UDP `sendto` to an address; `socket.sendmsg` | caught |
| `getaddrinfo`, `gethostbyname(_ex)`, `gethostbyaddr`, `getnameinfo` | caught |
| `asyncio` `sock_connect`, `open_connection` | caught |
| `urllib.request.urlopen`, `httpx.get`, `requests.get` | caught (each fails at teardown) |
| a thread started by the test | caught |
| `subprocess` of `sh` | refused |
| Python child, `multiprocessing` spawn and fork | child refused the connect |
| connect, `sendto` or UDP connect with a hostname | **bypass** (L2) |
| Python child with `-E` | **bypass** (L2) |
| `ctypes` `connect` or `system` | **bypass** (L2) |
| swallowed attempt at import or collection under xdist | **not reported** (L3) |

No test in the suite is marked `network`, so the marker is unused.

## HELICS stress (OD-32)

`pytest -n 8 -m "" -p no:cacheprovider tests/python/test_helics_integration.py`, 28 tests per run.
- Batch 1: 12 runs, load average 2.5 to 16 (this review's own Rust compile and nextest lines running at the same time,
  plus whatever else the shared host was doing). 12 passed, 0 failed, 84 to 91 s each.
- Batch 2: 10 runs with 40 busy-loop processes added to this review's own release build and nextest lines, load
  average 53 to 66 on 28 cores. 10 passed, 0 failed, 116 to 129 s each.
- 22 of 22 runs passed, no timeout, no worker crash, nothing left in `TMPDIR` but the empty
  `pytest-of-<user>/`. A serial run (`-n 0`) of the five HELICS files was started to exercise `helicsCleanupLibrary`
  across tests in one process: 154 passed (see the last section).
- The ledger's acceptance asks for 50 consecutive runs. 22 were run; the gap is stated, not hidden.

## Test quality

- Removed assertions in the range: the two start-time tests (L2 above), the three exact-attempt asserts replaced by the
  guard's own tests, and the port-range bound `hi + 1 < 32768`, replaced by `< 30000` plus a HELICS-defaults check.
  Nothing is weakened; the removed ones tested a constant.
- No `#[ignore]` added. The only new skip is `pytest.skip(..., allow_module_level=True)` for a missing `helics`, which
  replaces the per-test `skipif` with the same reason; helics is installed here, so it does not fire.
- Skips seen in the full pytest (19: 5 `@pytest.mark.skip` in `test_observer_diagnostics.py`, 8 in
  `test_ochre_parity.py:216`, 6 `ResStock test fixtures not available`) come from the environment and code the branch
  did not change; the committed census file `ledger/pytest-skip-census.txt` is empty, so the block's census diff fails
  on them regardless of this branch.
- Determinism: no `Utc::now`, `SystemTime::now`, `thread_rng`, `from_entropy`, `Local::now`, `time.sleep`,
  `time.time` in `tests/`, `benches/` or `crates/*/tests`; the one random draw is L7.

## Shipped text

Block `[writing]` expressions (em dash, ticket IDs, reviewer and agent words, `HARES#n`) run on the message and added
lines of all 13 commits in `38398bd4..6c2e34d5`: no match. All commits are authored by one human author, with no
trailer lines.

## Checks I could not run

- 50-run HELICS acceptance (22 run).
- `cargo deny` (not requested; no dependency changed in the range).
- A real SIGINT against a fleet fetch (L6).
- The pytest skip census: reported above; fixtures and `ochre` are partly absent in this worktree.

## Serial HELICS run

`pytest -n 0 -m ""` over the five HELICS files (`test_helics_integration`, `_federate`, `_dwelling`, `_fleet`,
`_broker_runner`) in one process, so `helicsCleanupLibrary()` runs after every integration test in the same library
instance: 154 passed in 155 s, nothing left in `TMPDIR` but `pytest-of-<user>/`.
