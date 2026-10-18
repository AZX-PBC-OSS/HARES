---
description: pytest discipline covering fixture scoping, parametrization, async tests with anyio/pytest-asyncio, mocks at boundaries, catching order- and isolation-dependence with randomised test order and parallel execution, xdist worker counts and crash handling, timeouts and faulthandler so a stall produces a diagnosis, marker selection versus addopts, tests that can never run, ranking findings by whether they can produce a wrong pass, mutation testing to check what coverage misses, and exercising real database engines via testcontainers
triggers:
  file_globs:
    - "**/test_*.py"
    - "**/*_test.py"
    - "**/conftest.py"
    - "**/pytest.ini"
  content_patterns:
    - "MagicMock"
    - "mock\\.patch"
    - "@patch"
---

# pytest

pytest gives you a lot of rope. Used well it produces tests that are short, fast, and catch real bugs. Used badly it produces fixture spaghetti, session-scoped state leaks, and tests that pass with no meaningful assertion. The rules below are the merge bar.

## Discover the project's testing setup first

Before adding tests, read `pyproject.toml` (`[tool.pytest.ini_options]`), `pytest.ini` / `setup.cfg`, the top-level `conftest.py`, and any `tests/` directory layout. Match the existing fixture style, async runner (`pytest-asyncio` vs `anyio`), HTTP client choice (`httpx.AsyncClient` vs sync `TestClient`), DB-fixture pattern (per-test transaction rollback vs schema reset), and marker conventions. If the project has an established factory module (`tests/factories.py`) or `pytest-factoryboy` setup, use it instead of constructing models inline.

## Test layout: mirror the source tree, one assertion focus per test

- Tests live in `tests/` (mirroring the package structure) or co-located (`module.py` + `test_module.py`). Pick one and stay consistent with the project.
- One test per behavior — not one giant test that asserts twelve things. When the second assertion fails you don't know whether the first still holds; readable failure output beats line-count.
- Test names describe behavior in plain English: `test_user_signup_rejects_duplicate_email`, not `test_signup_2`.
- AAA shape — Arrange, Act, Assert — visually separated by blank lines. If the Arrange section is half the test, that's a fixture waiting to be extracted.

## Fixture scope: function by default, escalate deliberately

```python
@pytest.fixture
def user(db: AsyncSession) -> User:
    return User(email='ada@example.com')

@pytest.fixture(scope='session')
def docker_postgres() -> Iterator[PostgresContainer]:
    with PostgresContainer('postgres:16') as pg:
        yield pg
```

- **Function scope is the default.** Each test gets a fresh instance — no shared mutable state, no order-dependence.
- **Module/class scope** only when setup is genuinely expensive AND the resource is logically read-only across tests in that scope.
- **Session scope** for processes that take seconds to start (containers, browsers, language servers). Pair with per-test cleanup that resets state inside the long-lived resource.
- **Session-scoped mutable state is an anti-pattern.** A session-scoped DB session shared across tests creates hidden ordering dependencies — see randomised ordering below for how this actually gets caught. Use a session-scoped *connection* and a function-scoped *transaction* that rolls back at teardown.

## `conftest.py` for shared fixtures, not for magic

- A fixture used by 2+ test files moves to the nearest common `conftest.py`. Used by 1 file → keep it in that file.
- `conftest.py` is auto-loaded — no import. Put fixtures, hooks (`pytest_collection_modifyitems`, `pytest_addoption`), and markers there. Never put test functions in `conftest.py`.
- Avoid deep `conftest.py` nesting (4+ levels) — fixture resolution becomes a treasure hunt. Flatten when you can.

## Randomised order and parallel execution: catching tests that depend on each other

A suite that only passes in the order it happens to run — because an earlier test left a row, a file, or a module-level cache behind for a later one to lean on — is a bug in the suite, not a flake to retry. Fixed collection order hides this indefinitely; nothing about a green run rules it out.

`pytest-randomly` (`pip install pytest-randomly`) shuffles module, class, and test order on every run and reseeds `random` (and `factory_boy`/`Faker`/`Model Bakery` if present) per test, so data built from randomness stays reproducible while ordering doesn't. Once installed it runs automatically — nothing to opt into per test.

- A failure that only reproduces under randomisation, not under `-p no:randomly`, confirms an ordering dependency — usually a session/module-scoped fixture mutated by one test and read by another (see fixture scope above).
- pytest prints the seed at the top of every run (`Using --randomly-seed=1234`). Reproduce a failure with `pytest --randomly-seed=1234`, then narrow to the failing module or class to isolate which pair of tests interacts — don't just rerun the full suite hoping to catch it again.
- `--randomly-dont-reorganize` exists to unblock a suite that has pre-existing order coupling while it's being fixed. Treat it as a temporary escape hatch, not a standing config — leaving it on permanently switches the safeguard off.

`pytest-xdist` (`pip install pytest-xdist`; run with `pytest -n auto`) distributes the suite across worker processes. The obvious payoff is wall-clock time — but a suite slow enough that nobody runs it before pushing has already stopped protecting anything, so that's a correctness argument wearing a performance costume, not just a convenience.

The sharper payoff: randomisation catches tests that depend on *order*; `-n auto` catches tests that depend on *exclusive access* — a shared database row, a fixed port, a temp path with a hardcoded name, a module-level cache, the real wall clock. Two workers touching the same thing at the same instant fail in a way no amount of serial reordering, randomised or not, will ever trigger. Run both, and "the suite passes" starts to mean "the suite would pass under any conditions" rather than "the suite passes the one way it happens to run." The usual root cause is the same one flagged above: a session-scoped fixture that's supposed to be read-only but isn't.

- A suite with tests that genuinely contend for one shared resource — a single database with no per-worker isolation, an external service with a tight rate limit — will get flakiness under `-n auto`, not speed, until that's fixed. The fix is per-worker isolation (a database/schema per worker via the `worker_id` fixture, a dynamically allocated port), not dropping xdist.
- Debug an xdist-only failure by reproducing it serially first (`pytest -n0` or by omitting `-n`) — a failure that only appears under parallel workers and can't be pinned down to one test running alone is telling you it's about shared state, not test logic.

### `-n auto` is not the number you think it is

When psutil is installed, xdist resolves `auto` to **physical** cores: `xdist/plugin.py` calls `psutil.cpu_count(logical=False)`. A 2-vCPU CI runner has one physical core, so `auto` gives you **one worker** and all the xdist overhead with none of the parallelism, while a developer box reporting 14 physical cores gets the cap. The same command therefore means different things on different machines, and the CI end of that is the one that quietly stops catching exclusive-access bugs.

State the worker count explicitly in CI (`-n 4`) rather than inheriting a number that depends on which machine ran it, and size it against the runner you actually use.

### A crashed worker must fail the run, not be replaced

xdist's default `max-worker-restart` is `numprocesses * 4`, and the scheduler re-queues the work unit the dead worker was executing, so the test that killed the worker runs again on its replacement, and kills that one too. Reproduced with a test that SIGKILLs its own worker: 9 executions of the crashing test under the default, 1 with `--max-worker-restart=0`.

What that looks like in a log is the problem. A hard crash reads as `replacing crashed worker gw0` buried among the progress dots while the run limps on, sometimes for hours, and may still end green if the schedule eventually routes around it. `--max-worker-restart=0` turns a worker death into an immediate run failure, named in the summary.

### A segfault under xdist has no traceback, so re-run serially

A worker's stderr belongs to the execnet channel and dies with the worker, so a native crash surfaces as `node down` and nothing else. The identical fault run serially prints a faulthandler traceback naming the frame. When you are diagnosing a crash rather than just observing one, take xdist out of the picture first.

## Make a stall produce a diagnosis, not a timeout

A test suite that hangs and is then killed by a CI job timeout tells you nothing except that it hung. Two settings turn that into evidence:

- **`faulthandler_timeout`** (pytest's own ini option) dumps the stack of every thread after N seconds of a single test, naming the frame the run is stuck in. This is the difference between "e2e hung" and "it is blocked in a socket read inside the client".
- **`pytest-timeout`** kills the test. Know its shape before relying on it: it defaults to `func_only=false` and arms in `pytest_runtest_protocol`, so **one budget covers setup, call and teardown as a single span**. Teardown does not get a fresh allowance, so a test whose fixture setup is slow can be killed during teardown with the finaliser half-run, leaving the resource it was releasing behind. Size the budget for the whole protocol, or set `func_only` deliberately and know that setup is then unbounded.

Set `faulthandler_timeout` below the job's `timeout-minutes` so the dump happens while the runner is still alive to capture it.

## Parametrize for the same behavior across inputs

```python
@pytest.mark.parametrize(
    ('email', 'expected'),
    [
        ('user@example.com', True),
        ('USER@EXAMPLE.COM', True),
        ('not-an-email', False),
        ('', False),
        ('a@b', False),
    ],
    ids=['simple', 'uppercase', 'no_at', 'empty', 'too_short'],
)
def test_email_validation(email: str, expected: bool) -> None:
    assert validate_email(email) == expected
```

- **One `parametrize` per axis of variation.** Stacking two `@parametrize` decorators creates the cartesian product — handy for matrix testing, dangerous when you didn't mean to multiply.
- **Always pass `ids=[...]`** for non-trivial values (objects, long strings, bytes). Default-generated IDs become `test_x[0-1-True-<User object at 0x...>]` which is unreadable in CI logs.
- **Don't parametrize away the test name's meaning.** If the inputs are semantically different (signup-flow vs login-flow), they're different tests, not parameters.

## `pytest.raises` for exception assertions, with `match` and `from`

```python
def test_workspace_missing_raises_typed_error() -> None:
    with pytest.raises(WorkspaceNotFound, match='wks-42') as exc_info:
        load_workspace('wks-42')
    assert exc_info.value.workspace_id == 'wks-42'
```

- Catch the **specific exception type**, never `Exception`.
- `match=` is a regex on `str(exc)` — use it to assert the message includes the relevant context (the bad ID, the failing path).
- Inspect attributes on `exc_info.value` for typed-error data, not `str(exc)` parsing.
- Use `pytest.raises((A, B))` for "any of these" only when the API genuinely promises both.

## Async tests: pick one runner, use it consistently

**`pytest-asyncio`** is the most common choice:

```python
# pyproject.toml
[tool.pytest.ini_options]
asyncio_mode = 'auto'                    # every async def is a test, no marker needed
asyncio_default_fixture_loop_scope = 'function'

@pytest.fixture
async def user_repo(db: AsyncSession) -> UserRepo:
    return UserRepo(db)

async def test_user_lookup(user_repo: UserRepo) -> None:
    user = await user_repo.find_by_email('ada@example.com')
    assert user is not None
```

**`anyio`** is the right choice when the code under test must support both `asyncio` and `trio`, or uses `anyio.create_task_group` / `anyio.move_on_after`:

```python
@pytest.mark.anyio
async def test_concurrent_fetch() -> None:
    async with anyio.create_task_group() as tg:
        ...
```

- **Set `asyncio_default_fixture_loop_scope = 'function'`** unless you have a very specific reason to share an event loop across tests. The default in older versions was a warning-level footgun.
- **Don't mix `pytest-asyncio` and `anyio` in the same project** unless the boundary is explicit and documented. Pick one runner.
- **Async fixtures need an async runner.** A sync `@pytest.fixture` can't `yield` an async resource cleanly; use `async def` fixtures and the runner handles teardown.

## FastAPI: `httpx.AsyncClient` over `TestClient` for async tests

```python
from httpx import AsyncClient, ASGITransport

@pytest.fixture
async def client(app: FastAPI) -> AsyncIterator[AsyncClient]:
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url='http://test') as ac:
        yield ac

async def test_get_me(client: AsyncClient, auth_headers: dict[str, str]) -> None:
    response = await client.get('/me', headers=auth_headers)
    assert response.status_code == 200
    assert response.json()['email'] == 'ada@example.com'
```

- `TestClient` (Starlette's sync client) blocks inside async test code and silently uses a different event loop. `AsyncClient` + `ASGITransport` exercises the real ASGI path.
- Override dependencies via `app.dependency_overrides[get_db] = lambda: test_db`. Always restore in teardown (`app.dependency_overrides.clear()` in a fixture's `yield` cleanup).

## Mocks at real boundaries — never on internal collaborators

- Mock the **process boundary**: HTTP clients (`httpx.AsyncClient.get`), filesystem (when real I/O is too expensive), external CLIs (`subprocess.run`), clocks (`freezegun`, `time-machine`), random.
- **Do NOT mock your own service / repository / domain function** to "isolate" the route — that's testing the mock, not the system. Drive the real path with a real (in-memory or test-DB) implementation.
- `unittest.mock.patch` and `monkeypatch` are equally valid; `monkeypatch` is pytest-native and auto-reverts at test end.
- `mocker` (from `pytest-mock`) wraps `unittest.mock` with auto-cleanup — preferred when you need spies (`mocker.spy(obj, 'method')`).

```python
def test_signup_sends_welcome_email(monkeypatch: MonkeyPatch, client: TestClient) -> None:
    sent: list[str] = []
    monkeypatch.setattr(email, 'send', lambda to, body: sent.append(to))
    client.post('/signup', json={'email': 'ada@example.com', 'password': 'p' * 12})
    assert sent == ['ada@example.com']
```

The assertion is on **observable behavior** (an email was queued for that address), not on call-count of an internal helper.

## Autospec the double — a bare `MagicMock` answers for anything

A plain `Mock`/`MagicMock` returns a fresh child mock for *any* attribute you touch. Rename or delete a method on the real object and nothing notices: the call returns a mock instead of raising, downstream assertions still pass, and the suite stays green against a method that no longer exists. The `unittest.mock` docs state the consequence directly — "if you refactor some of your code, rename members and so on, any tests for code that is still using the old api but uses mocks instead of the real objects will still pass. This means your tests can all pass even though your code is broken."

- `patch('pkg.mod.Client', autospec=True)` — or `create_autospec(obj)` — builds the double's surface **from the real object**, recursively, and gives mocked functions and methods the original's call signature, so a wrong-arity call raises `TypeError` and a renamed attribute raises `AttributeError`. Under `pytest-mock` the `mocker` fixture takes the same arguments as `mock.patch`, so `mocker.patch(..., autospec=True)` works as-is; there is no `mocker.create_autospec` — import that from `unittest.mock`.
- Reach for it whenever you patch something **you own**, whose surface can be renamed out from under the test by a refactor in the same repo.
- **Not always right, which is why it isn't the default.** Autospec introspects the spec object, so properties or descriptors that execute code on attribute access can make it unusable. And it cannot see attributes created in `__init__` rather than declared on the class: under autospec, `thing.a` raises `AttributeError` even though real instances have it. The documented fixes are to set the attribute on the mock after creation, or to give the class a default class attribute. Speccing is lazy, so introspection cost is not itself a reason to skip it.

## Database tests: the real engine, not mocked SQL semantics

A mocked or faked connection replays the SQL string's *intended* logic in Python — it cannot catch a wrong join, a reserved word left unquoted, a function called with the wrong signature, or an `INSERT` column list that has drifted from the schema. Those are real, shipped defects, and every one of them is green under a mocked connection because nothing ever asked the real engine to parse the statement. This is a narrower question than whether the suite reaches the system through its real entry point at all — that's the integration-testing concern's; this is specifically whether any test that touches SQL hands it to the engine that will actually run it.

- Run against the same engine version you deploy, via `testcontainers` (`pip install testcontainers`; e.g. `from testcontainers.postgres import PostgresContainer`) or a CI-provided service — never a stand-in engine. SQLite in-memory is tempting but lies about concurrent access, FK enforcement, and JSON/array operators; don't substitute it for the production engine to save setup time.
- The cost objection doesn't hold up: one container, started once per session and shared read-only across the run, costs seconds, not the minutes intuition suggests. The alternative is finding the defect after merge instead of before. A project with no database has no reason to reach for this — don't add a container for its own sake.
- Build the schema under test by applying the project's real migrations, not a hand-maintained parallel `CREATE TABLE` fixture — a fixture schema drifts from the one that ships, silently invalidating everything tested against it.
- Isolate between tests with a session-scoped engine/connection and a function-scoped transaction that rolls back at teardown (fall back to truncation only where the code under test manages its own transactions and can't nest inside the test's). Each test sees a clean, migrated schema; no test pollutes the next.
- Factories (`factory_boy`, `polyfactory`, hand-rolled `make_user(**overrides)`) over hardcoded test data. Tests express what's *relevant* to the assertion, not every required field.
- **A fixture that does not truncate is not isolation.** Combined with a table keyed on something global rather than per-test, a leftover row silently turns the INSERT under test into an UPDATE and every assertion still passes. Prove the isolation rather than assuming it: a test that asserts the table is empty at setup will fail the moment the fixture stops doing its job.
- **`DROP DATABASE` per test is slower than it looks.** It forces a global checkpoint and blocks until the checkpointer has flushed the whole cluster, so parallel teardown across xdist workers serialises on one checkpointer no matter how many workers you run. For a throwaway test container the disk cost that makes this bite is not something you need: start it with `fsync=off`, `synchronous_commit=off` and `full_page_writes=off`. Those settings trade crash durability for speed, which is exactly the right trade for a container whose entire contents are discarded at the end of the run, and exactly the wrong one anywhere else.

## Markers for selection, not magic

```python
@pytest.mark.slow
@pytest.mark.integration
def test_full_pipeline() -> None: ...
```

- Register markers in `pyproject.toml` to avoid warnings:

```toml
[tool.pytest.ini_options]
markers = [
    'slow: tests that take > 1s',
    'integration: tests that hit external services',
]
```

- Run subsets via `-m 'not slow'` in fast feedback loops, full suite in CI.
- **A command-line `-m` replaces `addopts` entirely. It does not combine with it.** A marker exclusion written into `addopts` as a safety guarantee ("this suite never runs the destructive tests") is silently lost by any lane that passes its own `-m`, and the exclusion's absence is invisible in the output. If an exclusion has to hold, enforce it somewhere a command-line flag cannot overwrite (a `collection_modifyitems` hook, or a conftest-level skip keyed on the environment) rather than in `addopts`.
- Don't gate behavior with markers (`if request.node.get_closest_marker('xyz'):` inside a fixture) — that's logic-by-marker and gets unreadable. Use fixtures with explicit names.

## Never pipe pytest into a filter

`pytest ... | tail -50` gives you `tail`'s exit code, which is 0 whatever pytest did. This has reported a genuinely red lane as green. If you need less output, use pytest's own controls (`-q`, `--tb=short`, `-p no:randomly` for a specific reproduction), redirect to a file and read the file, or set `set -o pipefail` in the shell. It is the same failure the CI concern covers for release pipelines.

## Rank findings by whether they can produce a wrong pass

A test that goes red for the wrong reason wastes an afternoon. A test that goes **green** for the wrong reason removes a check nobody knows is gone, and it will stay that way for as long as the code it guards keeps working by luck. When you are triaging test-suite problems, sort by that axis before sorting by anything else.

Order dependence is the canonical source of wrong passes, and the reason it survives is that local runs hide it: the habitual command pins the order, while CI draws a fresh `pytest-randomly` seed every run. Two shapes worth recognising, both measured:

- **A content-addressed cache.** A test generates a "unique slug" and believes that isolates it. The cache key is a hash of the *text*, so the slug is irrelevant: a preceding test that embedded identical chunk text has already populated the cache, the whole batch is served from it, and the call under test never happens. Every assertion still passes. The test is now asserting the cache works.
- **A non-truncating database fixture plus a globally-keyed table.** A leftover row from an earlier test turns the INSERT under test into an UPDATE. Nothing raises, the row has the expected values at the end, and the code path the test was written for did not run.

In both, the giveaway is that the test asserts on end state without asserting the work happened. Assert on the call, the effect, or a counter: something that distinguishes "did the thing" from "the thing was already true".

## Tests that can never run are not tests

A skipped test is a green check with no evidence behind it, and a whole file of them is a coverage claim the suite does not make. Two real examples: 18 tests behind an environment variable nothing in the project ever sets, and 8 behind an `except ImportError: pytest.skip(...)` for a package the project never declared as a dependency, so the import could never succeed anywhere.

- If a test needs a service or a credential, the mechanism that provides it must exist somewhere (a compose file, a testcontainer, a CI secret) and something must actually run the lane. Otherwise delete the test rather than leaving it as decoration.
- If a test needs an optional import, declare that package in the test dependency group. `pytest.importorskip` is for a genuinely optional integration, not for hiding a missing declaration.
- Check the skip count in CI, and treat a rise in it as a finding. `-rs` lists skips with their reasons in the summary.

## Coverage as a tool, not a target

- `pytest-cov` measures executed lines. 100% line coverage with mock-everything tests is worse than 70% with real-path tests.
- Set a coverage **floor** (`--cov-fail-under=80`) only after the suite is healthy — too-early floors push contributors toward shallow tests to clear the bar.
- `# pragma: no cover` for genuinely unreachable lines (defensive `assert_never`, `if TYPE_CHECKING:` blocks). Never to skip something hard to test — that's a design smell.

## Mutation testing for what coverage can't tell you

Line coverage says a line executed; it says nothing about whether a test would have caught that line's behavior changing. `mutmut` (`pip install mutmut`) closes that gap for Python: `mutmut run` rewrites the source in small ways (flips a comparison, drops a negation, changes a constant) and reruns the suite against each mutant. A mutant the suite still passes against — a **survivor** — means nothing in the suite would notice if that exact behavior broke, whatever the coverage report says.

- A full-repo run is usually impractical to run routinely — scope it to what changed. Target a specific module with a wildcard (`mutmut run "billing.invoice*"`) or set `source_paths` in `[tool.mutmut]` to the package under change; `mutmut run` remembers prior results, so re-running after a small change is incremental, not a restart.
- `mutmut browse` lets you inspect survivors and apply one to disk to see the exact change that slipped through — commit before doing this, mutmut says so for a reason.
- Not every survivor is a gap: dead defensive code, `__repr__` output, and log message text can mutate with nothing meaningfully broken. Read the survivor before writing a test for it, but don't dismiss one just because it's inconvenient — most point to a missing or too-loose assertion.
- Reserve this for logic-bearing modules — the same ones the coverage-judgement guidance above already flags as worth pursuing (business rules, parsers, error paths). Running it over a thin DTO or generated code burns time for a signal nobody needed.

## Hypothesis for property-based tests on data transformations

```python
from hypothesis import given, strategies as st

@given(st.text(), st.text())
def test_concat_length(a: str, b: str) -> None:
    assert len(a + b) == len(a) + len(b)
```

For pure functions over data (parsers, serializers, encoders, math), Hypothesis finds the input you didn't think of. Pin examples that previously failed via `@example(...)` so regressions stay caught.

## Footguns

- **`-n auto` resolving to one worker on a 2-vCPU runner**, because xdist counts physical cores when psutil is present. Set the count explicitly in CI.
- **A crashed xdist worker being replaced instead of failing the run.** The default `max-worker-restart` re-runs the killing test on each replacement. Use `--max-worker-restart=0`.
- **Diagnosing a native crash under xdist**, where the traceback died with the worker. Re-run serially.
- **A command-line `-m` silently replacing an `addopts` marker exclusion** relied on as a safety guarantee.
- **Piping pytest into `tail`/`head`/`grep`**, which reports the filter's exit code and can turn a red lane green.
- **A test that passes because a previous test already did the work**: a content-addressed cache hit, or an INSERT turned into an UPDATE by a leftover row. Assert that the work happened, not only that the end state is right.
- **A `pytest-timeout` budget sized for the test body**, when one budget in fact covers setup, call and teardown together.
- **Tests gated on an environment variable nothing sets, or an optional import that is not a declared dependency**, which are skips reported as green.
- **`@pytest.fixture(scope='session')` on a mutable resource** — silent state leak between tests. Order-dependent test failures appear in CI but not locally.
- **Async tests with no async runner installed** — they pass instantly because the coroutine is never awaited. Add `pytest-asyncio` or `anyio` and a `RuntimeWarning` filter that fails the build.
- **`TestClient` (sync) inside async tests** — uses a different event loop, can deadlock on shared resources. Use `httpx.AsyncClient`.
- **`freezegun` / `time-machine` not torn down** — subsequent tests see frozen time. Fixture-scope the time freeze and let the context manager exit.
- **Asserting on log message content as primary behavior** — fragile. Log content is for humans; assert on the structured event or the side effect.
- **`assert response.status_code == 200` with no body assertion** — passes even when the handler returns wrong data. Assert on the response shape too.
- **Tests that import the module under test at module top with side effects** — collection time runs the side effects. Push imports inside fixtures or use `pytest.importorskip`.
- **`pytest.skip(...)` without a reason string** — invisible in CI summaries. Always `pytest.skip('requires GPU')`.
- **Tests sharing a `tmp_path` by accident** — `tmp_path` is per-test; `tmp_path_factory` is shared. Use the right one.
- **`-x` / `--exitfirst` hiding cascading failures** — fine for local, never in CI. CI runs the full suite.
