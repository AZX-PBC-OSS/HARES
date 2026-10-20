---
description: "Modern Python 3.12+ patterns — strict typing, PEP 695 generics, async correctness and structured concurrency, async worker patterns, web framework integration (FastAPI/Django), error discipline, and project tooling"
triggers:
  file_globs:
    - "**/*.py"
    - "**/*.pyi"
---

# Python

Modern Python is statically typed, asynchronously safe by default, and tooled with fast checkers run through a single project manager — Ruff for linting and formatting, Pyright for type checking, uv for environments and dependencies. The rules below are the merge bar for new code on Python 3.12+. Older codebases should adopt them incrementally — never regress them on touched files.

## Discover the project's tooling first

Before writing or changing code, check the project's configuration: `pyproject.toml` (build backend, dependencies, tool sections for `ruff`, `pyright`/`mypy`/`ty`, `pytest`), `pyrightconfig.json` if present, `uv.lock` / `poetry.lock` / `requirements*.txt`, and any `.python-version` / `tool.uv` / `tool.poetry` block. Match the project's existing import style, formatter, type-checker, and minimum supported Python version. If the project pins `requires-python = ">=3.10"`, do not reach for `type X = ...` (PEP 695) or `match` exhaustiveness on `assert_never` without confirming the target. If the project has no formatter or linter configured, flag that as a gap — do not silently add tooling in a feature ticket.

Run the tools the way the project runs them. Where uv manages the project, that is `uv run ruff check`, `uv run pyright`, `uv run pytest` — the environment the gate uses is then the environment you tested in, and a check that passes locally passes in automation for the same reason.

## Type hints are mandatory on the public surface

- Every function/method parameter, every return type, every module-level constant, every class attribute. Internal locals are inferred.
- `from __future__ import annotations` only if you support Python < 3.10 — on modern targets it complicates `inspect.get_type_hints` and runtime-introspection libraries (Pydantic, FastAPI, attrs).
- Run a type checker at its strictest setting in CI — `typeCheckingMode = "strict"` for Pyright, `--strict` for mypy. Failing typecheck blocks merge; warnings are not optional. Astral's `ty` is in beta on `0.0.x` and its own README states it "does not yet have a stable API; breaking changes, including changes to diagnostics, may occur between any two versions" ([ty README](https://github.com/astral-sh/ty)) — worth trying, but a gate whose diagnostics move between patch releases is not yet a gate.
- Never `# type: ignore` without a specific error code AND a comment naming the reason: `# type: ignore[arg-type]  # third-party stub missing — see issue #...`. Bare `# type: ignore` masks every error on the line forever.
- Never `cast(X, value)` to silence a checker. `cast` performs no runtime check whatsoever — it is a pure instruction to the checker to stop asking. Cast at genuine boundaries (deserialization, dynamic dispatch) and validate the shape first, so the type you assert is one something actually verified.

## `Any` is contagious, and its damage is silent

A type checker is a check that runs outside whoever wrote the code: deterministic, on every line, at authoring time. `Any` is the switch that turns it off, locally, with no error to mark where checking stopped.

It does not stay local. Every expression derived from an `Any` is itself `Any`, so one loosely typed return value un-checks its callers, their callers' locals, and every attribute reached through them — attribute typos, wrong argument counts and wrong types all pass, silently, far from the annotation that caused it. Unlike a `# type: ignore`, which at least sits on one line and appears in a diff, `Any` propagates with no marker at all.

Reach for these instead, by shape of problem:

- **`object`** when you truly accept anything — it is the *safe* top type. `object` permits nothing until you narrow, so the checker keeps working; `Any` permits everything. Wanting "any value" almost always means `object` (or a `TypeVar`), and a bare `object` on something you then need to *use* is the wrong reach in the other direction — it means the type was never determined.
- **`Protocol`** for a structural contract — "anything with `.read()`" — instead of `Any` on a duck-typed parameter. No inheritance required from the caller, and the checker still verifies the shape.
- **Type parameters (`def first[T](items: list[T]) -> T`)** when the return type depends on the argument. This is the single most common thing `Any` is used to fake, and the one it fakes worst: the relationship between input and output is exactly what `Any` discards.
- **`ParamSpec` (`def wraps[**P, R](fn: Callable[P, R]) -> Callable[P, R]`)** for decorators and wrappers, so the wrapped signature survives. A decorator annotated `Callable[..., Any]` un-types every function it decorates.
- **`Literal[...]` or an `Enum`** for a closed value set, never `str`. Misspellings become errors and `assert_never` becomes possible.
- **`NewType('UserId', str)`** for identifiers and validated values, so passing a path where an ID is expected is a type error. Note that a PEP 695 alias (`type UserId = str`) is *transparent* and buys no such safety — it is documentation. `NewType` creates a distinct type; use it when argument-swap is the bug you're preventing.
- **`TypedDict`** for a structured dict payload — a JSON body, a config blob — instead of `dict[str, Any]`, which un-types every value read out of it.
- **`assert_never`** in the default branch of an exhaustive match (see below), so adding a variant fails the checker.

### Where `Any` is correct

`Any` is in the language because some things genuinely have no static type. These are not findings:

- **A decorator or wrapper that must accept arbitrary callables** — though prefer `ParamSpec` where the signature should be preserved, and `Callable[..., object]` where the return isn't used.
- **The immediate return of a genuinely dynamic boundary** — `json.loads`, `yaml.safe_load`, a `**kwargs` passthrough into an untyped library. Annotate the boundary and validate immediately; the point is that `Any` stops there rather than flowing on.
- **A third-party library with no stubs or wrong stubs.** Confine it: wrap the call in one typed adapter function so the `Any` dies at that function's annotated return type instead of spreading through the codebase.

The test in every case is containment. An `Any` that terminates at a validated boundary is doing its job. An `Any` that reaches business logic has disabled type checking for everything it touched on the way.

## Turn the checkers up — the defaults check less than you think

Both halves of Python's static tooling ship on a middle setting rather than their strictest, and the gap between the default and what the tool can do is where most real findings live.

**Type checking.** Pyright's default mode is `standard`, not `strict` ([Pyright configuration](https://microsoft.github.io/pyright/#/configuration)) — so a project that never set `typeCheckingMode` is running a middle setting, and the gap up to `strict` is where the findings are. Set it, in `[tool.pyright]` inside `pyproject.toml` or in `pyrightconfig.json` (if both exist, `pyrightconfig.json` wins entirely — a `[tool.pyright]` block sitting beside one is dead configuration).

Pyright is not configured as a bag of on/off flags. A mode sets a baseline and each `reportXxx` rule is then re-levelled on top of it, taking a *severity* — `"none" | "information" | "warning" | "error"` — rather than a boolean. What `strict` buys over `standard` is a block of rules moving from `"none"` to `"error"`, and they are mostly the ones that catch inference quietly failing: `reportUnknownParameterType`, `reportUnknownArgumentType`, `reportUnknownVariableType`, `reportUnknownMemberType`, `reportMissingParameterType`, `reportMissingTypeStubs`, plus `reportPrivateUsage`, `reportDeprecated` and `reportUnusedImport`. An unknown type is the checker telling you it gave up; under `standard` it says so silently.

Two consequences worth knowing:

- **Unannotated function bodies are analysed by default** (`analyzeUnannotatedFunctions` defaults to `true`), which is the opposite of mypy, where an un-annotated body is skipped entirely until `--check-untyped-defs` ([mypy command-line reference](https://mypy.readthedocs.io/en/stable/command_line.html)). On mypy, adding a file with no hints therefore produces zero errors and zero confidence; on Pyright it does not.
- **Missing third-party stubs degrade quietly.** `useLibraryCodeForTypes` defaults to `true`, so Pyright infers from library source when no stubs exist — incompletely. `reportMissingTypeStubs` is what tells you it happened, and that is `"none"` until strict.

Scope relaxations by directory rather than globally: Pyright's `executionEnvironments` takes a `root` and per-rule overrides under it, which is how test directories get the unknown-type rules relaxed while `src` stays strict. Relaxing the same rules repository-wide to make tests pass is the loss of the gate that mattered.

Note that **upstream Pyright's configuration reference lists no `reportAny` or `reportExplicitAny` rule** — those come from the basedpyright fork. Don't cite them as Pyright settings.

On mypy, the equivalent lever is `--strict`, with one gap worth naming: it bans *implicit* `Any` but not `Any` you write by hand — `--disallow-any-explicit` is a separate opt-in.

**Linting.** Ruff's default rule selection has broadened substantially across releases, so establish it for the version in use (`ruff check --show-settings --isolated`) rather than recalling it. What is stable enough to rely on: `ANN` (flake8-annotations) is not enabled by default, including **ANN401**, which flags `Any` in function arguments ([Ruff default rules](https://docs.astral.sh/ruff/default-rules/), [ANN401](https://docs.astral.sh/ruff/rules/any-type/)). Beyond whatever the default is, families worth selecting deliberately: `S` (bandit), `ASYNC`, `B`, `UP`, `SIM`, `RUF`, and `PGH`, which catches the blanket `# type: ignore` / `# noqa` this file bans below. Select what the project will actually act on; a selection nobody fixes is worse than a smaller one everyone does.

## A warning is a finding the tool couldn't prove

A warning means the tool found a shape that is *usually* wrong and cannot prove it is wrong here. That inability is the reason it warns rather than errors — it is handing you a judgement it isn't entitled to make. Silencing it answers that judgement with "I didn't look", which is the same trade as `Any` one level up: a mechanism converted back into trust. `DeprecationWarning` is the clearest case — it is a library telling you, ahead of time and for free, exactly what will break on the next upgrade.

- **Treat warnings as gate failures, not as a backlog.** Ruff exits non-zero on findings; make the type checker blocking too, and run tests with `filterwarnings = ["error"]` so a deprecation surfaces the day it appears rather than the day the dependency drops the API. A warning nobody must act on is one everybody learns to scroll past.
- **A suppression is sited exactly where the constraint is** — one rule, one line, naming the *constraint* rather than the code: `# noqa: S603  # argv is a fixed literal list, never user input`. The same directive with no trailing text is a shrug.
- **The same suppression repeated many times is not many exceptions.** It is one missing abstraction — wrap the pattern once and suppress there — or a rule that does not fit this project and should be turned off deliberately in configuration, with a reason, where a reader can see it. Deciding once, in the open, is the good outcome; deciding never, invisibly, at fifteen sites, is the bad one.

## Suppression in configuration counts as suppression

An in-line `# noqa` or `# type: ignore` is at least visible: it sits on the offending
line, appears in the diff that introduced it, and a reviewer reading the code sees it.
A suppression written into tool configuration is the same act performed invisibly — it
silences the warning for every current and future line in scope, does not appear
anywhere near the code it excuses, and survives long after the reason for it is gone.
**Config-file suppression is the more durable way to hide a warning and is held to a
stricter standard than the in-line form, not a looser one.**

These are all suppressions, and each needs the same justification an in-line ignore
would:

```toml
[tool.ruff.lint]
ignore = ["E501"]                       # rule disabled repository-wide

[tool.ruff.lint.per-file-ignores]
"app/services/*.py" = ["S", "ANN"]      # whole directory exempted from a rule family

[tool.ruff]
exclude = ["app/legacy"]                # files removed from linting entirely

[tool.pyright]
typeCheckingMode = "basic"              # the whole project's checking level lowered
reportUnknownMemberType = "none"        # a strict-mode rule switched off repository-wide

[[tool.pyright.executionEnvironments]]
root = "src/workers"                    # a package exempted from rules the rest of src keeps

[[tool.mypy.overrides]]
module = "app.workers.*"
ignore_errors = true                    # type checking switched off for a package

[tool.pytest.ini_options]
filterwarnings = ["ignore::DeprecationWarning"]   # the deprecation you were meant to fix

[tool.coverage.run]
omit = ["app/spawn/*"]                  # untested code made to look covered
```

Coverage `exclude_lines` is the same judgement in miniature — these two entries are not
equivalent:

```toml
[tool.coverage.report]
exclude_lines = [
    "if TYPE_CHECKING:",   # defensible — genuinely never executes at runtime
    "except .*:",          # not defensible — hides every untested error path
]
```

The test for any suppression is whether it excludes something unreachable or merely
something unverified.

Rules:

- **Never widen an existing ignore list to make a gate green.** Fix the finding. If it
  genuinely cannot be fixed now, suppress the narrowest possible scope — one rule, one
  file, in-line if the tool supports it — with a comment naming the reason.
- **Never add an `exclude` / `omit` / `ignore_errors` entry for code you just wrote.**
  Excluding new code from linting, type checking, or coverage is not a workaround; it
  removes the code from every gate the project has.
- **`filterwarnings = ["ignore::..."]`** hides the upgrade signal the library is
  sending. Filter the specific warning class from the specific module, never the class
  globally.
- **Coverage `omit` and `exclude_lines` change the number without changing the risk.**
  A module in `omit` is untested code that reports as covered.
- **Review the config diff.** Changes to `pyproject.toml` / `setup.cfg` / `ruff.toml` /
  `mypy.ini` lint, type, warning, and coverage sections are review-relevant changes to
  the project's quality bar, and are reviewed as such — not waved through as
  configuration.
- **Prefer a dated, attributed entry over an anonymous one.** A suppression with no
  recorded reason can never be safely removed, so it never is.

## Modern union and optional syntax

```python
# YES — built-in syntax, no import
def find(name: str) -> User | None: ...

# NO — legacy, requires typing import, more characters
def find(name: str) -> Optional[User]: ...
def find(name: str) -> Union[User, None]: ...
```

`X | Y` works at runtime on 3.10+. Use it everywhere. Same for `dict[str, int]` over `Dict[str, int]`, `list[T]` over `List[T]`, `tuple[int, ...]` over `Tuple[int, ...]`.

## PEP 695 generics and type aliases (3.12+)

```python
# Type alias — lazy evaluation, no forward-reference pain
type UserId = str
type Result[T] = T | ErrorInfo

# Generic function — type parameter in the signature
def first[T](items: list[T]) -> T:
    return items[0]

# Generic class — no Generic[T] inheritance
class Box[T]:
    def __init__(self, value: T) -> None:
        self._value = value
```

Drop the legacy `TypeVar('T')` + `Generic[T]` style on 3.12+. Bounds (`def longest[T: str](a: T, b: T) -> T:`) and constraints (`def f[T: (str, int)](x: T) -> T:`) live in the same brackets.

## Discriminated unions with `Literal` and exhaustive matches

```python
from typing import Literal, assert_never

type Shape = (
    tuple[Literal['circle'], float]      # radius
    | tuple[Literal['square'], float]    # side
    | tuple[Literal['triangle'], float, float, float]
)

def area(shape: Shape) -> float:
    match shape:
        case ('circle', r):
            return 3.14159 * r * r
        case ('square', s):
            return s * s
        case ('triangle', a, b, c):
            return _heron(a, b, c)
        case _:
            assert_never(shape)
```

`assert_never` makes the type checker fail when a new variant is added but not handled. A bare `raise` in the default does NOT catch it. Apply on every closed union you switch on.

## TypedDict for shaped dicts; dataclass/Pydantic for objects

- `TypedDict` — when an existing dict (third-party API response, JSON config) has known keys.
- `@dataclass(frozen=True, slots=True, kw_only=True)` — internal value types with no validation needs. `frozen` for hashability + accidental-mutation safety; `slots` cuts memory and forbids attribute typos; `kw_only` prevents argument-order swap bugs.
- Pydantic `BaseModel` — when validation, coercion, or JSON (de)serialization at a boundary is required. v2's Rust-backed core is meaningfully faster than v1's.

Don't use a dataclass where a `TypedDict` would do (no instance overhead, no `__init__` to maintain), and don't use a `TypedDict` where a Pydantic model would do (no validation, no coerced values).

## `Final`, `Self`, `override`, `assert_never`

- `Final[X]` on module-level constants the type checker should treat as immutable.
- `from typing import Self` for method return types in a base class — subclasses get the right type without redeclaration.
- `from typing import override` (3.12+) on every overridden method — the checker fails if the parent renames or removes the method.
- `assert_never` (above) on exhaustive match defaults.

## Async correctness

```python
import anyio

async def fetch_all(urls: list[str]) -> list[Response]:
    async with anyio.create_task_group() as tg:
        results: list[Response | None] = [None] * len(urls)
        async def fetch(i: int, url: str) -> None:
            results[i] = await client.get(url)
        for i, url in enumerate(urls):
            tg.start_soon(fetch, i, url)
    return [r for r in results if r is not None]
```

- **Structured concurrency by default.** `asyncio.TaskGroup` (3.11+) or `anyio.create_task_group()` over loose `asyncio.create_task` followed by `await asyncio.gather(...)`. Task groups cancel siblings on first failure; bare `gather` leaks tasks on exception.
- **`anyio` over raw `asyncio` for libraries** that need to support both `asyncio` and `trio`, or for cancellation scopes (`anyio.CancelScope`, `anyio.move_on_after`, `anyio.fail_after`) that are richer than `asyncio.timeout`.
- **`asyncio.timeout(...)` (3.11+)** over `asyncio.wait_for` — composes with task groups, cancels cleanly.
- **Never block the event loop.** No `time.sleep`, no `requests` (sync), no `open()` for large files, no CPU-heavy work in an async function. Use `await asyncio.sleep`, `httpx.AsyncClient`, `anyio.open_file`, or push CPU work to `asyncio.to_thread` / `anyio.to_thread.run_sync`.
- **Never swallow `CancelledError`.** A bare `except Exception:` catches it on Python ≤ 3.7; on 3.8+ it doesn't, but `except BaseException:` does. If you catch it, re-raise after cleanup. Cancellation is a control signal, not a fault.
- **Async generators and yield in cancel scopes** — yielding across a cancel-scope boundary inside an async generator can leak cancellation into the wrong scope or suppress it entirely. PEP 789 documents this class of bug and proposes a language-level fix, but it is still in Draft status (targeting 3.14, not yet implemented) — treat it as a known footgun to code around today, not a landed guarantee. Prefer plain async functions returning lists / async iterators built with `async for`.

## Async worker patterns

Where the project runs background workers — Celery, dramatiq, taskiq, or `asyncio` task queues — the patterns below apply.

- **Idempotent task bodies.** A worker that retries a task on failure re-executes the body. If the body is not idempotent (sends an email, charges a card, appends a row), a retry double-applies the effect. Use a deduplication key or an at-least-once-to-exactly-once bridge (an outbox table, a transactional inbox) to make retries safe.
- **Timeouts on every external call.** A worker stuck on a network call holds its slot indefinitely, reducing throughput. Set a timeout on every `httpx` call, every database query, every cache read — and let the timeout propagate to the task retry logic, not to a catch-all that swallows it.
- **Structured logging with task context.** Every log line in a worker should carry the task ID, the attempt number, and the correlation ID from the originating request. Without these, a failure in a background task is untraceable to the request that enqueued it.
- **Graceful shutdown.** A worker receiving a shutdown signal should finish in-flight tasks (or cancel them and record the cancellation), close database connections, and exit. A worker that exits mid-task without recording leaves the task in an ambiguous state — the queue cannot tell whether it succeeded or failed.
- **Don't block the event loop in async workers.** The same rule as async correctness above applies doubly in a worker: a `time.sleep` or a sync database call in an async worker stalls every other task on that worker. Push sync work to `asyncio.to_thread` or use async drivers.

## Web framework integration

Where the project uses a web framework (FastAPI, Starlette, Django, Flask), the async patterns above interact with the framework's request lifecycle:

- **FastAPI/Starlette:** `async def` handlers run on the event loop; `def` handlers run in a threadpool. Never mix sync I/O into `async def` — use `asyncio.to_thread` or switch the handler to `def`. Dependency injection with `yield` handles cleanup; a dependency that opens a resource without `yield` leaks it.
- **Django:** async views (`async def`) are supported but Django's ORM is still sync. Use `sync_to_async` wrappers or `django.http.HttpResponse` patterns that stay sync. A bare `await` on an ORM call in an async view raises `SynchronousOnlyOperation`.
- **Background tasks in the request lifecycle:** `BackgroundTasks` (FastAPI/Starlette) runs after the response is sent, in the same process. It is fine for trivial fire-and-forget work (a cache bust, a log line) but not for anything that needs durability — a process crash between response and background task silently drops the work. Use a real queue for anything that must not be lost.

## Error discipline

- **Custom exceptions, not bare `Exception`.** Define a domain exception hierarchy (`class WorkspaceError(Exception): ...` → `class WorkspaceReadError(WorkspaceError): ...`). Callers catch the specific subclass; `except WorkspaceError:` catches the family.
- **Chain causes.** `raise WorkspaceReadError(path) from err` — preserves the underlying traceback. Never `raise X()` inside an `except` without `from err` or `from None`.
- **Narrow `except` clauses.** `except (KeyError, ValueError):` over `except Exception:`. A bare `except:` catches `KeyboardInterrupt` and `SystemExit` and is almost always wrong.
- **Never `except: pass`** with no log, no comment. Every silent swallow is a bug-incubator.
- **Use `raise ... from None`** only when the underlying error genuinely is not useful to the caller (e.g. internal-implementation `KeyError` mapped to a public `LookupError`).

## Imports and module layout

- Absolute imports inside the package (`from mypkg.module import X`), not relative (`from .module import X`) once the package has more than one directory level — relative imports break on script-style invocation.
- One public class/function per module is overkill; one *concept* per module is right.
- `__all__` only when `from pkg import *` is a supported public API. Otherwise it's drift bait.
- No top-level side effects in modules. No `print`, no `open`, no network calls. Imports must be safe.

## Idioms that catch real bugs

- **`pathlib.Path` over `os.path` strings.** `Path('/a') / 'b'` is portable; `os.path.join('/a', 'b')` returns a `str` you have to track.
- **`dict.get(key, default)`** over `try: dict[key]; except KeyError:` for defaulted lookups.
- **`enumerate(items)`** over `for i in range(len(items)):`.
- **`zip(a, b, strict=True)`** (3.10+) when both iterables MUST be the same length — silent truncation is the default and has caused real outages.
- **f-strings with `=`** for debug logs: `logger.debug(f'{user_id=} {action=}')`.
- **`functools.cache` / `functools.lru_cache(maxsize=...)`** for pure-function memoization. Never on instance methods (leaks `self` references) — use `cached_property` instead.
- **Context managers (`with`)** for every file, lock, connection. Never `f = open(...)` without a matching `f.close()`.

## What NOT to flag

- **`Optional[X]` / `Union[X, Y]`** in a project whose `pyproject.toml` pins Python < 3.10 — the modern `X | None` syntax isn't available there; this is a target-version constraint, not a style lapse.
- **`# type: ignore[code]` with a comment naming the reason** — this is the correct, narrow form of suppression, not a smell. Only the bare or unattributed form is a finding.
- **`Any` in a decorator or `*args`/`**kwargs` passthrough**, and `Any` as the immediate return of `json.loads` / an unstubbed library, where it is validated or narrowed before going anywhere. Contained `Any` is `Any` working correctly. Flag the ones that reach business logic.
- **`object` used as the safe top type** on a parameter that is genuinely narrowed before use — that is the recommended replacement for `Any`, not an instance of the same problem. The finding is `object` on a value the code then needs to *use* without narrowing.
- **A ruff rule or mypy check disabled in `pyproject.toml` with a stated reason**, where it genuinely doesn't fit the project. That is the decision made in the open, which is what the warning rules are asking for. The finding is the same rule silenced at fifteen call sites with nobody ever deciding.
- **A project not running `--strict` or a wide ruff selection** — worth proposing, but turning either on is a migration with real work behind it, not a ticket-scoped edit. Note the gap; don't fail the change over it.
- **Not using PEP 695 generics (`type X = ...`, `class Box[T]`)** on a codebase pinned below 3.12 — flag the version constraint if relevant, not the missing syntax.
- **A module-level side effect that is itself the module's purpose** — a CLI entry script, a Pydantic-settings bootstrap read. The "no top-level side effects" rule targets *incidental* I/O in importable library modules, not scripts designed to run on import.

## Footguns

- **Mutable default arguments** — `def f(items: list[T] = []):` shares the list across all calls. Use `items: list[T] | None = None` and assign inside.
- **Late binding in closures inside loops** — `[lambda: i for i in range(3)]` all return `2`. Bind explicitly: `[lambda i=i: i for i in range(3)]`.
- **`is` vs `==`** — `is` checks identity, `==` checks equality. `x is None` / `x is True` / `x is False` are the only correct uses of `is` for value comparison.
- **Comparing floats with `==`** — use `math.isclose(a, b, rel_tol=...)`.
- **`datetime.now()` vs `datetime.now(tz=...)`** — naive datetimes silently drop timezone info. Always pass `tz=` (or use `datetime.now(UTC)` on 3.11+).
- **Iterating over a dict while mutating it** — `RuntimeError: dictionary changed size during iteration`. Iterate over `list(d.items())` if you must.
- **`subprocess.run(cmd_string, shell=True)`** — shell injection. Pass `args` as a list and never `shell=True` with user input.
- **`json.loads` returns `Any`** — type as `object` and validate with Pydantic / a `TypedDict` + runtime check.
- **`yaml.load` without `Loader`** — arbitrary code execution. Always `yaml.safe_load`.
