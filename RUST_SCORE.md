# Rust Best Practices Score: `itr`

Scored against [The Comprehensive Rust Best Practices Reference](the_comprehensive_rust_best_practices_reference.md).
Codebase: ~7,500 LOC, single binary, synchronous CLI, SQLite-backed, no network I/O.

---

## Overall Score: **B+ (82/100)**

| Section | Score | Weight | Weighted |
|---------|-------|--------|----------|
| Ownership & Borrowing | 90/100 | 20% | 18.0 |
| Error Handling | 85/100 | 20% | 17.0 |
| API Design | 78/100 | 20% | 15.6 |
| Performance & Memory | 72/100 | 15% | 10.8 |
| Code Organization | 88/100 | 15% | 13.2 |
| Testing & Tooling | 74/100 | 10% | 7.4 |
| **Total** | | **100%** | **82.0** |

---

## 1. Ownership & Borrowing — 90/100

### What the guide says
> Start with owned data in structs, borrow in function parameters. Reach for smart pointers or interior mutability only when your design genuinely requires it.

### What we do

**GOOD (follows guide):**

- **Structs own their data.** All models (`Issue`, `IssueDetail`, `IssueSummary`, etc.) use `String`, `Vec<String>`, `i64` — appropriate for serde serialization and DB row mapping. (`models.rs`)
- **Functions borrow parameters.** Consistent `&str`, `&[String]`, `&Connection` across all command handlers and `db.rs`. Example: `insert_issue()` takes `&str` for all string fields. (`db.rs:186-198`)
- **`as_deref()` used correctly.** `main.rs` converts `Option<String>` to `Option<&str>` before passing to DB layer. (`main.rs:72`)
- **No unnecessary smart pointers.** `Box<dyn ToSql>` used only where rusqlite requires trait objects. No `Rc`, `Arc` (not needed — single-threaded, no shared ownership). (`db.rs:245`)
- **Interior mutability justified.** Single `RefCell` in a `thread_local!` for `--fields` filter state — correct pattern for single-threaded thread-local. (`format.rs:7-8`)
- **No explicit lifetimes.** None needed — Rust's elision rules handle all cases. Functions are short-lived borrows with no complex reference hierarchies.

**ACCEPTABLE (pragmatic trade-offs):**

- **58 `.clone()` calls total.** Most are in non-hot paths (stats aggregation, batch processing). The notable concentration is `build_issue_summary()` with 11 consecutive `.clone()` calls per issue — called in list/ready/search paths. (`commands/mod.rs:44-57`)
- **`Cow<str>` not used.** The guide recommends it for data that "often passes through unmodified." Not applicable here — DB rows produce owned `String`s, and the CLI doesn't do passthrough processing.

**NOT APPLICABLE:**

- `mem::take`/`mem::replace` — no state machine transitions or move-out-of-`&mut` patterns.
- Entry API — used correctly in the one HashMap aggregation path (`stats.rs:40-60`).

### Practical impact of full adherence
Eliminating the 11-clone concentration in `build_issue_summary()` would require either `Cow<str>` in `IssueSummary` (complicates serde) or changing `build_issue_detail()` to take `&Issue` instead of `Issue` (cascading signature changes). For a CLI processing <1000 issues, **performance impact: negligible**. The clones copy small strings (status names, short titles). Not worth the complexity.

---

## 2. Error Handling — 85/100

### What the guide says
> Use `thiserror` for modules where callers match on variants; `?` for propagation; `unwrap()` only for proven invariants; separate `main()` from `run()`.

### What we do

**GOOD:**

- **`thiserror` error enum.** `ItrError` uses `#[derive(thiserror::Error)]` with `#[from]` for `rusqlite::Error`, `serde_json::Error`, and `std::io::Error`. Each variant has a machine-readable error code. (`error.rs:3-35`)
- **`?` operator used consistently.** 267 uses across all files. All command handlers return `Result<(), ItrError>` and propagate cleanly.
- **`main()`/`run_command()` separation.** `main()` (39-88) handles parsing and DB discovery. `run_command()` (90-456) dispatches to handlers. Errors bubble up and are handled by `handle_error()` which prints to stderr and exits. (`main.rs`)
- **No bare `unwrap()` on I/O.** Every `unwrap()` has a fallback: `unwrap_or_default()`, `unwrap_or_else()`, or `unwrap_or()`. Zero naked `unwrap()` calls on user input or I/O operations.
- **Soft fallbacks philosophy.** Invalid priority/kind/status values default to safe values with `REVIEW:` stderr notes instead of hard errors. Reference implementation in `add.rs:110-129` and `normalize.rs`.

**ACCEPTABLE:**

- **`process::exit()` used in 2 places.** `handle_error()` (`error.rs:77`) and format validation (`main.rs:47`). Both are terminal — no open file handles or incomplete transactions. The guide prefers `ExitCode` return, but this pattern (BurntSushi-style) is explicitly acknowledged as widely used.
- **No `.context()` chaining.** The project uses `thiserror` (not `anyhow`), so `.context()` isn't available. Error variants carry their own context (`NotFound(i64)`, `InvalidValue { field, value, valid }`). Adequate for a small CLI.

**NEEDS IMPROVEMENT:**

- **DB query failures silently default to `false`.** `blocks_active_issues()` and `is_blocked()` use `.unwrap_or(false)` — real DB errors (corruption) are masked. Should emit `REVIEW:` note to stderr. (`urgency.rs:136,143,169`)
- **FTS5 failures silently ignored.** `let _ = conn.execute_batch(...)` discards errors when FTS5 is unavailable. Acceptable (FTS5 is optional) but a debug log would help diagnosis. (`db.rs:1001,1019`)

### Practical impact of full adherence
Adding `REVIEW:` notes for masked DB errors is a small change (~5 lines per site) with zero performance impact. Switching from `process::exit()` to `ExitCode` return would require refactoring `handle_error()` to return instead of exit — moderate effort, no performance impact, marginal safety improvement (destructors already run properly in current paths).

---

## 3. API Design — 78/100

### What the guide says
> Newtype pattern for validation. Builder pattern for >5 parameters. Accept broad, return specific. Derive standard traits eagerly. Make illegal states unrepresentable.

### What we do

**GOOD:**

- **RFC 430 naming.** `CamelCase` types, `snake_case` functions, `SCREAMING_SNAKE` constants. No violations found.
- **Standard traits derived.** All public structs have `#[derive(Debug, Clone, Serialize, Deserialize)]`. `Format` enum additionally derives `Copy, PartialEq`. (`models.rs`, `format.rs:56-78`)
- **Enum for closed sets.** `Format` is a proper enum with exhaustive matching. `Commands`, `BatchAction`, `BulkAction`, `ConfigAction` use clap's derive enums. (`cli.rs`, `format.rs`)
- **Trait design is minimal and focused.** One custom trait: `HasUrgency` with a single method, used for generic sorting. Two implementors. (`commands/mod.rs:71-81`)

**ACCEPTABLE (intentional deviations):**

- **No newtype pattern for IDs, priority, status, kind.** These are raw `i64` and `String`. The guide says newtypes prevent type confusion. **Exception called out:** the soft-fallbacks philosophy explicitly requires accepting bad input and defaulting gracefully — newtypes with validating constructors would reject input at parse time, which conflicts with the design goal of never failing on agent input when a default exists. Runtime validation via `normalize.rs` + `REVIEW:` notes is the intentional alternative.
- **Strings for status/priority/kind instead of enums.** The guide prefers enums for closed sets. **Exception called out:** SQLite `CHECK` constraints enforce validity at the storage layer. The normalization layer maps synonyms (`urgent`->`critical`, `wip`->`in-progress`). Using String allows soft fallbacks — an enum would force a hard parse error. This is a conscious trade-off documented in CLAUDE.md.
- **No `impl Into<String>` or `impl AsRef<Path>`.** Functions accept concrete types. For a binary crate (not a library), this is appropriate — no external callers need generic ergonomics.

**NEEDS IMPROVEMENT:**

- **High-arity functions.** `list_issues()` has 12 parameters (`db.rs:314`). `add::run()` has 15 parameters (`commands/add.rs:13`). Both use `#[allow(clippy::too_many_arguments)]`. A `ListFilter` or `AddOptions` struct would improve readability and extensibility.
- **No `pub(crate)` annotations.** All module-internal functions use default visibility. For a small binary crate this works, but explicit visibility would document intent.
- **`PartialEq`/`Eq` not derived on models.** Not commonly needed in this codebase, but would enable easier testing assertions.

### Practical impact of full adherence
- **FilterOptions struct:** Moderate refactor (~50 lines changed across `db.rs`, `list.rs`, `main.rs`). Zero performance impact. Would make adding new filter fields easier.
- **Newtypes for priority/status/kind:** Large refactor (~200+ lines). Would **negatively impact** the soft-fallbacks philosophy — the whole point is accepting messy input. **Not recommended.**
- **`pub(crate)` annotations:** Trivial (~30 one-line changes). Zero performance impact. Documentation-only benefit.

---

## 4. Performance & Memory — 72/100

### What the guide says
> Use `with_capacity()` when size is known. Avoid unnecessary `format!()`. Configure release profile (LTO, codegen-units). Reuse collections.

### What we do

**GOOD:**

- **No async.** Synchronous code is correct for a local CLI with no network I/O. The guide explicitly says: "many CLI tools work perfectly with synchronous code."
- **Iterator chains used idiomatically.** `.filter().map().collect()` throughout. No manual loops where iterators would suffice. (`db.rs:370-397`, `util.rs:2-14`)
- **Minimal dependency footprint.** 6 direct dependencies, no async runtime, no network crates. Compile times are fast (~3.5s release).

**ACCEPTABLE:**

- **No `with_capacity()` anywhere.** Vectors are allocated with `Vec::new()` then grown dynamically. For typical issue counts (<1000), reallocation overhead is negligible. The guide says this matters for "known size" — most vectors here have unknown size (DB query results).
- **160 `format!()` calls, 174 `.to_string()` calls.** Many are unavoidable (building SQL placeholders, output strings). Some could be replaced with string literals or `write!()` to a buffer, but the guide acknowledges "profile before optimizing."

**NEEDS IMPROVEMENT:**

- **No release profile tuning.** `Cargo.toml` has no `[profile.release]` section. Defaults: `lto=false`, `codegen-units=16`, no strip. Adding `lto=true`, `codegen-units=1`, `strip=true` would reduce binary size (~50%) and potentially improve startup time. Easy 3-line addition.
- **`build_issue_summary()` clones 11 fields per issue.** Called in list/ready/search paths. For 1000 issues, that's 11,000 small-string clones. Measurable only at scale, but the densest allocation site in the codebase. (`commands/mod.rs:44-57`)
- **SQL placeholder generation allocates per-placeholder.** `format!("?{}", i)` in a loop creates N small strings. Could use a `write!()` to a single buffer. (`db.rs:252`)

### Practical impact of full adherence
- **Release profile:** 3 lines in `Cargo.toml`. Binary shrinks ~50%. LTO may add 5-10s to compile time. **Recommended.**
- **`with_capacity()`:** Sprinkle in ~10 call sites. Zero risk, marginal improvement. Worth doing when touching those paths.
- **Reducing `format!()`:** Diminishing returns. Most calls are in cold paths (error messages, output formatting). The hot path (`build_issue_summary`) would benefit more from structural changes than format reduction.

---

## 5. Code Organization — 88/100

### What the guide says
> Keep `main.rs` thin. Group by domain/feature. Use `pub(crate)` for internal helpers. Re-export key types.

### What we do

**GOOD:**

- **Thin `main.rs`.** 88 lines of orchestration + a dispatch match statement. No business logic. (`main.rs`)
- **Domain-organized modules.** `commands/` directory with one file per command (25 files). `db.rs` for storage, `format.rs` for output, `urgency.rs` for scoring, `normalize.rs` for input normalization. Clean separation.
- **Single responsibility per command handler.** Each `commands/*.rs` exports a `run()` function. No cross-command dependencies (except `mod.rs` shared helpers).
- **`db.rs` is monolithic but intentional.** ~730 lines containing all SQLite operations. Documented in CLAUDE.md as the largest file. Splitting would create artificial boundaries since all operations share the schema and connection.

**ACCEPTABLE:**

- **No crate-root re-exports.** Binary crate — no external consumers. Internal `use crate::models::*` imports are clear enough.
- **`commands/mod.rs` has shared helpers** (`build_issue_summary`, `build_issue_detail`, `sort_by_urgency_desc`). These are the right abstraction level — shared across list/ready/search/next.

### Practical impact of full adherence
Already well-organized. Splitting `db.rs` into `db/schema.rs`, `db/issues.rs`, `db/notes.rs` etc. would add ~6 files but gain nothing for a single-developer CLI. **Not recommended** unless the file exceeds ~1500 LOC.

---

## 6. Testing & Tooling — 74/100

### What the guide says
> Unit tests in `#[cfg(test)]` modules. Integration tests for public API. Doctests as documentation. Clippy with `pedantic`. CI pipeline: fmt, clippy, test, deny.

### What we do

**GOOD:**

- **Comprehensive integration test suite.** `tests/integration.sh` — 259 test cases covering all commands, edge cases, soft fallbacks, aliases, batch operations, multi-agent workflows. Uses `python3 -c` for JSON parsing. (`tests/integration.sh`)
- **CI pipeline enforces quality.** GitHub Actions: `cargo fmt --check` -> `cargo clippy -- -D warnings` -> release build + integration tests. Auto-versioning on push to main. (`.github/workflows/ci.yml`)
- **`justfile` provides developer shortcuts.** `just test`, `just lint`, `just verify` (full pre-push validation). (`justfile`)

**ACCEPTABLE:**

- **Limited unit tests.** 36 tests in `util.rs` covering pure utility functions. No unit tests in `db.rs`, `urgency.rs`, `normalize.rs`, or command handlers. Integration tests cover these paths, but unit tests would catch regressions faster and provide better failure localization.
- **Clippy uses defaults.** No `[lints.clippy]` in `Cargo.toml`, no `clippy.toml`. The guide recommends `all` + `pedantic` at warn level. Currently relies on CI's `-D warnings` with default lints only.

**NEEDS IMPROVEMENT:**

- **No doctests.** No `///` doc comments with runnable examples on any public function. The guide says "every public function should have a working example." For a binary crate this is less critical, but key modules like `normalize.rs` and `urgency.rs` would benefit.
- **No property-based testing.** `proptest` would be valuable for `normalize.rs` (fuzzy matching), `util.rs` (comma parsing), and `urgency.rs` (score computation). These have broad input spaces.
- **No snapshot testing.** `insta` would be ideal for CLI output validation — currently done via shell string matching which is brittle.
- **No `cargo-deny` or `cargo-audit`.** Dependency security scanning not configured. Low risk (6 deps, all well-known) but good hygiene.

### Practical impact of full adherence
- **Clippy `pedantic`:** May surface 20-50 warnings. Most will be trivial fixes. Some may conflict with the soft-fallbacks philosophy (e.g., suggesting `enum` where `String` is intentional). Low risk, moderate effort.
- **Doctests:** ~2 hours of work for key modules. Zero performance impact. Improves discoverability.
- **`cargo-deny`/`cargo-audit`:** 5 minutes to add to CI. No code changes.
- **`proptest`/`insta`:** Adds 2 dev-dependencies. Higher effort (~4-8 hours) but would catch edge cases the shell tests miss. **Recommended for `normalize.rs` especially.**

---

## Exceptions Register

Deviations from the guide that are **intentional and should be preserved**:

| Deviation | Guide Recommendation | Why We Differ | Reference |
|-----------|---------------------|---------------|-----------|
| No newtypes for priority/status/kind | Newtype pattern for validated data | Soft-fallbacks philosophy requires accepting bad input and defaulting gracefully | CLAUDE.md "Soft Fallbacks Philosophy" |
| Strings instead of enums for data values | Enums for closed sets | Must allow normalization of synonyms and graceful fallback to defaults | `normalize.rs`, CLAUDE.md |
| `process::exit()` in error handler | Return `ExitCode` from main | BurntSushi-style pattern; guide acknowledges as "widely used" | `error.rs:77` |
| No `anyhow` / `.context()` | `anyhow` at application boundary | `thiserror` enum is sufficient for small CLI; error variants carry their own context | `error.rs` |
| No async | Use async for concurrent I/O | No network I/O; guide says "many CLI tools work perfectly with synchronous code" | `Cargo.toml` |
| `db.rs` monolithic (~730 LOC) | Split by domain | All operations share schema/connection; splitting adds files without reducing complexity | CLAUDE.md |
| `#[allow(clippy::too_many_arguments)]` | Use structs for >5 params | Direct CLI arg mapping; consolidation adds indirection | `db.rs:312`, `add.rs:12` |

---

## Recommended Improvements (by effort/impact)

### Quick Wins (< 30 min, zero risk)

1. **Add release profile** to `Cargo.toml` — `lto=true`, `codegen-units=1`, `strip=true`. Smaller binary, potentially faster startup.
2. **Add `cargo-deny`** to CI — dependency license/advisory scanning. 5-minute setup.
3. **Emit `REVIEW:` notes** when DB queries fail in `urgency.rs:136,143,169` instead of silent `unwrap_or(false)`.

### Medium Effort (1-4 hours, low risk)

4. **Enable clippy `pedantic`** in `Cargo.toml` `[lints.clippy]` section. Fix or `#[expect()]` the results.
5. **Add `with_capacity()`** to known-size vector allocations in `db.rs` and command handlers.
6. **Extract `ListFilter` struct** to replace the 12-parameter `list_issues()` signature.

### Higher Effort (4-8 hours, moderate risk)

7. **Add `proptest` tests** for `normalize.rs` and `util.rs` — broad input spaces benefit from property-based testing.
8. **Add doctests** to key public functions in `normalize.rs`, `urgency.rs`, `format.rs`.
9. **Reduce clone concentration** in `build_issue_summary()` — refactor to take ownership of `Issue` when the caller doesn't need it afterward.

### Not Recommended (high effort, negative or negligible impact)

10. ~~Newtype wrappers for priority/status/kind~~ — conflicts with soft-fallbacks philosophy.
11. ~~Split `db.rs` into sub-modules~~ — artificial boundaries for a 730-line file.
12. ~~Switch to `anyhow`~~ — `thiserror` enum is sufficient and provides machine-readable error codes.
13. ~~Add `Cow<str>` to models~~ — complicates serde for negligible performance gain at current scale.

---

## Scoring Methodology

Each section scored on:
- **Adherence** (0-40): How closely the code follows the guide's recommendations.
- **Justification** (0-30): How well deviations are justified by project constraints.
- **Consistency** (0-30): How uniformly patterns are applied across the codebase.

Weights reflect relative importance for a synchronous CLI tool (ownership/errors weighted higher than performance/async).

---

*Generated 2026-03-12 by Claude Opus 4.6 against the itr codebase at commit `61799ed`.*
