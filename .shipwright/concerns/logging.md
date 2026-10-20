---
description: "Log emission mechanics: how an individual log statement is written — structured key-value fields over interpolated sentences, level selection, PII and secret exclusion, correlation-ID propagation"
triggers:
  file_globs:
    - "**/logger.ts"
    - "**/logger.js"
    - "**/logger.py"
    - "**/logging.py"
    - "**/log_config.py"
    - "**/logging_config.py"
    - "**/logging.conf"
    - "**/logging.yaml"
    - "**/logging.yml"
---

# Concern: Log Emission

This concern covers **how a log statement is written** once the decision to emit one has
been made: what shape it takes, which level it carries, what must never appear in it,
and how it correlates with the rest of a request's lines. *Whether* an operation is
observable at all — whether a failure is legible, whether a hang is visible — is a
separate question and belongs to the operability concern, not here.

It applies to any code that emits logs. Code that spawns external processes, consumes
queues, or performs network I/O is where its absence hurts most: those paths fail in
production and are diagnosed exclusively from their log output.

Unstructured logging and PII or secret leakage are blocking findings.

**Scope note.** The core rules below (log data not sentences, level discipline, what
never to log, correlation IDs) are language-independent. Sections headed with a
language name cite concrete libraries and apply only to projects already on that stack
— follow whatever structured logger the project has already adopted rather than
introducing one from here.

## References

Ecosystem-specific; consult only the row matching the project's stack.

- Python — stdlib `logging`: https://docs.python.org/3/library/logging.html
- Python — structlog: https://www.structlog.org/en/stable/
- Python — OpenTelemetry: https://opentelemetry-python.readthedocs.io/
- Python — Pydantic Logfire: https://docs.pydantic.dev/logfire/
- Node.js — pino: https://getpino.io/
- JavaScript — OpenTelemetry: https://opentelemetry.io/docs/languages/js/

## Core Principle — Log Data, Not Sentences

**Structured logging only.** Every log statement must emit machine-readable key-value
fields (JSON), not a human-readable interpolated sentence.

The rule is simple: **log the data, not a sentence about the data.**

```python
# Wrong — interpolated string; impossible to query or alert on
logger.info(f"User {user.id} created order {order.id} for ${amount:.2f}")

# Correct — structured fields; queryable, alertable, parseable
logger.info("order_created", user_id=user.id, order_id=order.id, amount_usd=float(amount))
```

```typescript
// Wrong
logger.info(`User ${userId} logged in from ${ip}`);

// Correct
logger.info({ userId, action: 'login' }, 'User logged in');
```

## Log Levels — Strict Discipline

Use the level that matches the actual severity. Wrong level usage degrades signal-to-noise
across the entire system.

| Level | When to use |
|---|---|
| **DEBUG** | Developer diagnostics — variable values, branch decisions, loop iterations. Never enabled in production by default. |
| **INFO** | Significant lifecycle events — service startup, request received, job started, job completed. |
| **WARNING** | Unexpected but recovered — parse failure with fallback, retry that succeeded, config default applied. |
| **ERROR** | Requires human attention — unhandled exception, external service unreachable, operation failed irrecoverably. |
| **CRITICAL / FATAL** | System cannot continue — data corruption detected, required dependency unavailable at boot. |

**Never use ERROR or WARNING for expected conditions.** A user not being found in the
database is a normal application outcome — log it at INFO or DEBUG, not WARNING or ERROR.

```python
# Wrong — "user not found" is an expected, handled condition
logger.warning("user_not_found", user_id=user_id)

# Correct
logger.info("user_lookup_miss", user_id=user_id)
```

## Python Structured Logging

*Applies only to Python projects.*

First, find the project's existing logger and follow it — a project with a configured
structured logger has a house style for event names, bound context, and formatter
configuration, and a feature ticket is not the place to introduce a second one. If the
project has no structured logging at all, that is a gap worth flagging; `structlog` and
the stdlib `logging` module with a JSON formatter are the two established options in
this ecosystem. Never use `print()` for logging.

The examples below use `structlog`; the rules they illustrate (keyword context,
`logger.exception()` in `except` blocks, request-scoped bound context) hold for any
structured logger.

### Setup with structlog

```python
import structlog

logger = structlog.get_logger(__name__)
```

### Pass context as keyword arguments — never f-strings

```python
# Wrong — unstructured; query-hostile
logger.info(f"Processing job {job_id} for tenant {tenant_id}")

# Correct — structured fields
logger.info("job_processing_started", job_id=job_id, tenant_id=tenant_id)
```

### Use `logger.exception()` inside except blocks

`logger.exception()` automatically captures the current exception and its stack
trace. Using `logger.error()` inside an `except` block is wrong — it discards the
traceback.

```python
# Wrong — stack trace silently discarded
try:
    result = await call_external_service(payload)
except ExternalServiceError as exc:
    logger.error("external_service_failed", error=str(exc))

# Correct — stack trace captured automatically
try:
    result = await call_external_service(payload)
except ExternalServiceError:
    logger.exception("external_service_failed", service="payments")
    raise
```

### Bind request-scoped context at the entry point

Use `structlog.contextvars.bind_contextvars` to attach shared context (request ID,
user ID, tenant ID) once per request so every subsequent log statement in that scope
carries it automatically.

```python
import structlog.contextvars

async def handle_request(request: Request) -> Response:
    structlog.contextvars.clear_contextvars()
    structlog.contextvars.bind_contextvars(
        # Only accept an inbound ID from a trusted upstream, and validate its shape —
        # the header is attacker-controlled and goes into every log line below.
        request_id=trusted_request_id(request) or generate_request_id(),
        user_id=request.state.user_id,
        path=request.url.path,
    )
    # Every logger.info/warning/error call below now includes request_id and user_id
    return await process(request)
```

### Stdlib `logging` with JSON formatter (alternative)

```python
import logging
import json

class JsonFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        payload = {
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
            "timestamp": self.formatTime(record),
        }
        if record.exc_info:
            payload["exc_info"] = self.formatException(record.exc_info)
        return json.dumps(payload)
```

## TypeScript Structured Logging

*Applies only to TypeScript/JavaScript projects.*

Follow the project's existing logger. If there is none, `pino` is the established
Node.js default (JSON output, child loggers, low overhead); other structured loggers
are fine if the project has already chosen one. Never use `console.log`,
`console.warn`, or `console.error` in production code.

The examples below use `pino`; the rules they illustrate (context object then message,
child loggers for scoped context) hold for any structured logger.

### Setup

```typescript
import pino from 'pino';

// Log level is the one configuration value a logger may read before the settings
// object exists, because the logger is constructed to report settings failures.
// Everything else the logger needs comes from the settings object.
const logger = pino({ level: process.env.LOG_LEVEL ?? 'info' });
export default logger;
```

### Log with a context object — not a template literal

```typescript
// Wrong — unstructured
logger.info(`Processing order ${orderId} for user ${userId}`);

// Correct — structured context object, then message
logger.info({ orderId, userId, amountCents }, 'order_processing_started');
```

### Child loggers for component-scoped context

```typescript
// Create once per component/service
const log = logger.child({ component: 'AuthService' });

// Every call on `log` includes { component: 'AuthService' } automatically
log.info({ userId }, 'session_created');
log.warn({ userId, reason: 'token_expired' }, 'auth_rejected');
```

## Correlation and Trace IDs

Every log statement within a request's lifecycle must carry a shared correlation ID so
all related log lines can be retrieved as a single trace from any log aggregator.

**Rules:**
- Generate a unique request/trace ID at the entry point: HTTP handler, queue consumer, job runner, cron worker.
- Thread it through every log statement and every downstream call (pass in headers as `x-request-id`, `traceparent`, etc.) within that request's scope.
- Never rely on log timestamps alone to correlate related events.

### Python — structlog context vars

```python
import uuid
import structlog.contextvars

def generate_request_id() -> str:
    return str(uuid.uuid4())

# In middleware or handler entry point:
structlog.contextvars.bind_contextvars(request_id=generate_request_id())

# Propagate to downstream HTTP calls:
headers["x-request-id"] = structlog.contextvars.get_contextvars().get("request_id")
```

### TypeScript — AsyncLocalStorage

```typescript
import { AsyncLocalStorage } from 'async_hooks';

interface RequestContext { requestId: string; }
const requestContext = new AsyncLocalStorage<RequestContext>();

// In HTTP middleware. An inbound header is attacker-controlled and lands unbounded in
// every log line for the request, so validate its shape or discard it. Accept an
// inbound ID only from a trusted upstream (your own gateway or proxy); on a
// public-facing edge, generate it and ignore what the client sent.
const REQUEST_ID = /^[\w-]{1,64}$/;

app.use((req, res, next) => {
  const header = req.headers['x-request-id']; // string | string[] | undefined
  const inbound = Array.isArray(header) ? header[0] : header;
  const requestId = inbound !== undefined && REQUEST_ID.test(inbound)
    ? inbound
    : crypto.randomUUID();
  requestContext.run({ requestId }, next);
});

// In any service — create a child logger with the stored context:
function getLogger(base: pino.Logger): pino.Logger {
  const ctx = requestContext.getStore();
  return ctx ? base.child({ requestId: ctx.requestId }) : base;
}
```

### TypeScript — logger child (simpler pattern)

```typescript
// Pass a child logger down through function calls
async function handleRequest(req: Request): Promise<Response> {
  const log = logger.child({ requestId: crypto.randomUUID(), path: req.url });
  return processRequest(req, log);
}
```

## What NOT to Log

**Credentials never appear in a log at any level.** The rest is a judgement about
purpose and jurisdiction, and the project's own policy is the authority.

**PII** — email addresses, full names, phone numbers, postal addresses. Log the opaque
identifier that lets an operator look the record up through an access-controlled path,
not the value itself.

**IP addresses** are the case where a blanket ban does real harm: they are personal data
under GDPR and equivalent regimes, *and* they are the primary evidence for abuse
investigation, rate-limit forensics, and incident response. So decide rather than
default — log them where there is a stated security or operational purpose, keep them
out of general application logs, and give the logs that carry them a retention period
short enough to match the purpose. Check what the project has already committed to in
its privacy notice or data-retention policy before adding a new field.
```python
# Wrong
logger.info("user_registered", email=user.email, name=user.full_name)

# Correct — log only the opaque identifier
logger.info("user_registered", user_id=user.id)
```

**Credentials, tokens, API keys, passwords** — ever, at any level, in any environment:
```python
# Wrong — catastrophic if logs are ever exposed
logger.debug("auth_header", authorization=request.headers["authorization"])

# Correct — log presence and type, never value
logger.debug("auth_header_present", scheme="Bearer")
```

**Full request/response bodies** at INFO or above — log only IDs, status codes, and
durations. Full bodies may be logged at DEBUG in development environments only, after
scrubbing sensitive fields.

**Stack traces at INFO level** — stack traces belong exclusively in ERROR (or
`logger.exception()`) calls. An INFO log with a stack trace is a misconfigured log level.

## The Start / Outcome / Duration Triple

The standard emission shape for any operation that can fail or take time. Every
significant operation emits three signals:

1. **A start event** — what operation is beginning, with key input identifiers (not full payloads)
2. **A completion or failure event** — outcome (success/failure), and why if failure
3. **Duration timing** — elapsed time for any external call (database query, HTTP call, queue publish)

```python
import time

async def fetch_user_orders(user_id: int) -> list[Order]:
    logger.info("fetch_user_orders_started", user_id=user_id)
    t0 = time.monotonic()
    try:
        orders = await db.query("SELECT * FROM orders WHERE user_id = $1", user_id)
        logger.info(
            "fetch_user_orders_completed",
            user_id=user_id,
            order_count=len(orders),
            duration_ms=round((time.monotonic() - t0) * 1000, 2),
        )
        return orders
    except Exception:
        logger.exception(
            "fetch_user_orders_failed",
            user_id=user_id,
            duration_ms=round((time.monotonic() - t0) * 1000, 2),
        )
        raise
```

```typescript
async function fetchUserOrders(userId: number, log: pino.Logger): Promise<Order[]> {
  log.info({ userId }, 'fetch_user_orders_started');
  const t0 = performance.now();
  try {
    const orders = await db.query('SELECT * FROM orders WHERE user_id = $1', [userId]);
    log.info(
      { userId, orderCount: orders.length, durationMs: Math.round(performance.now() - t0) },
      'fetch_user_orders_completed',
    );
    return orders;
  } catch (err) {
    log.error(
      { userId, durationMs: Math.round(performance.now() - t0), err },
      'fetch_user_orders_failed',
    );
    throw err;
  }
}
```

## Anti-Patterns — Never Do These

Language-independent:

| Anti-pattern | Correct alternative |
|---|---|
| Writing to stdout directly instead of through a logger | Use whatever structured logger the project has adopted |
| String interpolation in the log message | Pass context as structured fields; the message is a stable event name, not a sentence |
| Logging full request or response bodies at INFO | Log only IDs, status codes, and durations; full bodies at DEBUG only (dev environments, scrubbed) |
| Logging PII or secrets (tokens, passwords, API keys) | Log only opaque identifiers; see the section above for the PII judgement |
| Error or warning level for expected, handled conditions (e.g. user not found) | Info or debug for expected outcomes |
| Catching an exception and logging only its message | Log the exception with its stack trace attached, via whatever the language's logger offers for that |
| No correlation or request ID in log statements | Generate a request ID at the entry point and thread it through all logs in that scope |
| Unstructured string log messages with no queryable fields | Structured key-value fields |

Stack-specific, where the project is on that stack:

| Stack | Anti-pattern | Correct alternative |
|---|---|---|
| Python | `print()` for logging | `structlog` or stdlib `logging` with a JSON formatter |
| Python | `logger.error()` inside an `except` block without `exc_info` | `logger.exception()` — captures the stack trace automatically |
| Node.js | `console.log` / `console.warn` / `console.error` in production code | `pino` or the project's existing structured logger |

## Reviewer Checklist

- [ ] Nothing writes to stdout/stderr directly instead of through the project's logger (`print()` in Python, `console.*` in Node.js)
- [ ] All log statements use structured fields — no interpolated strings or template literals in the message
- [ ] Log levels match actual severity — no ERROR or WARNING for expected, handled conditions
- [ ] No credentials, tokens, or secrets in any log statement at any level; PII limited to opaque identifiers, with any IP-address logging tied to a stated purpose and a retention period
- [ ] Every logged exception carries its stack trace (in Python, `logger.exception()` rather than `logger.error()` inside `except`)
- [ ] A correlation/request ID is generated at every entry point (HTTP handler, queue consumer, job runner) and threaded through all log statements in that scope
- [ ] Duration is logged for all external calls (database queries, HTTP calls, queue operations)
- [ ] Every new background job, worker, or queue consumer has a start event log and a completion/failure event log
- [ ] Every code path that spawns an external process logs the invocation, the exit status, and the captured error output — a spawned process that fails is diagnosable only from what its caller logged
- [ ] The module has any logging at all — a new package with zero log statements on its failure paths is a gap, whether or not the ticket was about logging
