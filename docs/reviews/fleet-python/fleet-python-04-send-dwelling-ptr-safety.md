# Python binding: unsafe SendDwellingPtr safety audit
**Review ID**: fleet-python-04
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
crates/hares-python/src/py_gym.rs crates/hares-python/src/conversions.rs

## Vendor/Reference Files Consulted
N/A (PyO3 documented patterns for `py.detach` and `Py<T>` thread safety served as the reference; no vendor code provided.)

## Findings

### Finding 1: [Severity: low]
**Description**: The `unsafe impl Sync for SendDwellingPtr` safety justification is undocumented in the doc comment for `SendDwellingPtr`. The existing safety comment at `py_gym.rs:36-40` only describes the *lifetime* invariant (that `Py<PyDwelling>` handles outlive the wrapper) and mentions that `step_core` and `observation` lock `Mutex<Dwelling>`. It does not explain why `Sync` (concurrent `&SendDwellingPtr` sharing across threads, as required by Rayon's `par_iter`) is sound.

**Code Location**: `crates/hares-python/src/py_gym.rs:42-43`

**Root Cause**: `Sync` on a `*const T` pointer wrapper requires that concurrent shared references to the pointer (which Rayon threads each copy and dereference) produce sound concurrent `&T` access to the same allocation. The safety depends on all mutable state in `PyDwelling` being behind `Mutex<Dwelling>` (`py_dwelling.rs:514-524`), so concurrent `&self` method calls (`step_core_string`, `observation`) serialize through the mutex. The `Send` justification at lines 37-40 touches on this indirectly but never mentions `Sync` explicitly.

**Impact**: Reader cannot verify that the `Sync` impl was deliberate and reasoned about. The code is in fact sound because (a) `PyDwelling` is already `Sync` (enforced by the compiler when `simulate(&self)` at `py_dwelling.rs:586` captures `&PyDwelling` in a `py.detach(|| ...)` closure), and (b) all methods called through the raw pointer in the Rayon section (`step_core_string`, `observation_for_fields`, `observation`) take `&self` and lock `Mutex<Dwelling>` internally.

### Finding 2: [Severity: low]
**Description**: The safety comment at `py_gym.rs:92-94` enumerates the called methods that are GIL-free (`step_core()` and `observation()`) but omits `observation_for_fields()` (line 23-30, called at line 115). The `observation_for_fields` function also acquires `lock_dwelling_string(&dwelling.dwelling)`, so it is equally GIL-free, but it is not mentioned in the safety justification.

**Code Location**: `crates/hares-python/src/py_gym.rs:92-94`

**Root Cause**: Comment written before `observation_for_fields` was added or during an edit where the list wasn't updated. The behavior is safe—`observation_for_fields` follows the same `Mutex<Dwelling>` locking pattern—but the inconsistency weakens the justification for future reviewers.

**Impact**: Low. No actual unsoundness; this is a documentation drift issue. If a future method is added to the Rayon section that does NOT lock the mutex, a reviewer reading the comment might incorrectly trust it.

### Finding 3: [Severity: medium]
**Description**: The `PyRef<'_, PyDwelling>` borrows created at line 78-79 are held across `py.detach()` at line 88. While this is currently sound because the closure only captures `ptrs` (not `borrows`), and the compiler correctly rejects capturing `PyRef` in a `Send` closure, the pattern is subtle and the safety comment at line 84 (`// borrows live until end of function scope`) is a trailing single-line comment that is easy to miss during refactoring.

**Code Location**: `crates/hares-python/src/py_gym.rs:78-84`

**Root Cause**: The two-step extraction—create GIL-bound `PyRef` borrows, extract raw pointers, release GIL—is structurally correct but relies on the borrow checker preventing `borrows` from being moved/captured by the `detach` closure. A well-intentioned refactor that moves `borrows` inside the closure (e.g., wrapping the entire function body in `detach`) would be rejected by the compiler because `PyRef<'_, PyDwelling>` is `!Send`. This is actually a *strength* of the pattern (compiler-enforced), but the documentation does not explain this safety net, leaving it as institutional knowledge.

**Impact**: Medium. If a future maintainer misunderstands the pattern and restructures the code (e.g., extracting the raw pointer logic into a helper function that drops `borrows` early), the compiler would NOT catch lifetime violations because the raw pointer bypasses the borrow checker. The code is sound as written, but the implicit contract is fragile.

### Finding 4: [Severity: low]
**Description**: The action-mapping guardrail at `py_gym.rs:68-74` rejects non-empty actions with `PyNotImplementedError`. However, the rejection does not distinguish between a partially-implemented action handler that might produce invalid control signals and the current "TODO" state where *all* non-empty actions are rejected. The error message at line 70-72 tells users to call `dwelling.apply_control()` before `batch_step` and pass empty action vectors. There is no validation that `apply_control()` was actually called, so a user who passes empty actions without first applying controls gets silently stale simulation state.

**Code Location**: `crates/hares-python/src/py_gym.rs:68-74`

**Root Cause**: The two-step control application API (`apply_control()` then `batch_step` with empty actions) is inherently prone to misuse. The guardrail correctly prevents the unimplemented action-mapping code path from running, but it cannot enforce the prescribed workflow.

**Impact**: Low. No memory safety concern—stale control state is a simulation-correctness issue, not a safety issue. The simulation simply runs with existing equipment setpoints. The `NotImplementedError` ensures no invalid control signals are ever generated from partial implementations.

### Finding 5: [Severity: low]
**Description**: The `SendDwellingPtr` type wraps `*const PyDwelling` (a raw pointer), but the unsafe block at line 95 dereferences it to produce a `&PyDwelling` reference with essentially unbounded lifetime. The safety comment asserts this is safe because the `Py<PyDwelling>` handles and `PyRef` borrows keep the object alive, but there is no explicit documentation that Python GC cannot collect the `PyDwelling` while the parallel section runs.

**Code Location**: `crates/hares-python/src/py_gym.rs:95`

**Root Cause**: The `dwellings: Vec<Py<PyDwelling>>` parameter holds `Py<T>` handles that increment the Python reference count. Since the `Vec` lives for the entire function, the reference count stays above zero, preventing GC. This is a well-known PyO3 guarantee (`Py<T>` owns a Python reference), but it is not restated in the safety comment.

**Impact**: Low. The safety guarantee is correct but implicit. A reader unfamiliar with PyO3's reference-counting semantics might wonder whether GC could invalidate the pointer.

## Summary
- Total findings: 5
- Critical / High / Medium / Low: 0 / 0 / 1 / 4

### Safety Assessment
The `unsafe impl Send` and `unsafe impl Sync` for `SendDwellingPtr` are **sound**. The implementation correctly:

1. **Lifetime safety**: `Py<PyDwelling>` handles (reference-counted, preventing Python GC) and `PyRef<'_, PyDwelling>` borrows (preventing mutable aliasing/teardown) are both held for the entire function (`py_gym.rs:77-84`). The raw pointers are derived from valid borrows and only dereferenced while those borrows are alive.

2. **Thread safety (`Sync`)**: `PyDwelling` is already `Sync` (enforced by the compiler via `simulate(&self)` at `py_dwelling.rs:586` needing `&PyDwelling: Send` in `py.detach`). All mutable state in `PyDwelling` is behind `Mutex<Dwelling>` (`py_dwelling.rs:514-515`). Methods called through the raw pointer (`step_core_string`, `observation`, `observation_for_fields`) all take `&self` and acquire the mutex internally, serializing concurrent access.

3. **No use-after-free path**: The `dwellings` parameter holds reference-counted `Py<T>` handles for the entire function scope, preventing Python garbage collection of the underlying objects during the parallel section.

4. **Action mapping guardrail**: Non-empty actions are rejected at the API boundary (`py_gym.rs:68-74`), preventing any partially-implemented control signal generation from corrupting simulation state.

The findings are limited to documentation gaps—the safety invariants are correct but not exhaustively documented. The `Sync` justification, the GC-invariance guarantee, and the exact list of GIL-free methods called through the pointer would benefit from explicit documentation.

## Recommendations
1. Expand the `SendDwellingPtr` doc comment at `py_gym.rs:33-40` to explicitly document why `Sync` is sound: `PyDwelling` is `Sync`, all concurrent `&self` method calls serialize through `Mutex<Dwelling>`, and `Sync` is required for Rayon's `par_iter`.
2. Add `observation_for_fields` to the list of GIL-free methods in the safety comment at `py_gym.rs:92-94`.
3. Add a sentence to the safety comment at `py_gym.rs:92-94` stating that `Py<PyDwelling>` handles prevent Python GC during the parallel section.
4. Consider extracting the raw-pointer derivation logic into a small helper function (e.g., `fn dwelling_ptrs(dwellings: &[Py<PyDwelling>], py: Python<'_>) -> (Vec<PyRef<'_, PyDwelling>>, Vec<SendDwellingPtr>)`) that returns both the borrows and pointers together, making the lifetime coupling explicit in the type system and preventing partial drops.

## References / Citations
- PyO3 documentation on `Python::detach`: requires `Send` closure; `PyRef` is `!Send`, so `borrows` cannot be captured in the detach closure—this is a compiler-enforced safety net.
- PyO3 `Py<T>`: owns a Python reference; dropping decrements the reference count. As long as the `Vec<Py<PyDwelling>>` is alive, the underlying objects cannot be garbage collected.
- `py_dwelling.rs:585-603` (`simulate`): demonstrates the same `py.detach` pattern with `&self`, proving that `PyDwelling` is compiler-verified `Sync`.
- `py_dwelling.rs:1338-1357`: `step_core` and `observation` both take `&self` and lock `Mutex<Dwelling>` internally, confirming no thread-unsafe access through the raw pointer.
