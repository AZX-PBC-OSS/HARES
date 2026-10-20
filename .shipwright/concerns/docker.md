---
description: Docker and container builds covering multi-stage builds, non-root runtime, secret hygiene, .dockerignore, BuildKit caching, digest pinning, and verifying the properties you want against the built image rather than against the Dockerfile (no installer in the runtime stage, package data actually present, a compose command the image can run)
triggers:
  file_globs:
    - "**/Dockerfile"
    - "**/Dockerfile.*"
    - "**/*.Dockerfile"
    - "**/.dockerignore"
    - "**/docker-compose*.yml"
    - "**/docker-compose*.yaml"
    - "**/compose.yml"
    - "**/compose.yaml"
---

# Concern: Docker & Containers

Injected when tickets involve Dockerfiles, container builds, image publishing,
docker-compose, or container runtime configuration. All agents must treat insecure
defaults (running as root, secrets in ENV/ARG, missing .dockerignore) as blocking issues.

## 1. Multi-Stage Builds

Use a builder stage for compilation and a minimal distroless or slim image for the
final stage. This keeps the final image small, reduces attack surface, and avoids
shipping build tooling into production.

### Python (uv)

```dockerfile
# Builder: dependencies first, as their own layer, then the project itself
FROM python:3.13-slim AS builder
COPY --from=ghcr.io/astral-sh/uv:0.11.32@sha256:<digest> /uv /usr/local/bin/uv
WORKDIR /app
ENV UV_PYTHON=/usr/local/bin/python3.13
COPY pyproject.toml uv.lock ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-install-project --no-dev --extra prod
COPY src/ ./src/
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-dev --no-editable --extra prod

# Runtime: the venv and nothing else; uv is not present here
FROM python:3.13-slim AS final
WORKDIR /app
RUN groupadd --system app && useradd --system --gid app --no-create-home app
COPY --from=builder /app/.venv /app/.venv
ENV PATH="/app/.venv/bin:$PATH"
USER app
ENTRYPOINT ["my-service"]
```

Three properties are being bought here, and each has a way to fail:

- **`--locked`, never `--frozen`.** Both refuse to re-resolve. Only `--locked` fails when
  `pyproject.toml` and `uv.lock` disagree; `--frozen` builds a green image silently missing a
  dependency somebody added and never re-locked. Both files are already in the build context, so
  the check is local and needs no network.
- **`--no-dev` prunes a dependency group and has no effect on an extra.** A `dev` set declared
  as `[project.optional-dependencies]` cannot be excluded by the flag written to exclude it. A
  real deployment image shipped pytest, ruff, pyright, mkdocs and testcontainers for exactly
  this reason; moving `dev` to `[dependency-groups]` took it from 1.24GB to 968MB.
- **uv is absent from the runtime stage**, so nothing in the container can resync at start-up
  and reintroduce the packages the build removed. If the property you want is "no installer
  present", also strip the base image's system `pip` and the bundled `ensurepip` wheels, since a
  `python:*-slim` base ships both.

`--no-editable` deserves its own check. It is unsafe while any shipped module computes a repo
root from `Path(__file__).resolve().parents[N]`: measured inside an image, one such expression
resolved to `/app/.venv/lib/python3.13`, a directory that exists and contains no
`pyproject.toml`. The value was wrong rather than absent, so nothing raised and the code
carried on against the wrong root. Replace that pattern with an explicit setting, or
`importlib.resources` for packaged data, and fail loudly when a checkout the code genuinely
requires is not there.

**A copied venv is only valid on an image whose interpreter sits at the same path and
the same version as the builder's.** A venv is a directory of symlinks and a
`pyvenv.cfg` pointing at the interpreter that created it. Copying one from
`python:3.12-slim` (CPython 3.12 at `/usr/local/bin`) into a Debian-derived distroless
Python image (Debian's own Python, a different minor version, at `/usr/bin`) leaves a
dangling interpreter link and the container fails to start. If you want a distroless
final stage, build the venv against that image's exact interpreter — or ship a
statically-linked or self-contained artifact instead, which is why distroless suits Go
and Java far better than it suits Python.

### Node.js

```dockerfile
# Builder — install all deps and compile TypeScript
FROM node:20-slim AS builder
WORKDIR /app
COPY package.json package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY tsconfig.json ./
COPY src/ ./src/
RUN npm run build

# Final — production deps only, no devDependencies
FROM node:20-slim AS final
WORKDIR /app
COPY package.json package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --omit=dev
COPY --from=builder /app/dist ./dist
ENTRYPOINT ["node", "dist/main.js"]
```

## 2. Layer Caching — Order Matters

Copy dependency manifests first, install dependencies, then copy source code.
Source changes are frequent; dependency changes are not. Keeping them in separate
layers means a source edit does not invalidate the (expensive) dependency install layer.

**Before — source changes bust the dep cache:**

```dockerfile
# Wrong — COPY . . brings in everything including source; any file change
# invalidates everything below, including the expensive pip/npm install
COPY . .
RUN pip install -r requirements.txt
```

**After — dep cache survives source changes:**

```dockerfile
# Correct — manifests change rarely; source changes don't touch this layer
COPY requirements.txt ./
RUN pip install -r requirements.txt
# Now copy source — only this layer and below are invalidated on source changes
COPY src/ ./src/
```

## 3. Rootless Containers

Never run as root inside a container. If the container is compromised or escapes,
a root process in the container is root on the host. Always create a dedicated
non-root system user and switch to it before the final `ENTRYPOINT` or `CMD`.

**`USER` sets a default, not an enforced restriction.** It only changes the UID the
container starts with; it does not prevent root from running the container. Anyone
who can invoke the runtime can override it — `docker run --user 0 <image>` or a
Compose/Kubernetes-level `user:`/`runAsUser: 0` puts the container back on root
regardless of what the image declares, and an orchestrator's own defaults can do
the same without anyone typing a flag. Treat `USER` as necessary but not
sufficient: it removes root as the *accidental* default, but stopping root as a
*deliberate* choice needs a control the image itself cannot provide — a
platform-level policy that rejects or blocks root at runtime (for example, a
Kubernetes `securityContext.runAsNonRoot: true` / Pod Security admission policy,
an equivalent Compose/orchestrator constraint, or a rootless container runtime).
If the project's deployment target has such a control, point to it in the same
place the Dockerfile is reviewed; if it doesn't, say so as a gap rather than
implying `USER` alone closes it.

```dockerfile
# Debian/Ubuntu-based images (slim, bookworm, etc.)
RUN groupadd --system app && useradd --system --gid app --no-create-home app
USER app
```

```dockerfile
# Alpine-based images — BusyBox adduser/addgroup take short flags only;
# the GNU long options above are silently a different tool and will fail here.
RUN addgroup -S app && adduser -S -G app -H app
USER app
```

Verify from the image metadata rather than by running a command inside it — a
distroless or scratch final stage has no shell and no `whoami` to run:

```bash
docker inspect --format '{{.Config.User}}' <image>   # must not be empty or "root"
```

## 4. .dockerignore

Always include a `.dockerignore` file in every project with a Dockerfile. Without
it, the build client sends the entire build context (including `.git`, virtual
environments, and potentially secret files) to the builder on every build, increasing
build time and risking secret leakage into the image layer cache.

**`.dockerignore` scopes to the build context, not to a Dockerfile.** By default the
builder looks for a file literally named `.dockerignore` at the root of the
directory (or other context source) passed to the build command — so if several
Dockerfiles in a repo build from the same context, a single root `.dockerignore`
applies to all of them; it does not give any one image its own exclusion list. To
scope exclusions per Dockerfile, name the ignore file after the Dockerfile itself
and place it alongside it: `<dockerfile-name>.dockerignore` (for example
`build.Dockerfile.dockerignore` next to `build.Dockerfile`). A Dockerfile-specific
ignore file, when present, takes precedence over the root `.dockerignore` for that
build — the two are not merged. In a multi-Dockerfile repo, check which of these
two shapes the project actually needs before assuming one root file covers every
image correctly.

**A `.dockerignore` entry that excludes a file the Dockerfile `COPY`s fails only on a
cache-cold build.** With a warm cache the layer is reused and the missing file never shows up;
the first clean build somewhere else fails. `docker build --no-cache` is the check that finds
this, and it belongs in CI on any image whose contents matter.

Minimum `.dockerignore`:

```
.git
node_modules
__pycache__
.venv
*.pyc
.env*
dist/
build/
coverage/
**/*.test.ts
**/*.spec.ts
```

## 5. BuildKit Cache Mounts

Use `--mount=type=cache` to persist package manager caches across builds. Without
this, pip/uv/npm downloads the entire dependency tree on every build even when
nothing has changed.

BuildKit is enabled by default in Docker Engine 23.0+. If you are on an older
version, set `DOCKER_BUILDKIT=1` or use `docker buildx build`. All `--mount=type=cache`
examples below require BuildKit.

**uv / pip cache:**

```dockerfile
RUN --mount=type=cache,target=/root/.cache/uv uv sync --frozen
```

**npm cache:**

```dockerfile
RUN --mount=type=cache,target=/root/.npm npm ci
```

**pnpm cache:**

```dockerfile
RUN --mount=type=cache,target=/root/.local/share/pnpm/store pnpm install --frozen-lockfile
```

## 6. Build Secrets

Never put secrets in `ENV` or `ARG` instructions. Both are visible in
`docker inspect` output and in image history (`docker history --no-trunc`).
Any intermediate layer that captures a secret in an environment variable leaks it
permanently into the image.

**Wrong — secret visible in image history:**

```dockerfile
ARG API_KEY
ENV API_KEY=$API_KEY
```

**Correct — secret available only during the build step, never stored in a layer:**

```dockerfile
RUN --mount=type=secret,id=api_key API_KEY=$(cat /run/secrets/api_key) ./setup.sh
```

Pass the secret at build time:

```bash
docker build --secret id=api_key,src=./secrets/api_key.txt .
```

For runtime secrets (not build-time), inject via environment variables at container
start time, never bake them into the image.

## 7. Pin Base Images

Never use `latest` or an unversioned tag for base images. `latest` is a moving
target — it changes without notice, breaking reproducibility and creating supply
chain risk (a compromised upstream image silently becomes your new base).

**Wrong:**

```dockerfile
FROM python:latest
FROM node:lts
```

**A version tag alone is not a real pin.** Registry tags are mutable — the same
tag (`3.12.9-slim-bookworm`, not just `latest`) can be overwritten in place by the
upstream maintainer, by a CI pipeline pushing to the same tag, or by anyone with
push access if the registry is compromised, so `docker pull` on the "same" tag
can silently return different bytes tomorrow. A tag narrows *which* image you
mean; it does not freeze *what* that image contains.

**Correct — pin to a specific version tag *and* digest:**

```dockerfile
FROM python:3.12.9-slim-bookworm
FROM node:20.19.0-slim

# Pinning by digest is the reproducible option: the sha256 is a content hash,
# so the same reference can never resolve to different bytes
FROM python:3.12.9-slim-bookworm@sha256:abc123...
```

**Digest pinning trades one failure mode for the opposite one: it stops updating
by itself.** A digest is immutable by construction, so a digest-pinned image never
silently changes underneath you — and it also never silently picks up an upstream
security fix. Left alone, a digest pin ages exactly like an unpatched dependency:
correct on the day it was written, and increasingly behind afterward. Digest
pinning is only a net safety win paired with an automated refresh mechanism
(e.g. Dependabot's or Renovate's Docker digest-update support) that opens a PR
when the upstream tag's digest moves, so updates are reviewed and applied rather
than silently absent. A digest pin with no refresh process is a gap to flag, not
a finished mitigation.

## 8. HEALTHCHECK

A `HEALTHCHECK` declares how to tell a running container from a working one. Know which
of your runtimes actually reads it: Docker Engine, Docker Compose (`depends_on:
condition: service_healthy`) and ECS do. **Kubernetes does not** — it ignores the image's
`HEALTHCHECK` entirely and uses the liveness, readiness, and startup probes declared on
the pod spec. On Kubernetes the `HEALTHCHECK` is at best documentation of the right
probe endpoint; the probes themselves are the deliverable.

```dockerfile
# HTTP service — probe the health endpoint
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -f http://localhost:8000/health || exit 1
```

If `curl` is not available in a distroless image, use `wget` or compile a small
healthcheck binary into the image:

```dockerfile
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD wget -qO- http://localhost:8000/health || exit 1
```

A container without a `HEALTHCHECK` is treated as healthy immediately after start,
even if the application inside is still initialising or has crashed.

## 9. Image Scanning

Integrate image scanning into CI. Treat CRITICAL and HIGH CVEs as build failures;
do not ship images with known critical vulnerabilities.

**Docker Scout:**

```bash
docker scout cves <image>:<tag>
```

**Trivy (open source, widely used in CI):**

```bash
trivy image --exit-code 1 --severity CRITICAL,HIGH <image>:<tag>
```

Run scanning as a CI step after build and before push to the registry. Pin the
scanner version to avoid silent changes in scanning behaviour.

## Verify against the built image, not against the Dockerfile

Every property in this file is a property of an artifact, and reading the Dockerfile only tells
you what was intended. Three failures found this way, each of which passed review of the source:

- **A compose file running `uv run` against an image that contains no uv.** Those services
  cannot start. `docker run --rm <image> sh -c 'command -v uv'` answers the question in one
  line; the Dockerfile does not, because the absence of uv is precisely what it was written to
  achieve.
- **Non-Python package data shipping only by accident.** A `.sql` schema file, a `py.typed`
  marker, or any other non-Python file under a package directory may be present only because the
  build backend implicitly includes non-Python files under a named package. Declare the
  inclusion explicitly, and assert it against a **built wheel** (`python -m zipfile -l dist/*.whl`)
  rather than against the packaging config, because a config assertion passes happily when the artifact
  is already wrong.
- **A runtime stage that still has an installer.** Assert it: `command -v uv`, `command -v pip`,
  and the absence of `ensurepip`'s bundled wheels.

Where these matter, they are cheap enough to be tests: build the image in CI and run a handful
of assertions against it.

## 10. Anti-Patterns

| Anti-pattern | Risk | Correct alternative |
|---|---|---|
| Running as root | Container escape = root on host | Add non-root user, set `USER` — and enforce it at the runtime/orchestrator level, since `USER` alone is a default an invoker can override |
| `COPY . .` before installing deps | Source changes bust dep cache | Copy manifests first, install deps, then copy source |
| No `.dockerignore`, or one root `.dockerignore` assumed to scope per-image | Slow builds, potential secret leakage into layer cache, wrong exclusions applied across a multi-Dockerfile repo | Always include a `.dockerignore` at the context root; add `<dockerfile-name>.dockerignore` per Dockerfile when a repo builds more than one image from the same context |
| Secrets in `ENV` / `ARG` | Visible in `docker inspect` and image history | Use `--mount=type=secret` or runtime env injection |
| `FROM python:latest`, or a version tag treated as immutable | Non-reproducible builds, silent supply chain risk (tag) — or a frozen image that never receives security fixes (digest with no refresh process) | Pin to a version tag *and* digest, refreshed by an automated tool (Dependabot/Renovate) |
| Single fat stage | Large image, large attack surface, build tooling in prod | Multi-stage build with distroless or slim final stage |
| `uv sync --frozen` in a build | Green image silently missing a dependency the manifest declares and the lockfile does not | `uv sync --locked`, which fails and tells you to re-lock |
| Dev tooling in `[project.optional-dependencies]` | `--no-dev` cannot prune an extra, so test and lint tooling ships to production | Declare it in `[dependency-groups]` and keep `--no-dev` |
| `--no-editable` with code that derives a repo root from `__file__` parents | Path resolves inside the venv, exists, and is wrong, with no error anywhere | Explicit setting, or `importlib.resources` for packaged data |
| Trusting the Dockerfile that a property holds | A compose `uv run` against a uv-less image, or package data present only by implicit inclusion | Assert against the built image and the built wheel |
| No health signal for a service container | Runtime cannot distinguish running from working | `HEALTHCHECK` where the runtime reads it (Docker, Compose, ECS); liveness/readiness probes on Kubernetes |

## Reviewer Checklist

- [ ] Multi-stage build used — builder stage separated from final runtime stage
- [ ] Final stage uses a minimal base (distroless, `-slim`, or equivalent)
- [ ] Dependency manifests copied and deps installed before source is copied
- [ ] Non-root user created and `USER` set before `ENTRYPOINT` / `CMD` — verified via `docker inspect`, using the user-creation tool the base image actually ships (GNU `useradd` on Debian, BusyBox `adduser` on Alpine)
- [ ] Where the deployment target can enforce it, non-root is also required at the runtime/orchestrator level (e.g. a Kubernetes `runAsNonRoot`-equivalent policy) rather than relying on `USER` alone; where it can't, that's flagged as a gap
- [ ] `.dockerignore` present at the build context root and includes `.git`, `node_modules`, `.venv`, `.env*`; a repo with multiple Dockerfiles sharing a context uses per-Dockerfile `<name>.dockerignore` files where their exclusions genuinely differ
- [ ] BuildKit cache mounts (`--mount=type=cache`) used for package manager caches
- [ ] No secrets in `ENV` or `ARG` — secrets use `--mount=type=secret` or runtime injection
- [ ] Base images pinned to a specific version tag — no `latest` — and, where reproducibility matters, also pinned by digest with an automated refresh mechanism (Dependabot/Renovate) rather than a digest that will never move
- [ ] Service containers expose a health signal the target runtime reads — `HEALTHCHECK` on Docker/Compose/ECS, liveness and readiness probes on Kubernetes
- [ ] A copied virtualenv or runtime directory targets an interpreter present at the same path and version in the final stage
- [ ] Base images **and** any tool image copied from (`ghcr.io/astral-sh/uv`) are digest-pinned
- [ ] Python builds use `uv sync --locked`, not `--frozen`, and the set excluded by `--no-dev` is a `[dependency-groups]` group rather than an extra
- [ ] No installer in the runtime stage, verified by running `command -v uv` and `command -v pip` against the built image, not by reading the Dockerfile
- [ ] Non-Python package data is declared explicitly and asserted against a built wheel
- [ ] A compose file's `command` is runnable by the image it names, verified against the image
- [ ] A `--no-cache` build runs somewhere in CI, so a `.dockerignore` entry excluding a `COPY`ed file is caught before it reaches a cold builder
- [ ] Image scanning (`docker scout cves` or `trivy`) runs in CI with CRITICAL/HIGH as build failures
