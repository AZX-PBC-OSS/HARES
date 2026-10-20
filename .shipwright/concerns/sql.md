---
description: SQL/PostgreSQL — parameterized queries, schema and migration discipline, indexing, transactions, EXPLAIN-driven optimization
triggers:
  file_globs:
    - "**/*.sql"
    - "**/*.ddl"
    - "**/*.psql"
    - "**/migrations/**/*.py"
    - "**/alembic/versions/**/*.py"
    - "**/alembic.ini"
---

# Concern: SQL / PostgreSQL

Injected when tickets involve database queries, schema changes, migrations, or
ORM usage against a SQL database. Focused on PostgreSQL; principles apply broadly.

Target current PostgreSQL (18, released 2025-09-25). Where a practice depends on
a specific release, this file names that release rather than pinning the whole
document to it — check `SELECT version()` or the project's Docker image tag
against the release that introduced the feature before relying on it.

## References

- PostgreSQL 18 docs: https://www.postgresql.org/docs/18/
- PostgreSQL 18 release notes: https://www.postgresql.org/docs/release/18.0/
- Indexes: https://www.postgresql.org/docs/current/indexes.html
- Transaction isolation: https://www.postgresql.org/docs/current/transaction-iso.html
- EXPLAIN / query planning: https://www.postgresql.org/docs/current/using-explain.html
- Performance tips: https://www.postgresql.org/docs/current/performance-tips.html
- pg_stat_statements: https://www.postgresql.org/docs/current/pgstatstatements.html
- explain.depesz.com (EXPLAIN analyser): https://explain.depesz.com/
- pgMustard (EXPLAIN visualiser): https://www.pgmustard.com/

## Parameterized Queries — Non-Negotiable

**Never interpolate values into SQL strings.** Always use parameterized queries.
This prevents SQL injection and improves plan cache reuse.

```python
# Wrong — SQL injection risk
query = f"SELECT * FROM users WHERE email = '{email}'"

# Correct — asyncpg
await conn.fetch("SELECT * FROM users WHERE email = $1", email)

# Correct — psycopg3
await conn.execute("SELECT * FROM users WHERE id = %s", (user_id,))

# Correct — SQLAlchemy
result = await session.execute(select(User).where(User.email == email))
```

## Schema Design

- **Normalization**: 3NF by default. Denormalize only with measured justification and a comment explaining the trade-off.
- **Primary keys**: Always explicit.
  - Default choice: **`uuid` generated with `uuidv7()`** (native as of PostgreSQL 18, RFC 9562). A random key (`gen_random_uuid()` / UUIDv4) scatters inserts across the whole b-tree, causing page splits, write amplification, and poor buffer-cache locality on the hot end of the index. UUIDv7 encodes a millisecond timestamp in its high bits, so it sorts close to insertion order — new rows land at the right-hand edge of the index like a serial key does, and the ID doubles as a keyset-pagination cursor and a rough creation-order sort with no extra column. On PostgreSQL 18+, generate it with `DEFAULT uuidv7()` directly in the column definition; there is no more `pgcrypto`/application-side dependency for this. **Pre-18**, there is no built-in function — use an extension (`pg_uuidv7`) or generate it in application code (`uuid` npm package's `v7()`, Python's `uuid6` package); do not hand-roll the bit-layout.
  - Trade-offs, not a free win: a UUIDv7 value embeds its creation timestamp, so it leaks *when* the row was created to anyone who can see the ID — avoid it for identifiers exposed where that timing is sensitive (e.g. a per-user invite code). It is also 16 bytes against 8 for `bigint`, costing more per index entry at scale. Where enumeration risk and merge-friendliness (distributed/offline ID generation) don't apply, a `bigint`/`BIGSERIAL` identity column remains smaller, faster to compare, and simpler to reason about. Migrating an existing table's primary-key type is a multi-step, lock-sensitive operation (new column, dual-write, backfill, switch, drop) — not a change to make opportunistically alongside unrelated work.
  - `BIGSERIAL` (or `bigint generated always as identity`) is still the right call for internal, non-distributed, non-public-facing tables where none of the above trade-offs bite.
- **Types**: Use the most specific correct type.
  - `TIMESTAMPTZ` not `TIMESTAMP` — always store with timezone.
  - `JSONB` not `JSON` — `JSONB` is indexed and queryable.
  - `TEXT` not `VARCHAR(n)` unless the length is a real constraint.
  - `NUMERIC` (or integer cents) for money — never `FLOAT` or `DOUBLE PRECISION`.
  - `BOOLEAN` not `SMALLINT` or `CHAR(1)`.
- **Constraints**: `NOT NULL` everywhere meaningful. `CHECK` for invariants. `UNIQUE` for natural keys. Foreign keys with appropriate `ON DELETE` behaviour.
- **Naming**: snake_case for tables and columns. Match whatever singular/plural convention the existing schema already uses and never mix the two; on a greenfield schema prefer plural (`users`, `orders`) — the obvious singular forms of the commonest tables (`user`, `order`, `group`) are reserved words in PostgreSQL and other engines and force quoting at every reference. Index names: `idx_{table}_{columns}`.

## Indexing

Create indexes deliberately. Every `WHERE`, `ORDER BY`, `JOIN ON`, and foreign key
column is a candidate.

| Index type | Use when |
|---|---|
| B-tree (default) | Equality, range queries, sorting |
| GIN | Arrays, JSONB, full-text search (`tsvector`) |
| GiST | Geometric data, range types |
| BRIN | Large append-only tables with natural ordering (timestamps) |
| Partial | Only a subset of rows is queried (`WHERE deleted_at IS NULL`) |
| Expression | Queries filter on a computed value (`lower(email)`) |
| Covering (`INCLUDE`) | Avoid heap fetch for index-only scans |

```sql
-- Partial index for soft-delete pattern
CREATE INDEX idx_users_active_email ON users (email)
  WHERE deleted_at IS NULL;

-- Expression index for case-insensitive lookup
CREATE INDEX idx_users_lower_email ON users (lower(email));

-- Covering index — avoids table lookup for the listed columns
CREATE INDEX idx_orders_user_status ON orders (user_id)
  INCLUDE (status, created_at);

-- Always build large indexes concurrently in production
CREATE INDEX CONCURRENTLY idx_events_created ON events (created_at);
```

**PostgreSQL does NOT auto-index foreign key columns.** Always add an index on FK columns.

## N+1 Query Prevention

Never query inside a loop. Use JOINs, CTEs, or `WHERE id = ANY($1)` with an array:

```sql
-- Wrong — N+1
SELECT * FROM posts;
-- then for each post: SELECT * FROM users WHERE id = ?

-- Correct — single query with JOIN
SELECT p.*, u.name AS author_name
FROM posts p
JOIN users u ON u.id = p.author_id
WHERE p.published = true;

-- Correct — batch fetch with array
SELECT * FROM users WHERE id = ANY($1::int[]);

-- Correct — CTE for complex aggregation
WITH post_counts AS (
  SELECT author_id, count(*) AS total
  FROM posts
  GROUP BY author_id
)
SELECT u.id, u.name, coalesce(pc.total, 0) AS post_count
FROM users u
LEFT JOIN post_counts pc ON pc.author_id = u.id;
```

## Transactions

Use explicit transactions for all multi-step writes. Keep transactions short.

```sql
BEGIN;
  UPDATE accounts SET balance = balance - $1 WHERE id = $2;
  UPDATE accounts SET balance = balance + $1 WHERE id = $3;
  INSERT INTO ledger (from_id, to_id, amount) VALUES ($2, $3, $1);
COMMIT;
```

Isolation levels:

| Level | Use when |
|---|---|
| `READ COMMITTED` (default) | Simple CRUD on predetermined rows |
| `REPEATABLE READ` | Multi-query consistency required |
| `SERIALIZABLE` | Financial, inventory, complex integrity |

Handle serialization failures (`ERROR 40001`) with retry logic at `REPEATABLE READ`
and `SERIALIZABLE` levels.

Use `SELECT ... FOR UPDATE` for pessimistic locking on read-modify-write patterns.
Use `SKIP LOCKED` for queue/job processing to avoid contention.

## Query Analysis

Run `EXPLAIN (ANALYZE, BUFFERS)` before and after any significant query change:

```sql
EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
SELECT * FROM orders WHERE user_id = 123 ORDER BY created_at DESC LIMIT 10;
```

Warning signs in EXPLAIN output:
- `Seq Scan` on a large table — likely needs an index
- High `Rows Removed by Filter` — index is not selective enough
- `Sort` with `Disk: ...` — increase `work_mem` or add a covering index
- High `shared read` blocks — buffer cache miss; consider `VACUUM ANALYZE`

**Useful extensions for catching indexing and hot-path defects, not just describing them:**
- `pg_stat_statements` (bundled, `CREATE EXTENSION`) — tracks per-query call count, total/mean time, and rows across the actual production or staging workload. Finds the queries actually worth an `EXPLAIN`, rather than guessing from code review.
- `hypopg` — creates a *hypothetical* index (no build, no write cost, no lock) and lets `EXPLAIN` show whether the planner would use it, before paying for `CREATE INDEX CONCURRENTLY` on a large table. Pair with `pg_stat_statements`: find the expensive query, test the index hypothetically, then build it for real once confirmed.

## CTEs

Use CTEs for readability and to avoid N+1 patterns. Control materialization explicitly:

```sql
-- Force materialization (computed once, referenced multiple times)
WITH w AS MATERIALIZED (
  SELECT * FROM expensive_view
)
SELECT * FROM w WHERE status = 'active'
UNION ALL
SELECT * FROM w WHERE status = 'pending';

-- Allow planner to fold CTE into parent (enables predicate push-down)
WITH w AS NOT MATERIALIZED (
  SELECT * FROM users
)
SELECT * FROM w WHERE id = $1;
```

Recursive CTEs for hierarchical data (org charts, comment threads, BOM):
```sql
WITH RECURSIVE org AS (
  SELECT id, name, manager_id, 0 AS depth
  FROM employees WHERE manager_id IS NULL
  UNION ALL
  SELECT e.id, e.name, e.manager_id, o.depth + 1
  FROM employees e JOIN org o ON e.manager_id = o.id
)
SELECT * FROM org ORDER BY depth, name;
```

## Testing Against the Real Engine

A mocked connection or an in-memory stand-in replays the SQL string's *intended*
logic, not what the engine actually does with it — it cannot catch a reserved
word left unquoted, a wrong argument type, a join that silently fans out, or an
`INSERT` column list that has drifted from the schema. Every one of those is a
real, shipped defect that passes under a fake connection. Run tests that touch
SQL against the real engine version the project deploys (a container, e.g.
`testcontainers`, or a CI-provided service) and build the schema under test from
the project's actual migrations, not a hand-maintained parallel `CREATE TABLE`.
This is language-runner-specific guidance — see the project's test-framework
concern (e.g. `pytest.md`'s "Database tests: the real engine, not mocked SQL
semantics") for the fixture-level pattern; this file only states the SQL-level
rule so it holds even where that concern isn't loaded.

## Migrations

- Always write reversible migrations (down migration or rollback plan).
- Two-phase column removal: (1) remove application code references, deploy; (2) drop column in a second migration.
- Never rename a column without an alias period (add new column, dual-write, backfill, switch reads, drop old).
- Never run `prisma migrate dev` / `alembic upgrade head` in production pipelines — use the deploy variant.
- Test migrations against a copy of production data before applying.

## Pagination

Use keyset (cursor) pagination for large or live datasets — not OFFSET:

```sql
-- Offset pagination (fine for small, stable datasets)
SELECT * FROM posts ORDER BY created_at DESC LIMIT 20 OFFSET 400;

-- Keyset pagination (stable, performant at scale)
SELECT * FROM posts
WHERE (created_at, id) < ($last_created_at, $last_id)
ORDER BY created_at DESC, id DESC
LIMIT 20;
```

OFFSET pagination degrades as offset grows and produces drift on live data. A
UUIDv7 primary key (see Schema Design above) makes the cursor column double as
the identifier — no separate `created_at` tie-break needed for creation-order
listing, since the ID already sorts that way.

## Anti-Patterns — Never Do These

| Anti-pattern | Correct alternative |
|---|---|
| String-interpolated SQL | Parameterized queries always |
| `SELECT *` in production code | Explicit column list |
| Missing FK index | Add `CREATE INDEX` on FK column |
| No connection pooling | PgBouncer or driver-level pool |
| Long-running transactions | Keep transactions short and focused |
| OFFSET pagination on large tables | Keyset/cursor pagination |
| `FLOAT`/`DOUBLE` for money | `NUMERIC` or integer cents |
| `TIMESTAMP` without timezone | `TIMESTAMPTZ` |
| `JSON` instead of `JSONB` | `JSONB` for queryability and indexing |
| Querying inside a loop (N+1) | JOINs, CTEs, or batch fetch with `ANY` |
| Unindexed WHERE / ORDER BY columns | Add appropriate index |
| EAV schema for dynamic attributes | `JSONB` column with GIN index |
| Comma-separated values in a column | Array column or junction table |
| Storing passwords in the DB | Hash with bcrypt/argon2; store hash only |
| Random UUIDv4 primary key on a hot-write table | UUIDv7 (`uuidv7()`, PG18+) or `bigint identity` |
| Tests against a mocked/in-memory connection | Real engine via container, real migrations |

## Reviewer Checklist

- [ ] All queries use parameterized placeholders — no string interpolation
- [ ] No `SELECT *` in production code
- [ ] New `WHERE` / `ORDER BY` / `JOIN ON` columns have appropriate indexes
- [ ] Foreign key columns are indexed
- [ ] Multi-step writes are wrapped in a transaction
- [ ] Correct isolation level chosen for the use case
- [ ] No N+1 patterns — loop-based queries replaced with JOINs or batch fetches
- [ ] List endpoints use keyset pagination, not unbounded `LIMIT` or OFFSET at scale
- [ ] `TIMESTAMPTZ` used, not `TIMESTAMP`
- [ ] `NUMERIC` used for money, not `FLOAT`
- [ ] `EXPLAIN (ANALYZE, BUFFERS)` run on any new or modified non-trivial query
- [ ] Migrations are reversible and tested
- [ ] `CREATE INDEX CONCURRENTLY` used for large tables in production migrations
- [ ] New public-facing/distributed primary keys use UUIDv7, not UUIDv4 — with the enumeration/embedded-timestamp trade-off actually considered, not defaulted
- [ ] Tests touching SQL run against the real engine (container/CI service), not a mock
