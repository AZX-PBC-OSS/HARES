---
description: FastAPI production patterns — async-correct routes, dependency injection, Pydantic v2 schemas, security, error contracts
triggers:
  file_globs:
    - "**/routers/**/*.py"
    - "**/routes/**/*.py"
    - "**/endpoints/**/*.py"
    - "**/api/**/*.py"
    - "**/dependencies.py"
    - "**/deps.py"
---

# FastAPI

FastAPI is async-first, dependency-injected, and built on Pydantic v2 + Starlette. Routes are thin; business logic lives in services; persistence lives in repositories. The rules below cover the patterns that prevent the highest-cost production bugs.

## Discover the project's structure first

Before adding routes or schemas, read the project layout. A typical production layout separates `api/` (routers), `schemas/` (Pydantic request/response models), `services/` (business logic), `repositories/` or `db/` (data access), `core/` (config, security, middleware), and `main.py` (`FastAPI()` factory + router registration). Match the project's conventions: where DB sessions are produced, how settings are injected, what versioning prefix routers use, what the error-response contract looks like. Don't introduce a parallel structure in a feature ticket.

## `def` vs `async def` — pick correctly

```python
@router.get('/items/{id}')
async def get_item(id: int, db: AsyncSession = Depends(get_db)) -> ItemResponse:
    return await item_service.get(db, id)

@router.post('/parse-token')
def parse_token(token: str = Depends(oauth2_scheme)) -> TokenInfo:
    return decode_jwt(token)
```

- `async def` for **any handler that performs I/O** (DB, HTTP, file, queue). Blocking I/O inside `async def` stalls the event loop and degrades every concurrent request.
- `def` (sync) for **pure CPU-light handlers with no I/O** — JWT decode, in-memory transformation. FastAPI runs sync handlers in a threadpool; the threadpool overhead is real but acceptable for I/O-free work.
- **Never** call sync I/O (`requests`, `time.sleep`, blocking DB drivers) inside `async def`. Use `httpx.AsyncClient`, async DB drivers (`asyncpg`, `aiomysql`, `motor`), or push to `asyncio.to_thread` / `anyio.to_thread.run_sync` if no async API exists.

## Pydantic v2 schemas: separate request, response, internal

```python
from pydantic import BaseModel, ConfigDict, EmailStr, Field

class UserBase(BaseModel):
    model_config = ConfigDict(from_attributes=True, str_strip_whitespace=True)
    email: EmailStr
    display_name: str = Field(min_length=1, max_length=64)

class UserCreate(UserBase):
    password: str = Field(min_length=12)

class UserResponse(UserBase):
    id: UserId
    created_at: datetime
```

- **Three-layer split**: input (`UserCreate`), output (`UserResponse`), and internal/persisted (`User` ORM or domain object). Never let an input model leak into a response (`password` ends up serialized to clients) and never let an ORM model serialize directly (lazy-loaded relations explode at JSON time).
- **Declare the response shape once.** A return-type annotation (`-> UserResponse`) is enough — FastAPI derives validation, response filtering and OpenAPI from it, and an explicit `response_model=` is only needed where the wire shape must differ from what the function actually returns. Writing both is redundant and is a live drift risk: if the two disagree, `response_model` silently wins and the annotation the reader trusts is a lie.
- **`from_attributes=True`** to construct response models from ORM objects (`UserResponse.model_validate(orm_user)`).
- **`Field(...)` for validation, not bare types.** `min_length`, `max_length`, `ge`, `le`, `pattern` enforce the contract at the boundary.
- **`EmailStr`, `HttpUrl`, `IPvAnyAddress`, `SecretStr`** from Pydantic — use the precise types instead of `str`.
- **`SecretStr` for passwords/tokens in models** — `repr` and `str` redact the value, preventing log leakage.

## Dependency injection: composable, typed, scoped

```python
async def get_db() -> AsyncIterator[AsyncSession]:
    async with SessionLocal() as session:
        yield session

async def get_current_user(
    token: str = Depends(oauth2_scheme),
    db: AsyncSession = Depends(get_db),
) -> User:
    user = await user_service.from_token(db, token)
    if user is None:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, 'Invalid token')
    return user

CurrentUser = Annotated[User, Depends(get_current_user)]
DbSession = Annotated[AsyncSession, Depends(get_db)]

@router.get('/me')
async def me(user: CurrentUser) -> UserResponse:
    return UserResponse.model_validate(user)
```

- **`Annotated[T, Depends(...)]` aliases** — define once, reuse across routes. Cleaner signatures, single source of truth, easier to swap in tests.
- **Generator dependencies (`yield`)** for resources that need cleanup (DB sessions, file handles, locks). The teardown after `yield` runs even when the request errors.
- **One responsibility per dependency.** `get_db`, `get_current_user`, `require_admin`, `get_pagination_params` — small, composable, individually testable.
- **`dependencies=[...]` at the router or app level** for cross-cutting requirements (auth on every route in a router) — they run but their value is discarded, so they're for side effects (auth checks) not data.
- **`Depends(provider, use_cache=False)`** when the same dependency must run multiple times in one request (rare — usually means the dependency is wrong).

## Configuration: `pydantic-settings` with strict validation

```python
from pydantic import SecretStr
from pydantic_settings import BaseSettings, SettingsConfigDict

class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file='.env', env_file_encoding='utf-8', extra='forbid')

    database_url: SecretStr
    jwt_secret: SecretStr
    jwt_algorithm: str = 'HS256'
    access_token_minutes: int = 15

@lru_cache
def get_settings() -> Settings:
    return Settings()
```

- **`extra='forbid'`** — typo-detection. An unknown env var fails startup loudly.
- **`SecretStr` for credentials** — never logged accidentally.
- **`@lru_cache`** so settings parse once, not per request.
- **Inject via `Depends(get_settings)`** in routes/services that need them — never import a module-level settings instance at module top, which runs validation at import time and makes test overrides painful.
- **Configuration is read through the settings object, once, at the boundary.** No `os.environ` / `os.getenv` reads outside `Settings`: an undeclared env read is untyped, unvalidated, and invisible to anyone reading the class, and a spawned worker that re-derives config from its own ambient environment silently resolves a different configuration than the parent. Pass resolved settings to subprocesses explicitly.

## Error contract: `HTTPException` for client errors, exception handlers for domain errors

```python
class WorkspaceNotFound(Exception):
    def __init__(self, workspace_id: str) -> None:
        self.workspace_id = workspace_id

@app.exception_handler(WorkspaceNotFound)
async def workspace_not_found_handler(_: Request, exc: WorkspaceNotFound) -> JSONResponse:
    return JSONResponse(
        status_code=404,
        content={'error': 'workspace_not_found', 'workspace_id': exc.workspace_id},
    )
```

- **Domain layer raises domain exceptions** — `WorkspaceNotFound`, `UserAlreadyExists`. Never raise `HTTPException` from a service or repository; it ties business logic to HTTP.
- **Routes either let domain exceptions propagate** (with a registered handler) or catch and translate explicitly to `HTTPException`. The route is the HTTP boundary.
- **One error envelope across the API.** `{ "error": "<machine_code>", "detail": "<human>", ... }`. Clients write switch statements on `error`, never on prose.
- **Never leak stack traces, SQL, or file paths in 5xx responses.** Log them server-side; return `{ "error": "internal_error", "request_id": "..." }`.

## Authentication and authorization

- **OAuth2 + JWT (short-lived access token, longer refresh token)** is the modern default. Access tokens 5–15 minutes; refresh tokens days/weeks with rotation.
- **Hash passwords with `pwdlib`** (`PasswordHash.recommended()` — Argon2 with bcrypt fallback). `passlib` is unmaintained and broken on current Python — don't reach for it in new code; FastAPI's own docs have moved to `pwdlib`. Never SHA-256, never plain. Use cost parameters that match current OWASP guidance. Watch for this in optional form too: if the Argon2 implementation is an optional extra and the code falls back to a fast hash when it is absent, then every environment that skipped the extra is storing credentials under a fast hash, and a boot-time guard that only fires in production leaves development and staging exposed. Make the hashing dependency required.
- **`SecretStr` for the JWT signing secret** — must be loaded from env/secret-manager, never committed.
- **Encode/decode JWTs with `PyJWT`**, actively maintained. Avoid `python-jose` — unmaintained, with a known DoS CVE via JWE compression.
- **Verify the JWT `alg` header explicitly** — reject `alg: none` unconditionally; whitelist the algorithms you accept.
- **Signed non-session tokens (password reset, email confirmation, unsubscribe, signed URLs) are not a JWT job.** Use `itsdangerous` (`URLSafeTimedSerializer`) — small, actively maintained, does exactly this: sign a value, embed a timestamp, and reject it on read once `max_age` is exceeded (`serializer.loads(token, max_age=3600)`). Signing is not encryption: the payload is base64-readable, only tampering and expiry are prevented — never put a secret in the payload itself. The serializer's secret key is loaded from settings like any other credential, never hardcoded. Exception: a session cookie handled by the framework's own signed-session support doesn't need a second signing mechanism layered on top.
- **Authorization on every endpoint.** Authentication tells you who; authorization tells you what they can do. Object-level checks (can user X read record Y?) are where most real bugs live. OWASP API Top 10 puts broken access control at #1.
- **Scopes / roles via dependencies**, not ad-hoc `if user.is_admin:` checks scattered across routes. `require_role('admin')` as a dependency factory.

## CORS, rate limiting, security headers

- **`CORSMiddleware` with explicit `allow_origins`** — list the exact frontend origins. `allow_origins=['*']` with `allow_credentials=True` is a serious hole and **nothing stops you writing it**: Starlette's `CORSMiddleware` does not reject the combination, it works around the browser's ban on a wildcard-plus-credentials response by echoing the request's `Origin` header back in `Access-Control-Allow-Origin`. The result is credentialed cross-origin access from *every* origin, with no error at startup and no warning in the logs. Treat this pairing as a blocking review finding — there is no framework guard behind you.
- **Rate-limit credential and write endpoints** (login, password reset, signup, expensive queries). `slowapi` or `fastapi-limiter` with Redis are the standard choices.
- **Security headers via middleware**: `Strict-Transport-Security`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: strict-origin-when-cross-origin`, `Content-Security-Policy` (if you serve HTML).

## Database: async sessions, no N+1, no lazy load across the boundary

- **Async ORM (`SQLAlchemy 2.x` async, `Tortoise`, `SQLModel` async, `Piccolo`) over sync** in `async def` handlers. Pick the async driver underneath deliberately: `asyncpg` for Postgres, `asyncmy` for MySQL (`aiomysql` is unmaintained), `motor` for MongoDB.
- **Connection-pool sizing is derived, not guessed.** The ceiling is the database's own `max_connections` divided across every process that connects to it (workers × pool + overflow, plus migrations, cron, and any other service on the same instance) — exceed it and new connections are refused outright. Below that ceiling, size to the concurrency the app actually sustains and watch pool-wait time: rising wait means too small, idle connections holding DB memory means too large. Put a pooler (PgBouncer and equivalents) in front when the process count makes the arithmetic impossible.
- **Eager-load related entities you'll serialize** — `selectinload(User.posts)` for `UserResponse` that includes posts. Lazy loading inside `async def` triggers sync I/O on the event loop.
- **Transactions at the request boundary, not deeper.** A service method participates in the caller's transaction; it doesn't open its own.
- **`sqlmodel`/`SQLAlchemy 2.0` typed queries** — `select(User).where(User.email == email)`. Never f-string SQL with user input.

## Background work: `BackgroundTasks` for trivial, real queue for non-trivial

- `BackgroundTasks` runs **after the response is sent, in the same process**. Fine for "send a fire-and-forget log line" or "invalidate a cache". Not fine for "send an email" or "process a payment" — those need durability.
- For real background work, push to a queue (`taskiq`, `Celery`, `dramatiq`, AWS SQS) and return the task ID. `arq` is in maintenance mode (critical/security fixes only, no new features) — fine for an existing codebase already on it, not the pick for new work.

## App construction: a factory, a lifespan, and one way to read app state

```python
from contextlib import asynccontextmanager

@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    settings = get_settings()
    app.state.context = await AppContext.create(settings)
    yield
    await app.state.context.aclose()

def create_app(context: AppContext | None = None) -> FastAPI:
    app = FastAPI(lifespan=None if context is not None else lifespan)
    if context is not None:
        app.state.context = context
    app.include_router(users.router, prefix='/api')
    return app
```

- **Build the app in a `create_app()` factory, not as a module-level `app = FastAPI()`.** A module-level app is constructed at import time, which makes two apps in one process (a test and its subject) impossible and ties import order to configuration.
- **Take pre-built state as an optional parameter, and skip the lifespan when it is supplied.** This is the seam the whole test strategy hangs off: routes can then be exercised against fakes with no database, no broker, and no network, while production still goes through the real lifespan. Without it, every route test needs the full startup path.
- **`@app.on_event('startup' | 'shutdown')` is deprecated** — an async `lifespan` context manager is the modern, type-safe replacement, and the only one that guarantees teardown runs.
- **Resources hang off `app.state`, never module-level globals**, and are read back through exactly one accessor (a dependency, or a single typed helper). Module globals make two apps in a process share state; a single accessor is also the one place a cast is needed to recover the type.
- **A mounted sub-app's lifespan does not run, and exception handlers do not propagate into it.** If you mount sub-applications, register handlers on each and own their startup explicitly.

## OpenAPI hygiene

- Every route has `summary`, `description` (or a docstring), `response_model`, and explicit `status_code` for non-200 successes (`status.HTTP_201_CREATED` for POSTs that create).
- `responses={404: {'model': ErrorResponse}, 422: {'model': ValidationErrorResponse}}` so the generated spec covers the real error envelope, not just the happy path.
- Tag routers (`APIRouter(prefix='/users', tags=['users'])`) so the docs group sensibly.

## What NOT to flag

- **`def` (sync) route handlers** for CPU-light, I/O-free work — FastAPI runs them in a threadpool; this is a legitimate choice, not a missing `async`.
- **`BackgroundTasks`** for genuinely trivial fire-and-forget work (a log line, a cache bust) — it doesn't need a real queue; only flag it when the task is non-trivial enough to need durability (payments, emails).
- **`arq` or `aiomysql` already in use in an already-shipped codebase** the current ticket isn't touching — note it as a follow-up gap, not a blocking finding, unless the ticket is already changing that code.
- **`@app.on_event`** in an existing codebase not being touched — it's deprecated, not removed; migrating it is a separate ticket unless the current change already touches startup/shutdown wiring.

## Footguns

- **Defining `Depends(...)` as a default in `def` parameters with mutable values** — `def f(x: list = Depends(make_list))` re-evaluates per request, but bare `def f(x: list = [])` is the classic mutable-default trap.
- **Returning ORM objects directly from a route** — works in dev, breaks in prod when a relation lazy-loads or serialization picks up internal fields.
- **`response_model_exclude_unset=True` everywhere** — silently drops fields the client expected. Use only when you genuinely want sparse responses (PATCH semantics).
- **`SQLAlchemy` autoflush + async session** — autoflush mid-handler can hit the DB inside an `await`, masking the actual cause of a slow request. Configure flush behavior explicitly.
- **`uvicorn --reload` in production** — file-watcher overhead and unsafe reload semantics. Use `gunicorn -k uvicorn.workers.UvicornWorker` or `uvicorn` with `--workers N` and no reload.
- **Catching `Exception` in middleware and returning 500 silently** — masks every error from monitoring. Log structured + re-raise or return a structured response with a `request_id` correlation key.
- **`async def` route that does `time.sleep(5)`** — blocks the entire worker. Always `await asyncio.sleep`.
- **`request.json()` without `await`** — returns a coroutine, not the body. Linters catch most of these; the type checker catches the rest if you're strict.
- **Constructing an HTTP client per request** — `async with httpx.AsyncClient() as c:` inside a handler throws away the connection pool and pays a fresh TCP and TLS handshake every call. Build one client for the app's lifetime, hold it on `app.state`, and inject it; the injection point doubles as the test seam. The same line with no `timeout=` is the sharper half of the bug: httpx applies a default, but an unbounded wait on an auth or upstream path is how one slow dependency becomes an outage.
- **Testing through an ASGI transport does not run the lifespan.** `httpx.ASGITransport` drives the app directly, so anything the lifespan was meant to create is simply absent — usually surfacing as an `AttributeError` on `app.state`, or worse, as a test that passes because the code path silently no-ops. Pass pre-built state into the app factory, or drive the lifespan explicitly.
