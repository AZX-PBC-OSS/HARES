---
description: Neo4j/Cypher — graph modeling, parameterized Cypher, transaction functions, indexes and constraints, driver usage
triggers:
  file_globs:
    - "**/*.cypher"
    - "**/*.cql"
    - "**/cypher/**/*"
    - "**/neo4j/**/*"
---

# Concern: Neo4j / Cypher

Injected when tickets involve Neo4j graph database queries, schema, or driver usage.

Neo4j server moved to calendar versioning (`YYYY.MM[.PATCH]`, e.g. `2025.06`) from
January 2025 — there is no single "Neo4j 5" to target anymore, and a server release
has no fixed relationship to the driver major version. Match the driver major to the
server major independently: JavaScript `neo4j-driver` and the Python driver are both
on major version 6.x as of this writing. Check `neo4j-driver`'s installed version
against the target server's supported-driver table before relying on an API below.

## References

- Cypher Manual: https://neo4j.com/docs/cypher-manual/current/
- Graph data modeling guide: https://neo4j.com/docs/getting-started/data-modeling/
- Neo4j JavaScript driver: https://neo4j.com/docs/javascript-manual/current/
- Neo4j Python driver: https://neo4j.com/docs/python-manual/current/
- Operations manual (indexes, constraints): https://neo4j.com/docs/operations-manual/current/
- Performance tuning: https://neo4j.com/docs/operations-manual/current/performance/
- Testcontainers Neo4j module: https://testcontainers.com/modules/neo4j/

## When to Use a Graph Database

Graph is the right choice when:
- Data is highly connected and relationships are first-class (social networks, recommendations, access control, knowledge graphs)
- Queries involve variable-depth traversals (e.g., "find all people within 3 hops")
- Relationship patterns are the query focus, not just the data at nodes
- Schema is flexible and evolves frequently

Graph is NOT a good fit for: tabular reporting, simple CRUD with flat data, bulk
analytical queries over millions of rows (use a columnar store), or cases where
a relational join would be simpler.

## Graph Modeling Conventions

- **Nodes**: PascalCase labels — `:Person`, `:Movie`, `:Order`, `:Product`
- **Relationships**: ALL_CAPS with underscores, always directional — `[:ACTED_IN]`, `[:PURCHASED]`, `[:BELONGS_TO]`
- **Properties**: camelCase — `firstName`, `createdAt`, `totalAmount`

Design rules:
- Model facts as **relationship properties** when the fact belongs to the connection (e.g., `[:PURCHASED {quantity: 2, at: datetime()}]`)
- Model reified relationships (facts about a relationship) as an **intermediate node** (e.g., `(:Order)` between `:User` and `:Product`)
- Never store a list of IDs as a property — model it as relationships
- Use indexes/constraints to anchor MATCH patterns on labeled, indexed properties

## Parameterized Queries — Non-Negotiable

**Always use parameters (`$param`). Never interpolate values into Cypher strings.**
String interpolation is a Cypher injection risk and also prevents query plan caching.

```cypher
// Correct
MATCH (u:User {id: $userId})-[:PURCHASED]->(p:Product)
RETURN p.name, p.price

// Never do this — string interpolation in Cypher
// MATCH (u:User {id: "${userId}"}) RETURN u
```

JavaScript driver:
```typescript
const result = await session.run(
  'MATCH (u:User {id: $id}) RETURN u',
  { id: userId }
);
```

Python driver:
```python
result = await session.run(
    "MATCH (u:User {id: $id}) RETURN u",
    id=user_id,
)
```

## Core Cypher Patterns

**MATCH** — always anchor on labeled, indexed nodes:
```cypher
// Good — anchored on indexed label+property
MATCH (p:Person {id: $personId})-[:ACTED_IN]->(m:Movie)
WHERE m.released > 2000
RETURN m.title, m.released
ORDER BY m.released DESC
LIMIT 25

// Bad — full graph scan
MATCH (n) RETURN n
```

**MERGE** — create if not exists:
```cypher
// MERGE on a single unique identifier, then SET additional properties
MERGE (p:Person {id: $id})
ON CREATE SET
  p.createdAt = datetime(),
  p.name = $name,
  p.email = $email
ON MATCH SET
  p.lastSeenAt = datetime()
RETURN p
```

Never MERGE on multiple properties unless that combination is the unique key —
partial matches create duplicate nodes.

**WITH** to chain query stages and filter between them:
```cypher
MATCH (p:Person)-[:ACTED_IN]->(m:Movie)<-[:DIRECTED]-(d:Person)
WITH d.name AS director, count(m) AS movieCount
WHERE movieCount > 5
RETURN director, movieCount
ORDER BY movieCount DESC
```

**UNWIND** for batch creates from a list:
```cypher
UNWIND $events AS event
MERGE (e:Event {id: event.id})
SET e += event.properties
```

**OPTIONAL MATCH** for left-join semantics:
```cypher
MATCH (u:User {id: $userId})
OPTIONAL MATCH (u)-[:HAS_PROFILE]->(p:Profile)
RETURN u, p
```

**Variable-length paths** — always bound depth in production:
```cypher
// Correct — bounded
MATCH path = (a:Person {id: $id})-[:KNOWS*1..4]->(b:Person)
WHERE b.city = $city
RETURN path, length(path) AS hops
ORDER BY hops ASC LIMIT 50

// Never — unbounded path traversal (can scan the entire graph)
MATCH (a)-[*]->(b) RETURN a, b
```

## Indexes and Constraints

Always create indexes and constraints before querying or loading data.

```cypher
// Uniqueness constraint (creates index automatically)
CREATE CONSTRAINT person_id_unique FOR (p:Person) REQUIRE p.id IS UNIQUE;
CREATE CONSTRAINT user_email_unique FOR (u:User) REQUIRE u.email IS UNIQUE;

// Range index for non-unique, frequently filtered properties
CREATE INDEX person_name FOR (p:Person) ON (p.name);
CREATE INDEX order_created FOR (o:Order) ON (o.createdAt);

// Composite index for multi-property filter
CREATE INDEX order_user_date FOR (o:Order) ON (o.userId, o.createdAt);

// Full-text index for text search
CREATE FULLTEXT INDEX product_search FOR (p:Product) ON EACH [p.name, p.description];

// Verify indexes
SHOW INDEXES;
```

Add `IF NOT EXISTS` on constraint/index creation in any script that runs more than
once (setup scripts, migrations, application bootstrap) — re-running a bare `CREATE
CONSTRAINT`/`CREATE INDEX` against an already-provisioned database errors instead of
being a no-op:

```cypher
CREATE CONSTRAINT person_id_unique IF NOT EXISTS
FOR (p:Person) REQUIRE p.id IS UNIQUE;
```

## Query Profiling

Use `EXPLAIN` (without executing) and `PROFILE` (executes and measures):

```cypher
PROFILE
MATCH (p:Person {name: $name})-[:KNOWS]->(friend)
RETURN friend.name
```

Warning signs:
- `NodeByLabelScan` — no index on the filter property; add one
- High `db hits` relative to returned rows — query is not selective
- `CartesianProduct` — multiple disconnected MATCH clauses; restructure with WITH

## Transactions

Use explicit transactions for multi-statement writes. Use `executeWrite` / `executeRead`
on the session for automatic retry on transient errors:

```typescript
// JavaScript driver
const result = await session.executeWrite(async (tx) => {
  await tx.run('MERGE (u:User {id: $id}) SET u.name = $name', { id, name });
  return tx.run('MATCH (u:User {id: $id}) RETURN u', { id });
});

// Read operations — use executeRead for routing to replica
const records = await session.executeRead((tx) =>
  tx.run('MATCH (p:Person {id: $id}) RETURN p', { id })
);
```

Retry on `Neo.TransientError` — these are retriable (deadlock, leader re-election).

For bulk operations use `CALL { ... } IN TRANSACTIONS OF N ROWS` to avoid giant transactions:
```cypher
CALL {
  UNWIND $batch AS row
  MERGE (p:Person {id: row.id})
  SET p += row.properties
} IN TRANSACTIONS OF 1000 ROWS
```

This native subquery form is the first choice for batching in current Cypher. Reach
for **APOC Core** (`apoc.periodic.iterate`) only when a batch job needs something the
native form doesn't give you — e.g. driving the iteration from a Cypher query result
too large to `UNWIND` in one pass, or per-batch error callbacks. APOC Core ships
calendar-versioned to match the server (install the release whose year/month matches
the target Neo4j server) and is actively maintained; it is not a third-party gamble.

## Driver Session Lifecycle

Always close sessions. Use the driver's built-in connection pool — do not create a
new driver instance per request.

```typescript
// JavaScript — one driver, many sessions
const driver = neo4j.driver(uri, neo4j.auth.basic(user, password));

async function query<T>(fn: (session: neo4j.Session) => Promise<T>): Promise<T> {
  const session = driver.session({ database: 'neo4j' });
  try {
    return await fn(session);
  } finally {
    await session.close();
  }
}

// On application shutdown
await driver.close();
```

## Testing Against a Real Neo4j Instance

A mocked driver/session replays whatever the test author believes the Cypher does —
it cannot catch a MERGE that duplicates nodes because the match key wasn't actually
unique, a label typo, or a query that plans a cartesian product against real data
volume. Run tests that exercise Cypher against a real Neo4j server, e.g. the
[Testcontainers Neo4j module](https://testcontainers.com/modules/neo4j/) pinned to
the target server version, not a stubbed driver. This is the SQL-level rule; whether
the suite as a whole reaches this through the app's real entry point, versus a
self-assembled graph that never wires the real driver in, is the integration-testing
concern's question, not this one.

## Anti-Patterns — Never Do These

| Anti-pattern | Correct alternative |
|---|---|
| String interpolation in Cypher | Parameters (`$param`) always |
| `MATCH (n)` without a label | Anchor on labeled, indexed node |
| Unbounded path `-[*]->` | Bound depth: `-[*1..5]->` |
| Cartesian products (multiple unconnected MATCH) | Use WITH between stages |
| MERGE on multiple properties | MERGE on unique ID, then SET others |
| Storing IDs as a property array | Model as relationships |
| No index on WHERE/MATCH filter properties | CREATE INDEX or CONSTRAINT |
| Collecting unlimited related nodes | Use LIMIT |
| Ignoring dense node (supernode) problem | Relationship filtering or intermediate nodes |
| Creating a new driver instance per request | Singleton driver with session pool |
| Not closing sessions | Use try/finally or context manager |
| Bare `CREATE CONSTRAINT`/`CREATE INDEX` in a re-run script | Add `IF NOT EXISTS` |
| Testing Cypher against a mocked driver/session | Real server via Testcontainers or CI service |

## Reviewer Checklist

- [ ] All Cypher queries use parameters — no string interpolation
- [ ] MATCH patterns anchor on labeled, indexed nodes
- [ ] Variable-length paths are bounded (explicit min..max depth)
- [ ] No cartesian products (multiple unconnected MATCH without WITH)
- [ ] MERGE uses a single unique identifier; other properties set via ON CREATE/MATCH SET
- [ ] Indexes/constraints exist for all WHERE and MATCH filter properties
- [ ] New indexes defined with SHOW INDEXES verification step
- [ ] EXPLAIN/PROFILE run on non-trivial queries
- [ ] Write operations use executeWrite; read operations use executeRead
- [ ] Bulk operations use CALL { } IN TRANSACTIONS to avoid giant transactions
- [ ] Sessions are closed in finally blocks; driver is a singleton
- [ ] Transient errors handled with retry logic
- [ ] Constraint/index creation scripts are idempotent (`IF NOT EXISTS`)
- [ ] Cypher-touching tests run against a real Neo4j server, not a mocked driver
