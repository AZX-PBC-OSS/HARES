# Review and red team: fix-testhyg at 0216a35a (from 38398bd4)

Commits: a025464d (HELICS on federation events), d2d74874 (per-test temp
dirs), 6128246d (fixed local start), 2555f562 and db043dae (bench temp dirs,
fixed start), 0216a35a (one ResStock fetch path, offline guard). Ledger:
OD-32, BR-24; plan.md §3 `[stray]` lines.

Worktree `.workstrees/verify-testhyg` (detached at 0216a35a),
`CARGO_TARGET_DIR=.workstrees/target-verify-testhyg`, `-j 8`,
`TMPDIR=target/block-tmp`.

## Verdict: CHANGES-REQUIRED

1 high, 3 medium, 9 low. Every block line I ran passes. The HELICS
file passed every run the stale-process guard let start. The high
finding is a latent hang in the HELICS tests. It brings back OD-32's
original symptom, a crashed worker, whenever a federate fails before it
signals ready.

## Block lines run

| Line | Result |
|---|---|
| `[fmt]` | pass |
| the nine `[clippy*]` lines (all-features, default, dst, profiling, observe, observe+profiling, release, release+dst, release+observe) | all pass, `-D warnings` |
| `[test-debug]` nextest | 7238 run, 7238 passed, 0 skipped |
| `[test-release]` nextest | 7195 run, 7195 passed, 0 skipped |
| `[bench]` `--no-run` | pass |
| `[golden]` default, `-F dst` | both `compare-all` pass |
| `[python-build]` maturin develop --release | pass |
| `[pytest]` `-m "" -rs` | 1387 passed, 9 skipped, rc 0 |
| `[pytest]` census diff | fails here: 9 skips against an empty census (see below) |
| `[stray]` tree | no new paths |
| `[stray]` TMPDIR (Rust lines) | nothing left by any Rust test or bench |
| `[stray]` TMPDIR (pytest line) | only `pytest-of-rich/` (empty) and `uv-*.lock`, which pass |
| `[writing]` on all six commits (message + diff) | clean |

The 9 skips come from this environment. None is caused by the branch.
`ochre` was not installed into the fresh worktree venv. 3 skips say
"OCHRE not installed": `test_observer_diagnostics`, `test_ochre_parity`
and `test_thermal_trace`. The ResStock fixtures
(`tests/fixtures/building_energy_models`, `resstock_metadata.parquet`) are
not in this tree or in the main checkout. 6 skips in
`test_py_conversions.py` say "fixtures not available". I re-ran the three
OCHRE modules with `PYTHONPATH=vendors/OCHRE`: 3 passed, 3 xfailed, 13
skipped. 5 skips are `@pytest.mark.skip` on `test_observer_diagnostics.py`.
8 are `test_ochre_parity.py:216` "HARES has no column matching ...". The
branch changed only the output path in that module (and removed the dead
`_run_hares`), not the column logic. That run also left nothing in TMPDIR,
so `tmp_path_factory` directories are removed.

## HELICS acceptance (OD-32)

The command was `pytest -n 8 -m "" tests/python/test_helics_integration.py`,
run repeatedly. A concurrent `cargo build --workspace --all-targets
--release` ran in a loop. From run 5 on, a looping release nextest suite
ran as well. Load average was 4 to 60. Earlier runs overlapped the block's
clippy and nextest lines at load 60 to 67.

- 26 runs reached the tests: 3 in the first batch, 3 in the second and 20
  in the third. Each one was 21 passed, with no timeout and no worker
  crash.
- 12 runs failed all 21 tests at setup in `helics_environment_guard`: 9 in
  the first batch, 1 in the second and 2 in the third. Each time, a
  different pytest session (another worktree, and
  `/tmp/opencode/hares-head`) was running HELICS test files at the same
  time. See M1. By OD-32's own acceptance ("at full host load (the block's
  other lines running)"), these count as failures of the file.

The port allocator, the connected-check and teardown were checked
directly (`repro_broker_bind.py` in the scratchpad):

- 30 of 30 `create_broker(2, "zmq")` calls returned a connected broker.
- On a port held by a listening socket, the raw HELICS broker reports
  `is_connected() == False` after about 4.2 s. `create_broker(port=p)`
  then raises `could not listen on port p` and the broker is
  disconnected. The connected-check after create is correct. HELICS binds
  synchronously inside `helicsCreateBroker`.
- Range 20000-32767: Linux's default ephemeral range is 32768-60999.
  macOS and Windows (Vista onwards) use 49152-65535 by default. All three
  sit above this range, so the claim holds for defaults. Hosts that widen
  `ip_local_port_range`, and Windows Hyper-V/WSL excluded port ranges,
  are handled by the bind probe and the connected-check. They are not
  prevented. See L7 for two collisions inside the range.
- Teardown order in `_Federation.__exit__`
  (`tests/python/test_helics_integration.py:242-251`): the test's own
  federates disconnect, then the broker, then the federate threads are
  joined, then `wait_for_pending_aborts`, then `helicsCleanupLibrary`.
  The order is right. No `helicsCloseLibrary` call is left.
- No wall-clock waits are left in the integration file. The only bounds
  are each federate's own connect and grant timeouts, plus the
  `pytest.mark.timeout(120, method="thread")` backstop. See H1 for the
  unbounded event waits.

## Findings

### High

**H1. A federate that fails before `ready.set()` hangs the test until the
pytest-timeout thread method kills the worker.**
`tests/python/test_helics_integration.py:522, 582, 636, 717, 816, 877, 930,
1016, 1055, 1101, 1247, 1402, 1472`. At these 13 sites the test thread
blocks on `federate_ready.wait()` (or `baseline_ready.wait()`) with no
bound. `_FederateThread._run` catches the federate's exception and stores
it, but nothing sets the event. `HELICSDwelling`/`HELICSFleet`
construction, `register_*`, or federate creation against the broker can
each raise. When one does, the test never reaches `__exit__`. After 120 s
the thread-method timeout calls `os._exit` and the xdist worker crashes.
That is the symptom OD-32 was opened for. It also breaks OD-32's "tears
down on failure".

Repro, run against the worktree (the script's directory was later
removed by another process, so the script is given here):

```python
with t._Federation(n_federates=2) as federation:
    federate_ready = threading.Event()
    def _run_federate():
        federation.helics_dwelling(object(), "house_1", connect_timeout_s=-1.0)
        federate_ready.set()
    thread = federation.start(_run_federate, name="dwelling-federate")
    thread.join()
    # thread.exception == ValueError('connect_timeout_s must be ...')
    federate_ready.wait(5.0)  # -> False; the tests call wait() unbounded
```

Fix direction: make readiness a federation event that also fires when
the thread exits. For example, `_FederateThread` could own the ready
event and set it in a `finally`. The test then waits on it and re-raises
the thread's exception if the federate ended without becoming ready. That
needs no time bound.

### Medium

**M1. The stale-HELICS-process guard fails the whole file whenever another
pytest session runs HELICS tests, and the in-process brokers make it
obsolete.** `tests/python/test_helics_integration.py:64-68, 147-174`.
This code predates the branch, but it lives in the file OD-32 makes
deterministic, and it fails OD-32's acceptance as observed above: 12
runs, each with 21 errors. `_STALE_PROCESS_PATTERN`'s second alternative,
`python[0-9.]*\S*\s+\S*helics\S*\.py`, matches any command line that
contains `tests/python/<x>.py tests/python/test_helics_<y>.py`, because
`python` matches the `tests/python/` directory. Any concurrent session in
another worktree trips it. Since a025464d, each test runs on its own
uniquely named in-process broker, and the one zmq test uses an allocated
port. A foreign process can no longer hold federation state that these
tests join, so the guard is a false positive in every case it fires.
Remove it, or reduce it to the default-port check, which only matters if
a test uses the default ports. None does.

**M2. The offline guard does not cover fixtures above function scope or
code outside `socket.socket`.** `tests/python/conftest.py:39-95`.
`_offline_unless_marked_network` is a function-scoped autouse fixture, so
module, session and class fixtures, which pytest sets up first, run
unguarded. The same holds for import-time code and a pooled HTTP
connection opened before the fixture. Probed against a copy of the
conftest. Each probe reaches 192.0.2.1 or does DNS, and the test passes
with nothing recorded:

| Probe | Outcome |
|---|---|
| `@pytest.fixture(scope="module")` doing `socket.connect` | passes, unguarded |
| `_socket.socket(...).connect` | passes, unguarded |
| UDP `sendto` without `connect` | passes, unguarded |
| `socket.gethostbyname("example.com")` (a real lookup) | passes, unguarded |
| `subprocess.run([python, "-c", <connect>])` | passes, unguarded |
| control: plain `socket.connect` in the test body | caught, test errors in teardown |

No current module-scoped fixture touches the network: I checked the six
files that define them. The commit says the guard refuses "name lookups",
but `gethostbyname`, `gethostbyname_ex` and `getnameinfo` are not patched.

Fix direction: install the patches once per process with a
session-scoped autouse fixture, or with `pytest_configure` /
`pytest_runtest_protocol`, and switch them off only for `network`-marked
items. Patch the other resolver entry points too. Record attempts per
item so a module fixture's attempt fails the first test that uses it.
Subprocess, raw `_socket` and UDP can stay out of scope if the docstring
says so.

**M3. The fleet download path now uses httpx's default 5 s timeouts
instead of 120 s, and opens a client per building.**
`python/ochre_next/data/resstock.py:376`. Before 0216a35a,
`fetch_resstock_fleet` (with httpx installed) used one shared
`httpx.AsyncClient(follow_redirects=True, timeout=120.0)`. Every
fleet building now goes through `_try_download`, which builds
`httpx.Client(follow_redirects=True)` per call. That client has httpx's
default `Timeout(5.0)` for connect, read, write and pool, and no
connection reuse. With 16 concurrent S3 streams, a 5 s read stall that
used to be tolerated now raises `ReadTimeout`. Each one is retried with
backoff (`_MAX_DOWNLOAD_ATTEMPTS`), so a slow link now fails buildings it
used to download, and every building pays a fresh TLS handshake.
The urllib fallback (`resstock.py:402`) has no timeout at all, so one
stalled connection blocks its pool worker indefinitely. This is a change
in behaviour for real network use that the commit message does not
mention. Fix direction: give `_try_download` an explicit timeout (the old
120 s read bound) and an `urlopen(..., timeout=...)`. If per-building
clients cost too much, share one thread-safe `httpx.Client` across the
pool.

### Low

**L1. Leftover system-temp writers in tests and test-adjacent scripts.**
`tests/python/test_observer_diagnostics.py:73` still calls
`tempfile.mkdtemp()`, which creates a directory and never removes it.
The test is skipped today, so it cannot fail `[stray]` yet, but BR-24
says "every test". `tests/python/bench_4month_der.py:146`,
`tests/python/parity_diagnostics.py:154,308,324` and
`tests/diag_pv_trace.py:271` write to fixed `gettempdir()` or `/tmp`
paths. These are not collected tests, but fixed shared temp paths are
what BR-24 removes.

**L2. Two tests only pin a constant the helper writes.**
`tests/regression/mod.rs:33` (`regression_configs_start_at_a_stated_local_time`)
and `crates/hares-fleet/src/fleet.rs:1162`
(`simulation_config_starts_at_the_fixture_schedules_local_midnight`)
assert that a builder returns the literal it was given. A
`Utc::now()` regression would fail them, but so would any intended
change to the fixture start, and they test nothing about determinism.
A shared `fixture_start()` (L3) makes them redundant. Under the
test-quality rule (no performative tests), drop them or replace them
with a determinism assertion, such as two runs of a builder giving
identical results.

**L3. The fixed-start expression is copied five times.**
`tests/regression/helpers.rs:17-23`, `crates/hares-fleet/src/fleet.rs:1139-1144`,
`crates/hares-io/tests/comprehensive_tests.rs:40-44`, `benches/common.rs:~121`
and `benches/aggregation.rs:~149` each spell out
`FixedOffset::west_opt(7*3600)...with_ymd_and_hms(2021,1,1,0,0,0)`. The
same thing applies to the `(TempDir, PathBuf)` helpers, which are
written separately in `checkpoint.rs` (`checkpoint_path`),
`pv/mod.rs` (`temp_lut_path`) and `engine.rs` (`Scratch`). Both
are candidates for one test-support helper, or at least one per crate.

**L4. The fleet-path tests do not test what they name.**
`tests/python/test_resstock_data.py:988-1013`. The `httpx_installed`
parametrisation has no effect, because `fetch_resstock_building` is
mocked and nothing on the fleet path consults httpx any more.
"Returned in request order" is asserted with an instant mock, so
futures complete in order anyway. No test makes building 1 finish after
building 2. No test covers the `_WEATHER_LOCK` serialisation
(`resstock.py:32, 711`) either, which is the concurrency claim in the
commit. These coverage gaps are reported here, not filled.

**L5. Only one test now covers HELICS over the zmq transport.** Before
a025464d, every integration test ran over zmq. Now only
`test_multi_federate_fault_propagation_broker_cleanup`
(`test_helics_integration.py:1450`) does, and `HELICSFleet` over zmq
is covered nowhere. Running in-process is the right default for
isolation, but the transport is what users run. The zmq-specific
behaviour (the local `--port` that `core_init_string` adds against the
macOS deadlock) should keep at least one dwelling and one fleet
federation test over zmq.

**L6. An `assert` in a `finally` hides the test's own failure.**
`test_helics_integration.py:250`. If the test body raised and
`wait_for_pending_aborts()` is then false, the `AssertionError` replaces
the original exception, which survives only as `__context__`. Separately,
if one `fed.disconnect()` raises (line 245), the remaining federates are
not disconnected. The broker disconnect still releases them.

**L7. The allocator's range contains ports with existing meanings.**
`python/ochre_next/helics/broker.py:25-26`. 23404 and 23405 are the
HELICS zmq default ports. If the zmq test's broker draws them, a
`helics_environment_guard` starting at that moment in another worker
reports "HELICS default port 23404 is held". This is rare (about 3 in
12766 per broker) but real, and it goes away if M1 removes the guard.
30000-32767 is also the Kubernetes NodePort range. That does not break
anything, because NodePorts are not bound locally in iptables mode, but
the docstring's claim that the range is free is a default-only claim.

**L8. A fleet fetch cannot be interrupted, and its failure log is
misleading.** `python/ochre_next/data/resstock.py:951-962`. On
`KeyboardInterrupt` (or any `BaseException`) in the main thread,
`ThreadPoolExecutor.__exit__` waits for every queued download, because
`cancel_futures` is not set. On a large fleet, Ctrl-C waits for the whole
download. Every failure, including a weather or zone-validation failure
after the ZIP was fetched, is logged as "download failed ... url=<zip
url>". The old code logged post-download failures separately.

**L9. Nit: `benches/common.rs` `unique_temp_path(dir, ...)` no longer
returns a temp path.** It joins a counter onto the caller's directory,
so the name now misdescribes it.

## Checked with no finding

- Temp grep over `tests`, `benches`, `crates` and `python` for
  `temp_dir()`, `/tmp`, `mkdtemp`, `mkstemp`, `NamedTemporaryFile`,
  `gettempdir`, `into_path`, `keep`, `persist`, and for TempDirs dropped
  in the same statement. The only hits outside L1 are string literals in
  config-parse tests, which write nothing, and production code
  (`resstock.py` `TemporaryDirectory`, `helics/runner.py` `mkstemp`).
  The Rust lines left nothing in TMPDIR.
- Shared output paths: regression builders now name output
  `dwelling-<bldg_id>.csv` per test directory. Determinism and checkpoint
  runs reuse one path for runs that happen one after another and compare
  in-memory results, not files. Before the branch, checkpoint configs
  already shared one path through `clone()`. No test reads a file another
  run overwrote.
- `MISSING_ROOT = "/nonexistent-hares-fleet-test"`: nothing is written
  there, because construction fails on the HPXML first.
- Determinism grep over `tests` and `benches` (Rust and Python) for
  `Utc::now`, `Local::now`, `SystemTime::now`, `thread_rng`,
  `from_entropy`, `datetime.now`, `time.time()`, and unseeded
  `random`/`np.random`. It is clean apart from the `generated_at` metadata
  in the two oracle generator scripts. The remaining `SystemTime::now`
  (`checkpoint.rs:208`) is a production temp-file nonce.
- ResStock rewrite: results come back in request order (`zip` over
  futures). A partial failure skips that building and returns the rest.
  Per-building `_checksum_failures` lists are race-free. `_WEATHER_LOCK`
  is acquired once and never re-entered: `_fetch_weather_for_station`
  and `get_epw_for_fips` do not call `_fetch_weather`. So there is no
  deadlock, and the EPW archive and county CSV caches are written by one
  thread at a time inside a process. Cross-process races on the cache
  predate the branch. Retry: 5xx, 429 and timeouts back off with jitter
  per building, and 16 concurrent GETs are far below S3 request limits.
  Behaviour change: M3, L8.
- `tmp_path_retention_policy = "failed"`: after a passing session,
  `pytest-of-<user>/` is left empty and no `pytest-current` symlink
  remains. After a failing session the basetemp is kept, as intended, and
  `[stray]` then fails as well.
- `[writing]` on each commit's message and diff: clean.

## Housekeeping

Another process deleted this review's first scratch directory
(`scratchpad/testhyg`, where a 24 GB `tmp-pytest-before` tree that this
review did not write had filled `/tmp`). The logs above that were lost
with it had already been read. Every result they held is recorded here.
