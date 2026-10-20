---
description: Pydantic v2 — model definitions, validators, serialization, strict typing, pydantic-settings
triggers:
  file_globs:
    - "**/schemas.py"
    - "**/schemas/**/*.py"
    - "**/*_schemas.py"
    - "**/settings.py"
---

# Concern: Pydantic

Injected when tickets involve Pydantic models, validation, serialization, or
pydantic-settings. Applies in addition to the Python concern.

## References

- Pydantic v2 docs: https://docs.pydantic.dev/latest/
- Pydantic v2 validators: https://docs.pydantic.dev/latest/concepts/validators/
- Pydantic v2 migration guide: https://docs.pydantic.dev/latest/migration/
- pydantic-settings: https://docs.pydantic.dev/latest/concepts/pydantic_settings/
- Pydantic types reference: https://docs.pydantic.dev/latest/concepts/types/

## Target

Pydantic v2 (2.x). If the project uses v1, flag it and recommend migration.
The v1 API is deprecated — all new code must use v2 APIs.

## V1 → V2 Migration Reference

| Pydantic v1 (wrong) | Pydantic v2 (correct) |
|---|---|
| `class Config: orm_mode = True` | `model_config = ConfigDict(from_attributes=True)` |
| `@validator("field")` | `@field_validator("field")` |
| `@root_validator` | `@model_validator(mode="before")` or `mode="after"` |
| `.dict()` | `.model_dump()` |
| `.json()` | `.model_dump_json()` |
| `.parse_obj(data)` | `Model.model_validate(data)` |
| `.parse_raw(json_str)` | `Model.model_validate_json(json_str)` |
| `.schema()` | `Model.model_json_schema()` |
| `class Config:` | `model_config = ConfigDict(...)` |

## Model Design

Separate models for separate concerns — never reuse a single model across all contexts:

```python
from pydantic import BaseModel, EmailStr, Field, ConfigDict

# Base: shared fields
class UserBase(BaseModel):
    email: EmailStr
    name: str = Field(..., min_length=1, max_length=100)

# Create: fields required at creation time
class UserCreate(UserBase):
    password: str = Field(..., min_length=8)

# Update: all fields optional for PATCH
class UserUpdate(BaseModel):
    name: str | None = None
    email: str | None = None

# Public response: safe to expose
class UserPublic(UserBase):
    model_config = ConfigDict(from_attributes=True)  # enables ORM model → Pydantic
    id: int
    createdAt: datetime

# DB model: internal, may contain hashed_password, internal flags
class UserInDB(UserPublic):
    hashed_password: str
```

Use `model_config = ConfigDict(from_attributes=True)` on response models that are
constructed from ORM objects. This replaces `orm_mode = True`.

## Field Definitions

```python
from pydantic import BaseModel, Field
from typing import Annotated

class Product(BaseModel):
    # Required field with constraints
    name: str = Field(..., min_length=1, max_length=200)
    price: float = Field(..., gt=0, description="Price in USD")
    sku: str = Field(..., pattern=r"^[A-Z]{2}-\d{6}$")

    # Optional with default
    discount: float = Field(default=0.0, ge=0.0, le=1.0)
    tags: list[str] = Field(default_factory=list)

    # Annotated style (composable, reusable)
    quantity: Annotated[int, Field(ge=0, le=10_000)]
```

Use `Annotated` for reusable field constraints:
```python
from typing import Annotated
from pydantic import Field

PositiveInt   = Annotated[int, Field(gt=0)]
NonEmptyStr   = Annotated[str, Field(min_length=1)]
SkuCode       = Annotated[str, Field(pattern=r"^[A-Z]{2}-\d{6}$")]
```

Reach for this only where Pydantic has no type already. Do not hand-roll a pattern for
something Pydantic ships (`EmailStr`, `HttpUrl`, `UUID4`) — a home-grown regex is
weaker than the library's validator and shadows the real name at the import site.

Pydantic-provided types for common use cases:
```python
from pydantic import EmailStr, HttpUrl, AnyUrl, UUID4, SecretStr

class UserCreate(BaseModel):
    email: EmailStr           # validates email format
    website: HttpUrl | None   # validates URL
    api_key: SecretStr        # masked in repr/logs
    user_id: UUID4            # validates UUID v4 format
```

`EmailStr` requires the optional `email-validator` dependency — install with
`pydantic[email]`, not bare `pydantic`. Without it, `EmailStr` raises an import error
at class-definition time, not silently falling back to `str`.

For validating a value that isn't a `BaseModel` field — a bare `list[int]`, a
function argument, a `TypedDict` or `dataclass` you don't want to wrap in a model —
use `TypeAdapter` instead of building a throwaway model:
```python
from pydantic import TypeAdapter

ids = TypeAdapter(list[int]).validate_python(raw_ids)
```

## Validators

### Field validators (v2 API)

```python
from pydantic import field_validator

class UserCreate(BaseModel):
    email: str
    name: str

    @field_validator("email")
    @classmethod
    def normalise_email(cls, v: str) -> str:
        return v.lower().strip()

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: str) -> str:
        if not v.strip():
            raise ValueError("Name cannot be blank")
        return v.strip()
```

`@field_validator` runs after Pydantic's own type coercion. Use `mode="before"`
to validate before coercion:
```python
@field_validator("tags", mode="before")
@classmethod
def parse_tags(cls, v: str | list[str]) -> list[str]:
    if isinstance(v, str):
        return [tag.strip() for tag in v.split(",")]
    return v
```

### Model validators (cross-field validation)

```python
from pydantic import model_validator

class DateRange(BaseModel):
    start: date
    end: date

    @model_validator(mode="after")
    def check_dates(self) -> "DateRange":
        if self.start > self.end:
            raise ValueError("start must be before end")
        return self
```

Use `mode="before"` for pre-coercion transformation of the entire input dict.
Use `mode="after"` for cross-field validation after all fields are coerced.

For a single reusable constraint you want to attach to a type alias rather than
repeat as a decorator on every model that uses it, `Annotated` + `AfterValidator` /
`BeforeValidator` from `pydantic.functional_validators` compose the same way as the
`Annotated` field-constraint pattern above:
```python
from pydantic import AfterValidator
from typing import Annotated

def _strip_lower(v: str) -> str:
    return v.strip().lower()

NormalisedEmail = Annotated[str, AfterValidator(_strip_lower)]
```
Reach for `@field_validator` when the logic is specific to one model; reach for the
`Annotated` form when the same validation needs to be shared across models.

## Serialization

```python
user = UserPublic(id=1, email="a@b.com", name="Alice", createdAt=datetime.now())

# Dict
user.model_dump()
user.model_dump(exclude={"createdAt"})
user.model_dump(include={"id", "email"})

# For PATCH — only include explicitly set fields
class UserUpdate(BaseModel):
    name: str | None = None
    email: str | None = None

update = UserUpdate(name="Bob")
update.model_dump(exclude_unset=True)   # {"name": "Bob"} — email NOT included
update.model_dump(exclude_none=True)    # {"name": "Bob"} — email excluded because None

# JSON
user.model_dump_json()
user.model_dump_json(indent=2)
```

Always use `exclude_unset=True` for PATCH/partial-update operations so you only
apply fields the caller explicitly provided.

## Parsing / Validation

```python
# From dict
user = UserPublic.model_validate({"id": 1, "email": "a@b.com", "name": "Alice"})

# From ORM object (with from_attributes=True in ConfigDict)
user = UserPublic.model_validate(db_user)

# From JSON string
user = UserPublic.model_validate_json('{"id":1,"email":"a@b.com","name":"Alice"}')
```

`model_validate` returns a model instance or raises `ValidationError` — there is no
non-raising variant and no `(value, errors)` return. `strict=True` disables type
coercion for that call; it does not change the raising behaviour. To validate without
propagating a failure, catch `ValidationError` yourself.

Handle `ValidationError` explicitly at boundaries:
```python
from pydantic import ValidationError

try:
    user = UserCreate.model_validate(raw_input)
except ValidationError as exc:
    errors = exc.errors()  # list of dicts with loc, msg, type
    raise HTTPException(status_code=422, detail=errors) from exc
```

## Configuration (ConfigDict)

```python
from pydantic import BaseModel, ConfigDict

class StrictModel(BaseModel):
    model_config = ConfigDict(
        str_strip_whitespace=True,    # strip leading/trailing whitespace from str fields
        str_min_length=1,             # all str fields must be non-empty after stripping
        frozen=True,                  # instances are immutable (hashable)
        extra="forbid",               # reject unknown fields (useful for API input models)
        from_attributes=True,         # allow constructing from ORM objects
        validate_by_name=True,        # allow using field name OR alias
        validate_by_alias=True,       # allow using alias for validation (default)
    )
```

`validate_by_name` + `validate_by_alias` (Pydantic 2.11+) replace `populate_by_name`,
which is pending deprecation ahead of v3 — `populate_by_name=True` alone still works
today but new code should use the pair above.

Use `extra="forbid"` on request input models to reject unknown fields (security best
practice — prevents field injection). Use `extra="ignore"` on response/internal models
when you control only a subset of a larger object.

## pydantic-settings (Configuration Management)

```python
from functools import lru_cache

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict

class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        env_nested_delimiter="__",    # DATABASE__HOST becomes Settings.database.host
        case_sensitive=False,
    )

    # Required — will fail at startup if not set
    database_url: str
    secret_key: str

    # Optional with defaults
    debug: bool = False
    cors_origins: list[str] = Field(default_factory=list)
    max_connections: int = 10

# Cached accessor — parsed once, injected where used, overridable in tests
@lru_cache
def get_settings() -> Settings:
    return Settings()
```

Expose settings through a **cached accessor function**, not a module-level instance.
Both parse the environment exactly once, but the accessor is injectable and
overridable — a test (or a subcommand, or a worker with a different profile) can
substitute a `Settings` instance without patching module globals, and importing the
module no longer performs I/O and validation as a side effect. Where a framework
provides dependency injection, inject the accessor rather than importing a settings
object at module top.

A module-level `settings = Settings()` instance is acceptable only in a module that
nothing needs to override — a standalone script with no test suite. In an application,
prefer the accessor.

Rules:
- Never hardcode secrets — all sensitive values come from environment or secrets manager
- `SecretStr` type for sensitive fields prevents accidental logging
- Validate settings at startup so configuration errors fail fast before serving traffic
- Parse once behind a cached accessor — do not re-instantiate per request

### Configuration is read through the configuration object

**Configuration is read through the configuration object, once, at the boundary.
Scattered direct environment reads scatter the contract and defeat startup
validation.** Every value the process depends on is declared as a field on `Settings`,
so the set of inputs is enumerable from one place, typed, validated at startup, and
visible to anyone reading the class.

The one exception is the bootstrap read that selects *which* configuration to load — the env file, profile, or `Settings` subclass — because it necessarily happens before the settings object exists.

Any other `os.environ` / `os.getenv` read outside the settings class is a defect. It
is undeclared (nothing fails at startup when it is missing), untyped (a string where an
int or bool was meant), unvalidated, and invisible to every reader of `Settings`. The
consequence is worst across process boundaries: a spawned worker or subprocess that
reads the environment directly resolves a *different* configuration than the parent
that spawned it, silently, whenever the parent's effective config came from a `.env`
file, a CLI override, or a test fixture rather than from the ambient environment.

```python
# Wrong — undeclared, untyped, unvalidated; a spawned worker reading this resolves
# whatever is in its own environment, not the parent's effective configuration
import os

def build_client() -> Client:
    return Client(timeout=int(os.environ.get("API_TIMEOUT", "30")))

# Correct — declared on Settings, validated at startup, passed to the code that needs it
class Settings(BaseSettings):
    api_timeout_seconds: int = 30

def build_client(settings: Settings) -> Client:
    return Client(timeout=settings.api_timeout_seconds)
```

When spawning a subprocess or worker, pass the resolved configuration explicitly
(serialised settings, explicit environment mapping, CLI arguments) rather than relying
on the child to re-derive it from its ambient environment.

## Anti-Patterns — Never Do These

| Anti-pattern | Correct alternative |
|---|---|
| Pydantic v1 APIs (`.dict()`, `@validator`, `orm_mode`) | v2 equivalents |
| Single model for request + response + DB | Separate models per concern |
| `.model_dump()` for PATCH without `exclude_unset=True` | Always use `exclude_unset=True` for partial updates |
| `model_validate(raw) as SomeType` | Let Pydantic validate and raise `ValidationError` |
| Catching `ValidationError` and hiding errors | Surface field-level errors in 422 response |
| `extra="allow"` on public-facing input models | `extra="forbid"` to reject unknown fields |
| `str | None = None` for truly required fields | `str` with `Field(...)` — `...` means required |
| Nested secrets in Settings as plain `str` | `SecretStr` type |
| Re-instantiating `Settings()` per request | Cached accessor (`@lru_cache def get_settings()`), injected where used |
| Module-level `settings = Settings()` instance imported across the app | Cached accessor injected at use sites — module-level instances run I/O and validation at import time and cannot be overridden without patching globals |
| `os.environ` / `os.getenv` read outside the settings class | Declare the value as a `Settings` field and pass the settings object |
| Subprocess or worker re-deriving config from its own ambient environment | Pass the parent's resolved configuration explicitly |
| Manual dict construction from ORM objects | `model_validate(orm_obj)` with `from_attributes=True` |

## What NOT to flag

- `extra="ignore"` (the default) on a model that's never constructed from untrusted external input — `extra="forbid"` earns its place on public-facing request models, not on every model in the codebase.
- A single-model validator written as `@field_validator` instead of `Annotated` + `AfterValidator` — the decorator form is correct when the check is specific to one model; only flag it when the same logic is copy-pasted across multiple models.
- Existing, already-shipped code still using `populate_by_name=True` — it isn't removed, only pending deprecation ahead of v3. Flag it as a follow-up, not a blocker, unless the ticket is already touching that `ConfigDict`.
- `model_dump()` without `exclude_unset=True` on a full-replacement `PUT`-style update, where every field is expected to be present — `exclude_unset` is for partial `PATCH` semantics, not a blanket rule for every `model_dump()` call.

## Reviewer Checklist

- [ ] Only v2 APIs used — no `.dict()`, `@validator`, `orm_mode`, or `class Config:`
- [ ] Separate models for request input, response output, and DB/ORM representations
- [ ] `exclude_unset=True` used in PATCH/partial-update serialization
- [ ] `extra="forbid"` on public-facing request input models
- [ ] `model_config = ConfigDict(from_attributes=True)` on models built from ORM objects
- [ ] Validators use `@field_validator` (v2) with `@classmethod`
- [ ] Cross-field validation uses `@model_validator(mode="after")`
- [ ] `ValidationError` caught and surfaced as 422 with field-level detail
- [ ] pydantic-settings `Settings` is exposed via a cached accessor and injected where used — not a module-level instance imported across the app
- [ ] No `os.environ` / `os.getenv` reads outside the settings class, other than the bootstrap read that selects which env file or settings profile to load; every other configuration input is a declared, typed, validated `Settings` field
- [ ] Spawned subprocesses and workers receive the parent's resolved configuration explicitly rather than re-deriving it
- [ ] Sensitive settings use `SecretStr` type
- [ ] `SecretStr.get_secret_value()` only called where the actual value is needed
