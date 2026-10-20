---
description: Prisma ORM — schema design, migration discipline, client query patterns, transaction and error handling
triggers:
  file_globs:
    - "**/*.prisma"
    - "**/prisma/**/*"
---

# Concern: Prisma

Injected when tickets involve Prisma schema, migrations, or Prisma Client usage.
Apply the SQL concern alongside this one for query-level guidance.

## References

- Prisma docs: https://www.prisma.io/docs/
- Schema reference: https://www.prisma.io/docs/orm/reference/prisma-schema-reference
- Client API reference: https://www.prisma.io/docs/orm/reference/prisma-client-reference
- Prisma Migrate: https://www.prisma.io/docs/orm/prisma-migrate
- Error reference (P codes): https://www.prisma.io/docs/orm/reference/error-reference

## Target

Prisma ORM 7.x with TypeScript strict mode. PostgreSQL as primary database.

Prisma 7 is a breaking release from 6: the query engine is Rust-free (TS + WASM),
a **driver adapter is mandatory** for every database (`new PrismaClient()` with no
`adapter` throws), the client generator is `prisma-client` (not `prisma-client-js`)
with a required `output` path, and CLI configuration lives in `prisma.config.ts`
rather than `package.json`. If the project is still pinned to Prisma 6.x (a
supported choice — 6.x continues to receive patches, and MongoDB support hasn't
landed on 7 yet), the pre-7 forms noted inline below still apply; treat this file's
default as "if the project is on 7," not as retroactively wrong for 6.

## Schema Design

```prisma
generator client {
  provider = "prisma-client"        // "prisma-client-js" pre-7
  output   = "./generated/prisma"   // required on 7; generates outside node_modules
}

datasource db {
  provider = "postgresql"
  url      = env("DATABASE_URL")
}

model User {
  id        Int      @id @default(autoincrement())
  email     String   @unique
  name      String
  createdAt DateTime @default(now())
  updatedAt DateTime @updatedAt

  posts     Post[]
  profile   Profile?

  @@index([createdAt])
  @@map("users")          // explicit table name
}

model Post {
  id        Int      @id @default(autoincrement())
  title     String
  content   String?
  published Boolean  @default(false)
  authorId  Int
  author    User     @relation(fields: [authorId], references: [id], onDelete: Cascade)

  @@index([authorId])                        // FK always indexed
  @@index([published, createdAt])            // supports common filter+sort
  @@map("posts")
}
```

**Prisma does NOT auto-index foreign key columns on PostgreSQL.** Always add `@@index`
on every FK field.

Use `@@map` for explicit table names. Use `@map` for explicit column names when
the Prisma model name differs from the DB column.

Use `@db.Uuid`, `@db.Text`, `@db.Timestamptz` etc. for DB-specific types where
Prisma's defaults don't match what you need.

Prefer explicit `onDelete` on relations — the default (`SetNull` or `Restrict`)
may not be what you intend.

## Singleton Client

Never instantiate `PrismaClient` per-request — this exhausts the connection pool.
Use a singleton pattern:

```typescript
// lib/prisma.ts
import { PrismaClient } from '../generated/prisma/client'; // '@prisma/client' pre-7
import { PrismaPg } from '@prisma/adapter-pg';               // driver adapter — mandatory on 7

const adapter = new PrismaPg({ connectionString: process.env.DATABASE_URL });

const globalForPrisma = globalThis as unknown as { prisma: PrismaClient };

export const prisma =
  globalForPrisma.prisma ??
  new PrismaClient({
    adapter,
    log: process.env.NODE_ENV === 'development' ? ['query', 'warn', 'error'] : ['error'],
  });

if (process.env.NODE_ENV !== 'production') globalForPrisma.prisma = prisma;
```

On 7, `new PrismaClient()` with no `adapter` throws at construction — there is no
implicit engine-managed connection anymore. Pick the adapter for the driver actually
in use (`@prisma/adapter-pg` for `pg`, `@prisma/adapter-better-sqlite3`, etc.) rather
than defaulting to Postgres out of habit.

## Querying — Avoid N+1

Always use `include` or nested `select` to load relations in a single query:

```typescript
// Wrong — N+1: one query per post to get author
const posts = await prisma.post.findMany({ where: { published: true } });
for (const post of posts) {
  const author = await prisma.user.findUnique({ where: { id: post.authorId } });
}

// Correct — single query
const posts = await prisma.post.findMany({
  where: { published: true },
  include: {
    author: { select: { id: true, name: true } },
  },
  orderBy: { createdAt: 'desc' },
  take: 20,
});
```

Use `select` to fetch only needed fields — never load full models when you only
need a subset:

```typescript
const names = await prisma.user.findMany({
  select: { id: true, name: true },
});
```

Always paginate `findMany` calls — never fetch unbounded collections:

```typescript
// Cursor-based pagination (preferred for large/live datasets)
const posts = await prisma.post.findMany({
  take: 20,
  skip: cursor ? 1 : 0,
  cursor: cursor ? { id: cursor } : undefined,
  orderBy: { createdAt: 'desc' },
});

// Offset pagination (simple, fine for small datasets)
const posts = await prisma.post.findMany({
  take: 20,
  skip: page * 20,
  orderBy: { createdAt: 'desc' },
});
```

## Transactions

Use `$transaction` for all multi-step writes:

```typescript
// Batch transaction — simple, one DB roundtrip
const [user, post] = await prisma.$transaction([
  prisma.user.create({ data: { email } }),
  prisma.post.create({ data: { title, authorId: 1 } }),
]);

// Interactive transaction — for conditional logic
const result = await prisma.$transaction(async (tx) => {
  const { count } = await tx.account.updateMany({
    where: { id, balance: { gte: amount } },
    data: { balance: { decrement: amount } },
  });
  if (count === 0) throw new InsufficientBalanceError(id, amount);
  return tx.ledgerEntry.create({ data: { accountId: id, amount } });
});
```

**Reading a value, deciding in application code, then writing it back is a race, and
being inside `$transaction` does not fix it.** Prisma runs at the database's default
isolation level — `READ COMMITTED` on PostgreSQL and MySQL — under which two concurrent
transactions both read a balance of 100, both conclude 60 is affordable, and both
decrement. The account ends at -20 and neither transaction errors.

Three ways out, in order of preference: put the condition in the `WHERE` clause of the
write itself and check the affected-row count, as above; add a database `CHECK`
constraint so the invariant cannot be violated by any code path; or, when the decision
genuinely cannot be expressed as a predicate, lock the row (`SELECT ... FOR UPDATE` via
`$queryRaw`) or raise the isolation level (`{ isolationLevel: 'Serializable' }`) and
retry on `P2034`. Reserve the concurrency question for anything that reads-then-writes a
counter, balance, quota, or seat count.

## Error Handling

Handle Prisma-specific errors explicitly:

```typescript
import { Prisma } from '../generated/prisma/client'; // '@prisma/client' pre-7

try {
  await prisma.user.create({ data });
} catch (err) {
  if (err instanceof Prisma.PrismaClientKnownRequestError) {
    switch (err.code) {
      case 'P2002':
        throw new ConflictError(`Unique constraint violated on ${err.meta?.target}`);
      case 'P2025':
        throw new NotFoundError('Record not found');
      default:
        throw err;
    }
  }
  throw err;
}
```

Common error codes:
- `P2002` — unique constraint violation
- `P2003` — foreign key constraint violation
- `P2025` — record not found (for update/delete/findUniqueOrThrow)
- `P2034` — transaction conflict (retry eligible)

Use `findUniqueOrThrow` / `findFirstOrThrow` instead of null-checking:

```typescript
// Wrong — null check boilerplate
const user = await prisma.user.findUnique({ where: { id } });
if (!user) throw new NotFoundError('user', id);

// Correct — throws PrismaClientKnownRequestError P2025 if missing
const user = await prisma.user.findUniqueOrThrow({ where: { id } });
```

## Type Safety

Use Prisma-generated input types — never write plain objects and cast:

```typescript
import type { Prisma } from '../generated/prisma/client'; // '@prisma/client' pre-7

function createUserInput(data: Prisma.UserCreateInput): Prisma.UserCreateInput {
  return data;
}

// Type a select object directly against the generated *Select type
const postSelect: Prisma.PostSelect = {
  id: true,
  title: true,
  author: { select: { name: true } },
};
type PostWithAuthor = Prisma.PostGetPayload<{ select: typeof postSelect }>;
```

`Prisma.validator(...)` is removed in Prisma ORM 7 — annotate the object literal with
the generated `*Select`/`*Include`/`*WhereInput` type directly, as above, instead of
wrapping it in the old validator helper.

Never cast Prisma results to `any` or use untyped raw objects.

## Raw Queries

Prefer Prisma Client methods. Use raw queries only when Prisma cannot express the query.
Always use tagged template literals (Prisma escapes them) — never string interpolation:

```typescript
// Correct — tagged template, values are escaped
const users = await prisma.$queryRaw`
  SELECT * FROM users WHERE lower(email) = ${email.toLowerCase()}
`;

// Wrong — SQL injection risk
const users = await prisma.$executeRawUnsafe(
  `SELECT * FROM users WHERE email = '${email}'`  // NEVER
);
```

## Migration Workflow

```bash
# Development — detect schema changes, generate and apply migration
npx prisma migrate dev --name descriptive_name

# Production CI/CD — apply pending migrations only (no prompts, no reset)
npx prisma migrate deploy

# After pulling schema changes from DB
npx prisma db pull && npx prisma generate

# Regenerate client after any schema change
npx prisma generate
```

**On Prisma 7, `migrate dev` no longer regenerates the client automatically** — the
CI/build step must call `prisma generate` explicitly after any migration step, or the
build proceeds against a stale generated client. Pre-7, `migrate dev` did this for you
implicitly; don't assume that behaviour survived the upgrade.

**Never edit migration files after they have been applied to a shared database.**
Create a new migration to correct mistakes.

**Never use `prisma migrate reset` outside of local development** — it drops and
recreates the database.

**Never use `prisma migrate dev` in production or CI pipelines** — use `migrate deploy`.

For data migrations alongside schema changes, use a migration script called from
within the migration SQL, or a separate seeding step — not inline in schema migrations.

## Testing

```typescript
// Option 1: prisma-mock — only for unit-testing logic that merely *calls* Prisma
//   (e.g. a service function's branching), never for verifying that a query,
//   filter, or migration actually does what it's supposed to. A mock replays
//   what the test author believes the query does, not what Postgres does with
//   it, so it cannot catch a wrong `where` shape, a relation that doesn't load
//   what's expected, or a migration that drifted from the schema.
// Option 2: isolated test database (real Postgres, e.g. via testcontainers) with
//   transaction rollback — the default for anything that exercises a real query.

// Transaction rollback pattern for integration tests — run each test inside the
// interactive transaction and abort it, so the work is guaranteed to be one
// transaction on one connection.
class Rollback extends Error {}

async function inRolledBackTransaction(
  test: (tx: Prisma.TransactionClient) => Promise<void>,
): Promise<void> {
  try {
    await prisma.$transaction(async (tx) => {
      await test(tx);
      throw new Rollback();
    });
  } catch (err) {
    if (!(err instanceof Rollback)) throw err;
  }
}
```

Do **not** issue `BEGIN` and `ROLLBACK` as separate `$executeRaw` calls from
`beforeEach` / `afterEach`. Each raw call takes whatever connection the pool hands it,
so the `BEGIN`, the test's writes, and the `ROLLBACK` can land on three different
connections. The `BEGIN` then opens a transaction nobody ever writes to, the test's
writes autocommit, and the `ROLLBACK` discards nothing — the suite passes while leaving
rows behind, and the leak only surfaces later as an unrelated flaky test. Passing the
`tx` client into the test is what makes the isolation real.

Use `prisma db seed` with a typed seed script for reproducible test data. On Prisma 7
the seed command is configured in `prisma.config.ts`, not `package.json`:

```typescript
// prisma.config.ts
import { defineConfig, env } from 'prisma/config';

export default defineConfig({
  schema: 'prisma/schema.prisma',
  migrations: { path: 'prisma/migrations', seed: 'tsx prisma/seed.ts' },
  datasource: { url: env('DATABASE_URL') },
});
```

```json
// package.json — pre-7 only
{ "prisma": { "seed": "tsx prisma/seed.ts" } }
```

On 7, seeding is never triggered implicitly by a migration command (including
`migrate reset`) — it only runs when `prisma db seed` is invoked explicitly. Don't
rely on a pre-7 assumption that `migrate reset` repopulates data for you.

## Anti-Patterns — Never Do These

| Anti-pattern | Correct alternative |
|---|---|
| `new PrismaClient()` per request | Singleton client |
| `findMany` without `take` on large tables | Always paginate |
| `include` loading entire relation without limit | Nested `select` + `take` |
| Missing `@@index` on FK fields | Add explicit index |
| `$executeRawUnsafe` with string interpolation | Tagged template `$queryRaw` |
| `prisma migrate dev` in production | `prisma migrate deploy` |
| `prisma migrate reset` outside local dev | Never in shared environments |
| Editing applied migration files | Create a new corrective migration |
| Casting Prisma results to `any` | Use `Prisma.PostGetPayload<...>` types |
| Implicit many-to-many when relation has attributes | Explicit join model with own fields |
| `new PrismaClient()` with no `adapter` on Prisma 7 | Pass a driver adapter (`@prisma/adapter-pg`, etc.) |
| `Prisma.validator(...)` on Prisma 7 | Annotate the literal with the generated `*Select`/`*Include` type directly |
| Assuming `migrate dev`/`migrate reset` regenerates the client or seeds data on Prisma 7 | Run `prisma generate` and `prisma db seed` explicitly |

## Reviewer Checklist

- [ ] No `new PrismaClient()` outside the singleton module
- [ ] All `findMany` calls have `take` — no unbounded fetches
- [ ] Relations loaded with `include` use nested `select` to limit fields
- [ ] FK columns have `@@index` in the schema
- [ ] Multi-step writes use `$transaction`
- [ ] Read-then-write updates to a balance, counter, quota, or seat count enforce the condition in the write (`updateMany` + affected-count check), a `CHECK` constraint, a row lock, or a raised isolation level — not an `if` between a read and a write
- [ ] `PrismaClientKnownRequestError` codes handled (P2002, P2025 at minimum)
- [ ] Raw queries use tagged template literals — no `$executeRawUnsafe` with interpolation
- [ ] `prisma migrate deploy` (not `dev`) used in CI/CD pipeline
- [ ] New migration files are not edited after creation
- [ ] `prisma generate` runs after any schema change (not assumed automatic post-`migrate dev` on Prisma 7)
- [ ] Prisma-generated input types used — no plain object casts
- [ ] On Prisma 7: `PrismaClient` receives a driver adapter; imports come from the generated `output` path, not `@prisma/client`; seed command lives in `prisma.config.ts`
- [ ] Tests that exercise Prisma queries hit a real database (container/CI service), not a mocked `PrismaClient`
