# SimError::Panic string wrapper: panic message capture, panic hook reliability, fleet reliability impact
**Review ID**: xcut-07
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/error.rs`
- `crates/hares-core/src/engine.rs`
- `crates/hares-fleet/src/fleet.rs`
- `crates/hares-python/src/py_dwelling.rs`
- `crates/hares-python/src/py_gym.rs`

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: high] No custom panic hook installed — default Rust backtrace is printed per panic, swamping logging

**Description**: The codebase relies entirely on `std::panic::catch_unwind` for panic isolation. No custom panic hook is installed via `std::panic::set_hook`. When a panic occurs, the Rust default panic hook prints a full backtrace to stderr before the panic is caught by `catch_unwind`. In fleet simulations with thousands of dwellings and potentially many panics (see Finding 7), this floods stderr with backtraces and can cause significant I/O overhead, degrading fleet performance. Additionally, the default hook output interleaves with structured tracing/logging output, making it hard to correlate panic messages with the `SimError::Panic` entries in the result vector.

**Code Location**: No `std::panic::set_hook` or `std::panic::take_hook` call exists anywhere in the codebase. Panic catching occurs at:
- `crates/hares-core/src/engine.rs:116` — `run()` method
- `crates/hares-core/src/engine.rs:211` — `run_dwelling()` method
- `crates/hares-fleet/src/fleet.rs:235` — `simulate_parallel()` per-entry
- `crates/hares-fleet/src/fleet.rs:317` — `SteppableFleet::from_configs()` per-dwelling construction
- `crates/hares-fleet/src/fleet.rs:519` — `step_dwellings_parallel()` per-dwelling step
- `crates/hares-python/src/py_dwelling.rs:587` — `PyDwelling::simulate()`
- `crates/hares-python/src/py_dwelling.rs:608` — `PyDwelling::step()`
- `crates/hares-python/src/py_gym.rs:98` — `batch_step_py()`

**Root Cause**: No panic hook is configured. The default Rust hook runs before `catch_unwind` captures the panic, producing stderr output.

**Impact**: In a 10,000-dwelling fleet simulation, each panic prints a multi-line backtrace to stderr, creating noise that degrades log readability and adds I/O latency. If panics are frequent (e.g., numeric edge cases), this can measurably slow the fleet run.

### Finding 2: [Severity: high] `AssertUnwindSafe` suppresses legitimate `UnwindSafe` violations on `&mut Dwelling` — corrupted dwelling state is silently reused

**Description**: `step_dwellings_parallel` (`crates/hares-fleet/src/fleet.rs:519`) wraps `dwelling.step()` in `AssertUnwindSafe(|| dwelling.step())`. `&mut Dwelling` is not `UnwindSafe` because Rust's `UnwindSafe` auto-trait is designed to catch types whose logical invariants could be violated by a panic mid-mutation. By using `AssertUnwindSafe`, the code suppresses this safety check. After a panic is caught by `catch_unwind`, the `Dwelling` instance may have partially updated internal state (e.g., ports half-zeroed, solvers mid-integration, equipment state inconsistent). The `SteppableFleet::step()` method (`crates/hares-fleet/src/fleet.rs:390`) does **not** exclude previously-failed dwellings from subsequent steps. It always steps all dwellings. This means a dwelling that panicked in step N will be stepped again in step N+1 with potentially corrupted state, leading to cascading failures or silently incorrect results.

`Dwelling::from_config` wrapping with `AssertUnwindSafe` (`crates/hares-fleet/src/fleet.rs:317`) is sound because construction failure prevents the `Dwelling` from being added to the fleet. But the per-step wrapping is unsound for the dwelling's logical invariants.

**Code Location**:
- `crates/hares-fleet/src/fleet.rs:519` — `AssertUnwindSafe(|| dwelling.step())`
- `crates/hares-fleet/src/fleet.rs:390-411` — `SteppableFleet::step()` always steps all dwellings
- `crates/hares-core/src/dwelling/mod.rs:2210` — `run_timestep` guard checks only `clock.finished`, not previous failure state

**Root Cause**: No mechanism exists to mark a dwelling as permanently failed after a panic. The `AssertUnwindSafe` wrapper makes the compiler accept the code, but the semantic unsafety of re-stepping a panic-corrupted dwelling is unaddressed.

**Impact**: A dwelling that panics once will be retried on every subsequent step, producing repeated `SimError::Panic` results. Worse, if the panic left the dwelling in a subtly inconsistent state that does _not_ trigger another panic, the dwelling produces silently wrong results that are aggregated into fleet metrics.

### Finding 3: [Severity: high] `panic_payload_to_string` discards file/line/context information from assertion panics

**Description**: The `panic_payload_to_string` functions (duplicated across three crates: `crates/hares-core/src/engine.rs:401`, `crates/hares-fleet/src/fleet.rs:586`, `crates/hares-python/src/py_dwelling.rs:1799`) only downcast the panic payload to `&'static str` or `String`, then return the raw string. Rust's default panic hook formats the message with file, line number, column, and optional `Debug` payload (for `assert!`, `assert_eq!`, `unwrap`, `expect`). When `panic!("some message")` is used, the payload *is* a `String` containing the formatted message with file and line. However, for `assert!(condition)` macros, the payload is `&'static str "assertion failed: condition"` — the file and line come from the panic *location* metadata, not the payload string. The current `catch_unwind` + `downcast_ref` pattern only captures the payload string, losing the `PanicInfo::location()` file/line/column context that would be available in a custom panic hook.

**Code Location**:
- `crates/hares-core/src/engine.rs:401-409` — `panic_payload_to_string`
- `crates/hares-fleet/src/fleet.rs:586-594` — `panic_payload_to_string`
- `crates/hares-python/src/py_dwelling.rs:1799-1806` — `panic_payload_to_string`

**Root Cause**: The standard `catch_unwind` mechanism does not propagate `PanicInfo` (which carries `Location`). The code captures only the string payload, not the structured location metadata. A custom panic hook would be needed to capture `PanicInfo` into a thread-local or similar mechanism.

**Impact**: Panic messages in fleet results lack file and line numbers, making root-cause debugging of recurring panics much harder. An operator must grep the entire source tree for the assertion text rather than having a direct pointer to the failing line.

### Finding 4: [Severity: medium] No double-panic protection in error-handling paths

**Description**: After `catch_unwind` captures a panic, the error handling code performs fallible operations that could themselves panic, triggering a double-panic that aborts the process. Specifically:

1. `crates/hares-fleet/src/fleet.rs:256-258` — After catching a panic in `simulate_parallel()`, tracing warn is called with `%err` formatting. If `err` has a `Display` implementation that panics (unlikely but possible), the process aborts.

2. `crates/hares-core/src/engine.rs:126-186` — After catching a panic, `warnings.push(format!(...))` allocates a `String`. If the allocator is corrupted due to a prior near-panic, this allocation could panic, causing a double-panic and process abort.

3. `crates/hares-fleet/src/fleet.rs:246-247` — After catching a panic, `completed.fetch_add(1, Ordering::Relaxed)` is called. `AtomicUsize::fetch_add` is allocation-free and wont panic, so this path is safe.

**Code Location**:
- `crates/hares-core/src/engine.rs:169,253` — `warnings.push(format!(...))`
- `crates/hares-core/src/engine.rs:179-186` — `SimulationResults` construction
- `crates/hares-fleet/src/fleet.rs:256-258` — `tracing::warn!` with error formatting
- `crates/hares-fleet/src/fleet.rs:249-252` — `SimError::Panic` construction with `panic_payload_to_string`

**Root Cause**: The error-handling code after `catch_unwind` is not guaranteed panic-free. While the specific allocation paths are likely to succeed under normal conditions, corrupted allocator state from a prior panic (heap metadata corruption, double-free) can cause `String::clone` or `format!` to panic, triggering a process abort.

**Impact**: In a fleet of 10,000 dwellings, a single dwelling panic with allocator corruption can abort the entire fleet run. This defeats the purpose of per-dwelling panic isolation.

### Finding 5: [Severity: medium] Mutex poison not handled — a panic in a Python-threaded dwelling step poisons shared fleet state

**Description**: In the Python-facing code, `PyDwelling` wraps `Dwelling` in a `Mutex<Dwelling>` (`crates/hares-python/src/py_dwelling.rs:515`) and `PySteppableFleet` wraps `SteppableFleet` in a `Mutex<SteppableFleet>` (`crates/hares-python/src/py_fleet.rs:354`). If a panic occurs while the mutex is held (i.e., during `step_core()` inside `catch_unwind`), the mutex becomes *poisoned*. The lock functions (`lock_dwelling`, `lock_dwelling_string`, `lock_dwelling_hares`) correctly return an error on poisoned mutexes (`crates/hares-python/src/py_dwelling.rs:480-510`). However, in the Gym environment's `batch_step_py` (`crates/hares-python/src/py_gym.rs:98`), the `step_core_string` call internally locks the dwelling mutex. If that mutex is poisoned, `lock_dwelling_string` returns an `Err(String)`, which propagates up through `step_core_string` and is caught by `catch_unwind` as a normal error. But the mutex remains poisoned, and subsequent steps on that dwelling will continue to fail with "dwelling state is corrupted" errors until the dwelling is recreated.

The `SteppableFleet` mutex (`crates/hares-python/src/py_fleet.rs:354`) is more concerning: if any dwelling step panics while the fleet lock is held during `fleet.step()`, the entire fleet becomes inaccessible because the lock is poisoned. Actually, examining the code more closely: the fleet lock is released before stepping individual dwellings (since `fleet.step()` calls `step_dwellings_parallel` which doesn't hold any fleet-level lock), so fleet-level mutex poison is unlikely. But the per-dwelling mutex poison is a real concern.

**Code Location**:
- `crates/hares-python/src/py_dwelling.rs:515` — `pub(crate) dwelling: Mutex<Dwelling>`
- `crates/hares-python/src/py_fleet.rs:354` — `fleet: Mutex<SteppableFleet>`
- `crates/hares-python/src/py_dwelling.rs:479-510` — lock functions with poison detection
- `crates/hares-python/src/py_gym.rs:88-150` — Gym batch stepping

**Root Cause**: `catch_unwind` catches the panic but `Mutex::lock()` on a poisoned mutex still returns `PoisonError`. Once poisoned, a mutex remains poisoned permanently.

**Impact**: In Gym/RL training, a single panic poisons the dwelling's mutex, causing all subsequent steps on that dwelling to fail. The RL agent must detect and handle this, or training stalls. In the worst case, if the fleet-level mutex is poisoned, the entire training process cannot make progress.

### Finding 6: [Severity: medium] No `timestep` or structured `error_code` in `SimError::Panic` — fleet error aggregation is purely string-based

**Description**: The fleet-level `SimError::Panic` variant (`crates/hares-fleet/src/fleet.rs:81-82`) carries `bldg_id: i64` and `message: String`, but no `timestep` or structured error code. When analyzing fleet panics, an operator must parse free-text error messages to identify patterns. A structured field like `current_step: u64` would allow aggregating panics by timestep (e.g., "all panics cluster at hour 8760, step 525600") and identifying temporal patterns. An `error_code: &'static str` would enable programmatic categorization (e.g., `division_by_zero`, `invariant_violation`, `bounds_check`).

The `hares-types` `SimError::Panic(String)` variant (`crates/hares-types/src/error.rs:37`) is even less structured — a bare `String` with no dwelling ID or timestep. This variant appears to be unused in the main code paths (the fleet and engine define their own `SimError` and `SimStatus` types respectively), but it exists as a public API type and could confuse consumers.

**Code Location**:
- `crates/hares-types/src/error.rs:36-37` — `SimError::Panic(String)` (bare string, unused in production)
- `crates/hares-fleet/src/fleet.rs:81-82` — `SimError::Panic { bldg_id: i64, message: String }` (no timestep)
- `crates/hares-core/src/engine.rs:184,268` — `SimStatus::Failed(panic_payload_to_string(payload))` (no structured data)

**Root Cause**: Error types evolved incrementally; `bldg_id` was added to fleet `SimError` but `timestep` and `error_code` were not.

**Impact**: Fleet operators must grep/search raw text to triage failure modes. Temporal clustering of panics (a key diagnostic signal) requires manual correlation.

### Finding 7: [Severity: medium] Fleet aggregator correctly excludes failed dwellings from timeseries but not from per-dwelling metrics

**Description**: The `aggregate` function in `crates/hares-fleet/src/aggregation.rs:131` constructs `per_dwelling_metrics` for all dwellings, including failed ones. The `failed` flag (`DwellingMetrics.failed: bool`, line 27) is correctly set based on `SimStatus`. Failed dwellings are excluded from the weighted aggregate timeseries (line 149: `if matches!(outcome.status, SimStatus::Failed(_)) { continue; }`). However, `per_dwelling_metrics` includes both successful and failed dwellings as separate entries. This is sound behavior — it allows fleet reports to show failure statistics — but note that in the `DwellingMetrics` struct, the energy/power values for failed dwellings come from `SimulationResults` which may contain zero-metric placeholders (e.g., `empty_metrics()` at `crates/hares-core/src/engine.rs:173`). There is no structured "failed" sentinel for these scalar values; they appear as zero-energy results, which an incautious consumer might mistake for a low-consumption dwelling rather than a failure.

**Code Location**:
- `crates/hares-fleet/src/aggregation.rs:131-145` — per-dwelling metrics construction
- `crates/hares-fleet/src/aggregation.rs:148-150` — failed dwelling exclusion from timeseries
- `crates/hares-core/src/engine.rs:173` — `empty_metrics()` for failed dwelling results

**Root Cause**: `SimulationResults.metrics` is always populated, even for failed runs, with `empty_metrics()` (zeros). There is no `Option<SimulationMetrics>` or explicit failure sentinel in the metrics fields.

**Impact**: A fleet report showing per-dwelling energy consumption could mislead users into thinking a failed dwelling consumed 0 kWh, when in reality it failed before producing any output. The `failed` flag is present but could be overlooked.

### Finding 8: [Severity: low] `panic_payload_to_string` is duplicated across three crates

**Description**: The same `panic_payload_to_string` function exists identically (or near-identically) in three crates:
- `crates/hares-core/src/engine.rs:401-409`
- `crates/hares-fleet/src/fleet.rs:586-594`
- `crates/hares-python/src/py_dwelling.rs:1799-1806`

The `hares-core` and `hares-fleet` implementations are byte-for-byte identical. The `hares-python` version wraps the message with `"HARES internal panic: {msg}"`. This duplication violates DRY and means any future improvements (e.g., capturing `PanicInfo::location()`, adding structured metadata) must be applied in three places.

**Code Location**:
- `crates/hares-core/src/engine.rs:401-409`
- `crates/hares-fleet/src/fleet.rs:586-594`
- `crates/hares-python/src/py_dwelling.rs:1799-1806`

**Root Cause**: The function is a small utility; no shared utility crate was established for it.

**Impact**: Maintenance overhead. The inconsistency between the `hares-python` version (which prepends `"HARES internal panic: "`) and the others means the same panic produces different string representations depending on the entry point, complicating log grep/aggregation.

### Finding 9: [Severity: low] `DefaultHooks` interaction — if a dependency installs a panic hook, HARES has no defense

**Description**: Since HARES does not install a panic hook, any dependency (or user code) that calls `std::panic::set_hook` will change the panic reporting behavior for the entire process. If a dependency installs a panic hook that aborts (e.g., `std::panic::set_hook(Box::new(|_| std::process::abort()))`), the `catch_unwind` wrappers become useless — the process aborts before `catch_unwind` executes. In the Rust ecosystem, some profiling, fuzzing, or error-reporting libraries install global panic hooks. HARES has no `take_hook` / `set_hook` dance to mitigate this.

**Code Location**: No panic hook installation or restoration code exists in the codebase.

**Root Cause**: No sandboxing of the panic hook.

**Impact**: In environments where dependencies install aborting panic hooks (e.g., `cargo-fuzz`, some profiling frameworks), fleet simulations abort on the first dwelling panic instead of quarantining the dwelling.

## Summary
- Total findings: 9
- Critical: 0
- High: 3
- Medium: 4
- Low: 2

## Recommendations

1. **Install a per-thread custom panic hook** that suppresses default backtrace output and instead stores the `PanicInfo` (with file, line, and payload) in a thread-local cell. Use `std::panic::set_hook` + `std::panic::take_hook` to install before simulation, restore after. This addresses Finding 1, Finding 3, and Finding 9 simultaneously.

2. **Mark dwellings as permanently failed after a panic** by adding a `failed: bool` field to `Dwelling` (or the fleet's per-dwelling state). Before stepping, check this flag and skip. The `SteppableFleet::step()` return should include a `Skipped` variant for failed dwellings. Addresses Finding 2.

3. **Add a `current_step: u64` field to `SimError::Panic`** (and to `SimError::Engine`, `SimError::Failed`) in the fleet `SimError` enum. This enables temporal clustering analysis of panics. Consider adding an `error_code: Option<&'static str>` for programmatic categorization. Addresses Finding 6.

4. **Move `panic_payload_to_string` to `hares-types`** as a single shared utility, and in the fleet layer, also accept `PanicInfo` via a thread-local from the custom hook to include file/line context. Addresses Finding 8 and Finding 3.

5. **Add a `failed_dwellings: Vec<(i64, String, u64)>` field to `FleetResults`** (`crates/hares-fleet/src/aggregation.rs`) to surface failure statistics explicitly, rather than burying them as zero-metric per-dwelling entries. Addresses Finding 7.

6. **Audit post-`catch_unwind` error handling for allocation safety.** The most critical path is `crates/hares-core/src/engine.rs:169-186` and `crates/hares-core/src/engine.rs:252-271` where `warnings.push(format!(...))` and `SimulationResults` construction allocate. Consider pre-allocating warning strings or using a static fallback message on allocation failure. Addresses Finding 4.

7. **For mutex poison in Python environments:** after `catch_unwind` catches a panic in `PyDwelling::step()` or `PyDwelling::simulate()`, check whether the internal mutex is poisoned. If so, mark the dwelling as permanently unusable and surface this to Python as a distinct error type (e.g., `FatalDwellingError`), rather than retrying on each step. Addresses Finding 5.

## References / Citations
- Rust Reference: `std::panic::catch_unwind` and `UnwindSafe` semantics — https://doc.rust-lang.org/std/panic/fn.catch_unwind.html
- Rust Reference: `std::panic::set_hook` and `PanicInfo` — https://doc.rust-lang.org/std/panic/fn.set_hook.html
- Rust Reference: `std::sync::Mutex` poison semantics — https://doc.rust-lang.org/std/sync/struct.Mutex.html#poisoning
- Rust Reference: Double-panic behavior (process abort) — https://doc.rust-lang.org/std/panic/index.html
