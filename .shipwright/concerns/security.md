---
description: "Security: authentication, authorization/BOLA, injection (SQL, command, shell, template), path traversal, trust-boundary validation of external input, secrets handling, and output encoding/XSS — for changes that touch these paths"
triggers:
  content_patterns:
    - "dangerouslySetInnerHTML"
    - "shell\\s*=\\s*True"
    - "verify\\s*=\\s*False"
    - "(apiKey|api_key|API_KEY|api-key|secret|SECRET|Secret|password|PASSWORD|Password|token|TOKEN)\\s*[:=]\\s*['\"][^'\"]{4,}['\"]"
    - "f\"\\s*(SELECT|INSERT|UPDATE|DELETE)\\b"
    - "`\\s*(SELECT|INSERT|UPDATE|DELETE)\\b[^`]*\\$\\{"
---

# Security

Apply defence-in-depth. Every layer must be correct in isolation — never assume a caller already validated its input.

## About this concern's content triggers

The `content_patterns` above are heuristics for pulling this concern into a review, not detectors. `dangerouslySetInnerHTML`, `shell=True`, `verify=False`, a credential-shaped literal and an interpolated-SQL shape are each frequently benign in context — a fixture, a documented exception, a placeholder. They select the concern so a reviewer looks; they never assert a finding. Treat a match as "read this concern", not as "there is a vulnerability here".

## Discover the project's security tooling and conventions

Before flagging or fixing anything, check what the project already has: a static-analysis security scanner, a secret scanner, existing auth/session helpers, an existing path-containment or input-validation utility. Follow the established pattern rather than inventing a parallel one — a second validation helper living beside an existing one is itself a defect. Where no such tooling exists, flag the gap; setting it up is its own piece of work, not silent scope creep on a feature ticket.

## Never trust user input

Validate every input that crosses a trust boundary: user-entered text, messages crossing a process or privilege boundary (IPC, RPC, plugin/extension messaging), external file contents, network responses, and environment variables set outside your control.

Validate structure AND semantics: a string that parses as JSON is not automatically safe; a path that exists on disk is not automatically within the intended directory.

## Input validation: accept-list over reject-list

Validate by what is allowed, not by what is forbidden. A reject-list for path traversal (`../`, `..\\`) misses encoded and symlink-based variants; resolve-and-check — is the final real location still inside the allowed root? — is decisive.

## Path traversal / directory escape

Reject any derived path whose real, on-disk location — after resolving symlinks — is not inside the resolved real root. Two things defeat a naive version of this check: validating the path lexically instead of resolving it first (a symlink inside the root can point anywhere on disk), and comparing as a string prefix instead of by path segment (a sibling directory whose name merely starts with the root's name — `/data/foo-evil` against root `/data/foo` — passes a bare `startsWith` check). Reject before use; do not try to sanitise a bad path into a good one.

## Injection: command, query, and interpreter

Never build a command, query, path, or markup string by concatenating untrusted input. Use the interface the platform provides for passing data separately from syntax:

- **Shell/process invocation** — pass the command and its arguments as separate values to an interface that does not go through a shell, so input can never be parsed as shell syntax.
- **SQL/query** — parameterised queries or bound parameters only, never string concatenation. A column or table name that must come from user input needs a finite allow-list; parameters can bind values, not identifiers.
- **Templates, LDAP, XML, and other interpreters with their own syntax** — the same rule applies: pass data through the interface's own parameterisation, not string-building.

## Secrets hygiene

- Never log secrets (API keys, tokens, cookies, passwords, PII). Scrub them before they reach logs or stdout.
- Never commit secrets. A gitignored `.env` is necessary but not sufficient — audit before every PR. A secret that reaches version control is compromised and must be rotated, not deleted.
- Secrets in CI are environment variables; production secrets belong in a secrets manager, never in source.

## Privileged-process / untrusted-content boundary

Applies to any architecture where one process or context holds system access (filesystem, network, OS APIs) and another renders untrusted or web content — a desktop app's main/UI split, a browser extension, an embedded webview, a plugin host:

- Isolate the privileged side from the untrusted side by default; the untrusted side gets no direct access to system APIs.
- Expose only a narrow, typed bridge with individually validated calls — never the full privileged surface or the raw environment.
- Validate every payload crossing that bridge on the privileged side. The untrusted side can be compromised by injected content, so its claims are not trustworthy.
- Never evaluate dynamic code (`eval`, a `Function` constructor, or equivalent) on data from across that boundary.
- Don't load insecure or unreviewed remote content into the untrusted view.

## Authentication

- Session tokens in HTTP-only, Secure, SameSite cookies where the architecture uses cookie-based sessions — never in localStorage.
- Passwords hashed with a memory-hard password-hashing function (e.g. argon2, bcrypt), cost parameters matching current guidance — never a general-purpose hash such as plain SHA.
- Rate-limit login, password-reset, and other credential-linked endpoints.
- Log authentication events, success and failure, with scrubbed payloads.
- Use an established OAuth/OIDC library or framework feature; don't hand-roll the flow.

## Authorization

- Check authorization on every request, not just at login — an authenticated user is not an authorized user.
- Object-level permissions (can this user see this particular record?) are where most real bugs live.
- Default deny, explicit allow. Fail closed: when a check cannot be completed — dependency unreachable, timeout, malformed state — deny. Never let an error in an authorization or validation path fall through to the permissive branch.
- Least privilege by default: grant the narrowest permission, scope, and lifetime that works — file modes, tokens, database roles, CI credentials alike — and widen only with a stated reason.

## Output encoding / XSS

- Rely on the UI framework's default output escaping, and know where it stops: raw-HTML insertion, attribute injection via unescaped interpolation, URLs built from user input.
- Treat any explicit raw-HTML/raw-markup insertion of untrusted content as a decision that needs justification, not a default.
- Sanitise untrusted markup with a maintained sanitiser library rather than a hand-written one.
- Serve a restrictive content-security policy appropriate to the runtime.

## Dependencies and supply chain

Typosquatting, dependency confusion, hallucinated packages, lockfile pinning, release cooling-off, and staying current against advisories are covered in the dependencies concern — see that file rather than duplicating it here. What's specific to this concern: a known, unpatched vulnerability on a security-sensitive path (auth, crypto, parsing untrusted input) is a higher-priority finding than the same CVE somewhere inert.

## Logging and observability

- Log structured events with scrubbed fields, not raw request/response bodies.
- Correlation IDs on every log line for request tracing.
- Alert on unusual auth failure rates, unusual payload sizes at trust boundaries, and unusual external-command invocations.

## What NOT to flag

- A theoretical injection/traversal path with no reachable untrusted input — flag the pattern only where attacker-controlled data can actually reach it.
- A project's deliberate, documented choice of auth or crypto library, even if not the one you'd have picked.
- Defence-in-depth suggestions on code with no trust boundary at all — a purely internal helper never called with external input.

## Footguns

- Loose/coercing equality where strict equality is available — coercion can bypass a check silently.
- Reading or writing a file at a path built from user input without a containment check.
- A "convenience" process-spawn API that parses a single command string, instead of taking arguments as a separate list — the same shell-injection failure as building a raw command line.
- A general-purpose or predictable random source used for identifiers, tokens, or nonces instead of a cryptographically secure one.
- `alg: none`, or an unverified algorithm, accepted by a signed-token check — reject unconditionally; verify both the signing algorithm and the key.
- Error messages that leak internals (stack traces, query snippets, file paths) into production responses.
- A permissive cross-origin policy combined with credentials — never together.
