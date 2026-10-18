---
description: "HTTP API design (REST-shaped endpoints): method semantics, status codes, error contracts (RFC 9457), versioning, pagination — scoped to HTTP endpoint design, not GraphQL schemas or RPC/streaming contracts"
triggers:
  file_globs:
    - "**/openapi.yaml"
    - "**/openapi.yml"
    - "**/openapi.json"
    - "**/swagger.yaml"
    - "**/swagger.yml"
    - "**/swagger.json"
    - "**/*.openapi.yaml"
    - "**/*.openapi.yml"
    - "**/*.openapi.json"
    - "**/controllers/**/*"
    - "**/routers/**/*"
    - "**/endpoints/**/*"
    - "**/api/**/route.ts"
    - "**/api/**/route.js"
---

# Concern: REST API Design

Injected when tickets involve designing or implementing HTTP API endpoints,
response shapes, error handling, versioning, or pagination.

**Scope: REST over HTTP.** Everything below assumes resources addressed by URL, HTTP
methods carrying the verb, and status codes carrying the outcome. GraphQL and RPC-style
interfaces (gRPC, tRPC, JSON-RPC) answer these questions differently — a single endpoint,
errors in the payload, schema evolution instead of URI versioning, cursor conventions
defined by the transport. On those, take from this file only the parts that are about
API design rather than about HTTP: stable machine-readable error identifiers, no
unbounded list results, no internal detail in error payloads, additive-only changes
within a published contract, idempotency for operations that must not double-apply.

## References

- RFC 9110 (HTTP Semantics): https://www.rfc-editor.org/rfc/rfc9110
- RFC 9457 (Problem Details for HTTP APIs): https://www.rfc-editor.org/rfc/rfc9457
- RFC 6570 (URI Templates): https://www.rfc-editor.org/rfc/rfc6570
- OpenAPI 3.1 Specification: https://spec.openapis.org/oas/v3.1.0
- OWASP REST Security Cheat Sheet: https://cheatsheetseries.owasp.org/cheatsheets/REST_Security_Cheat_Sheet.html

## HTTP Method Semantics

| Method | Semantics | Idempotent | Safe | Body |
|---|---|---|---|---|
| GET | Retrieve | Yes | Yes | No |
| POST | Create / trigger | No | No | Yes |
| PUT | Full replace | Yes | No | Yes |
| PATCH | Partial update | Conditional | No | Yes |
| DELETE | Remove | Yes | No | Optional |

Safe methods (GET) must not produce side effects. Idempotent methods (GET, PUT, DELETE)
produce the same result when called multiple times with the same input.

## Resource-Oriented URL Design

URLs are **nouns** (resources), not verbs (actions):

```
# Correct — resources are nouns, hierarchy is logical
GET    /v1/users                        list users
POST   /v1/users                        create user
GET    /v1/users/{id}                   get user by ID
PUT    /v1/users/{id}                   full replace
PATCH  /v1/users/{id}                   partial update
DELETE /v1/users/{id}                   delete user

GET    /v1/users/{id}/orders            user's orders (ownership relationship)
POST   /v1/users/{id}/orders            create order for user

# Wrong — verbs in URL
GET  /getUser?id=123
POST /createOrder
POST /deleteItem/{id}
```

Use sub-resources for **ownership** relationships (`/users/{id}/orders`).
Use flat resources with query params for **filtering** (`/orders?userId={id}`).
Limit nesting to 2–3 levels maximum.

Resource names are **plural** (`/users`, `/orders`, `/products`).

Use sub-resource action verbs sparingly when an action doesn't map cleanly to CRUD:
```
POST /v1/orders/{id}/cancel
POST /v1/payments/{id}/refund
POST /v1/accounts/{id}/verify
```

## Status Codes

Use the correct status code — clients and proxies depend on them:

| Code | Meaning | Use when |
|---|---|---|
| 200 OK | Success | GET, PUT, PATCH succeeded |
| 201 Created | Resource created | POST created a resource; include `Location` header |
| 202 Accepted | Async work queued | Long-running operation started |
| 204 No Content | Success, no body | DELETE, PUT/PATCH with no response body |
| 400 Bad Request | Malformed request | Invalid JSON, missing required field, wrong type |
| 401 Unauthorized | Not authenticated | Missing or invalid auth token |
| 403 Forbidden | Not authorized | Authenticated but lacks permission |
| 404 Not Found | Resource missing | ID does not exist; also use to hide existence from unauthorised callers |
| 409 Conflict | State conflict | Duplicate resource, version mismatch, stale update |
| 410 Gone | Permanently removed | Deleted resource that previously existed |
| 422 Unprocessable Entity | Semantic validation failure | Syntactically valid but fails business rules |
| 429 Too Many Requests | Rate limited | Include `Retry-After` header |
| 500 Internal Server Error | Unexpected server failure | Never expose internal details |

**Never return 200 with an error in the body** — this breaks HTTP clients, proxies,
and monitoring tools that rely on status codes.

## Error Response Shape (RFC 9457)

All error responses must use a consistent, stable structure.
Adopt RFC 9457 Problem Details or an equivalent schema:

```json
{
  "type": "https://api.example.com/errors/validation-failed",
  "title": "Validation Failed",
  "status": 422,
  "detail": "The request body contains invalid fields.",
  "instance": "/requests/abc-123",
  "errors": [
    { "field": "email", "message": "Must be a valid email address" },
    { "field": "age",   "message": "Must be between 13 and 120" }
  ]
}
```

`Content-Type: application/problem+json`

Rules:
- `type`: stable URI identifying the error class (machine-readable)
- `title`: human-readable, static per error type (not per-instance)
- `status`: HTTP status code (repeated for client convenience)
- `detail`: instance-specific description
- `errors`: field-level validation details when applicable
- **Never include stack traces, SQL errors, or internal class names** in responses

Most frameworks emit their own default error shape — override it with a global exception
handler so every error type shares one envelope.

## Versioning

Version from day one. URI path versioning is the most explicit and cacheable:

```
/v1/users
/v2/users
```

Rules:
- Increment the major version for **breaking changes** (removing fields, changing types,
  changing required/optional status, changing URL structure)
- Non-breaking **additions** (new optional fields, new endpoints) are allowed within a version
- Maintain backwards compatibility within a major version
- Communicate deprecation via `Deprecation` and `Sunset` response headers before removal
- Never version via query string (`?version=2`) — hard to route, cache, and document

## Pagination

**Every list endpoint must be paginated.** Never return unbounded collections.

For large or live datasets use **keyset (cursor) pagination** — not OFFSET:

```json
GET /v1/posts?limit=20&after=eyJpZCI6IDEwMH0

{
  "data": [...],
  "pagination": {
    "nextCursor": "eyJpZCI6IDEyMH0",
    "hasMore": true
  }
}
```

Offset pagination (acceptable for small, stable datasets):
```json
GET /v1/posts?page=2&perPage=20

{
  "data": [...],
  "pagination": {
    "page": 2,
    "perPage": 20,
    "total": 843,
    "totalPages": 43
  }
}
```

Always enforce a maximum `limit`/`perPage` cap server-side.

## Filtering, Sorting, and Sparse Fieldsets

```
GET /v1/orders?status=active&userId=42&sort=createdAt&order=desc&fields=id,status,total
```

Document all supported filter and sort parameters in OpenAPI. Reject unknown params
with a 400 or silently ignore them — be consistent.

## Naming Conventions

Consistency is the whole rule; which convention you pick is not. Mixing conventions is a
breaking change in practice, so match what the API already publishes, and on a new API
match what its server language and its primary clients read naturally — camelCase is the
common choice in JavaScript-facing APIs, snake_case in Python- and Go-facing ones. Either
is fine; half of each is not.

- **JSON fields**: one convention across every endpoint, including nested objects
- **URL path segments**: kebab-case (`/user-profiles`) or snake_case — pick one
- **Query params**: same convention as the JSON fields they filter on

Never mix `userId` and `user_id` within the same API.

## Idempotency Keys

For non-idempotent POST operations that must not be retried (payments, emails, orders):
support `Idempotency-Key` header. Store the key + response; return the cached response
for duplicate requests within a window.

```
POST /v1/payments
Idempotency-Key: 550e8400-e29b-41d4-a716-446655440000
```

## Caching

```
Cache-Control: no-store                    # sensitive / user-specific data
Cache-Control: public, max-age=300         # public, cacheable resources
ETag: "abc123"                             # conditional requests
Last-Modified: Mon, 01 Jan 2024 00:00:00 GMT
Vary: Accept-Encoding, Authorization       # vary cache by relevant headers
```

## Rate Limiting Headers

```
X-RateLimit-Limit:     100
X-RateLimit-Remaining: 42
X-RateLimit-Reset:     1704067200
Retry-After:           60
```

Return 429 when limit exceeded, always with `Retry-After`.

## OpenAPI Documentation

Every endpoint must be documented in the OpenAPI spec:
- Request body schema with field descriptions
- Response schemas for ALL status codes (200, 400, 401, 404, 422, etc.)
- Examples for request and response
- Auth scheme on protected endpoints
- Pagination params documented

If the framework generates OpenAPI automatically, verify the generated docs are
accurate and complete after any endpoint change.

## Anti-Patterns — Never Do These

| Anti-pattern | Correct alternative |
|---|---|
| Verb in URL (`/getUser`, `/deleteItem`) | Noun resource + HTTP method |
| 200 response for errors | Correct HTTP status code |
| Inconsistent error response shapes | Single standard schema (RFC 9457) |
| No pagination on list endpoints | Always paginate with max cap |
| OFFSET pagination on large live datasets | Keyset/cursor pagination |
| Mixed naming conventions (`userId` and `user_id`) | One convention, applied everywhere |
| Breaking changes within a version | New major version |
| No versioning strategy | URI versioning from day one |
| Exposing sequential integer IDs publicly | UUIDs or opaque slugs |
| Stack traces / SQL errors in 500 response | Generic message; log details server-side |
| No rate limiting on auth/public endpoints | Rate limit with 429 + Retry-After |
| Query-string versioning (`?version=2`) | URI path versioning (`/v2/`) |
| Missing `Location` header on 201 | Include `Location: /v1/resources/{id}` |

## Reviewer Checklist

- [ ] URLs use nouns, not verbs
- [ ] Correct HTTP method for each operation (GET safe, PUT idempotent, etc.)
- [ ] Correct status codes (200 vs 201 vs 204; 400 vs 422 vs 404; never 200 for errors)
- [ ] All error responses use the project's standard shape
- [ ] No stack traces or internal details in error responses
- [ ] API is versioned (`/v1/`) — no breaking changes within a version
- [ ] All list endpoints paginated with enforced max limit
- [ ] Field naming is consistent with the rest of the API
- [ ] New endpoints documented in OpenAPI with all response status schemas
- [ ] Auth endpoints rate-limited
- [ ] Non-idempotent POST operations support idempotency keys (where applicable)
- [ ] Sequential integer IDs not exposed as public resource identifiers
